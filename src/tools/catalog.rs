use rmcp::{ErrorData, model::{CallToolResult, Content}};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{
    error::RegistryError,
    registry::client::RegistryClient,
    types::{ListRepositoriesOutput, ListTagsOutput},
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRepositoriesParams {
    /// Pagination cursor — last repository name from the previous page.
    pub last: Option<String>,
    /// Maximum repositories to return (1–1000). Defaults to 100.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTagsParams {
    /// Repository name, e.g. `library/nginx`.
    pub repository: String,
    /// Pagination cursor — last tag name from the previous page.
    pub last: Option<String>,
    /// Maximum tags to return (1–1000). Defaults to 100.
    pub limit: Option<u32>,
}

pub async fn list_repositories(
    registry: &RegistryClient,
    params: ListRepositoriesParams,
) -> Result<CallToolResult, ErrorData> {
    let limit = params.limit.unwrap_or(100).clamp(1, 1000);
    let last = params.last.unwrap_or_default();

    let catalog = registry
        .get_catalog(&last, limit)
        .await
        .map_err(registry_err)?;

    let repositories = catalog.repositories;
    let next_last = repositories.last().cloned().unwrap_or_default();
    let total_returned = repositories.len();

    let output = ListRepositoriesOutput {
        repositories,
        next_last,
        total_returned,
    };

    ok_json(&output)
}

pub async fn list_tags(
    registry: &RegistryClient,
    params: ListTagsParams,
) -> Result<CallToolResult, ErrorData> {
    let limit = params.limit.unwrap_or(100).clamp(1, 1000);
    let last = params.last.unwrap_or_default();

    let resp = registry
        .get_tags(&params.repository, &last, limit)
        .await
        .map_err(registry_err)?;

    let tags = resp.tags.unwrap_or_default();
    let next_last = tags.last().cloned().unwrap_or_default();
    let total_returned = tags.len();

    let output = ListTagsOutput {
        repository: params.repository,
        tags,
        next_last,
        total_returned,
    };

    ok_json(&output)
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Serialise a value to JSON and wrap it in a successful CallToolResult.
pub fn ok_json<T: serde::Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

/// Map a RegistryError to an MCP ErrorData.
pub fn registry_err(e: RegistryError) -> ErrorData {
    match e {
        RegistryError::NotFound { resource } => {
            ErrorData::invalid_params(format!("Not found: {resource}"), None)
        }
        RegistryError::Unauthorized => {
            ErrorData::internal_error("Registry authentication failed — check credentials", None)
        }
        RegistryError::Forbidden => {
            ErrorData::internal_error("Access denied by registry", None)
        }
        RegistryError::DeleteNotEnabled => {
            ErrorData::internal_error(
                "Registry does not permit deletions — enable storage.delete in registry config",
                None,
            )
        }
        RegistryError::UnexpectedStatus { status, body } => {
            ErrorData::internal_error(format!("Registry returned HTTP {status}: {body}"), None)
        }
        RegistryError::Transport(e) => {
            ErrorData::internal_error(format!("HTTP transport error: {e}"), None)
        }
        RegistryError::InvalidResponse(msg) => {
            ErrorData::internal_error(format!("Invalid registry response: {msg}"), None)
        }
    }
}
