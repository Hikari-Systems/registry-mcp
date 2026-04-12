use anyhow::{Context, Result, bail};
use hs_utils::config::{apply_env_overrides, deep_merge, deser_bool_or_str, deser_u16_or_str, prepare_config};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub registry: RegistryConfig,
    pub log: LogConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    #[serde(deserialize_with = "deser_u16_or_str")]
    pub port: u16,
    /// Semicolon-separated list of hosts allowed in the HTTP Host header
    /// (DNS rebinding protection). Empty string disables the check entirely.
    #[serde(rename = "allowedHosts", default, deserialize_with = "deser_semicolon_list")]
    pub allowed_hosts: Vec<String>,
    /// Semicolon-separated list of origins allowed in CORS preflight responses.
    /// Empty string disables CORS headers.
    #[serde(rename = "allowedOrigins", default, deserialize_with = "deser_semicolon_list")]
    pub allowed_origins: Vec<String>,
}

fn deser_semicolon_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    Ok(s.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect())
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
        bail!("registry:baseUrl must not be empty");
    }
    url::Url::parse(&cfg.registry.base_url)
        .with_context(|| format!("registry:baseUrl is not a valid URL: {}", cfg.registry.base_url))?;

    if !cfg.registry.bearer_token.is_empty() && !cfg.registry.username.is_empty() {
        tracing::warn!(
            "Both registry:bearerToken and registry:username are set — bearerToken takes precedence"
        );
    }

    Ok(())
}
