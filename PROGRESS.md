# Progress

## Status
COMPLETE

## Last Completed Task
17 — Dockerfile (2-stage, rust:1-bookworm builder + debian:bookworm-slim runtime, stub dep
     cache pattern, EXPOSE 3000, HEALTHCHECK via binary subcommand) and docker-compose.yml.
     End-to-end verified: all 6 tools tested against live S3-backed registry:3.0. Container
     healthcheck confirmed healthy (4 consecutive passes). All tasks complete.

## Previous Tasks
15 — All six tools implemented and building cleanly:
     - `list_repositories`, `list_tags` (catalog.rs)
     - `get_manifest`, `get_repository_disk_usage` (manifest.rs — parallel child manifest fetch
       via `future::join_all` for image indexes)
     - `delete_tag` (delete.rs — dry-run guard, digest resolution via HEAD)
     - `run_gc` (tools/gc.rs → gc/script.rs + gc/docker.rs — strategy resolution, progress
       notifications per log line for Docker strategy)

## Previous Tasks
7 — MCP server scaffold: `src/tools/mod.rs` (RegistryMcp struct, #[tool_router], #[tool_handler],
    ServerHandler with get_info), stub tool modules for all 6 tools, `src/main.rs` wired to
    StreamableHttpService on configurable host:port with graceful shutdown. Build clean.

## Previous Tasks
6 — Registry client: `src/registry/auth.rs` (basic auth header, bearer challenge parser, token
    fetch/cache) and `src/registry/client.rs` (RegistryClient with get_catalog, get_tags,
    get_manifest, delete_manifest, resolve_digest, get_index_descriptors; 401 → token retry
    flow). Added `base64 = "0.22"` as explicit dep (was transitive only).

## Previous Tasks
5 — Registry auth module scaffolded as part of task 6.
4 — Shared types: `src/types.rs` — OCI media type constants, raw API response shapes
    (`RawManifest`, `RawIndex`, `TagsResponse`, `CatalogResponse`), and all tool output structs.

## Previous Tasks
2 — Config module: `src/config.rs` using `hs_utils::config::{prepare_config, apply_env_overrides,
    deep_merge, deser_bool_or_str, deser_u16_or_str}` and `hs_utils::logging::init`. Dropped
    `tracing-subscriber` direct dep (owned by hs-utils). `/sandbox/config.json` overlay included.
    Startup validation of baseUrl, script/config paths, and credential precedence warning.

## Next Task
None — all 17 tasks complete.: `src/tools/mod.rs` (ServerHandler, tool registration), `src/main.rs`
    wired to Streamable HTTP transport. Server responds to MCP initialize before tools are added.

## Decisions Made
- **Q1 (multi-arch disk usage):** Always recurse into child manifests; parallelise fetches per
  index via `FuturesUnordered`. Option B ruled out — index-level size fields only cover manifest
  JSON, not layers.
- **Transport:** Streamable HTTP (not stdio). Server binds on `server.host:server.port`
  (default `0.0.0.0:3000`). `EXPOSE 3000` and `HEALTHCHECK` added to Dockerfile plan.
  `rmcp` feature: `transport-streamable-http-server`.
- **Q4 (GC streaming):** Docker GC streams log lines as MCP progress notifications via
  `ctx.peer.notify_progress()` during log consumption. Shell script strategy returns on
  completion only (no streaming possible with `wait_with_output`). Full logs always included
  in the final tool response regardless.
- **Q6 (rmcp API):** Resolved. Version `1.4.0`, edition `2024`. Key dependencies: `axum 0.8`,
  `schemars 1`. Streamable HTTP init uses `StreamableHttpService` + `LocalSessionManager` +
  axum `nest_service("/mcp", ...)`. Tools use `#[tool_router]` / `#[tool]` / `#[tool_handler]`
  macros. Tool param structs derive `serde::Deserialize + schemars::JsonSchema`.
  Progress notifications via `ctx.peer.notify_progress()`.

## Notes
- Plan is complete and awaiting approval before any Rust files are created.
- Open questions Q1, Q4, Q6, and the healthcheck question in §8 require decisions before the
  corresponding implementation tasks can begin.
- Sibling project `image-service` in the brief refers to `image-service-rs` (the Rust checkout).
  The `image-service` directory is the TypeScript predecessor and was not used as a reference.
