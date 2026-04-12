pub mod catalog;
pub mod delete;
pub mod manifest;
pub mod migrate;
pub mod tag;

use std::sync::Arc;

use rmcp::{
    ServerHandler,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::*,
    service::RequestContext,
    tool, tool_handler, tool_router,
};

use crate::registry::client::RegistryClient;

/// The MCP server handler.  All tool methods delegate to free functions in the
/// sub-modules; this struct is a thin dispatch layer required by `rmcp`.
#[derive(Clone)]
pub struct RegistryMcp {
    pub registry: Arc<RegistryClient>,
    #[allow(dead_code)]
    tool_router: ToolRouter<RegistryMcp>,
}

impl RegistryMcp {
    pub fn new(registry: Arc<RegistryClient>) -> Self {
        Self {
            registry,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl RegistryMcp {
    #[tool(description = "List repositories in the registry (paginated). \
        Pass `last` from the previous response to fetch the next page.")]
    async fn list_repositories(
        &self,
        Parameters(params): Parameters<catalog::ListRepositoriesParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        catalog::list_repositories(&self.registry, params).await
    }

    #[tool(description = "List tags for a named repository (paginated). \
        Pass `last` from the previous response to fetch the next page.")]
    async fn list_tags(
        &self,
        Parameters(params): Parameters<catalog::ListTagsParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        catalog::list_tags(&self.registry, params).await
    }

    #[tool(description = "Fetch the manifest for a tag or digest, including \
        media type, schema version, total compressed size, and per-layer details. \
        For multi-platform image indexes the child manifests are listed. \
        Note: large repositories with many multi-arch tags may take several seconds.")]
    async fn get_manifest(
        &self,
        Parameters(params): Parameters<manifest::GetManifestParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        manifest::get_manifest(&self.registry, params).await
    }

    #[tool(description = "Calculate the total on-disk blob footprint for all tags \
        in a repository, deduplicating layers shared across tags. \
        Returns a per-tag breakdown and a unique-blob total. \
        Multi-platform images are recursed into for accurate sizing. \
        Note: large repositories with many tags may take several seconds.")]
    async fn get_repository_disk_usage(
        &self,
        Parameters(params): Parameters<manifest::DiskUsageParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        manifest::get_repository_disk_usage(&self.registry, params).await
    }

    #[tool(description = "Delete a manifest by tag. Resolves the digest first and \
        returns it for confirmation. Protected by a `confirm` guard — defaults to \
        `false` (dry run). Set `confirm: true` to perform the actual deletion.")]
    async fn delete_tag(
        &self,
        Parameters(params): Parameters<delete::DeleteTagParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        delete::delete_tag(&self.registry, params).await
    }

    #[tool(description = "Remove one or more tag references (up to 20) without deleting \
        the underlying manifests. Other tags pointing at the same digest are unaffected. \
        Progress notifications are emitted as each tag is removed. \
        Use `delete_tag` (confirm: true) instead if you want to remove the manifest entirely.")]
    async fn untag(
        &self,
        Parameters(params): Parameters<tag::UntagParams>,
        ctx: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tag::untag(&self.registry, params, ctx).await
    }

    #[tool(description = "Create a new tag pointing at the same manifest as an existing \
        tag or digest. Equivalent to `docker tag` — no data is copied, only the manifest \
        reference is updated. `source` may be a tag name or a `sha256:…` digest.")]
    async fn tag_manifest(
        &self,
        Parameters(params): Parameters<tag::TagManifestParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tag::tag_manifest(&self.registry, params).await
    }

    #[tool(description = "Pull an image from an external registry and push it into \
        this registry (equivalent to docker pull + docker tag + docker push, but using \
        the OCI Distribution API directly — no Docker daemon required). \
        Handles single-platform images and multi-arch image indexes. \
        `source` accepts any standard image reference: `nginx:latest`, \
        `docker.io/library/nginx:latest`, `registry.example.com/myorg/myimage:v1.0`, etc. \
        For private source registries supply `source_username` and `source_password`; \
        if the source returns 401 without credentials the tool will ask you to retry.")]
    async fn migrate(
        &self,
        Parameters(params): Parameters<migrate::MigrateParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        migrate::migrate(&self.registry, params).await
    }
}

#[tool_handler]
impl ServerHandler for RegistryMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(Implementation::new(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(
            "MCP server for managing a self-hosted Docker Distribution (OCI) registry.\n\
            \n\
            Available tools:\n\
            - list_repositories: paginated list of all repositories in the registry\n\
            - list_tags: paginated list of tags for a given repository\n\
            - get_manifest: manifest details for a tag or digest (layers, size, platform info)\n\
            - get_repository_disk_usage: total deduplicated blob footprint for all tags in a repository\n\
            - tag_manifest: create a new tag pointing at an existing tag or digest (like `docker tag`)\n\
            - untag: remove one or more tag references (up to 20) without deleting the underlying manifest\n\
            - delete_tag: resolve a tag to its digest and delete the manifest entirely (dry-run by default)\n\
            - migrate: pull an image from an external registry and push it into this registry via the OCI API\n\
            \n\
            Example workflows:\n\
            \n\
            Cleaning up old tags: use list_tags to enumerate tags in a repository, \
            get_repository_disk_usage to understand which tags are consuming the most space, \
            then untag to remove stale tag references (CI branch tags, old feature tags) or \
            delete_tag (confirm: true) to permanently remove manifests you no longer need. \
            On self-hosted registries, deleted manifests do not free disk space immediately — \
            the registry garbage collector must be run externally after deletions to reclaim \
            storage. Managed registries (ECR, GAR, etc.) handle this automatically.\n\
            \n\
            Promoting a build: use tag_manifest to point a stable tag (e.g. `stable`, `production`) \
            at a tested image tag or digest without copying any data.\n\
            \n\
            Mirroring external images: use migrate to pull an image from Docker Hub or another \
            registry into this one. Multi-arch image indexes are handled transparently. \
            Blobs already present at the destination are skipped, so re-runs are safe."
                .to_string(),
        )
    }
}
