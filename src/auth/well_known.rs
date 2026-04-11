use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::{Value, json};

use crate::config::AuthConfig;

/// Shared state for the `.well-known` handlers.
pub struct WellKnownState {
    pub config: Arc<AuthConfig>,
    /// The public base URL of this MCP server (e.g. `https://registry-mcp.example.com`).
    /// Used to populate `resource` in the protected-resource metadata.
    pub server_url: String,
}

/// `GET /.well-known/oauth-authorization-server`
///
/// RFC 8414 Authorization Server Metadata.  Points Claude / ChatGPT at the
/// upstream OAuth AS configured in `auth.*`.  Returns 404 when auth is disabled
/// so that clients that strictly require an AS can fall through to other
/// discovery mechanisms.
pub async fn oauth_authorization_server(
    State(state): State<Arc<WellKnownState>>,
) -> impl IntoResponse {
    if !state.config.enabled {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "auth not enabled"}))).into_response();
    }

    let mut meta = json!({
        "issuer": state.config.issuer,
        "authorization_endpoint": state.config.authorization_endpoint,
        "token_endpoint": state.config.token_endpoint,
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none", "client_secret_basic", "client_secret_post"],
        "scopes_supported": ["openid", "profile", "email"],
    });

    if !state.config.registration_endpoint.is_empty() {
        meta["registration_endpoint"] = Value::String(state.config.registration_endpoint.clone());
    }

    Json(meta).into_response()
}

/// `GET /.well-known/oauth-protected-resource`
///
/// RFC 9396 Protected Resource Metadata.  Advertises this server as a protected
/// resource and points to the upstream AS.  Returned even when auth is disabled
/// so that discovery still works in mixed environments.
pub async fn oauth_protected_resource(
    State(state): State<Arc<WellKnownState>>,
) -> impl IntoResponse {
    let mut meta = json!({
        "resource": state.server_url,
        "bearer_methods_supported": ["header"],
        "resource_documentation": "https://github.com/Hikari-Systems/registry-mcp",
    });

    if state.config.enabled && !state.config.issuer.is_empty() {
        meta["authorization_servers"] = json!([state.config.issuer]);
    }

    Json(meta).into_response()
}
