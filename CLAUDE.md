# CLAUDE.md — registry-mcp

Guidance for AI assistants working on this codebase.

---

## What this service does

An MCP (Model Context Protocol) server that wraps a Docker Distribution / OCI registry. Exposes nine tools: `list_repositories`, `list_tags`, `get_manifest`, `get_repository_disk_usage`, `tag_manifest`, `untag`, `delete_tag`, `migrate`, and `run_gc`. Uses the Streamable HTTP MCP transport (rmcp 1.4.0). Garbage collection is handled either by shelling out to a script or by managing a `registry:3` Docker container lifecycle via bollard. OAuth 2.0 Bearer JWT authentication is optional; when enabled, the server validates tokens against a JWKS endpoint and stamps every tool call with the caller's identity for audit logging.

---

## Codebase map

```
src/
  main.rs                   — startup: config load, registry client, auth state, MCP server bind, healthcheck subcommand
  lib.rs                    — re-exports all modules for integration test access
  config.rs                 — Config struct (incl. AuthConfig), load() (3-layer JSON merge + env overrides), validate()
  error.rs                  — RegistryError and GcError enums (thiserror)
  types.rs                  — shared data types: raw API shapes, all tool output structs, GcStrategy enum
  migrate.rs                — parse_source(), copy_image(), blob copy helpers (used by tools/migrate.rs)
  auth/
    mod.rs                  — UserIdentity struct, CURRENT_USER task-local, current_user() helper
    jwt.rs                  — JwtValidator: JWKS fetch + 1-hour cache, JWT decode/verify (jsonwebtoken v9)
    middleware.rs           — axum auth_middleware: extracts Bearer token, validates, scopes task-local
    well_known.rs           — GET /.well-known/oauth-authorization-server and /.well-known/oauth-protected-resource
  registry/
    mod.rs                  — module re-exports
    auth.rs                 — basic auth header builder, bearer token fetch + in-memory cache
    client.rs               — RegistryClient: all OCI Distribution HTTP calls, 401 token retry
  tools/
    mod.rs                  — RegistryMcp struct (incl. identity field), audit! macro, #[tool_router], ServerHandler impl
    catalog.rs              — list_repositories, list_tags
    manifest.rs             — get_manifest, get_repository_disk_usage
    delete.rs               — delete_tag
    tag.rs                  — tag_manifest, untag
    migrate.rs              — migrate (MigrateParams, thin wrapper over migrate::copy_image)
    gc.rs                   — run_gc (strategy dispatch)
  gc/
    mod.rs                  — resolve_strategy() — script → docker → unavailable
    script.rs               — run_script(): spawn shell script, capture output
    docker.rs               — run_docker_gc(): full bollard container lifecycle
tests/
  gc_integration.rs         — integration tests for Docker GC cleanup (bollard)
  docker-compose.test.yml   — MinIO + registry:3 for integration tests
```

---

## Critical implementation decisions

### Config loading (`config.rs`)

**Do not use the `config` crate.** It lowercases JSON keys, breaking camelCase fields like `baseUrl`, `bearerToken`, `registryImage`. The loader uses `serde_json::Value` deep-merge (via `hs_utils::config`) and deserialises in one step.

Config priority (lowest → highest):
1. `config.json` in the working directory
2. `/sandbox/config.json` — silently ignored if absent; used for secrets at deploy time
3. Env vars with `__` separator, exact camelCase key names (e.g. `registry__baseUrl=https://...`)

### rmcp tool pattern (`tools/mod.rs`)

Tools use the `#[tool_router]` / `#[tool]` / `#[tool_handler]` macros from rmcp 1.4.0. The `RegistryMcp` struct must hold a `ToolRouter<Self>` field and call `Self::tool_router()` in its constructor. All business logic lives in free functions in the module tree — the `impl` block methods are thin dispatch wrappers only.

Tool parameter structs must derive both `serde::Deserialize` and `schemars::JsonSchema`.

### MCP transport

Streamable HTTP on `axum::Router` at the `/mcp` path:
```
http://<host>:<port>/mcp
```
Graceful shutdown via `CancellationToken` on SIGINT.

### Registry auth (`registry/auth.rs`, `registry/client.rs`)

- If `registry.bearerToken` is non-empty it is used directly.
- Otherwise, `registry.username` + `registry.password` are sent as Basic auth.
- On 401, the client parses the `WWW-Authenticate: Bearer realm=...` challenge, fetches a token, caches it in-memory, and retries the request once.

### GC strategies (`gc/`)

`resolve_strategy()` is called at tool invocation time (not startup) so operators can add/remove the script file without restarting the server.

**Docker strategy** (`gc/docker.rs`): bollard manages the full container lifecycle — inspect, optional pull, create (with `registry-mcp.gc=true` label and optional network mode), start, stream logs as MCP progress notifications, wait for exit, remove. The container is **always removed** after the run, even on non-zero exit. Cleanup errors are logged at `warn` and do not mask the GC result.

The `ctx: Option<RequestContext<RoleServer>>` parameter is `Some` in production (emits progress notifications) and `None` in integration tests (no MCP peer connected).

**Shell script strategy** (`gc/script.rs`): `tokio::process::Command` with `stdout` and `stderr` piped; `wait_with_output()` blocks until completion — no incremental streaming possible.

### Docker GC container label

Every GC container is created with label `registry-mcp.gc=true`. This lets integration tests count GC containers without interfering with the compose registry container (which has no such label). Do not remove this label.

### `get_repository_disk_usage` (`tools/manifest.rs`)

Fetches all tags, then all manifests. For OCI image indexes, recursively fetches child manifests via `future::join_all` (parallelised per index). Blob digests are deduplicated across tags. Tags that 404 mid-flight are added to `skipped_tags` rather than failing the whole call.

### `tag_manifest` (`tools/tag.rs`)

Fetches the raw manifest bytes and `Content-Type` from `source` (tag or digest) via `GET /v2/<name>/manifests/<source>`, then PUTs the identical bytes to `PUT /v2/<name>/manifests/<new_tag>`. No layer data is copied — only the manifest reference is written.

The digest in the response comes from the PUT's `Docker-Content-Digest` header. If the registry omits that header (non-standard behaviour), the digest from the preceding GET is used as a fallback.

Auth: the bearer token fetched during the GET is cached and reused for the PUT. No separate auth round-trip is needed.

### `untag` (`tools/tag.rs`)

Issues `DELETE /v2/<name>/manifests/<tag>` using the tag name directly (not the digest). Registry:3 treats this as removing only that tag reference — the manifest blob and any other tags pointing to the same digest are unaffected.

Contrast with `delete_tag`, which resolves the tag to a digest first and then issues `DELETE /v2/<name>/manifests/<digest>`, removing the manifest entirely regardless of how many tags reference it.

`untag` has no dry-run guard — the operation is scoped to a single tag reference and the manifest is preserved.

### `migrate` (`migrate.rs`, `tools/migrate.rs`)

Copies an image from an external registry into the managed registry using the OCI Distribution API only — no Docker daemon required.

**Source parsing** (`migrate::parse_source`): handles `[registry/]repository[:tag][@digest]`. The first path component is identified as a registry if it contains `.` or `:` or equals `localhost`; otherwise Docker Hub is assumed. `docker.io` is translated to `registry-1.docker.io`. Bare names like `nginx` get the `library/` prefix.

**Multi-arch flow** (`migrate::copy_image`):
1. Fetch the top-level manifest raw bytes from source
2. Check `Content-Type` to distinguish image index vs single-platform manifest
3. For an index: parse as `RawIndex` → for each child digest, fetch child manifest + copy blobs → push child by digest → push index by target tag
4. For single: copy blobs (config + layers) → push manifest by target tag

**Blob copy** (`migrate::copy_blob`): `HEAD /v2/<repo>/blobs/<digest>` on destination first — skip if 200, otherwise `GET` from source and `POST /v2/<repo>/blobs/uploads/` + `PUT <location>?digest=<digest>` to destination.

**Source auth**: `RegistryClient::from_creds(base_url, username, password)` builds a temporary client for the source registry with optional credentials. The existing 401-→-bearer-token retry logic in `RegistryClient::get` handles token challenges automatically.

**Soft auth failure**: If `copy_image` returns `RegistryError::Unauthorized` and no credentials were provided, `tools/migrate.rs` returns `MigrateOutput { requires_auth: true }` (a tool success, not an error) with a message asking the caller to retry with credentials. If credentials were provided but still rejected, it surfaces as a tool error.

### `delete_tag` (`tools/delete.rs`)

1. `HEAD /v2/<name>/manifests/<tag>` to resolve the digest from `Docker-Content-Digest` header
2. If `confirm: false` (default), returns a dry-run response without deleting
3. If `confirm: true`, issues `DELETE /v2/<name>/manifests/<digest>`

Registry must have `storage.delete.enabled: true` in its config. A `405` response is surfaced as `DeleteNotEnabled`.

### OAuth authentication (`auth/`)

The `auth` module implements the [MCP OAuth 2.0](https://spec.modelcontextprotocol.io/specification/basic/authentication/) pattern where registry-mcp is an OAuth **Protected Resource** and all token issuance is delegated to an external Authorization Server configured in `auth.*`.

**Discovery endpoints** (`auth/well_known.rs`):
- `GET /.well-known/oauth-authorization-server` — RFC 8414 metadata: advertises the upstream AS's `authorization_endpoint`, `token_endpoint`, `registration_endpoint`, PKCE methods. Returns 404 when `auth.enabled = false`.
- `GET /.well-known/oauth-protected-resource` — RFC 9396 metadata: names this server as the resource and lists the issuer. Always served.

Both are registered at the root axum router (not under `/mcp`) so they are reachable at the server origin without authentication.

**Middleware** (`auth/middleware.rs`):
`auth_middleware` is an axum `from_fn_with_state` middleware applied to the `/mcp` route. It:
1. Extracts `Authorization: Bearer <token>` from the request header.
2. When `auth.enabled = false`, calls `CURRENT_USER.scope(UserIdentity::anonymous(), next.run(req))` and passes through.
3. When `auth.enabled = true`, calls `JwtValidator::validate()`. On success, scopes the task-local with the identity. On failure, returns `401` with `WWW-Authenticate: Bearer error="invalid_token"`.

**JWT validation** (`auth/jwt.rs`):
`JwtValidator` holds an `AuthConfig`, a `reqwest::Client`, and an `RwLock<Option<JwksCache>>`. `validate(token)` flow:
1. `decode_header(token)` → extract `kid` and `alg`.
2. `find_key(kid, alg)` → check cache (TTL 1 hour) → if miss/stale, `fetch_jwks()` → refresh cache → return key.
3. `DecodingKey::from_jwk(jwk)` + `Validation` with `iss` and optionally `aud`.
4. `decode::<Claims>(token, &key, &validation)` → build `UserIdentity { sub, email, display_name }`.
5. Key selection: prefer exact `kid` match in the JWKS; fall back to first key with a matching algorithm.

**Identity threading — task-local pattern** (`auth/mod.rs`):
The rmcp `StreamableHttpService` factory (`FnMut() -> Result<Handler>`) creates a new `RegistryMcp` per session but does not receive request data. To thread the per-request identity from the axum middleware into the factory:

```rust
tokio::task_local! {
    pub static CURRENT_USER: UserIdentity;
}
```

The middleware calls `CURRENT_USER.scope(identity, next.run(req)).await` — this sets the task-local for the duration of the entire request in the current Tokio task. The factory, which runs synchronously within that same task, reads it with `CURRENT_USER.try_with(|u| u.clone()).unwrap_or_default()`. This is safe because Tokio task-locals are scoped per-task with no sharing across concurrent sessions.

`current_user()` is a convenience wrapper around `try_with` that falls back to `UserIdentity::anonymous()` when called outside a scoped task (e.g. in tests).

**Audit logging** (`tools/mod.rs`):
The `audit!` macro emits a `tracing::info!` to the `audit` target with `user_sub`, `user_email`, `op`, and operation-specific fields (repository, tag, confirm, etc.). It uses Debug (`?`) format for all caller-provided values so it works uniformly across `String`, `Option<bool>`, and other types. Called at the top of every `RegistryMcp` tool method before delegating to the free function.

**`AuthConfig`** (`config.rs`): new top-level config section with fields `enabled`, `issuer`, `jwks_uri`, `audience`, `authorization_endpoint`, `token_endpoint`, `registration_endpoint`. All override via env vars using the standard `auth__fieldName` pattern.

**`server__publicUrl` env var** (`main.rs`): overrides the `resource` field in `/.well-known/oauth-protected-resource`. Required when the server is behind a reverse proxy. Defaults to `http://<server.host>:<server.port>`.

### Healthcheck subcommand (`main.rs`)

`./registry-mcp healthcheck` opens a stdlib TCP connection to `127.0.0.1:<server.port>` and exits 0 on success. No async runtime, no external deps. Used in `HEALTHCHECK` Dockerfile directive. Port is read from the `server__port` env var (default 3000) — not from `config.json` — to avoid a full config parse.

### `reqwest` and TLS

`reqwest` is configured with `default-features = false, features = ["rustls-tls", "json"]`. No OpenSSL / pkg-config dependency.

---

## hs-utils dependency

Shared utilities are pulled from `hs-utils-rs` via git tag:

```toml
hs-utils = { git = "https://github.com/Hikari-Systems/hs-utils-rs", tag = "v0.2.5" }
```

**Always use a git+tag reference, never a path dependency.** Tag before updating services. Functions used from `hs_utils`:
- `config::{prepare_config, apply_env_overrides, deep_merge, deser_bool_or_str, deser_u16_or_str}`
- `logging::init`

---

## Docker build notes

Two-stage build (`rust:1-bookworm` builder → `debian:bookworm-slim` runtime). No Alpine / musl — proc-macro crates require the dynamic linker.

Stub `src/main.rs` (`fn main() {}`) pattern caches dependency compilation so only the application layer rebuilds on source changes:

```dockerfile
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release --locked
RUN rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release --locked
```

The healthcheck uses the binary subcommand (no `curl` in the runtime image):
```dockerfile
HEALTHCHECK --interval=10s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/app/registry-mcp", "healthcheck"]
```

---

## Integration tests

Located in `tests/gc_integration.rs`. Require Docker and the test compose stack (`tests/docker-compose.test.yml`). Skipped unless `REGISTRY_INTEGRATION_TEST=1` is set.

The two tests (`test_docker_gc_cleanup_on_success`, `test_docker_gc_cleanup_on_failure`) are annotated `#[serial]` (via `serial_test`) because they both query Docker by the `registry-mcp.gc=true` label and would interfere if run in parallel.

To run:
```bash
docker compose -f tests/docker-compose.test.yml up -d --wait
REGISTRY_INTEGRATION_TEST=1 cargo test --test gc_integration -- --nocapture
docker compose -f tests/docker-compose.test.yml down -v
```

---

## Common gotchas

- Config field names are **case-sensitive**. `baseurl` is not `baseUrl`. Env vars use the same casing: `registry__baseUrl`, not `registry__baseurl`.
- `delete_tag` defaults to `confirm: false` (dry run). The digest is resolved and reported, but nothing is deleted until the caller passes `confirm: true`.
- `run_gc` defaults to both `dry_run: true` and `delete_untagged: true`. Pass `dry_run: false` explicitly to perform real GC.
- Blob storage is not reclaimed by `delete_tag` alone — GC must be run afterward to free disk space.
- The GC container is created with no restart policy. If it exits non-zero the result is surfaced in the tool response (`exit_code`, `stderr`), not as a Rust error.
- `gc.dockerNetwork: "host"` is required when the GC container needs to reach a storage backend (e.g. MinIO) that is only exposed on the host network.
- `auth.enabled: false` (the default) disables token validation entirely — every request runs as `anonymous`. This is intentional for local/trusted deployments.
- The `/.well-known/oauth-authorization-server` endpoint returns **404** when `auth.enabled = false`. This is correct — it signals to clients that OAuth is not configured, rather than advertising non-functional endpoints.
- `auth__jwksUri` and `auth__issuer` must both be set when `auth.enabled = true`. The server will start without them but every request will fail validation.
- JWKS keys are cached for one hour. If your AS rotates keys more frequently, a validation failure will trigger an immediate cache refresh and retry.
- The `CURRENT_USER` task-local is scoped per Tokio task — do not try to read it from a spawned `tokio::task::spawn` inside a tool, as the task-local will not propagate across `spawn` boundaries. Use `current_user()` before spawning and capture the result.
- `server__publicUrl` must be set to the public HTTPS URL when running behind a reverse proxy; otherwise `/.well-known/oauth-protected-resource` returns the internal bind address in the `resource` field, which OAuth clients may reject.
