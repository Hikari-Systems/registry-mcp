use std::sync::Arc;

use rmcp::{
    ErrorData, RoleServer,
    model::{CallToolResult, NumberOrString, ProgressNotificationParam, ProgressToken},
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{registry::client::RegistryClient, types::{TagManifestOutput, UntagFailure, UntagOutput}};

use super::catalog::ok_json;

const MAX_UNTAG: usize = 20;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UntagParams {
    /// Repository name, e.g. `library/nginx`
    pub repository: String,
    /// Tags to remove (1–20). Each tag reference is deleted individually —
    /// the manifest and any other tags pointing to the same digest are unaffected.
    pub tags: Vec<String>,
}

/// Remove one or more tag references without deleting the underlying manifests.
/// Uses DELETE by tag name rather than by digest, so other tags pointing at
/// the same manifest remain intact.
pub async fn untag(
    registry: &Arc<RegistryClient>,
    params: UntagParams,
    ctx: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    if params.tags.is_empty() {
        return Err(ErrorData::invalid_params("tags must not be empty", None));
    }
    if params.tags.len() > MAX_UNTAG {
        return Err(ErrorData::invalid_params(
            format!("too many tags: {} requested, maximum is {MAX_UNTAG}", params.tags.len()),
            None,
        ));
    }

    let total = params.tags.len();
    let mut removed = Vec::with_capacity(total);
    let mut failed: Vec<UntagFailure> = Vec::new();

    for (i, tag) in params.tags.iter().enumerate() {
        match registry.delete_tag_reference(&params.repository, tag).await {
            Ok(()) => {
                removed.push(tag.clone());
                ctx.peer.notify_progress(ProgressNotificationParam {
                    progress_token: ProgressToken(NumberOrString::String("untag".into())),
                    progress: (i + 1) as f64,
                    total: Some(total as f64),
                    message: Some(format!("Removed '{tag}' ({}/{total})", i + 1)),
                }).await.ok();
            }
            Err(e) => {
                failed.push(UntagFailure { tag: tag.clone(), error: e.to_string() });
                ctx.peer.notify_progress(ProgressNotificationParam {
                    progress_token: ProgressToken(NumberOrString::String("untag".into())),
                    progress: (i + 1) as f64,
                    total: Some(total as f64),
                    message: Some(format!("Failed '{tag}': {e} ({}/{total})", i + 1)),
                }).await.ok();
            }
        }
    }

    let message = match (removed.len(), failed.len()) {
        (r, 0) => format!("{r} tag(s) removed from {}.", params.repository),
        (0, f) => format!("All {f} tag(s) failed to remove from {}.", params.repository),
        (r, f) => format!("{r} tag(s) removed, {f} failed from {}.", params.repository),
    };

    ok_json(&UntagOutput {
        repository: params.repository,
        removed,
        failed,
        message,
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
