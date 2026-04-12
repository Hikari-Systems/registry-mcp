use serde::{Deserialize, Serialize};

// ── OCI / Docker manifest media types ────────────────────────────────────────

pub const MEDIA_TYPE_OCI_INDEX: &str = "application/vnd.oci.image.index.v1+json";
pub const MEDIA_TYPE_DOCKER_MANIFEST_LIST: &str =
    "application/vnd.docker.distribution.manifest.list.v2+json";

// ── Raw OCI/Docker API response shapes ───────────────────────────────────────

/// A content descriptor as it appears in manifest JSON.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Descriptor {
    #[serde(rename = "mediaType", default)]
    pub media_type: String,
    pub digest: String,
    pub size: i64,
    /// Platform info — present in image index entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Platform {
    pub architecture: String,
    pub os: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// A single-platform image manifest (OCI or Docker schema v2).
#[derive(Debug, Deserialize)]
pub struct RawManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    #[serde(rename = "mediaType", default)]
    pub media_type: String,
    pub config: Descriptor,
    pub layers: Vec<Descriptor>,
}

/// A multi-platform image index (OCI) or manifest list (Docker).
#[derive(Debug, Deserialize)]
pub struct RawIndex {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    pub manifests: Vec<Descriptor>,
}

/// Tag list response from `/v2/<name>/tags/list`.
#[derive(Debug, Deserialize)]
pub struct TagsResponse {
    pub tags: Option<Vec<String>>,
}

/// Catalog response from `/v2/_catalog`.
#[derive(Debug, Deserialize)]
pub struct CatalogResponse {
    pub repositories: Vec<String>,
}

// ── Tool output shapes ────────────────────────────────────────────────────────

/// Output of `list_repositories`.
#[derive(Debug, Serialize)]
pub struct ListRepositoriesOutput {
    pub repositories: Vec<String>,
    pub next_last: String,
    pub total_returned: usize,
}

/// Output of `list_tags`.
#[derive(Debug, Serialize)]
pub struct ListTagsOutput {
    pub repository: String,
    pub tags: Vec<String>,
    pub next_last: String,
    pub total_returned: usize,
}

/// Per-layer detail in `get_manifest` output.
#[derive(Debug, Serialize)]
pub struct LayerInfo {
    pub digest: String,
    pub media_type: String,
    pub size_bytes: i64,
}

/// An entry in an image index (multi-platform manifest list).
#[derive(Debug, Serialize)]
pub struct IndexEntry {
    pub digest: String,
    pub media_type: String,
    pub size_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
}

/// Output of `get_manifest`.
#[derive(Debug, Serialize)]
pub struct GetManifestOutput {
    pub repository: String,
    pub reference: String,
    pub digest: String,
    pub media_type: String,
    pub schema_version: u32,
    pub total_size_bytes: i64,
    pub is_image_index: bool,
    /// Present for single-platform manifests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<LayerInfo>,
    /// Present for single-platform manifests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layers: Option<Vec<LayerInfo>>,
    /// Present for image indexes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifests: Option<Vec<IndexEntry>>,
}

/// Per-tag summary in `get_repository_disk_usage` output.
#[derive(Debug, Serialize)]
pub struct TagDiskUsage {
    pub tag: String,
    pub digest: String,
    pub layer_count: usize,
    pub total_size_bytes: i64,
}

/// Output of `get_repository_disk_usage`.
#[derive(Debug, Serialize)]
pub struct DiskUsageOutput {
    pub repository: String,
    pub unique_blob_count: usize,
    pub unique_size_bytes: i64,
    pub tags: Vec<TagDiskUsage>,
    /// Tags that could not be fetched (e.g. deleted mid-flight).
    pub skipped_tags: Vec<String>,
}

/// A tag that could not be removed in an `untag` call.
#[derive(Debug, Serialize)]
pub struct UntagFailure {
    pub tag: String,
    pub error: String,
}

/// Output of `untag`.
#[derive(Debug, Serialize)]
pub struct UntagOutput {
    pub repository: String,
    pub removed: Vec<String>,
    pub failed: Vec<UntagFailure>,
    pub message: String,
}

/// Output of `tag_manifest`.
#[derive(Debug, Serialize)]
pub struct TagManifestOutput {
    pub repository: String,
    pub source: String,
    pub new_tag: String,
    pub digest: String,
    pub message: String,
}

/// Output of `delete_tag`.
#[derive(Debug, Serialize)]
pub struct DeleteTagOutput {
    pub repository: String,
    pub tag: String,
    pub digest: String,
    pub deleted: bool,
    pub message: String,
}

/// Output of `migrate`.
#[derive(Debug, Serialize)]
pub struct MigrateOutput {
    pub source: String,
    pub target_repository: String,
    pub target_tag: String,
    pub digest: String,
    pub is_multi_arch: bool,
    pub manifests_copied: usize,
    pub blobs_copied: usize,
    pub blobs_skipped: usize,
    /// True when the source registry returned 401 and no credentials were supplied.
    /// Caller should retry with `source_username` and `source_password`.
    pub requires_auth: bool,
    pub message: String,
}
