use std::sync::Arc;

use rmcp::{ErrorData, model::CallToolResult};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{registry::client::RegistryClient, types::{TagManifestOutput, UntagOutput}};

use super::catalog::{ok_json, registry_err};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UntagParams {
    /// Repository name, e.g. `library/nginx`
    pub repository: String,
    /// Tag to remove. Only this tag reference is deleted — the manifest and
    /// any other tags pointing to the same digest are unaffected.
    pub tag: String,
}

/// Remove a single tag reference without deleting the underlying manifest.
/// Uses DELETE by tag name rather than by digest, so other tags pointing at
/// the same manifest remain intact.
pub async fn untag(
    registry: &Arc<RegistryClient>,
    params: UntagParams,
) -> Result<CallToolResult, ErrorData> {
    registry
        .delete_tag_reference(&params.repository, &params.tag)
        .await
        .map_err(registry_err)?;

    ok_json(&UntagOutput {
        message: format!(
            "Tag '{}' removed from {}. The manifest is still present and reachable by digest or other tags.",
            params.tag, params.repository
        ),
        repository: params.repository,
        tag: params.tag,
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TagManifestParams {
    /// Repository name, e.g. `library/nginx`
    pub repository: String,
    /// Source tag or digest to copy from, e.g. `latest` or `sha256:abc123…`
    pub source: String,
    /// New tag name to create or overwrite
    pub new_tag: String,
}

/// Create a new tag pointing at the same manifest as an existing tag or digest.
/// Equivalent to `docker tag` — fetches the raw manifest from `source` and PUTs
/// it under `new_tag` in the same repository.
pub async fn tag_manifest(
    registry: &Arc<RegistryClient>,
    params: TagManifestParams,
) -> Result<CallToolResult, ErrorData> {
    let raw = registry
        .get_manifest_raw(&params.repository, &params.source)
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

    let returned_digest = registry
        .put_manifest(
            &params.repository,
            &params.new_tag,
            &raw.content_type,
            raw.bytes,
        )
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

    // The registry may or may not echo the digest back — fall back to the one
    // we received from the GET if the PUT response omitted it.
    let digest = if returned_digest.is_empty() {
        raw.digest
    } else {
        returned_digest
    };

    ok_json(&TagManifestOutput {
        message: format!(
            "Tag '{}' created pointing to {}.",
            params.new_tag, digest
        ),
        repository: params.repository,
        source: params.source,
        new_tag: params.new_tag,
        digest,
    })
}
