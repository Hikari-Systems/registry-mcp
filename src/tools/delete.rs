use rmcp::{ErrorData, model::CallToolResult};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{
    registry::client::RegistryClient,
    types::DeleteTagOutput,
};

use super::catalog::{ok_json, registry_err};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteTagParams {
    /// Repository name, e.g. `library/nginx`.
    pub repository: String,
    /// Tag to delete.
    pub tag: String,
    /// Must be `true` to perform the actual deletion. Defaults to `false` (dry run).
    pub confirm: Option<bool>,
}

pub async fn delete_tag(
    registry: &RegistryClient,
    params: DeleteTagParams,
) -> Result<CallToolResult, ErrorData> {
    let confirm = params.confirm.unwrap_or(false);

    // Always resolve the digest first so we can return it for confirmation.
    let digest = registry
        .resolve_digest(&params.repository, &params.tag)
        .await
        .map_err(registry_err)?;

    if !confirm {
        let output = DeleteTagOutput {
            repository: params.repository.clone(),
            tag: params.tag.clone(),
            digest: digest.clone(),
            deleted: false,
            message: format!(
                "Dry run — set confirm: true to delete manifest {digest} \
                (tag: {tag} in {repo})",
                tag = params.tag,
                repo = params.repository,
            ),
        };
        return ok_json(&output);
    }

    registry
        .delete_manifest(&params.repository, &params.tag)
        .await
        .map_err(registry_err)?;

    let output = DeleteTagOutput {
        repository: params.repository,
        tag: params.tag,
        digest: digest.clone(),
        deleted: true,
        message: format!(
            "Manifest {digest} deleted. Run run_gc to reclaim blob storage."
        ),
    };

    ok_json(&output)
}
