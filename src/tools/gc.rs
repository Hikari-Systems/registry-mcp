use std::sync::Arc;

use rmcp::{ErrorData, RoleServer, model::CallToolResult, service::RequestContext};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{
    config::Config,
    gc::{self, docker, script},
    types::{GcStrategy, RunGcOutput},
};

use super::catalog::ok_json;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunGcParams {
    /// If `true`, report what would be deleted without removing anything. Defaults to `true`.
    pub dry_run: Option<bool>,
    /// Pass `--delete-untagged` to the GC command. Defaults to `true`.
    pub delete_untagged: Option<bool>,
}

pub async fn run_gc(
    config: Arc<Config>,
    params: RunGcParams,
    ctx: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let dry_run = params.dry_run.unwrap_or(true);
    let delete_untagged = params.delete_untagged.unwrap_or(true);

    let strategy = gc::resolve_strategy(&config.gc);

    let output: RunGcOutput = match strategy {
        GcStrategy::Script => {
            script::run_script(&config, dry_run, delete_untagged)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        }
        GcStrategy::Docker => {
            docker::run_docker_gc(&config, dry_run, delete_untagged, Some(ctx))
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        }
        GcStrategy::Unavailable => RunGcOutput {
            strategy: GcStrategy::Unavailable,
            dry_run,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            message: "No GC strategy is configured. \
                To enable GC, set one of: \
                gc.scriptPath (path to a shell script) or \
                gc.registryConfigPath (path to a registry config.yml for Docker-based GC)."
                .to_string(),
        },
    };

    ok_json(&output)
}
