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

