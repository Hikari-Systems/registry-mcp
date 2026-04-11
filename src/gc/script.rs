use std::path::Path;

use anyhow::Result;
use tokio::process::Command;

use crate::{
    config::Config,
    error::GcError,
    types::{GcStrategy, RunGcOutput},
};

/// Execute the configured shell script GC strategy.
/// The script receives flags as arguments and config values as environment variables.
pub async fn run_script(cfg: &Config, dry_run: bool, delete_untagged: bool) -> Result<RunGcOutput, GcError> {
    let path = Path::new(&cfg.gc.script_path);
    if !path.exists() {
        return Err(GcError::ScriptNotFound(path.to_path_buf()));
    }

    let mut cmd = Command::new(path);

    if dry_run {
        cmd.arg("--dry-run");
    }
    if delete_untagged {
        cmd.arg("--delete-untagged");
    }

    // Forward registry config as environment variables so the script can
    // interact with the registry without re-reading config itself.
    cmd.env("REGISTRY_URL", &cfg.registry.base_url)
        .env("REGISTRY_USERNAME", &cfg.registry.username)
        .env("REGISTRY_PASSWORD", &cfg.registry.password)
        .env("DRY_RUN", if dry_run { "true" } else { "false" })
        .env("DELETE_UNTAGGED", if delete_untagged { "true" } else { "false" });

    let output = cmd.output().await?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code().unwrap_or(-1);

    let message = if output.status.success() {
        format!(
            "GC script completed successfully{}.",
            if dry_run { " (dry run)" } else { "" }
        )
    } else {
        format!("GC script exited with code {exit_code}.")
    };

    Ok(RunGcOutput {
        strategy: GcStrategy::Script,
        dry_run,
        exit_code: Some(exit_code),
        stdout,
        stderr,
        message,
    })
}
