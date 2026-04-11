use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    middleware::Next,
    response::IntoResponse,
};

use super::{CURRENT_USER, UserIdentity, jwt::JwtValidator};

/// Axum middleware state shared across requests.
pub struct AuthState {
    pub enabled: bool,
    pub validator: Arc<JwtValidator>,
}

/// Extract a Bearer token from the `Authorization` header.
fn extract_bearer(req: &Request<Body>) -> Option<&str> {
    req.headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Axum middleware that validates Bearer JWT tokens and sets the task-local
/// `CURRENT_USER`.  When `auth.enabled` is false, every request runs as
/// `UserIdentity::anonymous()` with no token required.
pub async fn auth_middleware(
    State(state): State<Arc<AuthState>>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    if !state.enabled {
        // Auth disabled — run anonymously.
        return CURRENT_USER
            .scope(UserIdentity::anonymous(), next.run(req))
            .await;
    }

    let token = match extract_bearer(&req) {
        Some(t) => t.to_string(),
        None => {
            return unauthorized("Bearer token required").into_response();
        }
    };

    match state.validator.validate(&token).await {
        Ok(identity) => {
            tracing::debug!(
                user.sub = %identity.sub,
                user.email = ?identity.email,
                "authenticated"
            );
            CURRENT_USER.scope(identity, next.run(req)).await
        }
        Err(e) => {
            tracing::warn!(error = %e, "token validation failed");
            unauthorized(&format!("Token validation failed: {e}")).into_response()
        }
    }
}

fn unauthorized(detail: &str) -> impl IntoResponse {
    (
        StatusCode::UNAUTHORIZED,
        [(
            axum::http::header::WWW_AUTHENTICATE,
            r#"Bearer error="invalid_token""#,
        )],
        detail.to_string(),
    )
}
