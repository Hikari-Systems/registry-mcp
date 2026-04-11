use crate::{
    error::RegistryError,
    registry::client::{RawManifestBytes, RegistryClient},
    types::{Descriptor, MEDIA_TYPE_DOCKER_MANIFEST_LIST, MEDIA_TYPE_OCI_INDEX, RawIndex, RawManifest},
};

// ── Source image reference parsing ───────────────────────────────────────────

/// Parsed components of a source image reference such as
/// `docker.io/library/nginx:latest` or `registry.example.com:5000/myimage:v1`.
pub struct SourceImage {
    /// Registry base URL, e.g. `https://registry-1.docker.io`
    pub base_url: String,
    /// Repository path within the registry, e.g. `library/nginx`
    pub repository: String,
    /// Tag or digest, e.g. `latest` or `sha256:abc…`
    pub reference: String,
}

fn is_registry_component(s: &str) -> bool {
    s.contains('.') || s.contains(':') || s == "localhost"
}

/// Parse a Docker/OCI image reference into its components.
///
/// Supported forms:
/// - `nginx` → Docker Hub `library/nginx:latest`
/// - `myorg/myimage:v1` → Docker Hub `myorg/myimage:v1`
/// - `docker.io/library/nginx:latest` → Docker Hub `library/nginx:latest`
/// - `registry.example.com/repo:tag` → explicit registry
/// - `registry.example.com:5000/repo:tag` → explicit registry with port
/// - `image@sha256:abc…` → pinned by digest
pub fn parse_source(source: &str) -> Result<SourceImage, String> {
    // Strip digest (@sha256:...)
    let (without_digest, digest) = if let Some(pos) = source.find('@') {
        (&source[..pos], Some(source[pos + 1..].to_string()))
    } else {
        (source, None)
    };

    // Strip tag — the rightmost `:` that is NOT inside a hostname:port.
    // A colon is part of a port when the text after it contains a `/`.
    let (name, tag) = if let Some(pos) = without_digest.rfind(':') {
        let after = &without_digest[pos + 1..];
        if !after.contains('/') {
            (&without_digest[..pos], Some(after.to_string()))
        } else {
            (without_digest, None)
        }
    } else {
        (without_digest, None)
    };

    let reference = digest.or(tag).unwrap_or_else(|| "latest".to_string());

    // Split name into registry and repository.
    let (raw_registry, repository) = if let Some(slash) = name.find('/') {
        let first = &name[..slash];
        if is_registry_component(first) {
            (first.to_string(), name[slash + 1..].to_string())
        } else {
            // No registry prefix (e.g. "myorg/myimage") — Docker Hub.
            ("docker.io".to_string(), name.to_string())
        }
    } else {
        // No slash at all (e.g. "nginx") — Docker Hub with implicit library/ prefix.
        ("docker.io".to_string(), format!("library/{name}"))
    };

    let base_url = if raw_registry == "docker.io" {
        "https://registry-1.docker.io".to_string()
    } else {
        format!("https://{raw_registry}")
    };

    Ok(SourceImage { base_url, repository, reference })
}

// ── Copy statistics ───────────────────────────────────────────────────────────

pub struct CopyStats {
    pub is_multi_arch: bool,
    pub manifests_copied: usize,
    pub blobs_copied: usize,
    pub blobs_skipped: usize,
    pub final_digest: String,
}

// ── High-level copy orchestration ────────────────────────────────────────────

/// Copy a source image (single-platform or multi-arch index) into the target
/// registry under `target_repo:target_tag`.
///
/// For multi-arch images each child manifest and its blobs are copied first,
/// then the index manifest is pushed under `target_tag`.
pub async fn copy_image(
    src: &RegistryClient,
    dst: &RegistryClient,
    src_img: &SourceImage,
    target_repo: &str,
    target_tag: &str,
) -> Result<CopyStats, RegistryError> {
    let raw = src
        .get_manifest_raw(&src_img.repository, &src_img.reference)
        .await?;

    let is_index = raw.content_type == MEDIA_TYPE_OCI_INDEX
        || raw.content_type == MEDIA_TYPE_DOCKER_MANIFEST_LIST;

    let mut stats = CopyStats {
        is_multi_arch: is_index,
        manifests_copied: 0,
        blobs_copied: 0,
        blobs_skipped: 0,
        final_digest: raw.digest.clone(),
    };

    if is_index {
        let index: RawIndex = serde_json::from_slice(&raw.bytes)
            .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?;

        // Copy each child manifest and all its blobs.
        for child in &index.manifests {
            copy_single_by_ref(
                src,
                dst,
                &src_img.repository,
                &child.digest,
                target_repo,
                &child.digest,
                &mut stats,
            )
            .await?;
        }

        // Push the index manifest under the target tag.
        let returned = dst
            .put_manifest(target_repo, target_tag, &raw.content_type, raw.bytes)
            .await?;
        if !returned.is_empty() {
            stats.final_digest = returned;
        }
        stats.manifests_copied += 1;
    } else {
        // Use the already-fetched raw bytes directly.
        copy_single_raw(
            src,
            dst,
            &src_img.repository,
            raw,
            target_repo,
            target_tag,
            &mut stats,
        )
        .await?;
    }

    Ok(stats)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Fetch a manifest by reference then copy its blobs and the manifest itself.
/// Used for each child manifest in a multi-arch index.
async fn copy_single_by_ref(
    src: &RegistryClient,
    dst: &RegistryClient,
    src_repo: &str,
    src_reference: &str,
    target_repo: &str,
    target_reference: &str,
    stats: &mut CopyStats,
) -> Result<(), RegistryError> {
    let raw = src.get_manifest_raw(src_repo, src_reference).await?;
    copy_single_raw(src, dst, src_repo, raw, target_repo, target_reference, stats).await
}

/// Copy all blobs from an already-fetched single-platform manifest, then push
/// the manifest itself to the target registry.
async fn copy_single_raw(
    src: &RegistryClient,
    dst: &RegistryClient,
    src_repo: &str,
    raw: RawManifestBytes,
    target_repo: &str,
    target_reference: &str,
    stats: &mut CopyStats,
) -> Result<(), RegistryError> {
    let manifest: RawManifest = serde_json::from_slice(&raw.bytes)
        .map_err(|e| RegistryError::InvalidResponse(e.to_string()))?;

    copy_blob(src, dst, src_repo, target_repo, &manifest.config, stats).await?;
    for layer in &manifest.layers {
        copy_blob(src, dst, src_repo, target_repo, layer, stats).await?;
    }

    dst.put_manifest(target_repo, target_reference, &raw.content_type, raw.bytes)
        .await?;
    stats.manifests_copied += 1;
    Ok(())
}

/// Copy a single blob from source to destination, skipping if it already exists.
async fn copy_blob(
    src: &RegistryClient,
    dst: &RegistryClient,
    src_repo: &str,
    dst_repo: &str,
    descriptor: &Descriptor,
    stats: &mut CopyStats,
) -> Result<(), RegistryError> {
    if dst.blob_exists(dst_repo, &descriptor.digest).await? {
        stats.blobs_skipped += 1;
        return Ok(());
    }
    let data = src.get_blob(src_repo, &descriptor.digest).await?;
    dst.push_blob(dst_repo, &descriptor.digest, data).await?;
    stats.blobs_copied += 1;
    Ok(())
}
