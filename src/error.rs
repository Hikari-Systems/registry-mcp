use std::path::PathBuf;
use thiserror::Error;

/// Errors returned by OCI Distribution API calls.
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("not found: {resource}")]
    NotFound { resource: String },

    #[error("registry authentication failed — check credentials")]
    Unauthorized,

    #[error("access denied by registry")]
    Forbidden,

    #[error("registry does not permit deletions — enable storage.delete in registry config")]
    DeleteNotEnabled,

    #[error("registry returned HTTP {status}: {body}")]
    UnexpectedStatus { status: u16, body: String },

    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),

    #[error("invalid registry response: {0}")]
    InvalidResponse(String),
}

/// Errors returned by GC strategy execution.
#[derive(Debug, Error)]
pub enum GcError {
    #[error("GC script not found or not executable: {0}")]
    ScriptNotFound(PathBuf),

    #[error("Failed to spawn GC script: {0}")]
    ScriptSpawnFailed(#[from] std::io::Error),

    #[error("Docker API error: {0}")]
    DockerApi(#[from] bollard::errors::Error),

    #[error("Image pull failed for '{image}': {detail}")]
    ImagePullFailed { image: String, detail: String },

    #[error("Container creation failed: {0}")]
    ContainerCreateFailed(String),
}
