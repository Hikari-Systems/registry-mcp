pub mod catalog;
pub mod delete;
pub mod gc;
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

use crate::{config::Config, registry::client::RegistryClient};

/// The MCP server handler.  All tool methods delegate to free functions in the
/// sub-modules; this struct is a thin dispatch layer required by `rmcp`.
#[derive(Clone)]
pub struct RegistryMcp {
    pub config: Arc<Config>,
    pub registry: Arc<RegistryClient>,
    #[allow(dead_code)]
    tool_router: ToolRouter<RegistryMcp>,
}

impl RegistryMcp {
    pub fn new(config: Arc<Config>, registry: Arc<RegistryClient>) -> Self {
        Self {
            config,
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
        `false` (dry run). Set `confirm: true` to perform the actual deletion. \
        Note: blobs are not immediately reclaimed — run `run_gc` afterwards.")]
    async fn delete_tag(
        &self,
        Parameters(params): Parameters<delete::DeleteTagParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        delete::delete_tag(&self.registry, params).await
    }

    #[tool(description = "Remove a single tag reference without deleting the underlying \
        manifest. Other tags pointing at the same digest are unaffected. \
        Use `delete_tag` (confirm: true) instead if you want to remove the manifest entirely.")]
    async fn untag(
        &self,
        Parameters(params): Parameters<tag::UntagParams>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        tag::untag(&self.registry, params).await
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

    #[tool(description = "Run garbage collection on the registry. Selects strategy \
        automatically: shell script (if configured) → Docker container → Unavailable. \
        Defaults to dry_run=true — set `dry_run: false` to permanently remove \
        unreferenced blobs. Docker GC streams log lines as progress notifications.")]
    async fn run_gc(
        &self,
        Parameters(params): Parameters<gc::RunGcParams>,
        ctx: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        gc::run_gc(Arc::clone(&self.config), params, ctx).await
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
            "MCP server for managing a self-hosted Docker Distribution (OCI) registry. \
            Provides tools to browse repositories and tags, inspect manifests and disk \
            usage, delete tags (soft-delete), and run garbage collection."
                .to_string(),
        )
    }
}
