use std::sync::Arc;

use anyhow::Result;
use reqwest::{
    Client, Response, StatusCode,
    header::{ACCEPT, AUTHORIZATION, HeaderValue},
};

use crate::{
    config::RegistryConfig,
    error::RegistryError,
    types::{
        CatalogResponse, MEDIA_TYPE_DOCKER_MANIFEST_LIST,
        MEDIA_TYPE_OCI_INDEX, RawIndex, RawManifest, TagsResponse,
    },
};

use super::auth::{
    TokenCache, cache_token, clear_token, fetch_bearer_token, parse_bearer_challenge,
    resolve_auth_header,
};

/// Accept header value covering all manifest media types the server understands.
const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, \
    application/vnd.oci.image.index.v1+json, \
    application/vnd.docker.distribution.manifest.v2+json, \
    application/vnd.docker.distribution.manifest.list.v2+json, \
    application/json";

pub struct RegistryClient {
    client: Client,
    base_url: String,
    cfg: RegistryConfig,
    token_cache: Arc<TokenCache>,
}

impl RegistryClient {
    pub fn new(cfg: RegistryConfig) -> Result<Self> {
        let mut builder = Client::builder().use_rustls_tls();

        if cfg.insecure_skip_verify {
            tracing::warn!("TLS certificate verification is disabled — insecureSkipVerify=true");
            builder = builder.danger_accept_invalid_certs(true);
        }

        if !cfg.ca_cert_file.is_empty() {
            let pem = std::fs::read(&cfg.ca_cert_file)?;
            let cert = reqwest::Certificate::from_pem(&pem)?;
            builder = builder.add_root_certificate(cert);
        }

        let client = builder.build()?;
        let base_url = cfg.base_url.trim_end_matches('/').to_string();

        Ok(Self {
            client,
            base_url,
            cfg,
            token_cache: TokenCache::new(),
        })
    }

    /// Execute a GET request, handling 401 → token fetch → retry automatically.
    async fn get(&self, url: &str, accept: &str) -> Result<Response, RegistryError> {
        let resp = self.get_once(url, accept).await?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            // Attempt to acquire a bearer token from the WWW-Authenticate challenge.
            let challenge = resp
                .headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_bearer_challenge);

            if let Some(token_url) = challenge {
                clear_token(&self.token_cache).await;
                match fetch_bearer_token(&self.client, &token_url, &self.cfg).await {
                    Ok(token) => {
                        cache_token(&self.token_cache, token).await;
                        // Retry with the new token.
                        return self.get_once(url, accept).await.and_then(check_status);
                    }
                    Err(e) => {
                        tracing::warn!("Bearer token fetch failed: {e}");
                    }
                }
            }
            return Err(RegistryError::Unauthorized);
        }

        check_status(resp)
    }

    async fn get_once(&self, url: &str, accept: &str) -> Result<Response, RegistryError> {
        let mut req = self.client.get(url).header(ACCEPT, accept);
        if let Some(auth) = resolve_auth_header(&self.cfg, &self.token_cache).await {
            req = req.header(AUTHORIZATION, HeaderValue::from_str(&auth)
                .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?);
        }
        req.send().await.map_err(RegistryError::Transport)
    }

    /// HEAD a manifest to obtain its digest without downloading the body.
    async fn head_manifest(&self, repository: &str, reference: &str) -> Result<String, RegistryError> {
        let url = format!("{}/v2/{}/manifests/{}", self.base_url, repository, reference);
        let mut req = self
            .client
            .head(&url)
            .header(ACCEPT, MANIFEST_ACCEPT);
        if let Some(auth) = resolve_auth_header(&self.cfg, &self.token_cache).await {
            req = req.header(AUTHORIZATION, HeaderValue::from_str(&auth)
                .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?);
        }
        let resp = req.send().await.map_err(RegistryError::Transport)?;

        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(RegistryError::Unauthorized);
        }
        let resp = check_status(resp)?;

        resp.headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| {
                RegistryError::InvalidResponse(
                    "manifest response missing Docker-Content-Digest header".into(),
                )
            })
    }

    /// Fetch the paginated repository catalog.
    pub async fn get_catalog(
        &self,
        last: &str,
        limit: u32,
    ) -> Result<CatalogResponse, RegistryError> {
        let mut url = format!("{}/v2/_catalog?n={}", self.base_url, limit);
        if !last.is_empty() {
            url.push_str("&last=");
            url.push_str(last);
        }
        let resp = self.get(&url, "application/json").await?;
        resp.json::<CatalogResponse>()
            .await
            .map_err(|e| RegistryError::InvalidResponse(e.to_string()))
    }

    /// Fetch the paginated tag list for a repository.
    pub async fn get_tags(
        &self,
        repository: &str,
        last: &str,
        limit: u32,
    ) -> Result<TagsResponse, RegistryError> {
        let mut url = format!("{}/v2/{}/tags/list?n={}", self.base_url, repository, limit);
        if !last.is_empty() {
            url.push_str("&last=");
            url.push_str(last);
        }
        let resp = self.get(&url, "application/json").await?;
        resp.json::<TagsResponse>()
            .await
            .map_err(|e| RegistryError::InvalidResponse(e.to_string()))
    }

    /// Fetch a manifest by tag or digest.  Returns either a single-platform
    /// manifest or an image index, plus the canonical digest from the response header.
    pub async fn get_manifest(
        &self,
        repository: &str,
        reference: &str,
    ) -> Result<ManifestResult, RegistryError> {
        let url = format!("{}/v2/{}/manifests/{}", self.base_url, repository, reference);
        let resp = self.get(&url, MANIFEST_ACCEPT).await?;

        let digest = resp
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(reference)
            .to_string();

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();

        let body = resp
            .text()
            .await
            .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?;

        let is_index = content_type == MEDIA_TYPE_OCI_INDEX
            || content_type == MEDIA_TYPE_DOCKER_MANIFEST_LIST;

        if is_index {
            let idx: RawIndex = serde_json::from_str(&body)
                .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?;
            return Ok(ManifestResult::Index { digest, inner: idx, media_type: content_type });
        }

        // Treat anything else as a single-platform manifest.
        let m: RawManifest = serde_json::from_str(&body)
            .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?;
        let mt = if content_type.is_empty() {
            m.media_type.clone()
        } else {
            content_type
        };
        Ok(ManifestResult::Single { digest, inner: m, media_type: mt })
    }

    /// Fetch the digest for a tag (HEAD request), then delete the manifest.
    /// Returns the resolved digest.
    pub async fn delete_manifest(
        &self,
        repository: &str,
        tag: &str,
    ) -> Result<String, RegistryError> {
        let digest = self.head_manifest(repository, tag).await?;
        let url = format!("{}/v2/{}/manifests/{}", self.base_url, repository, digest);

        let mut req = self.client.delete(&url);
        if let Some(auth) = resolve_auth_header(&self.cfg, &self.token_cache).await {
            req = req.header(AUTHORIZATION, HeaderValue::from_str(&auth)
                .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?);
        }
        let resp = req.send().await.map_err(RegistryError::Transport)?;
        check_status(resp)?;
        Ok(digest)
    }

    /// Fetch the digest for a tag without deleting (used for dry-run of delete_tag).
    pub async fn resolve_digest(
        &self,
        repository: &str,
        tag: &str,
    ) -> Result<String, RegistryError> {
        self.head_manifest(repository, tag).await
    }

    /// Delete a tag reference by name without touching the underlying manifest.
    /// Issues DELETE by tag name (not digest), so other tags pointing at the same
    /// manifest are unaffected.  Contrast with `delete_manifest`, which deletes by
    /// digest and removes the manifest entirely.
    pub async fn delete_tag_reference(
        &self,
        repository: &str,
        tag: &str,
    ) -> Result<(), RegistryError> {
        let url = format!("{}/v2/{}/manifests/{}", self.base_url, repository, tag);
        let mut req = self.client.delete(&url);
        if let Some(auth) = resolve_auth_header(&self.cfg, &self.token_cache).await {
            req = req.header(
                AUTHORIZATION,
                HeaderValue::from_str(&auth)
                    .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?,
            );
        }
        let resp = req.send().await.map_err(RegistryError::Transport)?;
        check_status(resp)?;
        Ok(())
    }

    /// Fetch the raw manifest bytes, content-type, and digest for a tag or digest.
    /// Used by `tag_manifest` to copy a manifest under a new tag name.
    pub async fn get_manifest_raw(
        &self,
        repository: &str,
        reference: &str,
    ) -> Result<RawManifestBytes, RegistryError> {
        let url = format!("{}/v2/{}/manifests/{}", self.base_url, repository, reference);
        let resp = self.get(&url, MANIFEST_ACCEPT).await?;

        let digest = resp
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .unwrap_or(reference)
            .to_string();

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();

        let bytes = resp
            .bytes()
            .await
            .map_err(RegistryError::Transport)?
            .to_vec();

        Ok(RawManifestBytes { digest, content_type, bytes })
    }

    /// PUT a manifest under a new tag name. Returns the canonical digest from
    /// the `Docker-Content-Digest` response header.
    pub async fn put_manifest(
        &self,
        repository: &str,
        tag: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<String, RegistryError> {
        let url = format!("{}/v2/{}/manifests/{}", self.base_url, repository, tag);

        let mut req = self
            .client
            .put(&url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(body);

        if let Some(auth) = resolve_auth_header(&self.cfg, &self.token_cache).await {
            req = req.header(
                AUTHORIZATION,
                HeaderValue::from_str(&auth)
                    .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?,
            );
        }

        let resp = req.send().await.map_err(RegistryError::Transport)?;
        let resp = check_status(resp)?;

        Ok(resp
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string())
    }
}

/// Raw manifest bytes returned by `get_manifest_raw`, used for re-tagging.
pub struct RawManifestBytes {
    pub digest: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// The parsed result of a manifest fetch — either a single-platform image or
/// a multi-platform image index.
pub enum ManifestResult {
    Single {
        digest: String,
        media_type: String,
        inner: RawManifest,
    },
    Index {
        digest: String,
        media_type: String,
        inner: RawIndex,
    },
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn check_status(resp: Response) -> Result<Response, RegistryError> {
    match resp.status() {
        StatusCode::OK | StatusCode::ACCEPTED | StatusCode::NO_CONTENT | StatusCode::CREATED => {
            Ok(resp)
        }
        StatusCode::UNAUTHORIZED => Err(RegistryError::Unauthorized),
        StatusCode::FORBIDDEN => Err(RegistryError::Forbidden),
        StatusCode::NOT_FOUND => Err(RegistryError::NotFound {
            resource: resp.url().path().to_string(),
        }),
        StatusCode::METHOD_NOT_ALLOWED => Err(RegistryError::DeleteNotEnabled),
        s => {
            // Consume the body for the error message but don't propagate body errors.
            let status = s.as_u16();
            // We can't await here in a sync context, so return what we have.
            Err(RegistryError::UnexpectedStatus {
                status,
                body: format!("(body not captured for status {})", status),
            })
        }
    }
}
