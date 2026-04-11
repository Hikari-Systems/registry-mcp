pub mod jwt;
pub mod middleware;
pub mod well_known;

/// Identity extracted from a validated Bearer JWT.  Carried through each MCP
/// request via a Tokio task-local so the rmcp factory and tool dispatch layer
/// can read it without changes to the rmcp transport API.
#[derive(Clone, Debug)]
pub struct UserIdentity {
    /// `sub` claim — always present after validation (or "anonymous" when auth is disabled).
    pub sub: String,
    /// `email` claim, if the AS includes it.
    pub email: Option<String>,
    /// Human-readable name: `preferred_username` → `name` → None.
    pub display_name: Option<String>,
}

impl UserIdentity {
    pub fn anonymous() -> Self {
        Self {
            sub: "anonymous".to_string(),
            email: None,
            display_name: None,
        }
    }
}

impl Default for UserIdentity {
    fn default() -> Self {
        Self::anonymous()
    }
}

// Task-local slot.  The auth middleware sets this before calling the next
// service layer.  The StreamableHttpService factory reads it to stamp each
// RegistryMcp session with the caller's identity.
tokio::task_local! {
    pub static CURRENT_USER: UserIdentity;
}

/// Read the current task-local identity, falling back to anonymous if the
/// middleware hasn't set it (e.g. when auth is disabled).
pub fn current_user() -> UserIdentity {
    CURRENT_USER.try_with(|u| u.clone()).unwrap_or_default()
}
