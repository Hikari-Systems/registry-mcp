use anyhow::Result;
use base64::Engine;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::RegistryConfig;

/// Cached bearer token state shared across all client calls.
#[derive(Debug, Default)]
pub struct TokenCache {
    token: RwLock<Option<String>>,
}

impl TokenCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

/// Build an HTTP Basic `Authorization` header value from username and password.
pub fn basic_auth_header(username: &str, password: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD
        .encode(format!("{username}:{password}"));
    format!("Basic {encoded}")
}

/// Parse a `WWW-Authenticate: Bearer realm="...",service="...",scope="..."` header
/// and return the token endpoint URL with query parameters.
pub fn parse_bearer_challenge(header: &str) -> Option<String> {
    let rest = header.strip_prefix("Bearer ")?;
    let mut realm = "";
    let mut service = "";
    let mut scope = "";

    for part in rest.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("realm=\"") {
            realm = v.trim_end_matches('"');
        } else if let Some(v) = part.strip_prefix("service=\"") {
            service = v.trim_end_matches('"');
        } else if let Some(v) = part.strip_prefix("scope=\"") {
            scope = v.trim_end_matches('"');
        }
    }

    if realm.is_empty() {
        return None;
    }

    let mut url = realm.to_string();
    let mut sep = '?';
    if !service.is_empty() {
        url.push(sep);
        url.push_str("service=");
        url.push_str(service);
        sep = '&';
    }
    if !scope.is_empty() {
        url.push(sep);
        url.push_str("scope=");
        url.push_str(scope);
    }
    Some(url)
}

#[derive(Deserialize)]
struct TokenResponse {
    token: Option<String>,
    access_token: Option<String>,
}

/// Fetch a bearer token from the registry's auth endpoint.
/// Tries Basic auth credentials if both username and password are non-empty.
pub async fn fetch_bearer_token(
    client: &reqwest::Client,
    challenge_url: &str,
    cfg: &RegistryConfig,
) -> Result<String> {
    let mut req = client.get(challenge_url);
    if !cfg.username.is_empty() && !cfg.password.is_empty() {
        req = req.header(
            reqwest::header::AUTHORIZATION,
            basic_auth_header(&cfg.username, &cfg.password),
        );
    }
    let resp = req.send().await?;
    let body: TokenResponse = resp.error_for_status()?.json().await?;
    let token = body
        .token
        .or(body.access_token)
        .ok_or_else(|| anyhow::anyhow!("Bearer token response contained no token field"))?;
    Ok(token)
}

/// Resolve the `Authorization` header value to use for a request, given the
/// registry config and the optional cached bearer token.
///
/// Priority:
/// 1. Static `bearerToken` from config (if non-empty)
/// 2. Cached bearer token from a previous `fetch_bearer_token` call
/// 3. Basic auth from username/password (if non-empty)
/// 4. No auth header (returns `None`)
pub async fn resolve_auth_header(
    cfg: &RegistryConfig,
    cache: &TokenCache,
) -> Option<String> {
    if !cfg.bearer_token.is_empty() {
        return Some(format!("Bearer {}", cfg.bearer_token));
    }
    if let Some(token) = cache.token.read().await.as_deref() {
        return Some(format!("Bearer {token}"));
    }
    if !cfg.username.is_empty() && !cfg.password.is_empty() {
        return Some(basic_auth_header(&cfg.username, &cfg.password));
    }
    None
}

/// Store a freshly fetched token in the cache.
pub async fn cache_token(cache: &TokenCache, token: String) {
    *cache.token.write().await = Some(token);
}

/// Clear the cached token (called when a 401 is received so the next request
/// will re-fetch).
pub async fn clear_token(cache: &TokenCache) {
    *cache.token.write().await = None;
}
