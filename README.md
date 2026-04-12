# registry-mcp

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

An MCP (Model Context Protocol) server for managing a Docker Distribution / OCI registry. Exposes registry operations as tools that an LLM can call — browsing repositories and tags, inspecting manifests, deleting tags, and migrating images.

Licensed under the [Apache License 2.0](LICENSE).

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

### docker-compose

```yaml
services:
  registry-mcp:
    image: ghcr.io/hikari-systems/registry-mcp:latest
    ports:
      - "3000:3000"
    volumes:
      - /path/to/configs/registry-mcp:/sandbox
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
  "log": {
    "level": "info"
  }
}
```

### Environment variable examples

All config keys can be set via `-e` using `__` as the depth separator and exact camelCase names:

```bash
# Basic auth
docker run -p 3000:3000 \
  -e registry__baseUrl=https://registry.example.com \
  -e registry__username=myuser \
  -e registry__password=mypassword \
  ghcr.io/hikari-systems/registry-mcp:latest

# Static bearer token instead of basic auth
docker run -p 3000:3000 \
  -e registry__baseUrl=https://registry.example.com \
  -e registry__bearerToken=mytoken \
  ghcr.io/hikari-systems/registry-mcp:latest

# Self-signed certificate registry
docker run -p 3000:3000 \
  -e registry__baseUrl=https://registry.internal \
  -e registry__username=myuser \
  -e registry__password=mypassword \
  -e registry__insecureSkipVerify=true \
  ghcr.io/hikari-systems/registry-mcp:latest

# Change log level and bind port
docker run -p 8080:8080 \
  -e registry__baseUrl=https://registry.example.com \
  -e server__port=8080 \
  -e log__level=debug \
  ghcr.io/hikari-systems/registry-mcp:latest
```

### Field reference

Keys use `:` as a depth separator, reflecting the JSON structure. When setting a key via an environment variable, replace `:` with `__` — e.g. `registry:baseUrl` becomes `registry__baseUrl=https://...`.

| Key | Default | Description |
|---|---|---|
| `server:host` | `0.0.0.0` | Bind address |
| `server:port` | `3000` | TCP port |
| `registry:baseUrl` | *(required)* | Base URL of the registry, e.g. `https://registry.example.com`. No trailing slash. |
| `registry:username` | `""` | HTTP Basic auth username. Used when `registry:bearerToken` is empty. |
| `registry:password` | `""` | HTTP Basic auth password. |
| `registry:bearerToken` | `""` | Static bearer token. Takes precedence over Basic auth when non-empty. |
| `registry:insecureSkipVerify` | `false` | Skip TLS certificate verification. For self-signed registries only. |
| `registry:caCertFile` | `""` | Path to a PEM CA certificate to add to the trust store. |
| `log:level` | `info` | Tracing filter: `error`, `warn`, `info`, `debug`, `trace`. |

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

## Tools

| Tool | Description |
|---|---|
| `list_repositories` | Paginated list of repository names from `_catalog` |
| `list_tags` | Paginated list of tags for a repository |
| `get_manifest` | Manifest details: media type, layers, total compressed size |
| `get_repository_disk_usage` | Aggregate blob footprint for all tags, deduplicated |
| `tag_manifest` | Create a new tag pointing at an existing tag or digest (equivalent to `docker tag`) |
| `untag` | Remove one or more tag references (up to 20) without deleting the manifest or other tags pointing to the same digest |
| `delete_tag` | Delete a manifest entirely by tag (dry-run by default, `confirm: true` to execute) |
| `migrate` | Pull an image from an external registry and push it into this registry — no Docker daemon required, full multi-arch support |

---

### `list_repositories`

Returns all repository names visible to the configured credentials. Results are paginated — pass `last` from one response as the cursor for the next page.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `last` | no | Pagination cursor — last repository name from the previous response |
| `limit` | no | Page size, 1–1000. Defaults to 100 |

**Example response**

```json
{
  "repositories": [
    "library/nginx",
    "library/postgres",
    "myapp/api",
    "myapp/worker"
  ],
  "next_last": "myapp/worker",
  "total_returned": 4
}
```

To fetch the next page, call again with `last: "myapp/worker"`. When `total_returned` is less than `limit`, you have reached the end.

---

### `list_tags`

Returns all tags for a single repository. Pagination works the same way as `list_repositories`.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `repository` | yes | Repository name, e.g. `library/nginx` |
| `last` | no | Pagination cursor — last tag name from the previous response |
| `limit` | no | Page size, 1–1000. Defaults to 100 |

**Example response**

```json
{
  "repository": "myapp/api",
  "tags": ["1.0.0", "1.1.0", "1.2.0", "latest", "stable"],
  "next_last": "stable",
  "total_returned": 5
}
```

---

### `get_manifest`

Fetches the manifest for a tag or digest and returns its structure. For multi-platform image indexes the child manifests are listed with their platform details. For single-platform manifests the config blob and all layers are listed individually.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `repository` | yes | Repository name, e.g. `library/nginx` |
| `reference` | yes | Tag name or digest, e.g. `latest` or `sha256:abc123…` |

**Example — single-platform image**

```json
{
  "repository": "myapp/api",
  "reference": "1.2.0",
  "digest": "sha256:d4e5f6...",
  "media_type": "application/vnd.oci.image.manifest.v1+json",
  "schema_version": 2,
  "total_size_bytes": 52428800,
  "is_image_index": false,
  "config": {
    "digest": "sha256:a1b2c3...",
    "media_type": "application/vnd.oci.image.config.v1+json",
    "size_bytes": 4096
  },
  "layers": [
    {
      "digest": "sha256:111aaa...",
      "media_type": "application/vnd.oci.image.layer.v1.tar+gzip",
      "size_bytes": 29360128
    },
    {
      "digest": "sha256:222bbb...",
      "media_type": "application/vnd.oci.image.layer.v1.tar+gzip",
      "size_bytes": 23064576
    }
  ]
}
```

**Example — multi-platform image index**

```json
{
  "repository": "library/nginx",
  "reference": "latest",
  "digest": "sha256:4724b8...",
  "media_type": "application/vnd.docker.distribution.manifest.list.v2+json",
  "schema_version": 2,
  "total_size_bytes": 0,
  "is_image_index": true,
  "manifests": [
    {
      "digest": "sha256:amd64digest...",
      "media_type": "application/vnd.docker.distribution.manifest.v2+json",
      "size_bytes": 1234,
      "platform": { "architecture": "amd64", "os": "linux" }
    },
    {
      "digest": "sha256:arm64digest...",
      "media_type": "application/vnd.docker.distribution.manifest.v2+json",
      "size_bytes": 1234,
      "platform": { "architecture": "arm64", "os": "linux", "variant": "v8" }
    }
  ]
}
```

---

### `get_repository_disk_usage`

Calculates the total on-disk blob footprint for all tags in a repository. Blobs shared between tags (common base layers) are counted only once in the unique total. Multi-platform image indexes are recursed into so every platform's layers are included. Tags that disappear mid-flight (deleted concurrently) are listed in `skipped_tags` rather than causing the whole call to fail.

This is useful to understand how much storage is in use or to identify which tags are contributing the most to storage costs.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `repository` | yes | Repository name, e.g. `myapp/api` |

**Example response**

```json
{
  "repository": "myapp/api",
  "unique_blob_count": 8,
  "unique_size_bytes": 157286400,
  "tags": [
    {
      "tag": "1.0.0",
      "digest": "sha256:aaa111...",
      "layer_count": 4,
      "total_size_bytes": 83886080
    },
    {
      "tag": "1.1.0",
      "digest": "sha256:bbb222...",
      "layer_count": 4,
      "total_size_bytes": 88080384
    },
    {
      "tag": "latest",
      "digest": "sha256:bbb222...",
      "layer_count": 4,
      "total_size_bytes": 88080384
    }
  ],
  "skipped_tags": []
}
```

Note that `latest` and `1.1.0` point to the same digest — their blobs are counted once in `unique_size_bytes` even though both tags appear in the per-tag breakdown.

---

### `tag_manifest`

Creates a new tag pointing at the same manifest as an existing tag or digest. Equivalent to `docker tag` — no layer data is copied, only the manifest reference is written. This is instant regardless of image size.

`source` can be a tag name (`latest`, `1.2.0`) or a full digest (`sha256:abc123…`).

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `repository` | yes | Repository name, e.g. `myapp/api` |
| `source` | yes | Existing tag or digest to point the new tag at |
| `new_tag` | yes | Tag name to create or overwrite |

**Example — promote staging to production**

```json
{
  "repository": "myapp/api",
  "source": "1.2.0",
  "new_tag": "stable"
}
```

**Example response**

```json
{
  "repository": "myapp/api",
  "source": "1.2.0",
  "new_tag": "stable",
  "digest": "sha256:d4e5f6...",
  "message": "Tag 'stable' created pointing to sha256:d4e5f6..."
}
```

---

### `untag`

Removes one or more tag references (up to 20) without touching the underlying manifests. Other tags pointing at the same digest are unaffected. The manifests themselves — and any tags still referencing them — remain fully intact and pullable.

Progress notifications are emitted as each tag is removed. Tags that fail are collected in `failed` rather than aborting the whole call.

This is useful for bulk-cleaning temporary or CI tags (`pr-123`, `branch-main`) without disrupting `latest` or version tags that point to the same image.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `repository` | yes | Repository name, e.g. `myapp/api` |
| `tags` | yes | Array of tag names to remove (1–20) |

**Example — remove several CI branch tags**

```json
{
  "repository": "myapp/api",
  "tags": ["branch-feature-x", "branch-feature-y", "pr-42"]
}
```

**Example response**

```json
{
  "repository": "myapp/api",
  "removed": ["branch-feature-x", "branch-feature-y", "pr-42"],
  "failed": [],
  "message": "3 tag(s) removed from myapp/api."
}
```

If some tags fail (e.g. already deleted), the successful removals are still applied and both lists are populated:

```json
{
  "repository": "myapp/api",
  "removed": ["branch-feature-x", "branch-feature-y"],
  "failed": [{ "tag": "pr-42", "error": "not found: myapp/api:pr-42" }],
  "message": "2 tag(s) removed, 1 failed from myapp/api."
}
```

Contrast with `delete_tag`: `untag` removes tag references only; `delete_tag` resolves the tag to a digest and deletes the manifest entirely, which also removes all other tags pointing to it.

---

### `delete_tag`

Deletes a manifest from the registry by resolving the tag to its digest and issuing a manifest delete. This removes the manifest and makes it unreachable by any tag — including any other tags that happened to point at the same digest.

Protected by a `confirm` guard: the default call is a dry run that resolves and reports the digest without deleting anything. Set `confirm: true` to perform the actual deletion.

> **Note:** deleting a manifest removes the manifest object but the underlying layer blobs remain in storage until garbage collection is run. See [Garbage Collection](#garbage-collection).

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `repository` | yes | Repository name, e.g. `myapp/api` |
| `tag` | yes | Tag to delete |
| `confirm` | no | Set to `true` to execute. Defaults to `false` (dry run) |

**Example — dry run first**

```json
{ "repository": "myapp/api", "tag": "1.0.0" }
```

Response:

```json
{
  "repository": "myapp/api",
  "tag": "1.0.0",
  "digest": "sha256:aaa111...",
  "deleted": false,
  "message": "Dry run — set confirm: true to delete manifest sha256:aaa111... (tag: 1.0.0 in myapp/api)"
}
```

**Then confirm**

```json
{ "repository": "myapp/api", "tag": "1.0.0", "confirm": true }
```

Response:

```json
{
  "repository": "myapp/api",
  "tag": "1.0.0",
  "digest": "sha256:aaa111...",
  "deleted": true,
  "message": "Manifest sha256:aaa111... deleted."
}
```

---

### `migrate`

Copies an image from any OCI-compatible registry into the managed registry using the OCI Distribution API directly. No Docker daemon is required. Supports single-platform images and multi-arch image indexes — for multi-arch, every platform's layers are copied and the index manifest is reassembled at the destination.

Blobs already present at the destination are skipped, so re-running a migration is safe and fast.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `source` | yes | Full image reference — see format table below |
| `target_repository` | yes | Repository in this registry to push to, e.g. `library/nginx` |
| `target_tag` | yes | Tag to create or overwrite, e.g. `latest` |
| `source_username` | no | Username for private source registries |
| `source_password` | no | Password or token for private source registries |

**Source reference format**

| Example | Interpreted as |
|---|---|
| `nginx` | Docker Hub `library/nginx:latest` |
| `nginx:1.25` | Docker Hub `library/nginx:1.25` |
| `myorg/myimage:v1` | Docker Hub `myorg/myimage:v1` |
| `registry.example.com/myimage:v2` | `registry.example.com` / `myimage:v2` |
| `registry.example.com:5000/myimage:v2` | `registry.example.com:5000` / `myimage:v2` |
| `myimage@sha256:abc…` | Docker Hub, pinned by digest |

**Example — mirror a public image**

```json
{
  "source": "nginx:1.25-alpine",
  "target_repository": "mirrors/nginx",
  "target_tag": "1.25-alpine"
}
```

Response:

```json
{
  "source": "nginx:1.25-alpine",
  "target_repository": "mirrors/nginx",
  "target_tag": "1.25-alpine",
  "digest": "sha256:4724b8...",
  "is_multi_arch": true,
  "manifests_copied": 16,
  "blobs_copied": 26,
  "blobs_skipped": 0,
  "requires_auth": false,
  "message": "Migrated to mirrors/nginx/1.25-alpine. 16 manifest(s), 26 blob(s) copied, 0 already present."
}
```

**Example — copy from a private registry**

```json
{
  "source": "registry.example.com/myapp/api:v2.3",
  "target_repository": "myapp/api",
  "target_tag": "v2.3",
  "source_username": "robot$myapp",
  "source_password": "secret"
}
```

**Auth soft-failure**

If the source registry requires credentials and none are provided, the tool returns a successful result (not an error) with `requires_auth: true`:

```json
{
  "requires_auth": true,
  "message": "Source registry requires authentication. Retry with source_username and source_password.",
  ...
}
```

This allows the LLM to prompt the user for credentials and retry rather than surfacing an opaque error.

---

## Building from source

```bash
cargo build --release
./target/release/registry-mcp
```

Requires Rust 1.85+ (edition 2024).

## Running integration tests

The migrate integration tests require Docker and the test compose stack:

```bash
docker compose -f tests/docker-compose.test.yml up -d --wait
REGISTRY_INTEGRATION_TEST=1 cargo test --test migrate_integration -- --nocapture
docker compose -f tests/docker-compose.test.yml down -v
```

The compose stack starts MinIO (S3-compatible storage) and a `registry:3` instance backed by it, with deletions enabled.

---

## Garbage Collection

Docker Distribution uses a two-phase mark-and-sweep garbage collector. When you delete a tag or manifest via the API, only the reference is removed — the underlying layer blobs stay in storage. Disk space is not reclaimed until GC is run explicitly.

GC must be performed by the registry binary itself, pointed at the same storage backend as the live registry. You cannot simply delete blobs from S3 or a filesystem directly.

To run GC against a `registry:3` instance:

```bash
registry garbage-collect /etc/docker/registry/config.yml --delete-untagged
```

`storage.delete.enabled: true` must be present in the registry config, otherwise the registry API will also return `405 Method Not Allowed` for manifest deletes.

For full details on how the collector works and how to configure it, see the [official garbage collection documentation](https://distribution.github.io/distribution/about/garbage-collection/).
