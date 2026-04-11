use registry_mcp::{auth, config, registry, tools};

use std::{net::TcpStream, sync::Arc, time::Duration};

use axum::middleware;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

use auth::{
    jwt::JwtValidator,
    middleware::{AuthState, auth_middleware},
    well_known::{WellKnownState, oauth_authorization_server, oauth_protected_resource},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck();
    }

    let cfg = config::load()?;
    hs_utils::logging::init(&cfg.log.level);
    config::validate(&cfg)?;

    let registry = registry::client::RegistryClient::new(cfg.registry.clone())?;
    let cfg = Arc::new(cfg);
    let registry = Arc::new(registry);

    // Build the HTTP client used by the JWT validator (reuse rustls, no OpenSSL).
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let validator = JwtValidator::new(cfg.auth.clone(), http_client);

    let auth_state = Arc::new(AuthState {
        enabled: cfg.auth.enabled,
        validator: Arc::clone(&validator),
    });

    // Derive the server's public URL for the protected-resource metadata.
    // Operators can override this with `server__publicUrl` if behind a proxy.
    let public_url = std::env::var("server__publicUrl").unwrap_or_else(|_| {
        format!("http://{}:{}", cfg.server.host, cfg.server.port)
    });

    let well_known_state = Arc::new(WellKnownState {
        config: Arc::new(cfg.auth.clone()),
        server_url: public_url,
    });

    let ct = CancellationToken::new();
    let ct_shutdown = ct.clone();

    let cfg_factory = Arc::clone(&cfg);
    let registry_factory = Arc::clone(&registry);

    let service = StreamableHttpService::new(
        move || {
            // Read the identity stamped by the auth middleware for this request's task.
            let identity = auth::current_user();
            Ok(tools::RegistryMcp::new(
                Arc::clone(&cfg_factory),
                Arc::clone(&registry_factory),
                identity,
            ))
        },
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default().with_cancellation_token(ct.child_token()),
    );

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);

    let mcp_router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&auth_state),
            auth_middleware,
        ));

    let router = axum::Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            axum::routing::get(oauth_authorization_server),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            axum::routing::get(oauth_protected_resource),
        )
        .with_state(Arc::clone(&well_known_state))
        .merge(mcp_router);

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("registry-mcp listening on http://{addr}/mcp");
    if cfg.auth.enabled {
        tracing::info!(issuer = %cfg.auth.issuer, "OAuth authentication enabled");
    } else {
        tracing::info!("OAuth authentication disabled — all requests are anonymous");
    }

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            ct_shutdown.cancel();
        })
        .await?;

    Ok(())
}

fn healthcheck() -> anyhow::Result<()> {
    let port: u16 = std::env::var("server__port")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let addr = format!("127.0.0.1:{port}");
    TcpStream::connect_timeout(&addr.parse()?, Duration::from_secs(5))
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("Healthcheck failed — could not connect to {addr}: {e}"))
}
