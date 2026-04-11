use anyhow::{Context, Result, bail};
use hs_utils::config::{apply_env_overrides, deep_merge, deser_bool_or_str, deser_u16_or_str, prepare_config};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub registry: RegistryConfig,
    pub gc: GcConfig,
    pub log: LogConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    /// When false the /mcp endpoint accepts all requests with an anonymous identity.
    #[serde(deserialize_with = "deser_bool_or_str")]
    pub enabled: bool,

    /// OAuth 2.0 issuer URL — must match the `iss` claim in incoming JWTs.
    /// E.g. `https://accounts.google.com` or `https://auth.example.com/realms/myrealm`.
    pub issuer: String,

    /// JWKS URI used to fetch the public keys for JWT signature verification.
    /// E.g. `https://accounts.google.com/.well-known/jwks.json`
    #[serde(rename = "jwksUri")]
    pub jwks_uri: String,

    /// Expected `aud` claim value.  Leave empty to skip audience validation.
    pub audience: String,

    /// Advertised in `/.well-known/oauth-authorization-server`.
    /// Full URL of the `/authorize` endpoint on the upstream AS.
    #[serde(rename = "authorizationEndpoint")]
    pub authorization_endpoint: String,

    /// Advertised in `/.well-known/oauth-authorization-server`.
    /// Full URL of the `/token` endpoint on the upstream AS.
    #[serde(rename = "tokenEndpoint")]
    pub token_endpoint: String,

    /// Advertised in `/.well-known/oauth-authorization-server`.
    /// Full URL of the dynamic client registration endpoint (RFC 7591), if supported.
    #[serde(rename = "registrationEndpoint")]
    pub registration_endpoint: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    #[serde(deserialize_with = "deser_u16_or_str")]
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RegistryConfig {
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    pub username: String,
    pub password: String,
    #[serde(rename = "bearerToken")]
    pub bearer_token: String,
    #[serde(rename = "insecureSkipVerify", deserialize_with = "deser_bool_or_str")]
    pub insecure_skip_verify: bool,
    #[serde(rename = "caCertFile")]
    pub ca_cert_file: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GcConfig {
    #[serde(rename = "scriptPath")]
    pub script_path: String,
    #[serde(rename = "registryConfigPath")]
    pub registry_config_path: String,
    #[serde(rename = "dockerSocket")]
    pub docker_socket: String,
    #[serde(rename = "registryImage")]
    pub registry_image: String,
    /// Docker network to attach the GC container to. Empty string = Docker default (bridge).
    /// Set to "host" to use host networking (required when MinIO is exposed on the host).
    #[serde(rename = "dockerNetwork")]
    pub docker_network: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LogConfig {
    pub level: String,
}

/// Load config from `config.json` (or `$CONFIG_PATH`), apply an optional
/// `/sandbox/config.json` overlay, resolve `[SECRET]:` references, and apply
/// env-var overrides.  Uses `hs_utils::config` throughout.
pub fn load() -> Result<Config> {
    let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.json".to_string());
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read config file: {path}"))?;
    let mut root: Value = serde_json::from_str(&text).context("Failed to parse config.json")?;

    if let Ok(overlay_text) = std::fs::read_to_string("/sandbox/config.json") {
        if let Ok(overlay) = serde_json::from_str::<Value>(&overlay_text) {
            deep_merge(&mut root, overlay);
        }
    }

    prepare_config(&mut root);
    apply_env_overrides(&mut root);
    serde_json::from_value(root).context("Failed to deserialise config")
}

/// Validate config at startup before binding the server.
pub fn validate(cfg: &Config) -> Result<()> {
    if cfg.registry.base_url.is_empty() {
        bail!("registry.baseUrl must not be empty");
    }
    url::Url::parse(&cfg.registry.base_url)
        .with_context(|| format!("registry.baseUrl is not a valid URL: {}", cfg.registry.base_url))?;

    if !cfg.registry.bearer_token.is_empty() && !cfg.registry.username.is_empty() {
        tracing::warn!(
            "Both registry.bearerToken and registry.username are set — bearerToken takes precedence"
        );
    }

    if !cfg.gc.script_path.is_empty() {
        use std::os::unix::fs::PermissionsExt;
        let p = std::path::Path::new(&cfg.gc.script_path);
        if !p.exists() {
            bail!("gc.scriptPath '{}' does not exist", cfg.gc.script_path);
        }
        let mode = std::fs::metadata(p)
            .with_context(|| format!("Cannot read gc.scriptPath '{}'", cfg.gc.script_path))?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            bail!("gc.scriptPath '{}' is not executable", cfg.gc.script_path);
        }
    }

    if !cfg.gc.registry_config_path.is_empty()
        && !std::path::Path::new(&cfg.gc.registry_config_path).exists()
    {
        bail!(
            "gc.registryConfigPath '{}' does not exist",
            cfg.gc.registry_config_path
        );
    }

    Ok(())
}
