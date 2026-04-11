# registry-mcp

An MCP (Model Context Protocol) server for managing a Docker Distribution / OCI registry. Exposes registry operations as tools that an LLM can call — browsing repositories and tags, inspecting manifests, deleting tags, and running garbage collection.

## Tools

| Tool | Description |
|---|---|
| `list_repositories` | Paginated list of repository names from `_catalog` |
| `list_tags` | Paginated list of tags for a repository |
| `get_manifest` | Manifest details: media type, layers, total compressed size |
| `get_repository_disk_usage` | Aggregate blob footprint for all tags, deduplicated |
| `delete_tag` | Soft-delete a manifest by tag (dry-run by default, `confirm: true` to execute) |
| `run_gc` | Run garbage collection (dry-run by default) |

## Running

### Docker

```bash
docker run -p 3000:3000 \
  -e registry__baseUrl=https://registry.example.com \
  -e registry__username=myuser \
  -e registry__password=mypassword \
  ghcr.io/hikari-systems/registry-mcp:latest
```

Mount a config file for more complete configuration (see [Configuration](#configuration)):

```bash
docker run -p 3000:3000 \
  -v /path/to/config.json:/sandbox/config.json \
  ghcr.io/hikari-systems/registry-mcp:latest
```

For Docker-based GC, also mount the Docker socket:

```bash
docker run -p 3000:3000 \
  -v /path/to/config.json:/sandbox/config.json \
  -v /var/run/docker.sock:/var/run/docker.sock \
  ghcr.io/hikari-systems/registry-mcp:latest
```

### docker-compose

```yaml
services:
  registry-mcp:
    image: ghcr.io/hikari-systems/registry-mcp:latest
    ports:
      - "3000:3000"
    volumes:
      - /path/to/configs/registry-mcp:/sandbox
      # Uncomment for Docker-based GC:
      # - /var/run/docker.sock:/var/run/docker.sock
```

### Binary

```bash
# From a config file in the working directory:
CONFIG_PATH=/etc/registry-mcp/config.json ./registry-mcp

# Or via env vars:
registry__baseUrl=https://registry.example.com ./registry-mcp
```

---

## Configuration

Configuration is loaded in priority order (lowest → highest):

1. `config.json` in the working directory (baked into the Docker image as defaults)
2. `/sandbox/config.json` — deep-merged on top; intended for secrets and environment-specific values
3. Environment variables using `__` as a path separator with exact camelCase key names

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
    "registryImage": "registry:3",
    "dockerNetwork": ""
  },
  "log": {
    "level": "info"
  }
}
```

### Field reference

**`server.host`** — bind address (default `0.0.0.0`)

**`server.port`** — TCP port (default `3000`); env override: `server__port=3000`

**`registry.baseUrl`** *(required)* — base URL of the registry, e.g. `https://registry.example.com`. No trailing slash.

**`registry.username` / `registry.password`** — HTTP Basic auth credentials. Used when `bearerToken` is empty.

**`registry.bearerToken`** — static bearer token. Takes precedence over Basic auth when non-empty.

**`registry.insecureSkipVerify`** — skip TLS certificate verification. For self-signed registries only.

**`registry.caCertFile`** — path to a PEM CA certificate to add to the trust store.

**`gc.scriptPath`** — absolute path to a shell script for GC. Takes priority over Docker-based GC when set (see [Garbage Collection](#garbage-collection)).

**`gc.registryConfigPath`** — path to a registry `config.yml` to mount into the GC container. Required for Docker-based GC.

**`gc.dockerSocket`** — Docker socket path (default `/var/run/docker.sock`).

**`gc.registryImage`** — registry image for Docker-based GC (default `registry:3`).

**`gc.dockerNetwork`** — Docker network to attach the GC container to. Empty uses Docker's default bridge. Set to `host` if the GC container needs to reach a MinIO or other storage backend exposed on the host.

**`log.level`** — tracing filter: `error`, `warn`, `info`, `debug`, `trace` (default `info`)

### Environment variable examples

```bash
registry__baseUrl=https://registry.example.com
registry__username=admin
registry__password=secret
gc__registryConfigPath=/etc/registry/config.yml
log__level=debug
```

---

## Garbage Collection

The `run_gc` tool supports two strategies, resolved in priority order:

```
gc.scriptPath set and file exists  →  Shell script
gc.registryConfigPath set and file exists  →  Docker container
neither                            →  Unavailable (tool returns a message, no error)
```

Strategy is resolved at call time, not startup — you can add or remove the script without restarting the server.

### Docker strategy (bollard)

The server uses [bollard](https://github.com/fussybeaver/bollard) to manage a GC container lifecycle entirely from Rust:

1. Checks if `gc.registryImage` (`registry:3` by default) is present locally; pulls if not
2. Creates a container with `gc.registryConfigPath` mounted at `/etc/docker/registry/config.yml`
3. Runs `/bin/registry garbage-collect /etc/docker/registry/config.yml [--dry-run] [--delete-untagged]`
4. Streams log output line-by-line as MCP progress notifications
5. Waits for exit and captures the exit code
6. Removes the container (always, even if GC failed)

The registry `config.yml` must point at the same storage backend your live registry uses. Example for S3:

```yaml
version: 0.1
storage:
  s3:
    accesskey: AKIAIOSFODNN7EXAMPLE
    secretkey: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
    bucket: my-registry-bucket
    region: us-east-1
  delete:
    enabled: true
```

`delete.enabled: true` is required for `delete_tag` to work. Without it, the registry returns `405 Method Not Allowed`.

The GC container does not run a registry HTTP listener — `REGISTRY_HTTP_ADDR` is intentionally cleared.

**Network access:** the GC container runs on Docker's default bridge network. If your storage backend (e.g. MinIO) is only reachable on the host, set `gc.dockerNetwork` to `host`.

### Shell script strategy

Set `gc.scriptPath` to the absolute path of an executable script. The server invokes it as:

```
<scriptPath> [--dry-run] [--delete-untagged]
```

The following environment variables are forwarded to the script:

| Variable | Value |
|---|---|
| `REGISTRY_URL` | `config.registry.baseUrl` |
| `REGISTRY_USERNAME` | `config.registry.username` |
| `REGISTRY_PASSWORD` | `config.registry.password` |
| `DRY_RUN` | `true` or `false` |
| `DELETE_UNTAGGED` | `true` or `false` |

stdout and stderr are captured in full and returned in the tool response.

---

## Connecting to Claude or ChatGPT

The server uses the [Streamable HTTP](https://spec.modelcontextprotocol.io/specification/basic/transports/#streamable-http) MCP transport. Once running, the MCP endpoint is:

```
http://<host>:<port>/mcp
```

LLM providers need a **publicly reachable HTTPS URL**. Use [ngrok](https://ngrok.com) to expose a local instance during development.

### ngrok setup

```bash
# Install ngrok (https://ngrok.com/download), then:
ngrok http 3000
```

ngrok prints a forwarding URL like:

```
Forwarding  https://abc123.ngrok-free.app -> http://localhost:3000
```

Your MCP URL is:

```
https://abc123.ngrok-free.app/mcp
```

### Registering with Claude.ai

1. Go to **claude.ai → Settings → Integrations**
2. Click **Add integration**
3. Enter the MCP URL: `https://abc123.ngrok-free.app/mcp`
4. Save — Claude will discover the available tools automatically

### Registering with ChatGPT (Actions / Connectors)

1. Go to **platform.openai.com → My GPTs** (or a custom GPT editor)
2. Under **Actions**, click **Add action**
3. Set the server URL to `https://abc123.ngrok-free.app/mcp`
4. OpenAI will fetch the tool schema and expose the tools to the model

> **Note:** Keep the ngrok session running while using the integration. The free tier generates a new URL on each restart — update the registration URL if it changes.

### Permanent deployment

For a stable URL, run the container behind a reverse proxy (nginx, Caddy, Traefik) with a real TLS certificate, or deploy behind an AWS ALB / CloudFront distribution.

---

## Building from source

```bash
cargo build --release
./target/release/registry-mcp
```

Requires Rust 1.85+ (edition 2024).

## Running integration tests

The GC integration tests require Docker and the test compose stack:

```bash
docker compose -f tests/docker-compose.test.yml up -d --wait
REGISTRY_INTEGRATION_TEST=1 cargo test --test gc_integration -- --nocapture
docker compose -f tests/docker-compose.test.yml down -v
```

The compose stack starts MinIO (S3-compatible storage) and a `registry:3` instance backed by it, with deletions enabled.
