# registry-mcp

An MCP (Model Context Protocol) server for managing a Docker Distribution / OCI registry. Exposes registry operations as tools that an LLM can call — browsing repositories and tags, inspecting manifests, deleting tags, and running garbage collection.

## Tools

| Tool | Description |
|---|---|
| `list_repositories` | Paginated list of repository names from `_catalog` |
| `list_tags` | Paginated list of tags for a repository |
| `get_manifest` | Manifest details: media type, layers, total compressed size |
| `get_repository_disk_usage` | Aggregate blob footprint for all tags, deduplicated |
| `tag_manifest` | Create a new tag pointing at an existing tag or digest (equivalent to `docker tag`) |
| `untag` | Remove a single tag reference without deleting the manifest or other tags pointing to the same digest |
| `delete_tag` | Delete a manifest entirely by tag (dry-run by default, `confirm: true` to execute) |
| `migrate` | Pull an image from an external registry and push it into this registry — no Docker daemon required, full multi-arch support |
| `run_gc` | Run garbage collection (dry-run by default) |

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

This is useful before running GC to understand how much storage is in use, or to identify which tags are contributing the most to storage costs.

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

Removes a single tag reference without touching the underlying manifest. Other tags pointing at the same digest are unaffected. The manifest itself — and any tags still referencing it — remain fully intact and pullable.

This is useful for cleaning up temporary or CI tags (`pr-123`, `branch-main`) without disrupting `latest` or version tags that point to the same image.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `repository` | yes | Repository name, e.g. `myapp/api` |
| `tag` | yes | Tag to remove |

**Example — remove a CI branch tag**

```json
{
  "repository": "myapp/api",
  "tag": "branch-feature-x"
}
```

**Example response**

```json
{
  "repository": "myapp/api",
  "tag": "branch-feature-x",
  "message": "Tag 'branch-feature-x' removed from myapp/api. The manifest is still present and reachable by digest or other tags."
}
```

Contrast with `delete_tag`: `untag` removes the tag reference only; `delete_tag` resolves the tag to a digest and deletes the manifest entirely, which also removes all other tags pointing to it.

---

### `delete_tag`

Deletes a manifest from the registry by resolving the tag to its digest and issuing a manifest delete. This removes the manifest and makes it unreachable by any tag — including any other tags that happened to point at the same digest.

Protected by a `confirm` guard: the default call is a dry run that resolves and reports the digest without deleting anything. Set `confirm: true` to perform the actual deletion.

> **Note:** deleting a manifest only removes the manifest object. The underlying layer blobs remain in storage until garbage collection is run. Use `run_gc` afterwards to reclaim disk space.

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
  "message": "Manifest sha256:aaa111... deleted. Run run_gc to reclaim blob storage."
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

### `run_gc`

Runs garbage collection on the registry to permanently delete blobs that are no longer referenced by any manifest. This is the step required after `delete_tag` to actually reclaim disk space.

Two strategies are supported, selected automatically:
- **Shell script** (`gc:scriptPath`) — invokes your own GC script, if configured
- **Docker container** (`gc:registryConfigPath`) — bollard spins up a short-lived `registry:3` container pointing at the same storage backend and runs `registry garbage-collect`. Log output is streamed as MCP progress notifications in real time.

Defaults to `dry_run: true` — always inspect the output before running for real.

See [Garbage Collection](#garbage-collection) for setup instructions.

**Parameters**

| Parameter | Required | Description |
|---|---|---|
| `dry_run` | no | Report what would be deleted without removing anything. Defaults to `true` |
| `delete_untagged` | no | Pass `--delete-untagged` to remove manifests with no tags. Defaults to `true` |

**Example — dry run (default)**

```json
{}
```

Response:

```json
{
  "strategy": "docker",
  "dry_run": true,
  "exit_code": 0,
  "stdout": "INFO[0000] Deleting blob: sha256:aaa111...\nINFO[0000] Deleting blob: sha256:bbb222...\n",
  "stderr": "",
  "message": ""
}
```

**Example — real GC run**

```json
{ "dry_run": false, "delete_untagged": true }
```

If GC is not configured, the response includes a message explaining what to set up rather than returning an error:

```json
{
  "strategy": "unavailable",
  "dry_run": true,
  "exit_code": null,
  "stdout": "",
  "stderr": "",
  "message": "No GC strategy is configured. To enable GC, set one of: gc.scriptPath (path to a shell script) or gc.registryConfigPath (path to a registry config.yml for Docker-based GC)."
}
```

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
  },
  "auth": {
    "enabled": false,
    "issuer": "",
    "jwksUri": "",
    "audience": "",
    "authorizationEndpoint": "",
    "tokenEndpoint": "",
    "registrationEndpoint": ""
  }
}
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
| `gc:scriptPath` | `""` | Absolute path to a shell script for GC. Takes priority over Docker-based GC when set. See [Garbage Collection](#garbage-collection). |
| `gc:registryConfigPath` | `""` | Path to a registry `config.yml` to mount into the GC container. Required for Docker-based GC. |
| `gc:dockerSocket` | `/var/run/docker.sock` | Docker socket path. |
| `gc:registryImage` | `registry:3` | Registry image used for Docker-based GC. |
| `gc:dockerNetwork` | `""` | Docker network for the GC container. Empty uses Docker's default bridge. Set to `host` if the GC container needs to reach a storage backend (e.g. MinIO) on the host. |
| `log:level` | `info` | Tracing filter: `error`, `warn`, `info`, `debug`, `trace`. |
| `auth:enabled` | `false` | When `false`, all MCP requests are accepted and attributed to `anonymous`. Set to `true` to require a valid Bearer JWT on every request. |
| `auth:issuer` | `""` | OAuth 2.0 issuer URL. Must match the `iss` claim in incoming JWTs and is advertised in `/.well-known/oauth-authorization-server`. |
| `auth:jwksUri` | `""` | URL of the JWKS endpoint used to fetch the public keys for JWT signature verification, e.g. `https://accounts.google.com/.well-known/jwks.json`. Keys are cached for one hour. |
| `auth:audience` | `""` | Expected `aud` claim value. Leave empty to skip audience validation. |
| `auth:authorizationEndpoint` | `""` | Advertised in `/.well-known/oauth-authorization-server`. The `/authorize` URL on the upstream AS. |
| `auth:tokenEndpoint` | `""` | Advertised in `/.well-known/oauth-authorization-server`. The `/token` URL on the upstream AS. |
| `auth:registrationEndpoint` | `""` | Advertised in `/.well-known/oauth-authorization-server`. The dynamic client registration URL (RFC 7591), if your AS supports it. Required for Claude to register automatically. |

---

## Authentication

registry-mcp implements the OAuth 2.0 Protected Resource pattern described in the MCP specification. When enabled, it validates a Bearer JWT on every request to `/mcp` and records the caller's identity in the audit log. The actual OAuth flow — user login, token issuance — is handled by an external Authorization Server (your own OAuth provider or any OIDC-compatible service).

### How it works

```
Claude / ChatGPT
  │
  ├─ GET /.well-known/oauth-authorization-server   ← discover AS endpoints
  ├─ redirect user → upstream AS for login
  ├─ receive authorization code → exchange for access token
  │
  └─ POST /mcp  Authorization: Bearer <access_token>
                     │
                     ├─ auth middleware validates JWT (JWKS, iss, aud, exp)
                     ├─ user identity stamped into session (sub, email, display_name)
                     └─ tool call proceeds; identity logged in audit output
```

Two discovery endpoints are always served at the server root:

| Endpoint | Standard | Notes |
|---|---|---|
| `GET /.well-known/oauth-authorization-server` | RFC 8414 | Advertises `authorization_endpoint`, `token_endpoint`, `registration_endpoint`. Returns 404 when `auth:enabled` is false. |
| `GET /.well-known/oauth-protected-resource` | RFC 9396 | Declares this server as a protected resource. Always served, even when auth is disabled. |

### Setting up with an OIDC provider

Any provider that issues RS256 JWTs and exposes a JWKS endpoint works. The configuration below uses generic field names — substitute the actual URLs from your provider's discovery document (`/.well-known/openid-configuration`).

```json
{
  "auth": {
    "enabled": true,
    "issuer": "https://auth.example.com",
    "jwksUri": "https://auth.example.com/.well-known/jwks.json",
    "audience": "registry-mcp",
    "authorizationEndpoint": "https://auth.example.com/oauth/authorize",
    "tokenEndpoint": "https://auth.example.com/oauth/token",
    "registrationEndpoint": "https://auth.example.com/oauth/register"
  }
}
```

Or via environment variables:

```bash
auth__enabled=true
auth__issuer=https://auth.example.com
auth__jwksUri=https://auth.example.com/.well-known/jwks.json
auth__audience=registry-mcp
auth__authorizationEndpoint=https://auth.example.com/oauth/authorize
auth__tokenEndpoint=https://auth.example.com/oauth/token
auth__registrationEndpoint=https://auth.example.com/oauth/register
```

### Common provider examples

**Google OAuth 2.0**

```json
{
  "auth": {
    "enabled": true,
    "issuer": "https://accounts.google.com",
    "jwksUri": "https://www.googleapis.com/oauth2/v3/certs",
    "audience": "<your-google-client-id>",
    "authorizationEndpoint": "https://accounts.google.com/o/oauth2/v2/auth",
    "tokenEndpoint": "https://oauth2.googleapis.com/token",
    "registrationEndpoint": ""
  }
}
```

Google does not support dynamic client registration, so `registrationEndpoint` should be left empty. You will need to register the Claude or ChatGPT redirect URI in the Google Cloud Console manually.

**Keycloak**

```json
{
  "auth": {
    "enabled": true,
    "issuer": "https://keycloak.example.com/realms/myrealm",
    "jwksUri": "https://keycloak.example.com/realms/myrealm/protocol/openid-connect/certs",
    "audience": "registry-mcp",
    "authorizationEndpoint": "https://keycloak.example.com/realms/myrealm/protocol/openid-connect/auth",
    "tokenEndpoint": "https://keycloak.example.com/realms/myrealm/protocol/openid-connect/token",
    "registrationEndpoint": "https://keycloak.example.com/realms/myrealm/clients-registrations/openid-connect"
  }
}
```

Enable "Client Registration" under Realm Settings in the Keycloak admin UI.

### Dynamic client registration (Claude)

Claude requires an OAuth client to be registered before it can complete the authorization flow. If your AS exposes a `registrationEndpoint` (RFC 7591), Claude registers itself automatically when you add the integration — no manual step required.

If your AS does not support dynamic registration (e.g. Google), you must register a client manually and configure the provider with Claude's redirect URI before adding the integration:

1. Create an OAuth client in your provider's console
2. Add Claude's redirect URI (displayed during integration setup)
3. Leave `registrationEndpoint` empty in the config

### PKCE

Claude and ChatGPT both use the Authorization Code flow with PKCE (RFC 7636, `S256`). The `/.well-known/oauth-authorization-server` response advertises `"code_challenge_methods_supported": ["S256"]`. Your AS must support PKCE — all modern providers do.

### Public URL

The `/.well-known` endpoints and the auth redirect flow require that registry-mcp be reachable at a stable public HTTPS URL. Set the `server__publicUrl` environment variable if the server is running behind a reverse proxy:

```bash
server__publicUrl=https://registry-mcp.example.com
```

This value is returned in the `resource` field of `/.well-known/oauth-protected-resource`. If not set it defaults to `http://<server.host>:<server.port>`, which is only suitable for local development.

### Audit logging

Every tool call emits a structured log line at `INFO` level to the `audit` target regardless of whether authentication is enabled:

```
INFO audit: user_sub="alice@example.com" user_email=Some("alice@example.com") op="delete_tag" repository="myapp/api" tag="v1.0" confirm=Some(true)
```

When auth is disabled, `user_sub` is `"anonymous"`. The fields logged per operation are:

| Tool | Extra fields |
|---|---|
| `list_repositories` | — |
| `list_tags` | `repository` |
| `get_manifest` | `repository`, `reference` |
| `get_repository_disk_usage` | `repository` |
| `delete_tag` | `repository`, `tag`, `confirm` |
| `untag` | `repository`, `tag` |
| `tag_manifest` | `repository`, `source`, `new_tag` |
| `run_gc` | `dry_run`, `delete_untagged` |
| `migrate` | `source` |

To capture audit logs separately from application logs, filter on the `audit` target in your log aggregation stack (e.g. with a `tracing_subscriber` env filter of `audit=info`).

### Token validation details

- **Algorithm:** any algorithm supported by your AS; the key is selected by `kid` from the JWKS. Falls back to the first key with a matching algorithm if `kid` is absent.
- **Claims verified:** `iss` (must match `auth:issuer`), `aud` (must match `auth:audience` if set), `exp` (expiry enforced).
- **Identity claims read:** `sub` (always), `email`, `preferred_username`, `name`.
- **JWKS caching:** keys are fetched once and cached for one hour. A JWKS refresh is triggered automatically if no matching key is found for a new `kid`.
- **On validation failure:** the server returns `401 Unauthorized` with `WWW-Authenticate: Bearer error="invalid_token"`. The MCP session is not established.

---

## Garbage Collection

### How registry GC works

Docker Distribution (the software behind `registry:3`) uses a two-phase mark-and-sweep garbage collector. When you delete a tag via the registry API, only the manifest reference is removed — the underlying layer blobs remain in storage. GC is the process that walks all remaining manifests, identifies every blob still referenced, and deletes everything else.

The critical constraint is that **GC must be run by the registry binary itself**, pointed at the same storage backend as the live registry. You cannot simply delete files from S3 or a filesystem directly — the registry binary understands the content-addressable blob graph and is the only thing that can safely determine which blobs are unreferenced.

This means running GC requires spinning up a `registry:3` process with:
- The same storage backend configuration as the live registry (same S3 bucket, same credentials, same endpoint)
- `storage.delete.enabled: true` in the config
- Access to the storage backend over the network

registry-mcp handles this automatically via the Docker strategy described below.

### Strategy selection

```
gc:scriptPath set and file exists          →  Shell script
gc:registryConfigPath set and file exists  →  Docker container (bollard)
neither                                    →  Unavailable (tool returns a message, not an error)
```

Strategy is resolved at call time, not startup — you can add or remove the script file without restarting the server.

---

### Docker strategy (bollard)

[bollard](https://github.com/fussybeaver/bollard) is the Rust Docker API client used to manage the GC container lifecycle entirely from within registry-mcp. No shell, no `docker` CLI — the container is created, started, monitored, and removed via the Docker API over the socket.

**Lifecycle:**

1. Inspects the local Docker daemon for `gc:registryImage` (`registry:3` by default); pulls if not present
2. Creates a short-lived container with:
   - `gc:registryConfigPath` bind-mounted read-only at `/etc/docker/registry/config.yml`
   - `REGISTRY_HTTP_ADDR` cleared (the container runs GC only — no HTTP listener)
   - `gc:dockerNetwork` applied if set
3. Runs `/bin/registry garbage-collect /etc/docker/registry/config.yml [--dry-run] [--delete-untagged]`
4. Streams log output line-by-line as MCP progress notifications in real time
5. Waits for the process to exit and captures the exit code
6. Removes the container — always, even if GC failed

The tool response includes `strategy`, `dry_run`, `exit_code`, `stdout`, and `stderr` regardless of success or failure.

#### Giving registry-mcp access to the Docker daemon

bollard connects to the socket at `gc:dockerSocket` (default `/var/run/docker.sock`). When running registry-mcp in a container, the socket must be bind-mounted:

```bash
docker run -p 3000:3000 \
  -v /path/to/gc-config.yml:/etc/registry-mcp/gc-config.yml \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -e gc__registryConfigPath=/etc/registry-mcp/gc-config.yml \
  ghcr.io/hikari-systems/registry-mcp:latest
```

Or in docker-compose:

```yaml
services:
  registry-mcp:
    image: ghcr.io/hikari-systems/registry-mcp:latest
    volumes:
      - /path/to/configs/registry-mcp:/sandbox
      - /var/run/docker.sock:/var/run/docker.sock
```

On Linux the Docker socket is owned by `root:docker`. The registry-mcp container runs as `nobody` — if you see permission errors on the socket, either add the container user to the `docker` group or adjust socket permissions on the host.

#### Creating the GC config from a live registry

The GC container needs a `config.yml` that mirrors the live registry's storage configuration. If your live registry is configured entirely via environment variables (a common pattern with docker-compose), you need to write an equivalent `config.yml` file and make it available to registry-mcp.

Example — if your live registry is configured like this:

```yaml
# docker-compose.yml (live registry)
services:
  registry:
    image: registry:3
    environment:
      REGISTRY_STORAGE: s3
      REGISTRY_STORAGE_S3_ACCESSKEY: AKIAIOSFODNN7EXAMPLE
      REGISTRY_STORAGE_S3_SECRETKEY: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
      REGISTRY_STORAGE_S3_BUCKET: my-registry-bucket
      REGISTRY_STORAGE_S3_REGION: us-east-1
      REGISTRY_STORAGE_DELETE_ENABLED: "true"
```

The equivalent `config.yml` to give to the GC container is:

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

Save this file somewhere accessible to registry-mcp (e.g. `/etc/registry-mcp/gc-config.yml` or inside the `/sandbox` mount) and set:

```bash
gc__registryConfigPath=/etc/registry-mcp/gc-config.yml
```

`storage.delete.enabled: true` must be present in the GC config. Without it the registry binary refuses to delete anything, and `delete_tag` also returns `405 Method Not Allowed` from the live registry.

#### Network access for the GC container

The GC container is created on Docker's default bridge network. It needs to be able to reach the storage backend (S3, MinIO, etc.) over the network.

- **AWS S3:** no special network config needed — outbound HTTPS to AWS works from the default bridge.
- **Self-hosted MinIO on the same host:** the GC container cannot reach `localhost` on the host from inside the default bridge. Set `gc:dockerNetwork` to `host` to use host networking, then use `http://127.0.0.1:<minio-port>` as the endpoint in the GC config.
- **MinIO in the same docker-compose stack:** set `gc:dockerNetwork` to the compose network name (typically `<project>_default`) so the GC container can resolve the MinIO service by hostname.

---

### Shell script strategy

Set `gc:scriptPath` to the absolute path of an executable script. The server invokes it as:

```
<scriptPath> [--dry-run] [--delete-untagged]
```

The following environment variables are forwarded to the script:

| Variable | Value |
|---|---|
| `REGISTRY_URL` | `registry:baseUrl` |
| `REGISTRY_USERNAME` | `registry:username` |
| `REGISTRY_PASSWORD` | `registry:password` |
| `DRY_RUN` | `true` or `false` |
| `DELETE_UNTAGGED` | `true` or `false` |

stdout and stderr are captured in full and returned in the tool response. Unlike the Docker strategy, there is no incremental streaming — output is returned only after the script exits.

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
