# CLAUDE.md — registry-mcp

Guidance for AI assistants working on this codebase.

---

## What this service does

An MCP (Model Context Protocol) server that wraps a Docker Distribution / OCI registry. Exposes six tools: `list_repositories`, `list_tags`, `get_manifest`, `get_repository_disk_usage`, `delete_tag`, and `run_gc`. Uses the Streamable HTTP MCP transport (rmcp 1.4.0). Garbage collection is handled either by shelling out to a script or by managing a `registry:3` Docker container lifecycle via bollard.

---

## Codebase map

```
src/
  main.rs                   — startup: config load, registry client, MCP server bind, healthcheck subcommand
  lib.rs                    — re-exports all modules for integration test access
  config.rs                 — Config struct, load() (3-layer JSON merge + env overrides), validate()
  error.rs                  — RegistryError and GcError enums (thiserror)
  types.rs                  — shared data types: raw API shapes, all tool output structs, GcStrategy enum
  registry/
    mod.rs                  — module re-exports
    auth.rs                 — basic auth header builder, bearer token fetch + in-memory cache
    client.rs               — RegistryClient: all OCI Distribution HTTP calls, 401 token retry
  tools/
    mod.rs                  — RegistryMcp struct, #[tool_router], #[tool_handler], ServerHandler impl
    catalog.rs              — list_repositories, list_tags
    manifest.rs             — get_manifest, get_repository_disk_usage
    delete.rs               — delete_tag
    tag.rs                  — tag_manifest
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
