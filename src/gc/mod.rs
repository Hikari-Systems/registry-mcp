pub mod docker;
pub mod script;

use crate::{config::GcConfig, types::GcStrategy};

/// Resolve which GC strategy to use based on configuration.
/// Priority: shell script → Docker → Unavailable.
pub fn resolve_strategy(cfg: &GcConfig) -> GcStrategy {
    if !cfg.script_path.is_empty() && std::path::Path::new(&cfg.script_path).exists() {
        return GcStrategy::Script;
    }
    if !cfg.registry_config_path.is_empty()
        && std::path::Path::new(&cfg.registry_config_path).exists()
    {
        return GcStrategy::Docker;
    }
    GcStrategy::Unavailable
}
