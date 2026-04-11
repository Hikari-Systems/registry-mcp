/// Integration tests for the `migrate` tool.
///
/// Require the test compose stack (MinIO + registry:3) and network access to
/// Docker Hub.  Skipped unless `REGISTRY_INTEGRATION_TEST=1` is set.
///
/// To run:
///   docker compose -f tests/docker-compose.test.yml up -d --wait
///   REGISTRY_INTEGRATION_TEST=1 cargo test --test migrate_integration -- --nocapture
///   docker compose -f tests/docker-compose.test.yml down -v

use std::sync::Arc;

use registry_mcp::{
    config::RegistryConfig,
    migrate::{copy_image, parse_source},
    registry::client::RegistryClient,
    tools::migrate::{MigrateParams, migrate},
};

macro_rules! require_integration {
    () => {
        if std::env::var("REGISTRY_INTEGRATION_TEST").as_deref() != Ok("1") {
            eprintln!("Skipping — set REGISTRY_INTEGRATION_TEST=1 to run");
            return;
        }
    };
}

const REGISTRY_URL: &str = "http://localhost:5001";

fn local_registry() -> Arc<RegistryClient> {
    let cfg = RegistryConfig {
        base_url: REGISTRY_URL.to_string(),
        username: String::new(),
        password: String::new(),
        bearer_token: String::new(),
        insecure_skip_verify: false,
        ca_cert_file: String::new(),
    };
    Arc::new(RegistryClient::new(cfg).expect("build local registry client"))
}

// ── parse_source unit tests (no network, no #[serial] needed) ────────────────

#[test]
fn parse_source_bare_name() {
    let img = parse_source("nginx").unwrap();
    assert_eq!(img.base_url, "https://registry-1.docker.io");
    assert_eq!(img.repository, "library/nginx");
    assert_eq!(img.reference, "latest");
}

#[test]
fn parse_source_name_and_tag() {
    let img = parse_source("nginx:alpine").unwrap();
    assert_eq!(img.base_url, "https://registry-1.docker.io");
    assert_eq!(img.repository, "library/nginx");
    assert_eq!(img.reference, "alpine");
}

#[test]
fn parse_source_org_image() {
    let img = parse_source("myorg/myimage:v1").unwrap();
    assert_eq!(img.base_url, "https://registry-1.docker.io");
    assert_eq!(img.repository, "myorg/myimage");
    assert_eq!(img.reference, "v1");
}

#[test]
fn parse_source_docker_io_explicit() {
    let img = parse_source("docker.io/library/nginx:latest").unwrap();
    assert_eq!(img.base_url, "https://registry-1.docker.io");
    assert_eq!(img.repository, "library/nginx");
    assert_eq!(img.reference, "latest");
}

#[test]
fn parse_source_custom_registry() {
    let img = parse_source("registry.example.com/myorg/myimage:v2").unwrap();
    assert_eq!(img.base_url, "https://registry.example.com");
    assert_eq!(img.repository, "myorg/myimage");
    assert_eq!(img.reference, "v2");
}

#[test]
fn parse_source_custom_registry_with_port() {
    let img = parse_source("registry.example.com:5000/myimage:v2").unwrap();
    assert_eq!(img.base_url, "https://registry.example.com:5000");
    assert_eq!(img.repository, "myimage");
    assert_eq!(img.reference, "v2");
}

#[test]
fn parse_source_digest() {
    let img = parse_source("nginx@sha256:abc123").unwrap();
    assert_eq!(img.repository, "library/nginx");
    assert_eq!(img.reference, "sha256:abc123");
}

// ── Network integration tests ─────────────────────────────────────────────────

/// Pull debian:bookworm-slim from Docker Hub (multi-arch manifest list) and
/// push it to the local test registry.  Validates that the tag appears and
/// that we copied at least one manifest and one blob.
#[tokio::test]

async fn test_migrate_multi_arch() {
    require_integration!();

    let dst = local_registry();
    let src_img = parse_source("debian:bookworm-slim").unwrap();
    let src_client = RegistryClient::from_creds(&src_img.base_url, None, None).unwrap();

    let stats = copy_image(&src_client, &dst, &src_img, "library/debian", "bookworm-slim")
        .await
        .expect("migrate should succeed");

    println!(
        "is_multi_arch={} manifests_copied={} blobs_copied={} blobs_skipped={}",
        stats.is_multi_arch, stats.manifests_copied, stats.blobs_copied, stats.blobs_skipped
    );
    println!("digest: {}", stats.final_digest);

    assert!(stats.is_multi_arch, "debian:bookworm-slim should be a manifest list");
    assert!(!stats.final_digest.is_empty());
    assert!(stats.manifests_copied >= 2, "expected index + at least 1 child manifest");
    assert!(stats.blobs_copied >= 1, "expected at least 1 blob copied");

    // Verify the tag exists in the local registry.
    let tags_resp = dst
        .get_tags("library/debian", "", 100)
        .await
        .expect("get_tags after migrate");
    let tags = tags_resp.tags.unwrap_or_default();
    assert!(
        tags.contains(&"bookworm-slim".to_string()),
        "tag bookworm-slim missing after migrate; found: {tags:?}"
    );
}

/// Second migrate of the same image should skip all blobs (already present).
#[tokio::test]

async fn test_migrate_idempotent() {
    require_integration!();

    let dst = local_registry();
    let src_img = parse_source("debian:bookworm-slim").unwrap();
    let src_client = RegistryClient::from_creds(&src_img.base_url, None, None).unwrap();

    // First copy (may already be present from the previous test — that's fine).
    copy_image(&src_client, &dst, &src_img, "library/debian", "bookworm-slim-idem")
        .await
        .expect("first migrate");

    // Second copy — all blobs should be skipped.
    let stats = copy_image(&src_client, &dst, &src_img, "library/debian", "bookworm-slim-idem")
        .await
        .expect("second migrate");

    println!(
        "second run: blobs_copied={} blobs_skipped={}",
        stats.blobs_copied, stats.blobs_skipped
    );

    assert_eq!(stats.blobs_copied, 0, "expected 0 blobs copied on second run");
    assert!(stats.blobs_skipped > 0, "expected blobs_skipped > 0 on second run");
}

/// Test the tool-level handler soft-failure path when the source registry
/// returns 401 and no credentials were supplied.  Uses a private GHCR image
/// that returns 401 without auth.
#[tokio::test]

async fn test_migrate_requires_auth_soft_failure() {
    require_integration!();

    let dst = local_registry();

    let params = MigrateParams {
        source: "ghcr.io/hikari-systems/nonexistent-private-image:latest".to_string(),
        target_repository: "test/private".to_string(),
        target_tag: "latest".to_string(),
        source_username: None,
        source_password: None,
    };

    let result = migrate(&dst, params)
        .await
        .expect("tool should return Ok even on 401");

    let content = &result.content[0];
    let json_str = match &content.raw {
        rmcp::model::RawContent::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    let val: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    println!("requires_auth response: {val:#}");
    assert_eq!(
        val["requires_auth"], true,
        "expected requires_auth=true, got: {val:#}"
    );
}

/// Migrate an image then retag it inside the local registry; both operations
/// should yield the same digest.
#[tokio::test]

async fn test_migrate_then_retag() {
    require_integration!();

    let dst = local_registry();
    let src_img = parse_source("debian:bookworm-slim").unwrap();
    let src_client = RegistryClient::from_creds(&src_img.base_url, None, None).unwrap();

    let stats = copy_image(&src_client, &dst, &src_img, "library/debian", "bookworm-slim-retag-src")
        .await
        .expect("migrate");

    let raw = dst
        .get_manifest_raw("library/debian", "bookworm-slim-retag-src")
        .await
        .expect("get_manifest_raw");

    let returned = dst
        .put_manifest("library/debian", "bookworm-slim-retag-dst", &raw.content_type, raw.bytes)
        .await
        .expect("put_manifest");

    let retagged_digest = if returned.is_empty() { raw.digest } else { returned };

    println!("original digest={} retagged_digest={}", stats.final_digest, retagged_digest);

    assert_eq!(
        stats.final_digest, retagged_digest,
        "retagged digest should match migrated digest"
    );
}
