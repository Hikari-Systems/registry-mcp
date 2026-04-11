use std::collections::HashSet;

use futures_util::future;
use rmcp::{ErrorData, model::CallToolResult};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{
    registry::client::{ManifestResult, RegistryClient},
    types::{
        DiskUsageOutput, GetManifestOutput, IndexEntry, LayerInfo, TagDiskUsage,
    },
};

use super::catalog::{ok_json, registry_err};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetManifestParams {
    /// Repository name, e.g. `library/nginx`.
    pub repository: String,
    /// Tag name or digest (`sha256:…`).
    pub reference: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiskUsageParams {
    /// Repository name, e.g. `library/nginx`.
    pub repository: String,
}

pub async fn get_manifest(
    registry: &RegistryClient,
    params: GetManifestParams,
) -> Result<CallToolResult, ErrorData> {
    let result = registry
        .get_manifest(&params.repository, &params.reference)
        .await
        .map_err(registry_err)?;

    let output = match result {
        ManifestResult::Single { digest, media_type, inner } => {
            let total: i64 = inner.layers.iter().map(|l| l.size).sum::<i64>() + inner.config.size;
            GetManifestOutput {
                repository: params.repository,
                reference: params.reference,
                digest,
                media_type,
                schema_version: inner.schema_version,
                total_size_bytes: total,
                is_image_index: false,
                config: Some(LayerInfo {
                    digest: inner.config.digest,
                    media_type: inner.config.media_type,
                    size_bytes: inner.config.size,
                }),
                layers: Some(
                    inner.layers
                        .into_iter()
                        .map(|l| LayerInfo {
                            digest: l.digest,
                            media_type: l.media_type,
                            size_bytes: l.size,
                        })
                        .collect(),
                ),
                manifests: None,
            }
        }
        ManifestResult::Index { digest, media_type, inner } => {
            let total: i64 = inner.manifests.iter().map(|m| m.size).sum();
            let entries = inner.manifests
                .into_iter()
                .map(|m| IndexEntry {
                    digest: m.digest,
                    media_type: m.media_type,
                    size_bytes: m.size,
                    platform: m.platform,
                })
                .collect();
            GetManifestOutput {
                repository: params.repository,
                reference: params.reference,
                digest,
                media_type,
                schema_version: inner.schema_version,
                total_size_bytes: total,
                is_image_index: true,
                config: None,
                layers: None,
                manifests: Some(entries),
            }
        }
    };

    ok_json(&output)
}

pub async fn get_repository_disk_usage(
    registry: &RegistryClient,
    params: DiskUsageParams,
) -> Result<CallToolResult, ErrorData> {
    // Collect all tags (exhaust pagination).
    let mut all_tags: Vec<String> = Vec::new();
    let mut last = String::new();
    loop {
        let resp = registry
            .get_tags(&params.repository, &last, 1000)
            .await
            .map_err(registry_err)?;
        let tags = resp.tags.unwrap_or_default();
        if tags.is_empty() {
            break;
        }
        last = tags.last().cloned().unwrap_or_default();
        let done = tags.len() < 1000;
        all_tags.extend(tags);
        if done {
            break;
        }
    }

    let mut tag_summaries: Vec<TagDiskUsage> = Vec::new();
    let mut skipped_tags: Vec<String> = Vec::new();
    let mut seen_digests: HashSet<String> = HashSet::new();
    let mut unique_size_bytes: i64 = 0;

    for tag in all_tags {
        let manifest = match registry.get_manifest(&params.repository, &tag).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Skipping tag '{tag}' in '{}': {e}", params.repository);
                skipped_tags.push(tag);
                continue;
            }
        };

        match manifest {
            ManifestResult::Single { digest, inner, .. } => {
                let layer_digests: Vec<(String, i64)> = inner.layers
                    .iter()
                    .map(|l| (l.digest.clone(), l.size))
                    .collect();

                let tag_total: i64 = layer_digests.iter().map(|(_, s)| s).sum();

                for (d, size) in &layer_digests {
                    if seen_digests.insert(d.clone()) {
                        unique_size_bytes += size;
                    }
                }

                tag_summaries.push(TagDiskUsage {
                    tag,
                    digest,
                    layer_count: layer_digests.len(),
                    total_size_bytes: tag_total,
                });
            }
            ManifestResult::Index { digest, inner, .. } => {
                // Fetch all child manifests in parallel.
                let child_futures: Vec<_> = inner.manifests
                    .iter()
                    .map(|m| registry.get_manifest(&params.repository, &m.digest))
                    .collect();

                let child_results = future::join_all(child_futures).await;

                let mut tag_total: i64 = 0;
                let mut tag_layer_count: usize = 0;

                for child in child_results {
                    match child {
                        Ok(ManifestResult::Single { inner: child_m, .. }) => {
                            for layer in &child_m.layers {
                                tag_total += layer.size;
                                if seen_digests.insert(layer.digest.clone()) {
                                    unique_size_bytes += layer.size;
                                }
                            }
                            tag_layer_count += child_m.layers.len();
                        }
                        Ok(ManifestResult::Index { .. }) => {
                            // Nested index — skip to avoid unbounded recursion.
                            tracing::warn!(
                                "Nested image index encountered in '{}@{}' — skipping",
                                params.repository,
                                digest,
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to fetch child manifest in '{}@{}': {e}",
                                params.repository,
                                digest,
                            );
                        }
                    }
                }

                tag_summaries.push(TagDiskUsage {
                    tag,
                    digest,
                    layer_count: tag_layer_count,
                    total_size_bytes: tag_total,
                });
            }
        }
    }

    let output = DiskUsageOutput {
        repository: params.repository,
        unique_blob_count: seen_digests.len(),
        unique_size_bytes,
        tags: tag_summaries,
        skipped_tags,
    };

    ok_json(&output)
}
