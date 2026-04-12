# CLAUDE.md — registry-mcp

Guidance for AI assistants working on this codebase.

---

## What this service does

An MCP (Model Context Protocol) server that wraps a Docker Distribution / OCI registry. Exposes eight tools: `list_repositories`, `list_tags`, `get_manifest`, `get_repository_disk_usage`, `delete_tag`, `tag_manifest`, `untag`, and `migrate`. Uses the Streamable HTTP MCP transport (rmcp 1.4.0).

---

## Codebase map

```
src/
  main.rs                   — startup: config load, registry client, MCP server bind, healthcheck subcommand
  lib.rs                    — re-exports all modules for integration test access
  config.rs                 — Config struct, load() (3-layer JSON merge + env overrides), validate()
  error.rs                  — RegistryError enum (thiserror)
  types.rs                  — shared data types: raw API shapes, all tool output structs
  migrate.rs                — parse_source(), copy_image(), blob copy helpers (used by tools/migrate.rs)
  registry/
    mod.rs                  — module re-exports
    auth.rs                 — basic auth header builder, bearer token fetch + in-memory cache
    client.rs               — RegistryClient: all OCI Distribution HTTP calls, 401 token retry
  tools/
    mod.rs                  — RegistryMcp struct, #[tool_router], #[tool_handler], ServerHandler impl
    catalog.rs              — list_repositories, list_tags
    manifest.rs             — get_manifest, get_repository_disk_usage
    delete.rs               — delete_tag
    tag.rs                  — tag_manifest, untag
    migrate.rs              — migrate (MigrateParams, thin wrapper over migrate::copy_image)
tests/
  migrate_integration.rs    — integration tests for migrate tool
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

### `get_repository_disk_usage` (`tools/manifest.rs`)

Fetches all tags, then all manifests. For OCI image indexes, recursively fetches child manifests via `future::join_all` (parallelised per index). Blob digests are deduplicated across tags. Tags that 404 mid-flight are added to `skipped_tags` rather than failing the whole call.

### `tag_manifest` (`tools/tag.rs`)

Fetches the raw manifest bytes and `Content-Type` from `source` (tag or digest) via `GET /v2/<name>/manifests/<source>`, then PUTs the identical bytes to `PUT /v2/<name>/manifests/<new_tag>`. No layer data is copied — only the manifest reference is written.

The digest in the response comes from the PUT's `Docker-Content-Digest` header. If the registry omits that header (non-standard behaviour), the digest from the preceding GET is used as a fallback.

Auth: the bearer token fetched during the GET is cached and reused for the PUT. No separate auth round-trip is needed.

### `untag` (`tools/tag.rs`)

Accepts a `tags: Vec<String>` (1–20 items). Validates length at entry; returns `ErrorData::invalid_params` if the list is empty or exceeds 20. Iterates sequentially — each tag issues `DELETE /v2/<name>/manifests/<tag>` using the tag name directly (not the digest). Registry:3 treats this as removing only that tag reference — the manifest blob and any other tags pointing to the same digest are unaffected.

A progress notification is emitted after each attempt (success or failure). Failed tags are collected into `failed: Vec<UntagFailure>` rather than aborting the loop. The response always returns both `removed` and `failed` lists.

Contrast with `delete_tag`, which resolves the tag to a digest first and then issues `DELETE /v2/<name>/manifests/<digest>`, removing the manifest entirely regardless of how many tags reference it.

`untag` has no dry-run guard — the operation is scoped to tag references and manifests are preserved.

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

Located in `tests/migrate_integration.rs`. Require Docker and the test compose stack (`tests/docker-compose.test.yml`). Skipped unless `REGISTRY_INTEGRATION_TEST=1` is set.

To run:
```bash
docker compose -f tests/docker-compose.test.yml up -d --wait
REGISTRY_INTEGRATION_TEST=1 cargo test --test migrate_integration -- --nocapture
docker compose -f tests/docker-compose.test.yml down -v
```

---

## Common gotchas

- Config field names are **case-sensitive**. `baseurl` is not `baseUrl`. Env vars use the same casing: `registry__baseUrl`, not `registry__baseurl`.
- `delete_tag` defaults to `confirm: false` (dry run). The digest is resolved and reported, but nothing is deleted until the caller passes `confirm: true`.
- Blob storage is not reclaimed by `delete_tag` alone — the registry's own garbage collector must be run externally to free disk space.
