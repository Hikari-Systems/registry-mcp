use std::sync::Arc;

use rmcp::{ErrorData, model::CallToolResult};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{
    error::RegistryError,
    migrate::{copy_image, parse_source},
    registry::client::RegistryClient,
    types::MigrateOutput,
};

use super::catalog::{ok_json, registry_err};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MigrateParams {
    /// Full source image reference, e.g. `docker.io/library/nginx:latest`,
    /// `registry.example.com/myorg/myimage:v1.0`, or just `nginx:alpine`.
    /// Multi-arch (image index) sources are fully supported.
    pub source: String,
    /// Repository in this registry to push to, e.g. `library/nginx`
    pub target_repository: String,
    /// Tag to create or overwrite, e.g. `latest`
    pub target_tag: String,
    /// Source registry username. Leave unset for public registries.
    /// If the source requires auth and this is absent, the tool will return
    /// a soft failure asking you to retry with credentials.
    pub source_username: Option<String>,
    /// Source registry password or access token.
    pub source_password: Option<String>,
}

/// Pull an image from an external registry and push it into the managed registry.
///
/// Handles single-platform images and multi-arch image indexes.  On a 401 from
/// the source registry with no credentials supplied, returns a soft failure
/// (`requires_auth: true`) so the caller can prompt the user and retry.
pub async fn migrate(
    registry: &Arc<RegistryClient>,
    params: MigrateParams,
) -> Result<CallToolResult, ErrorData> {
    let src_img = parse_source(&params.source)
        .map_err(|e| ErrorData::invalid_params(format!("Invalid source reference: {e}"), None))?;

    let has_creds = params.source_username.is_some();

    let src_client = RegistryClient::from_creds(
        &src_img.base_url,
        params.source_username.as_deref(),
        params.source_password.as_deref(),
    )
    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

    match copy_image(&src_client, registry, &src_img, &params.target_repository, &params.target_tag).await {
        Ok(stats) => ok_json(&MigrateOutput {
            source: params.source,
            target_repository: params.target_repository.clone(),
            target_tag: params.target_tag.clone(),
            digest: stats.final_digest,
            is_multi_arch: stats.is_multi_arch,
            manifests_copied: stats.manifests_copied,
            blobs_copied: stats.blobs_copied,
            blobs_skipped: stats.blobs_skipped,
            requires_auth: false,
            message: format!(
                "Migrated to {}/{}. {} manifest(s), {} blob(s) copied, {} already present.",
                params.target_repository,
                params.target_tag,
                stats.manifests_copied,
                stats.blobs_copied,
                stats.blobs_skipped,
            ),
        }),

        // 401 with no credentials → soft failure asking caller to retry.
        Err(RegistryError::Unauthorized) if !has_creds => ok_json(&MigrateOutput {
            source: params.source,
            target_repository: params.target_repository,
            target_tag: params.target_tag,
            digest: String::new(),
            is_multi_arch: false,
            manifests_copied: 0,
            blobs_copied: 0,
            blobs_skipped: 0,
            requires_auth: true,
            message: "Source registry requires authentication. \
                Retry with source_username and source_password."
                .into(),
        }),

        Err(e) => Err(registry_err(e)),
    }
}
