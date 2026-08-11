use a3s_use_core::{OkfCapabilityProjection, PlanScope, UseError, UseResult};
use a3s_use_extension::{ExtensionLifecycleIdentity, ExtensionRegistry, ExtensionRouteLease};

use super::{
    OkfKnowledgeCitation, OkfKnowledgeClient, OkfKnowledgeReadResponse, OkfKnowledgeSearchRequest,
    OkfKnowledgeSearchResponse,
};

/// Host-side acquisition service for one exact published OKF generation.
///
/// The underlying Registry route lease participates in lifecycle drain. A
/// newer cutover may be published while an existing lease is in use, but
/// receipt-owned retirement cannot remove that generation until the lease is
/// dropped. New acquisitions for hidden, revoked, or stale generations fail
/// closed.
#[derive(Clone)]
pub struct OkfKnowledgeLeaseProvider {
    registry: ExtensionRegistry,
    client: OkfKnowledgeClient,
}

impl OkfKnowledgeLeaseProvider {
    pub fn new(registry: ExtensionRegistry, client: OkfKnowledgeClient) -> Self {
        Self { registry, client }
    }

    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }

    pub fn client(&self) -> &OkfKnowledgeClient {
        &self.client
    }

    /// Acquire a lease only when the exact projection is currently published
    /// and the installed package evidence matches it. `None` means the
    /// generation is not currently callable (not installed, hidden, revoked,
    /// incompatible, or already draining).
    pub async fn acquire(
        &self,
        projection: &OkfCapabilityProjection,
    ) -> UseResult<Option<OkfKnowledgeLease>> {
        projection.validate()?;
        let identity = ExtensionLifecycleIdentity::new(
            &projection.surface.package_id,
            &projection.package_digest,
            &projection.manifest_digest,
            projection.generation,
        )?;
        let Some(route_lease) = self
            .registry
            .acquire_published_lifecycle_generation(&identity)
            .await?
        else {
            return Ok(None);
        };
        validate_route_binding(&route_lease, projection)?;
        route_lease.verify_integrity().await?;
        Ok(Some(OkfKnowledgeLease {
            projection: projection.clone(),
            client: self.client.clone(),
            route_lease,
        }))
    }
}

/// A single exact-generation Knowledge session. Search and subsequent read
/// calls must use this value object so the caller cannot silently switch to a
/// different package generation between retrieval steps.
pub struct OkfKnowledgeLease {
    projection: OkfCapabilityProjection,
    client: OkfKnowledgeClient,
    route_lease: ExtensionRouteLease,
}

impl OkfKnowledgeLease {
    pub fn projection(&self) -> &OkfCapabilityProjection {
        &self.projection
    }

    pub fn scope(&self) -> &PlanScope {
        &self.projection.scope
    }

    /// Search only the projection pinned when this lease was acquired.
    pub async fn search(
        &self,
        query: impl Into<String>,
        limit: usize,
    ) -> UseResult<OkfKnowledgeSearchResponse> {
        self.route_lease.verify_integrity().await?;
        let request = OkfKnowledgeSearchRequest::new(
            self.projection.scope.clone(),
            query,
            limit,
            vec![self.projection.clone()],
        )?;
        let result = self.client.search(&request).await;
        self.route_lease.verify_integrity().await?;
        result
    }

    /// Read one citation returned by this lease's search, retaining the same
    /// package, surface, generation, projection, and source digest.
    pub async fn read(
        &self,
        citation: &OkfKnowledgeCitation,
        max_bytes: usize,
    ) -> UseResult<OkfKnowledgeReadResponse> {
        self.route_lease.verify_integrity().await?;
        let request = super::OkfKnowledgeReadRequest::new(
            self.projection.scope.clone(),
            self.projection.clone(),
            citation.clone(),
            max_bytes,
        )?;
        let result = self.client.read(&request).await;
        self.route_lease.verify_integrity().await?;
        result
    }
}

fn validate_route_binding(
    route_lease: &ExtensionRouteLease,
    projection: &OkfCapabilityProjection,
) -> UseResult<()> {
    let extension = route_lease.extension();
    let receipt = &extension.receipt;
    if receipt.package_id != projection.surface.package_id
        || receipt.lifecycle_generation != Some(projection.generation)
        || receipt.package_sha256.as_deref() != projection.package_digest.strip_prefix("sha256:")
        || receipt.manifest_sha256
            != projection
                .manifest_digest
                .strip_prefix("sha256:")
                .unwrap_or_default()
        || !receipt.enabled
    {
        return Err(lease_binding_error(
            "The published package receipt does not match the exact OKF capability projection.",
        ));
    }
    let Some(surface) = extension
        .manifest
        .okf
        .iter()
        .find(|surface| surface.id == projection.surface.surface.id)
    else {
        return Err(lease_binding_error(
            "The published package does not contain the projected OKF surface.",
        ));
    };
    if surface.bundle != projection.bundle {
        return Err(lease_binding_error(
            "The published OKF surface bundle differs from the exact capability projection.",
        ));
    }
    Ok(())
}

fn lease_binding_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_lease_binding_invalid", message)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use a3s_use_core::{
        OkfCapabilityProjection, PlanQualifiedSurfaceRef, PlanScope, PlanScopeKind,
        PluginSurfaceKind, PluginSurfaceRef,
    };
    use a3s_use_extension::{
        load_okf_bundle_files, ExtensionLifecycleIdentity, ExtensionLifecyclePackage,
        ExtensionPaths,
    };

    use super::*;
    use crate::okf_knowledge::{
        OkfKnowledgeStageRequest, OkfKnowledgeStageSpec, SqliteOkfKnowledgeAdapter,
    };

    struct LeaseFixture {
        _temporary: tempfile::TempDir,
        registry: ExtensionRegistry,
        identity: ExtensionLifecycleIdentity,
        provider: OkfKnowledgeLeaseProvider,
        projection: OkfCapabilityProjection,
    }

    #[tokio::test]
    async fn hidden_generation_rejects_new_leases_while_existing_read_drains() {
        let fixture = lease_fixture(17).await;
        let lease = fixture
            .provider
            .acquire(&fixture.projection)
            .await
            .unwrap()
            .unwrap();
        let before = lease.search("runtime execution", 5).await.unwrap();
        assert!(!before.hits.is_empty());

        fixture
            .registry
            .hide_lifecycle_package(&fixture.identity)
            .await
            .unwrap();
        assert!(fixture
            .provider
            .acquire(&fixture.projection)
            .await
            .unwrap()
            .is_none());

        let draining = lease.search("runtime execution", 5).await.unwrap();
        let read = lease
            .read(
                &draining.hits[0].citation,
                usize::try_from(fixture.projection.bundle.limits.max_document_bytes).unwrap(),
            )
            .await
            .unwrap();
        assert!(read
            .content
            .contains("Runtime execution remains host-owned"));

        let error = fixture
            .registry
            .drain_lifecycle_package(&fixture.identity, Duration::from_millis(1))
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.extension.drain_timeout");
        drop(lease);
        fixture
            .registry
            .drain_lifecycle_package(&fixture.identity, Duration::from_secs(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retained_package_drift_invalidates_an_existing_knowledge_lease() {
        let fixture = lease_fixture(23).await;
        let lease = fixture
            .provider
            .acquire(&fixture.projection)
            .await
            .unwrap()
            .unwrap();
        let package_root = fixture.registry.lifecycle_package_root(&fixture.identity);
        tokio::fs::write(
            package_root.join("README.md"),
            b"changed after lease acquisition\n",
        )
        .await
        .unwrap();

        let error = lease.search("runtime execution", 5).await.unwrap_err();
        assert_eq!(error.code, "use.extension.package_digest_mismatch");
    }

    async fn lease_fixture(generation: u64) -> LeaseFixture {
        let temporary = tempfile::tempdir().unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
        );
        let registry = ExtensionRegistry::new(paths.clone());
        let source = fixture_package_root();
        let candidate = ExtensionLifecyclePackage::prepare_local("acme/knowledge", &source, true)
            .await
            .unwrap();
        let identity = ExtensionLifecycleIdentity::new(
            candidate.package_id(),
            candidate.package_digest(),
            candidate.manifest_digest(),
            generation,
        )
        .unwrap();
        let surface = candidate.manifest().okf[0].clone();
        let files = load_okf_bundle_files(&surface, &source).await.unwrap();
        let scope = PlanScope {
            kind: PlanScopeKind::Workspace,
            id: "lease-test-workspace".to_owned(),
        };
        let stage_spec = OkfKnowledgeStageSpec {
            operation_id: format!("lease-stage-{generation}"),
            scope: scope.clone(),
            surface: PlanQualifiedSurfaceRef {
                package_id: candidate.package_id().to_owned(),
                surface: PluginSurfaceRef {
                    kind: PluginSurfaceKind::Okf,
                    id: surface.id.clone(),
                },
            },
            generation,
            package_digest: candidate.package_digest().to_owned(),
            manifest_digest: candidate.manifest_digest().to_owned(),
            bundle: surface.bundle,
        };
        let client = OkfKnowledgeClient::new(Arc::new(
            SqliteOkfKnowledgeAdapter::from_extension_paths(&paths),
        ));
        let staged = client
            .stage(OkfKnowledgeStageRequest::new(stage_spec, files).unwrap())
            .await
            .unwrap();
        let promoted = client.promote(&staged.receipt).await.unwrap();
        let projection =
            OkfCapabilityProjection::from_promoted(&promoted.receipt, &promoted.observation)
                .unwrap();

        registry
            .commit_lifecycle_package(&identity, &candidate)
            .await
            .unwrap();
        let cutover_key = format!("sha256:{generation:064x}");
        registry
            .publish_lifecycle_package_with_durable_cutover(&identity, &cutover_key)
            .await
            .unwrap();
        registry
            .complete_lifecycle_cutover(&cutover_key)
            .await
            .unwrap();
        let provider = OkfKnowledgeLeaseProvider::new(registry.clone(), client);
        LeaseFixture {
            _temporary: temporary,
            registry,
            identity,
            provider,
            projection,
        }
    }

    fn fixture_package_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates/extension/fixtures/packages/plugin-v3-okf/package")
    }
}
