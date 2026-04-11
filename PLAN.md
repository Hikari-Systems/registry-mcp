# PLAN.md — registry-mcp

> Planning document for the `registry-mcp` MCP server.
> No code is written until this plan is reviewed and approved.

---

## §1 — Crate Structure

Standalone crate (not part of a workspace — matches sibling services). Single binary.

```
registry-mcp/
  Cargo.toml
  Cargo.lock
  config.json          — default config baked into the image
  PLAN.md
  PROGRESS.md
  src/
    main.rs            — entry point: load config, build server, bind HTTP transport
    config.rs          — config schema, loading logic (config.json + env overrides)
    error.rs           — typed error enums and anyhow integration
    types.rs           — shared data types: manifest structs, layer descriptors, GC results
    tools/
      mod.rs           — ServerHandler impl, tool registration, dispatch
      catalog.rs       — list_repositories, list_tags
      manifest.rs      — get_manifest, get_repository_disk_usage
      delete.rs        — delete_tag
      gc.rs            — run_gc (strategy resolution + execution)
    registry/
      mod.rs           — module re-exports
      client.rs        — reqwest HTTP client, auth header injection, response parsing
      auth.rs          — basic auth and bearer token acquisition/caching
    gc/
      mod.rs           — GcStrategy enum, resolve() function
      script.rs        — shell script execution and output capture
      docker.rs        — bollard-based Docker GC: pull check, create, start, logs, wait, remove
```

**Module responsibilities:**

| Module | Responsibility |
|---|---|
| `tools/mod.rs` | `ServerHandler` impl; routes incoming tool calls to handler functions |
| `tools/catalog.rs` | `list_repositories`, `list_tags` — paginated OCI catalog and tag APIs |
| `tools/manifest.rs` | `get_manifest`, `get_repository_disk_usage` — manifest fetch and blob aggregation |
| `tools/delete.rs` | `delete_tag` — digest resolution, confirm guard, manifest delete |
| `tools/gc.rs` | `run_gc` — strategy resolution and execution |
| `registry/client.rs` | All OCI Distribution HTTP calls via `reqwest`; auth header injection |
| `registry/auth.rs` | Basic auth credential formatting; bearer token fetch and in-memory cache |
| `gc/script.rs` | Spawn shell script, capture stdout/stderr, interpret exit code |
| `gc/docker.rs` | Full Docker GC lifecycle via `bollard` |
| `config.rs` | Load `config.json`, apply env overrides, deserialise into `Config` struct |
| `error.rs` | `RegistryError`, `GcError`, and `anyhow` usage policy |
| `types.rs` | `Manifest`, `Descriptor`, `LayerInfo`, `GcOutcome`, and other shared structs |

---

## §2 — Configuration Design

Config is loaded in priority order (lowest → highest):

1. `config.json` in the working directory (baked into the Docker image as defaults)
2. `/sandbox/config.json` — silently ignored if absent; used for secrets/env overrides
3. Environment variables using `__` as path separator with exact camelCase key names
   (e.g. `registry__password=secret`)

Loading uses the same pattern as `image-service-rs`: `serde_json::Value` deep-merge,
then single `serde_json::from_value` deserialisation step. No `config` crate (it
lowercases keys, breaking camelCase fields).

### Full schema

```json
{
  "server": {
    "host": "0.0.0.0",
    "port": 3000
  },
  "registry": {
    "baseUrl": "https://registry.example.com",
    "username": "",
    "password": "",
    "bearerToken": "",
    "insecureSkipVerify": false,
    "caCertFile": ""
  },
  "gc": {
    "scriptPath": "",
    "registryConfigPath": "",
    "dockerSocket": "/var/run/docker.sock",
    "registryImage": "registry:3"
  },
  "log": {
    "level": "info"
  }
}
```

### Field descriptions

**`server.host`** *(default `0.0.0.0`)* — Bind address for the HTTP server.

**`server.port`** *(default `3000`)* — TCP port. Override via `server__port` env var.

**`registry.baseUrl`** *(required)* — Base URL of the registry, e.g. `https://registry.example.com`. No trailing slash.

**`registry.username` / `registry.password`** *(optional)* — Credentials for HTTP Basic auth. Used if `bearerToken` is absent.

**`registry.bearerToken`** *(optional)* — Static bearer token. Takes precedence over basic auth when non-empty.

**`registry.insecureSkipVerify`** *(optional, default `false`)* — Skip TLS certificate verification. For development against self-signed registries.

**`registry.caCertFile`** *(optional)* — Path to a PEM CA certificate to add to the trust store.

**`gc.scriptPath`** *(optional)* — Absolute path to a shell script. When set, takes priority over all other GC strategies.

**`gc.registryConfigPath`** *(optional)* — Path to a registry `config.yml` to mount into the GC container. Required for Docker GC strategy.

**`gc.dockerSocket`** *(default `/var/run/docker.sock`)* — Docker socket path for `bollard`.

**`gc.registryImage`** *(default `registry:3`)* — Registry image used for the Docker GC container.

**`log.level`** *(default `info`)* — Tracing filter string (`error`, `warn`, `info`, `debug`, `trace`).

### Validation rules

Validated at startup, before the MCP server starts:

- `registry.baseUrl` must be non-empty and parse as a valid URL.
- At most one of `username+password` or `bearerToken` may be non-empty (warn if both set; bearerToken wins).
- If `gc.registryConfigPath` is non-empty, the file must exist and be readable.
- If `gc.scriptPath` is non-empty, the file must exist and be executable.

---

## §3 — Tool Inventory

### `list_repositories`

**Description:** Returns a paginated list of repository names from the OCI Distribution `_catalog` endpoint.

**Input schema:**

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `last` | `string` | no | `""` | Pagination cursor — last repository name seen |
| `limit` | `integer` | no | `100` | Maximum number of repositories to return (1–1000) |

**Output (success):**
```json
{
  "repositories": ["library/nginx", "myapp/api"],
  "next_last": "myapp/api",
  "total_returned": 2
}
```
`next_last` is the last name returned, to be passed as `last` in the next call. Empty string when no further pages.

**Output (error):** MCP tool error with `code` and `message` fields.

**Endpoints:** `GET /v2/_catalog?n=<limit>&last=<last>`

**Side effects:** None.

---

### `list_tags`

**Description:** Returns paginated tags for a named repository.

**Input schema:**

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | `string` | yes | — | Repository name, e.g. `library/nginx` |
| `last` | `string` | no | `""` | Pagination cursor |
| `limit` | `integer` | no | `100` | Maximum tags to return (1–1000) |

**Output (success):**
```json
{
  "repository": "library/nginx",
  "tags": ["1.25", "1.26", "latest"],
  "next_last": "latest",
  "total_returned": 3
}
```

**Output (error):** MCP tool error. `404` from registry → `RepositoryNotFound`.

**Endpoints:** `GET /v2/<name>/tags/list?n=<limit>&last=<last>`

**Side effects:** None.

---

### `get_manifest`

**Description:** Fetches the manifest for a tag or digest and returns a structured breakdown including media type, total compressed size, and per-layer details.

**Input schema:**

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | `string` | yes | — | Repository name |
| `reference` | `string` | yes | — | Tag name or digest (`sha256:…`) |

**Output (success):**
```json
{
  "repository": "library/nginx",
  "reference": "latest",
  "digest": "sha256:abc123…",
  "media_type": "application/vnd.oci.image.manifest.v1+json",
  "schema_version": 2,
  "total_size_bytes": 54321000,
  "config": {
    "digest": "sha256:…",
    "media_type": "application/vnd.oci.image.config.v1+json",
    "size_bytes": 1234
  },
  "layers": [
    {
      "digest": "sha256:…",
      "media_type": "application/vnd.oci.image.layer.v1.tar+gzip",
      "size_bytes": 27160500
    }
  ],
  "is_image_index": false
}
```

For OCI image index / manifest lists: `is_image_index: true`, `layers` is empty,
and a `manifests` array lists child manifest entries (platform, digest, size).
Total size is the sum of all child manifest sizes (not recursively fetched —
see §7).

**Output (error):** `404` → `ManifestNotFound`.

**Endpoints:**
- `GET /v2/<name>/manifests/<reference>` with `Accept` headers for OCI and Docker media types.

**Side effects:** None.

---

### `get_repository_disk_usage`

**Description:** Calculates the aggregate on-disk blob footprint for all tags in a repository,
deduplicating layers that are referenced by multiple tags. Returns per-tag breakdown and totals.

**Input schema:**

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | `string` | yes | — | Repository name |

**Output (success):**
```json
{
  "repository": "library/nginx",
  "unique_blob_count": 12,
  "unique_size_bytes": 82400000,
  "tags": [
    {
      "tag": "latest",
      "digest": "sha256:…",
      "layer_count": 6,
      "total_size_bytes": 54321000
    }
  ]
}
```

`unique_size_bytes` counts each blob digest once regardless of how many tags reference it.
`tags[].total_size_bytes` is the sum of that tag's layers (including shared ones, for per-tag context).

**Endpoints:** `GET /v2/<name>/tags/list` then `GET /v2/<name>/manifests/<tag>` for each tag.

**Side effects:** None. Potentially many HTTP requests for repos with many tags.

---

### `delete_tag`

**Description:** Soft-deletes a manifest by resolving its digest from the tag, then issuing a manifest delete. Protected by a `confirm` guard — defaults to `false` (dry run).

**Input schema:**

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | `string` | yes | — | Repository name |
| `tag` | `string` | yes | — | Tag to delete |
| `confirm` | `bool` | no | `false` | Must be `true` to perform actual deletion |

**Output (success, `confirm: false`):**
```json
{
  "repository": "library/nginx",
  "tag": "old-build",
  "digest": "sha256:abc…",
  "deleted": false,
  "message": "Dry run — set confirm: true to delete manifest sha256:abc…"
}
```

**Output (success, `confirm: true`):**
```json
{
  "repository": "library/nginx",
  "tag": "old-build",
  "digest": "sha256:abc…",
  "deleted": true,
  "message": "Manifest sha256:abc… deleted. Run GC to reclaim blob storage."
}
```

**Output (error):** `404` → `TagNotFound`. `405` → `DeleteNotEnabled` (registry config does not permit deletions).

**Endpoints:**
1. `GET /v2/<name>/manifests/<tag>` (with `Accept` for OCI/Docker types) — read `Docker-Content-Digest` response header to obtain the digest.
2. `DELETE /v2/<name>/manifests/<digest>` — only if `confirm: true`.

**Preconditions:** Registry must have deletion enabled (`storage.delete.enabled: true` in registry config). The tool surfaces `405` as a descriptive error.

**Side effects:** Removes the manifest reference. Blobs are not immediately reclaimed — GC must be run separately.

---

### `run_gc`

**Description:** Runs garbage collection using the configured strategy. Resolves strategy in priority order: shell script → Docker container → Unavailable.

**Input schema:**

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `dry_run` | `bool` | no | `true` | If `true`, reports what would be deleted without removing anything |
| `delete_untagged` | `bool` | no | `true` | Pass `--delete-untagged` to the registry GC command |

**Output (success):**
```json
{
  "strategy": "docker",
  "dry_run": true,
  "exit_code": 0,
  "stdout": "…",
  "stderr": "…",
  "message": "GC completed successfully (dry run)."
}
```

**Output (`Unavailable`):**
```json
{
  "strategy": "unavailable",
  "dry_run": true,
  "exit_code": null,
  "stdout": "",
  "stderr": "",
  "message": "No GC strategy configured. Set gc.scriptPath or gc.registryConfigPath in config."
}
```

**Output (non-zero exit):** Tool succeeds (MCP level); `exit_code` is non-zero; `message` describes failure. Callers inspect `exit_code` to detect GC failure.

**Side effects:** If `dry_run: false`, blob storage is modified. Manifests that were soft-deleted via `delete_tag` have their blobs permanently removed.

---

## §4 — GC Strategy Detail

### Strategy resolution order

```
gc.scriptPath non-empty AND file exists → ShellScript
gc.registryConfigPath non-empty AND file exists → Docker
otherwise → Unavailable
```

Resolution happens at call time (not startup) so operators can add/remove the script without restarting the MCP server.

---

### Shell script strategy

**Invocation:**
```
<scriptPath> [--dry-run] [--delete-untagged]
```

Flags are appended when the corresponding input fields are `true`. No positional arguments. The script is responsible for all registry interaction; the MCP server simply executes and observes it.

**Environment variables forwarded:**
- `REGISTRY_URL` — value of `config.registry.baseUrl`
- `REGISTRY_USERNAME` — value of `config.registry.username`
- `REGISTRY_PASSWORD` — value of `config.registry.password`
- `DRY_RUN` — `"true"` or `"false"`
- `DELETE_UNTAGGED` — `"true"` or `"false"`

**Execution:**
- Uses `tokio::process::Command` with `stdout(Stdio::piped())` and `stderr(Stdio::piped())`.
- Awaits completion with `child.wait_with_output()`.
- Exit code 0 → success. Non-zero → failure (captured, surfaced in output, not propagated as a Rust error).

**Output:** stdout and stderr captured in full and returned in the tool response.

---

### Docker GC strategy

Uses `bollard` with the Docker socket at `config.gc.dockerSocket`.

**Step 1 — Image pull check:**
```rust
docker.inspect_image(&config.gc.registry_image).await
```
If `Ok(_)` → image present, skip pull.
If `Err(_)` → pull via:
```rust
docker.create_image(Some(CreateImageOptions { from_image: &image, .. }), None, None)
    .try_collect::<Vec<_>>().await?
```

**Step 2 — Container create:**
```rust
docker.create_container(None::<CreateContainerOptions<&str>>, Config {
    image: Some(&config.gc.registry_image),
    cmd: Some(build_gc_cmd(dry_run, delete_untagged)),
    env: Some(build_gc_env(&config)),
    host_config: Some(HostConfig {
        binds: Some(vec![
            format!("{}:/etc/docker/registry/config.yml:ro",
                    config.gc.registry_config_path),
        ]),
        ..Default::default()
    }),
    ..Default::default()
}).await?
```

`build_gc_cmd` produces:
```
["/bin/registry", "garbage-collect",
 "/etc/docker/registry/config.yml",
 "--dry-run"?,          // if dry_run
 "--delete-untagged"?]  // if delete_untagged
```

`build_gc_env` produces:
```
["REGISTRY_HTTP_ADDR=", ...]   // no HTTP listener needed for GC-only run
```

**Step 3 — Container start:**
```rust
docker.start_container(&container_id, None::<StartContainerOptions<&str>>).await?
```

**Step 4 — Log capture with progress notifications:**
```rust
docker.logs(&container_id, Some(LogsOptions {
    follow: true,
    stdout: true,
    stderr: true,
    ..Default::default()
}))
```
Stream is consumed line-by-line. Each line is:
1. Appended to the full log buffer (returned in the final response).
2. Emitted as a progress notification via `ctx.peer.notify_progress(...)`.

Errors from `notify_progress` are ignored (`.ok()`) — the client may not subscribe to
notifications, and a broken notification channel must not abort GC.

**Step 5 — Wait for exit:**
```rust
docker.wait_container(&container_id, None::<WaitContainerOptions<&str>>)
    .try_next().await?
```
Captures `StatusCode` (exit code).

**Step 6 — Cleanup (always, even on failure):**
```rust
docker.remove_container(&container_id, Some(RemoveContainerOptions {
    force: true,
    ..Default::default()
})).await
// error here is logged but not propagated if GC itself succeeded
```
Cleanup is run in a `finally`-style block: if the GC produced an error, cleanup is attempted, then the GC error is returned. If cleanup itself fails, that error is logged at `warn` level and the GC result takes precedence.

---

### Unavailable strategy

Returns a structured response (not a Rust error):
```json
{
  "strategy": "unavailable",
  "message": "No GC strategy is configured. To enable GC, set one of: gc.scriptPath (path to a shell script) or gc.registryConfigPath (path to a registry config.yml for Docker-based GC)."
}
```

---

## §5 — Error Handling Strategy

### Error taxonomy

**Typed enum: `RegistryError`** — errors from OCI HTTP API calls:
```rust
pub enum RegistryError {
    NotFound { resource: String },
    Unauthorized,
    Forbidden,
    DeleteNotEnabled,
    UnexpectedStatus { status: u16, body: String },
    Transport(reqwest::Error),
    InvalidResponse(String),
}
```

**Typed enum: `GcError`** — errors from GC strategy execution:
```rust
pub enum GcError {
    ScriptNotFound(PathBuf),
    ScriptSpawnFailed(std::io::Error),
    DockerApi(bollard::errors::Error),
    ImagePullFailed(String),
    ContainerCreateFailed(String),
    ContainerRunFailed { exit_code: i64, stderr: String },
}
```

**`anyhow::Error`** — used in `main.rs` for startup failures (config load, validation)
and in any code path where the error is unrecoverable and context-only.

### Registry API error mapping

| HTTP status | `RegistryError` variant | MCP tool response |
|---|---|---|
| 401 | `Unauthorized` | Error: "Registry authentication failed — check credentials." |
| 403 | `Forbidden` | Error: "Access denied by registry." |
| 404 | `NotFound` | Error: "Not found: `<resource>`." |
| 405 | `DeleteNotEnabled` | Error: "Registry does not permit deletions — enable storage.delete in registry config." |
| 4xx/5xx other | `UnexpectedStatus` | Error: "Registry returned HTTP `<status>`: `<body>`." |

### Docker API errors

`bollard::errors::Error` is wrapped in `GcError::DockerApi`. Each step of the Docker GC
flow is individually matched:
- Pull failure → `GcError::ImagePullFailed` with image name
- Container create failure → `GcError::ContainerCreateFailed` with bollard message
- Non-zero exit → not a Rust error; surfaced as `exit_code` in the tool response

### Partial failures

- **GC container starts but exits non-zero:** The container's stdout/stderr and exit code
  are captured and returned in the tool response. This is a GC-level failure, not a
  Rust-level error. The MCP tool call itself succeeds (returns a value, not an error).

- **GC container cleanup fails after GC error:** Cleanup error is logged at `warn`.
  The original GC error is returned to the caller.

- **`get_repository_disk_usage` with a tag that 404s mid-flight:** The tag is skipped and
  a `skipped_tags` array is included in the response, rather than failing the entire call.

---

## §6 — Implementation Order

Each task is a single coherent unit of work. `PROGRESS.md` is updated on completion of each.

1. **Project scaffold** — `cargo new registry-mcp`, populate `Cargo.toml` with all dependencies, create `config.json` with defaults, verify `cargo build` succeeds.

2. **Config module** — implement `src/config.rs`: `Config` struct with all fields, `Config::load()` using `serde_json::Value` layering + env override, startup validation. Unit-test with a test JSON blob.

3. **Error types** — implement `src/error.rs`: `RegistryError` and `GcError` enums with `Display` and `std::error::Error` impls; `impl From<RegistryError> for anyhow::Error`.

4. **Shared types** — implement `src/types.rs`: `Manifest`, `Descriptor`, `LayerInfo`, `ManifestIndex`, `GcOutcome`, `GcStrategy` enum, and all output structs for tool responses.

5. **Registry auth** — implement `src/registry/auth.rs`: `build_basic_auth_header()`, `fetch_bearer_token()` (handles `WWW-Authenticate` challenge), and `AuthState` for in-memory token caching.

6. **Registry client** — implement `src/registry/client.rs`: `RegistryClient` struct wrapping `reqwest::Client`; methods: `get_catalog`, `get_tags`, `get_manifest`, `delete_manifest`. All methods inject auth, handle 401 token refresh, and map HTTP errors to `RegistryError`.

7. **MCP server scaffold** — implement `src/main.rs` (startup) and `src/tools/mod.rs` (`ServerHandler` impl, tool registration). Server binds Streamable HTTP transport, starts, and responds to MCP `initialize` without any tools yet.

8. **`list_repositories` tool** — implement `src/tools/catalog.rs`: call `registry.get_catalog`, map to output struct, register tool.

9. **`list_tags` tool** — implement `src/tools/catalog.rs`: call `registry.get_tags`, map to output struct, register tool.

10. **`get_manifest` tool** — implement `src/tools/manifest.rs`: fetch manifest, parse OCI/Docker schema, detect image index, map to output struct.

11. **`get_repository_disk_usage` tool** — implement `src/tools/manifest.rs`: fetch all tags, fetch all manifests, deduplicate blob digests, compute totals, handle per-tag 404s gracefully.

12. **`delete_tag` tool** — implement `src/tools/delete.rs`: HEAD/GET manifest to resolve digest, confirm guard, DELETE manifest, map 405 to `DeleteNotEnabled`.

13. **Shell script GC** — implement `src/gc/script.rs`: `run_script()` — validate path, spawn via `tokio::process::Command`, capture output, return `GcOutcome`.

14. **Docker GC** — implement `src/gc/docker.rs`: `run_docker_gc()` — bollard client init, image check, pull, container create/start/log/wait/remove, return `GcOutcome`.

15. **`run_gc` tool** — implement `src/tools/gc.rs`: `resolve_strategy()`, dispatch to script/docker/unavailable, register tool.

16. **End-to-end verification** — run against a live Docker Distribution registry; test all six tools including `delete_tag confirm: true` and `run_gc dry_run: false`.

17. **Dockerfile and docker-compose.yml** — write Dockerfile following established conventions (see §8), write `docker-compose.yml`, verify `docker build` and container startup.

---

## §7 — Open Questions

### Q1 — Multi-arch manifest indexes in disk usage [RESOLVED]

Always recurse into child manifests. The index's own `size` fields only cover the manifest JSON
(kilobytes) — not layer data — so Option B would produce meaningless totals. Child manifest
fetches within a single index are parallelised via `FuturesUnordered`. The tool description
notes that repos with many multi-arch tags may take several seconds.

### Q2 — Pagination strategy for large catalogs [LOW RISK]

The `_catalog` and `tags/list` endpoints return paginated results via `Link` header. Two options:

**A)** Expose pagination to the caller (current plan — `last` cursor, `limit`).
**B)** Collect all results internally and return everything.

*Proposed:* Option A — expose pagination. The MCP caller controls page size. This avoids
timeouts on registries with thousands of repositories. **No blocker; already reflected in §3.**

### Q3 — `delete_tag` referrers chain (OCI 1.1) [LOW RISK]

OCI 1.1 introduces a `subject` field in manifests linking referrers (SBOMs, signatures, etc.).
Deleting a manifest does not automatically clean up referrers.

*Proposed:* Skip referrers chain in initial implementation. Document in tool description that
referrers are not removed. **No blocker.**

### Q4 — GC log streaming vs completion [RESOLVED]

`rmcp` supports progress notifications via `ctx.peer.notify_progress()` on the
`RequestContext<RoleServer>` passed to any tool method. During Docker GC log streaming,
each log line is emitted as a progress notification with `message: Some(log_line)`.
The final tool response still returns the complete captured output for clients that do
not process notifications.

Pattern:
```rust
#[tool(description = "...")]
async fn run_gc(
    &self,
    Parameters(params): Parameters<RunGcParams>,
    ctx: RequestContext<RoleServer>,
) -> Result<CallToolResult, McpError> {
    // ... emit each log line:
    ctx.peer.notify_progress(ProgressNotificationParam {
        progress_token: ProgressToken(NumberOrString::String("gc".into())),
        progress: lines_seen as f64,
        total: None,
        message: Some(line.clone()),
    }).await.ok(); // ignore send errors (client may not be listening)
    // ...
}
```

Shell script output is collected via `wait_with_output()` (blocking until completion), so
no incremental notifications are possible for the script strategy — full output on completion
only. Only the Docker strategy streams incrementally.

### Q5 — Bearer token vs basic auth precedence [LOW RISK]

Config allows both `bearerToken` and `username`/`password`. If both are set, `bearerToken` wins.
*Proposed:* Log a warning at startup that both are set and bearerToken is used. **No blocker.**

### Q6 — `rmcp` crate API stability [RESOLVED]

Confirmed against the SDK source (`modelcontextprotocol/rust-sdk`, tag `1.4.0`).

**Version:** `1.4.0`. Edition `2024`.

**Feature flags required:**
```toml
rmcp = { version = "1.4.0", features = ["server", "transport-streamable-http-server"] }
# "macros" and "base64" are in default features — no need to list them explicitly
```

**Additional required dependencies** (not in rmcp defaults):
```toml
axum     = { version = "0.8", features = ["http1", "tokio"] }
schemars = "1"   # for JsonSchema derive on tool param structs
```

**Streamable HTTP server init pattern** (from `examples/servers/src/counter_streamhttp.rs`):
```rust
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
    session::local::LocalSessionManager,
};

let ct = tokio_util::sync::CancellationToken::new();
let service = StreamableHttpService::new(
    || Ok(MyHandler::new()),
    LocalSessionManager::default().into(),
    StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token()),
);
let router = axum::Router::new().nest_service("/mcp", service);
let listener = tokio::net::TcpListener::bind(&addr).await?;
axum::serve(listener, router)
    .with_graceful_shutdown(async move { tokio::signal::ctrl_c().await.unwrap(); ct.cancel(); })
    .await?;
```

**Tool definition pattern** (from `examples/servers/src/common/counter.rs`):
```rust
#[derive(Clone)]
struct RegistryMcp {
    config: Arc<Config>,
    tool_router: ToolRouter<RegistryMcp>,
}

#[tool_router]
impl RegistryMcp {
    fn new(config: Arc<Config>) -> Self {
        Self { config, tool_router: Self::tool_router() }
    }

    #[tool(description = "List repositories in the registry")]
    async fn list_repositories(
        &self,
        Parameters(params): Parameters<ListRepositoriesParams>,
    ) -> Result<CallToolResult, McpError> {
        // delegate to free function in tools::catalog
        catalog::list_repositories(&self.config, params).await
            .map(|r| CallToolResult::success(vec![Content::text(serde_json::to_string(&r)?)]))
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    }
}

#[tool_handler]
impl ServerHandler for RegistryMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
    }
}
```

**Tool parameter structs** must derive `serde::Deserialize` and `schemars::JsonSchema`:
```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ListRepositoriesParams {
    last: Option<String>,
    limit: Option<u32>,
}
```

**Progress notifications** — tool methods can accept `ctx: RequestContext<RoleServer>` as
a final parameter and call `ctx.peer.notify_progress(...)` (see Q4 above).

**Coding constraint note:** The `ToolRouter<Self>` field is an rmcp requirement — the struct
must hold it and call `Self::tool_router()` in its constructor. All business logic remains in
free functions in the module tree; the `impl` blocks on the handler struct are thin dispatch
wrappers only.

---

## §8 — Docker Containerisation

Conventions extracted from `image-service-rs` (the canonical Rust sibling service).

### 1. Base image and builder image

| Stage | Image |
|---|---|
| Builder | `rust:1-bookworm` |
| Runtime | `debian:bookworm-slim` |

No Alpine/musl. No `cargo-chef`. Static linking is not used (proc-macro crates require the dynamic linker at build time).

### 2. Multi-stage build structure

Two stages (not three — `registry-mcp` has no ImageMagick dependency):

**Stage 1 — `builder` (`rust:1-bookworm`)**
1. Copy `Cargo.toml` + `Cargo.lock`
2. Create stub `src/main.rs` (`fn main() {}`)
3. `cargo build --release --locked` — caches dependency compilation
4. `rm -rf src`
5. Copy real `src/` and `config.json`
6. `touch src/main.rs` — forces re-link
7. `cargo build --release --locked`

**Stage 2 — `runtime` (`debian:bookworm-slim`)**
1. Install `ca-certificates` (for TLS to registry and Docker socket)
2. Copy binary from builder
3. Copy `config.json` from builder
4. Set `WORKDIR /app`, `USER nobody`
5. Set `CMD`

### 3. Binary name and entrypoint

Binary name: `registry-mcp` (matches crate name).
Placed at: `/app/registry-mcp`.

```dockerfile
EXPOSE 3000
CMD ["/app/registry-mcp"]
```

**HEALTHCHECK:** The server now listens on HTTP. A stdlib TCP healthcheck subcommand
(matching the `hs-utils-rs` pattern — no `curl` dependency) probes `server.host:server.port`.
Alternatively the MCP `/health` path can be probed if `rmcp` exposes one. Default interval
matches sibling services (`--interval=10s --timeout=5s --start-period=15s --retries=3`).

```dockerfile
HEALTHCHECK --interval=10s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/app/registry-mcp", "healthcheck"]
```

A `healthcheck` subcommand will be added in task 16 (verification), before the Dockerfile is written.

### 4. Build arguments and environment variables

No `ARG` directives. All config is provided at runtime via `/sandbox/config.json` or environment variables.

Runtime env vars (examples for `docker-compose.yml`):
```
server__port=3000
registry__baseUrl=https://registry.example.com
registry__username=user
registry__password=secret
gc__registryConfigPath=/registry/config.yml
log__level=info
```

### 5. Workspace vs standalone crate

Standalone crate — matches all sibling Rust services. No Cargo workspace.

### 6. Naming and tagging conventions

Based on sibling services: no public registry references found in the Dockerfiles themselves.
Image naming follows the organisation's deployment convention (assumed to be `ghcr.io/Hikari-Systems/registry-mcp:<version>`). **Confirm naming convention before task 17.**

### 7. Makefile and build scripts

Neither `hs-utils-rs` nor `image-service-rs` have a `Makefile` or build script. No Makefile will be created for `registry-mcp`. Build is via `docker build .` directly.

A `docker-compose.yml` will be provided for local development, mirroring `image-service-rs`'s compose file structure.

### Dockerfile (planned — not written until task 17)

```dockerfile
# ─── Stage 1: Rust builder ────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

WORKDIR /build

# Cache dependency compilation — only reruns when Cargo.toml or Cargo.lock change.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release --locked 2>&1 | tail -5
RUN rm -rf src

# Build the application
COPY src ./src
COPY config.json ./

# Touch main.rs so cargo re-links against the real source
RUN touch src/main.rs
RUN cargo build --release --locked

# ─── Stage 2: Runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/registry-mcp /app/registry-mcp
COPY --from=builder /build/config.json /app/config.json

USER nobody

EXPOSE 3000

HEALTHCHECK --interval=10s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/app/registry-mcp", "healthcheck"]

CMD ["/app/registry-mcp"]
```

### Deviations from sibling conventions

None. The two-stage pattern is a strict subset of `image-service-rs`'s three-stage pattern (ImageMagick stage omitted — not needed here).

---

## Cargo.toml dependency plan

```toml
[package]
name = "registry-mcp"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "registry-mcp"
path = "src/main.rs"

[dependencies]
rmcp               = { version = "1.4.0", features = ["server", "transport-streamable-http-server"] }
axum               = { version = "0.8", features = ["http1", "tokio"] }
tokio              = { version = "1", features = ["full"] }
tokio-util         = "0.7"   # CancellationToken is in default features; "sync" feature flag does not exist
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
schemars           = "1"                                         # JsonSchema derive for tool params
reqwest            = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }
bollard            = "0.17"
anyhow             = "1"
thiserror          = "2"
tracing            = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
futures-util       = "0.3"
url                = "2"

[profile.release]
opt-level     = "s"
strip         = true
codegen-units = 1
lto           = true
```

`aws-sdk-s3` is excluded from the initial implementation (no direct S3 access needed).
