//! Integration tests for the Docker GC container lifecycle.
//!
//! These tests require the test compose stack to be running:
//!
//!   docker compose -f tests/docker-compose.test.yml up -d --wait
//!
//! Then run with:
//!
//!   REGISTRY_INTEGRATION_TEST=1 cargo test --test gc_integration -- --nocapture
//!
//! Tear down afterward:
//!
//!   docker compose -f tests/docker-compose.test.yml down -v

use std::collections::HashMap;
use std::io::Write as _;

use bollard::{Docker, container::ListContainersOptions};
use serial_test::serial;
use tempfile::NamedTempFile;

use registry_mcp::{
    config::{Config, GcConfig, LogConfig, RegistryConfig, ServerConfig},
    gc::docker::run_docker_gc,
    types::GcStrategy,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Skip the test if `REGISTRY_INTEGRATION_TEST` is not set.
macro_rules! require_integration {
    () => {
        if std::env::var("REGISTRY_INTEGRATION_TEST").is_err() {
            eprintln!("Skipping integration test — set REGISTRY_INTEGRATION_TEST=1 to run.");
            return;
        }
    };
}

/// Count all containers (running + stopped) that carry the `registry-mcp.gc=true` label.
/// The compose registry does not have this label, so parallel test runs don't interfere.
async fn count_gc_containers(docker: &Docker) -> usize {
    let opts = ListContainersOptions::<String> {
        all: true,
        filters: HashMap::from([("label".into(), vec!["registry-mcp.gc=true".into()])]),
        ..Default::default()
    };
    docker.list_containers(Some(opts)).await.unwrap_or_default().len()
}

/// Write a registry config YAML that points GC at the test MinIO instance.
/// The GC container runs with host networking, so `127.0.0.1:9000` reaches the
/// MinIO service exposed on the host by the compose stack.
fn write_registry_config() -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("tempfile");
    write!(
        f,
        r#"version: 0.1
storage:
  s3:
    accesskey: minioadmin
    secretkey: minioadmin
    bucket: registry
    region: us-east-1
    regionendpoint: http://127.0.0.1:9000
    forcepathstyle: true
  delete:
    enabled: true
"#
    )
    .expect("write registry config");
    f
}

/// Build a test `Config` that uses the temp registry config file.
fn make_config(registry_config_path: &str) -> Config {
    Config {
        server: ServerConfig {
            host: "0.0.0.0".into(),
            port: 3000,
        },
        registry: RegistryConfig {
            base_url: "http://localhost:5001".into(),
            username: String::new(),
            password: String::new(),
            bearer_token: String::new(),
            insecure_skip_verify: false,
            ca_cert_file: String::new(),
        },
        gc: GcConfig {
            script_path: String::new(),
            registry_config_path: registry_config_path.to_string(),
            docker_socket: "/var/run/docker.sock".into(),
            registry_image: "registry:3".into(),
            // Host networking lets the GC container reach MinIO on 127.0.0.1:9000.
            docker_network: "host".into(),
        },
        log: LogConfig {
            level: "info".into(),
        },
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// GC succeeds (exit 0) and the container is removed afterward.
#[tokio::test]
#[serial]
async fn test_docker_gc_cleanup_on_success() {
    require_integration!();

    let registry_config = write_registry_config();
    let cfg = make_config(registry_config.path().to_str().unwrap());

    let docker = Docker::connect_with_local(
        &cfg.gc.docker_socket,
        120,
        bollard::API_DEFAULT_VERSION,
    )
    .expect("connect to Docker");

    let containers_before = count_gc_containers(&docker).await;

    // Dry run first.
    let result = run_docker_gc(&cfg, true, true, None::<rmcp::service::RequestContext<rmcp::RoleServer>>)
        .await
        .expect("run_docker_gc should not return a Rust error");

    assert_eq!(result.strategy, GcStrategy::Docker);
    assert!(result.dry_run);
    assert_eq!(
        result.exit_code,
        Some(0),
        "GC dry run failed — stderr: {}",
        result.stderr
    );

    // Container count must be unchanged — cleanup ran.
    let containers_after = count_gc_containers(&docker).await;
    assert_eq!(
        containers_before, containers_after,
        "GC container was not cleaned up after a successful dry run"
    );

    // Real run.
    let result = run_docker_gc(&cfg, false, true, None::<rmcp::service::RequestContext<rmcp::RoleServer>>)
        .await
        .expect("run_docker_gc should not return a Rust error");

    assert_eq!(result.exit_code, Some(0), "GC real run failed — stderr: {}", result.stderr);

    let containers_after = count_gc_containers(&docker).await;
    assert_eq!(
        containers_before, containers_after,
        "GC container was not cleaned up after a successful real run"
    );
}

/// GC exits non-zero (bad storage config) and the container is still removed.
///
/// This exercises the cleanup-on-failure branch: `run_container` returns an
/// `Ok(RunGcOutput)` with a non-zero exit code, and the outer function must
/// still call `remove_container` before returning.
#[tokio::test]
#[serial]
async fn test_docker_gc_cleanup_on_failure() {
    require_integration!();

    // Write a registry config that points at a port with nothing listening.
    let mut bad_config_file = NamedTempFile::new().expect("tempfile");
    write!(
        bad_config_file,
        r#"version: 0.1
storage:
  s3:
    accesskey: minioadmin
    secretkey: minioadmin
    bucket: registry
    region: us-east-1
    regionendpoint: http://127.0.0.1:19999
    forcepathstyle: true
  delete:
    enabled: true
"#
    )
    .expect("write bad registry config");

    let cfg = make_config(bad_config_file.path().to_str().unwrap());

    let docker = Docker::connect_with_local(
        &cfg.gc.docker_socket,
        120,
        bollard::API_DEFAULT_VERSION,
    )
    .expect("connect to Docker");

    let containers_before = count_gc_containers(&docker).await;

    // run_docker_gc should succeed at the Rust level (it returns Ok even on
    // non-zero GC exit), but the exit code should be non-zero.
    let result = run_docker_gc(&cfg, true, true, None)
        .await
        .expect("run_docker_gc should not return a Rust error even when GC fails");

    assert_eq!(result.strategy, GcStrategy::Docker);
    assert_ne!(
        result.exit_code,
        Some(0),
        "Expected non-zero exit from GC with unreachable storage, got 0"
    );

    // Container must still be gone — cleanup ran despite the non-zero exit.
    let containers_after = count_gc_containers(&docker).await;
    assert_eq!(
        containers_before, containers_after,
        "GC container was not cleaned up after a failed GC run"
    );
}
