use std::collections::HashMap;

use bollard::{
    Docker,
    container::{
        CreateContainerOptions, LogsOptions, RemoveContainerOptions,
        StartContainerOptions, WaitContainerOptions,
    },
    image::CreateImageOptions,
    models::HostConfig,
};
use futures_util::{StreamExt, TryStreamExt};
use rmcp::{
    RoleServer,
    model::{NumberOrString, ProgressNotificationParam, ProgressToken},
    service::RequestContext,
};

use crate::{
    config::Config,
    error::GcError,
    types::{GcStrategy, RunGcOutput},
};

/// Run GC via a Docker container using the registry image's built-in
/// `garbage-collect` command.  Log lines are emitted as MCP progress
/// notifications in real time; the full log is also returned in the response.
///
/// `ctx` is optional — pass `None` in tests where no MCP peer is connected.
pub async fn run_docker_gc(
    cfg: &Config,
    dry_run: bool,
    delete_untagged: bool,
    ctx: Option<RequestContext<RoleServer>>,
) -> Result<RunGcOutput, GcError> {
    let docker = Docker::connect_with_local(&cfg.gc.docker_socket, 120, bollard::API_DEFAULT_VERSION)?;

    // ── Step 1: Image pull check ──────────────────────────────────────────────
    let image = &cfg.gc.registry_image;
    if docker.inspect_image(image).await.is_err() {
        tracing::info!("Image '{image}' not found locally — pulling");
        let mut pull_stream = docker.create_image(
            Some(CreateImageOptions {
                from_image: image.as_str(),
                ..Default::default()
            }),
            None,
            None,
        );
        while let Some(item) = pull_stream.next().await {
            match item {
                Ok(info) => tracing::debug!("Pull: {:?}", info.status),
                Err(e) => {
                    return Err(GcError::ImagePullFailed {
                        image: image.clone(),
                        detail: e.to_string(),
                    });
                }
            }
        }
    }

    // ── Step 2: Build command and create container ────────────────────────────
    let mut cmd = vec![
        "/bin/registry".to_string(),
        "garbage-collect".to_string(),
        "/etc/docker/registry/config.yml".to_string(),
    ];
    if dry_run {
        cmd.push("--dry-run".to_string());
    }
    if delete_untagged {
        cmd.push("--delete-untagged".to_string());
    }

    // Suppress HTTP listener — this container only runs GC, not a registry server.
    let env = vec!["REGISTRY_HTTP_ADDR=".to_string()];

    let bind = format!(
        "{}:/etc/docker/registry/config.yml:ro",
        cfg.gc.registry_config_path
    );

    let network_mode = if cfg.gc.docker_network.is_empty() {
        None
    } else {
        Some(cfg.gc.docker_network.clone())
    };

    let container_id = docker
        .create_container(
            None::<CreateContainerOptions<&str>>,
            bollard::container::Config {
                image: Some(image.as_str()),
                cmd: Some(cmd.iter().map(String::as_str).collect()),
                env: Some(env.iter().map(String::as_str).collect()),
                labels: Some(HashMap::from([
                    ("registry-mcp.gc", "true"),
                ])),
                host_config: Some(HostConfig {
                    binds: Some(vec![bind]),
                    network_mode,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| GcError::ContainerCreateFailed(e.to_string()))?
        .id;

    // ── Steps 3–5: Start, stream logs, wait ───────────────────────────────────
    let result = run_container(&docker, &container_id, dry_run, ctx.as_ref()).await;

    // ── Step 6: Cleanup (always, even on failure) ─────────────────────────────
    if let Err(e) = docker
        .remove_container(
            &container_id,
            Some(RemoveContainerOptions { force: true, ..Default::default() }),
        )
        .await
    {
        tracing::warn!("Failed to remove GC container '{container_id}': {e}");
    }

    result
}

/// Start the container, stream its logs as progress notifications, wait for
/// exit, and return the outcome.  Separated so cleanup always runs.
async fn run_container(
    docker: &Docker,
    container_id: &str,
    dry_run: bool,
    ctx: Option<&RequestContext<RoleServer>>,
) -> Result<RunGcOutput, GcError> {
    docker
        .start_container(container_id, None::<StartContainerOptions<&str>>)
        .await?;

    // Stream logs with follow=true, collect into buffers and emit notifications.
    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();
    let mut line_count: f64 = 0.0;

    let mut log_stream = docker.logs(
        container_id,
        Some(LogsOptions::<&str> {
            follow: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        }),
    );

    while let Some(item) = log_stream.next().await {
        match item {
            Ok(output) => {
                use bollard::container::LogOutput;
                let (line, is_stderr) = match &output {
                    LogOutput::StdOut { message } => {
                        (String::from_utf8_lossy(message).into_owned(), false)
                    }
                    LogOutput::StdErr { message } => {
                        (String::from_utf8_lossy(message).into_owned(), true)
                    }
                    _ => continue,
                };

                if is_stderr {
                    stderr_buf.push_str(&line);
                } else {
                    stdout_buf.push_str(&line);
                }

                // Emit each line as a progress notification (if a peer is connected).
                line_count += 1.0;
                if let Some(ctx) = ctx {
                    let notif = ProgressNotificationParam {
                        progress_token: ProgressToken(NumberOrString::String("gc".into())),
                        progress: line_count,
                        total: None,
                        message: Some(line.trim_end_matches('\n').to_string()),
                    };
                    ctx.peer.notify_progress(notif).await.ok();
                }
            }
            Err(e) => {
                tracing::warn!("Log stream error from GC container: {e}");
                break;
            }
        }
    }

    // Wait for the container to finish.
    let exit_code = docker
        .wait_container(container_id, None::<WaitContainerOptions<&str>>)
        .try_next()
        .await
        .ok()
        .flatten()
        .map(|w| w.status_code as i32)
        .unwrap_or(-1);

    let message = if exit_code == 0 {
        format!(
            "GC completed successfully{}.",
            if dry_run { " (dry run)" } else { "" }
        )
    } else {
        format!("GC container exited with code {exit_code}.")
    };

    Ok(RunGcOutput {
        strategy: GcStrategy::Docker,
        dry_run,
        exit_code: Some(exit_code),
        stdout: stdout_buf,
        stderr: stderr_buf,
        message,
    })
}
