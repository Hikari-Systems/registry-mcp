use registry_mcp::{config, registry, tools};

use std::{net::TcpStream, sync::Arc, time::Duration};

use tower_http::{cors::CorsLayer, trace::TraceLayer};

use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `healthcheck` subcommand: TCP probe against the configured server port.
    // Uses stdlib only — no async runtime required — matching the hs-utils pattern.
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck();
    }

    let cfg = config::load()?;
    hs_utils::logging::init(&cfg.log.level);
    config::validate(&cfg)?;

    let registry = registry::client::RegistryClient::new(cfg.registry.clone())?;
    let cfg = Arc::new(cfg);
    let registry = Arc::new(registry);

    let ct = CancellationToken::new();
    let ct_shutdown = ct.clone();

    let registry_factory = Arc::clone(&registry);

    let service = StreamableHttpService::new(
        move || Ok(tools::RegistryMcp::new(Arc::clone(&registry_factory))),
        LocalSessionManager::default().into(),
        {
            let base = StreamableHttpServerConfig::default()
                .with_cancellation_token(ct.child_token());
            if cfg.server.allowed_hosts.is_empty() {
                base.disable_allowed_hosts()
            } else {
                base.with_allowed_hosts(cfg.server.allowed_hosts.iter().cloned())
            }
        },
    );

    let cors = if cfg.server.allowed_origins.is_empty() {
        None
    } else {
        use http::header::{AUTHORIZATION, CONTENT_TYPE};
        use tower_http::cors::AllowOrigin;

        let origins = cfg.server.allowed_origins.iter()
            .filter_map(|o| o.parse().ok())
            .collect::<Vec<http::HeaderValue>>();

        Some(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods([
                    http::Method::GET,
                    http::Method::POST,
                    http::Method::OPTIONS,
                    http::Method::DELETE,
                ])
                .allow_headers([CONTENT_TYPE, AUTHORIZATION, "mcp-session-id".parse().unwrap()])
                .expose_headers(["mcp-session-id".parse().unwrap()])
                .allow_credentials(true),
        )
    };

    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(TraceLayer::new_for_http());
    let router = if let Some(cors) = cors {
        router.layer(cors)
    } else {
        router
    };
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("registry-mcp listening on http://{addr}/mcp");

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            ct_shutdown.cancel();
        })
        .await?;

    Ok(())
}

/// Stdlib TCP healthcheck — no async, no external deps.
/// Reads server.port from the environment override or falls back to 3000.
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
