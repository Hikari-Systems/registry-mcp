use std::sync::Arc;
use std::time::{Duration, Instant};

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation,
    decode, decode_header,
    jwk::{Jwk, JwkSet},
};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::config::AuthConfig;

use super::UserIdentity;

// Refresh JWKS at most once per hour.
const JWKS_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    #[allow(dead_code)]
    iss: String,
    email: Option<String>,
    preferred_username: Option<String>,
    name: Option<String>,
}

struct JwksCache {
    jwks: JwkSet,
    fetched_at: Instant,
}

/// Shared state for JWT validation: holds the HTTP client and a cached JWKS.
pub struct JwtValidator {
    config: AuthConfig,
    client: reqwest::Client,
    cache: RwLock<Option<JwksCache>>,
}

impl JwtValidator {
    pub fn new(config: AuthConfig, client: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            config,
            client,
            cache: RwLock::new(None),
        })
    }

    /// Validate a raw Bearer token string and return the caller's identity.
    pub async fn validate(&self, token: &str) -> Result<UserIdentity, AuthError> {
        let header = decode_header(token).map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        let kid = header.kid.as_deref().unwrap_or("");
        let alg = header.alg;

        let jwk = self.find_key(kid, alg).await?;
        let decoding_key =
            DecodingKey::from_jwk(&jwk).map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        let mut validation = Validation::new(alg);
        if !self.config.audience.is_empty() {
            validation.set_audience(&[&self.config.audience]);
        } else {
            validation.validate_aud = false;
        }
        if !self.config.issuer.is_empty() {
            validation.set_issuer(&[&self.config.issuer]);
        }

        let data = decode::<Claims>(token, &decoding_key, &validation)
            .map_err(|e| AuthError::InvalidToken(e.to_string()))?;

        let c = data.claims;
        let display_name = c.preferred_username.or(c.name);
        Ok(UserIdentity {
            sub: c.sub,
            email: c.email,
            display_name,
        })
    }

    /// Find a JWK by kid/alg, fetching or refreshing the JWKS as needed.
    async fn find_key(&self, kid: &str, alg: Algorithm) -> Result<Jwk, AuthError> {
        // Try cache first.
        {
            let cache = self.cache.read().await;
            if let Some(ref c) = *cache {
                if c.fetched_at.elapsed() < JWKS_TTL {
                    if let Some(jwk) = pick_key(&c.jwks, kid, alg) {
                        return Ok(jwk.clone());
                    }
                }
            }
        }

        // Cache miss or stale — refresh.
        let jwks = self.fetch_jwks().await?;
        let jwk = pick_key(&jwks, kid, alg)
            .ok_or_else(|| AuthError::KeyNotFound(kid.to_string()))?
            .clone();

        *self.cache.write().await = Some(JwksCache {
            jwks,
            fetched_at: Instant::now(),
        });

        Ok(jwk)
    }

    async fn fetch_jwks(&self) -> Result<JwkSet, AuthError> {
        let url = &self.config.jwks_uri;
        if url.is_empty() {
            return Err(AuthError::Config("auth.jwksUri is not configured".to_string()));
        }
        self.client
            .get(url)
            .send()
            .await
            .map_err(|e| AuthError::Fetch(e.to_string()))?
            .error_for_status()
            .map_err(|e| AuthError::Fetch(e.to_string()))?
            .json::<JwkSet>()
            .await
            .map_err(|e| AuthError::Fetch(format!("failed to parse JWKS: {e}")))
    }
}

fn pick_key<'a>(jwks: &'a JwkSet, kid: &str, alg: Algorithm) -> Option<&'a Jwk> {
    // Prefer exact kid match; fall back to first key with matching algorithm.
    if !kid.is_empty() {
        if let Some(jwk) = jwks.find(kid) {
            return Some(jwk);
        }
    }
    // Fall back: first key whose declared alg matches (or any key if none declare alg).
    jwks.keys.iter().find(|k| {
        k.common
            .key_algorithm
            .map(|a| format!("{a:?}").eq_ignore_ascii_case(&format!("{alg:?}")))
            .unwrap_or(true)
    })
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("signing key not found (kid={0})")]
    KeyNotFound(String),
    #[error("JWKS fetch failed: {0}")]
    Fetch(String),
    #[error("auth configuration error: {0}")]
    Config(String),
}
