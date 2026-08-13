use std::collections::BTreeMap;

use a3s_use_core::{
    OkfCapabilityProjection, PlanScope, PluginPackageLock, PluginPackageLockHost,
    PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult, VerifiedPluginCatalogRecord,
};
use a3s_use_extension::{
    inspect_cached_plugin, inspect_remote_plugin, resolve_cached_remote_package_lock,
    resolve_remote_package_lock, search_cached_plugins, search_remote_plugins, PluginCatalogHost,
    PluginCatalogInspection, PluginCatalogPage, PluginCatalogSearch, PluginCatalogSnapshot,
    RegistrySourceStore,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::okf_knowledge::{
    OkfKnowledgeBindingStore, OkfKnowledgeClient, OkfKnowledgeLease, OkfKnowledgeLeaseProvider,
    SqliteOkfKnowledgeAdapter,
};

use super::{current_host_target, CognitivePackageManager};

const GENERATION_DIGEST_DOMAIN: &str = "a3s.use.cognitive-generation.v1";
const CAPABILITY_SNAPSHOT_DIGEST_DOMAIN: &str = "a3s.use.capability-snapshot.v1";
const CANONICAL_DIGEST_PREFIX: &[u8] = b"agentic-ontology-canonical-v1\0";

fn assert_send_sync<T: Send + Sync>() {}

const _: fn() = || {
    assert_send_sync::<CognitiveCapabilityLease>();
    assert_send_sync::<CognitiveCapabilityEvidence>();
    assert_send_sync::<CognitiveCatalogSearchResult>();
};

/// Registry access policy for embedded catalog and lock operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitiveRegistryAccess {
    /// Refresh and verify TUF metadata before reading catalog records.
    Refreshed,
    /// Use only the last verified, still-valid local TUF snapshot.
    Cached,
}

/// One verified page from all enabled, host-configured Registries.
///
/// Each source page retains its own TUF snapshot identity. A package release
/// with the same catalog-record digest is returned once; conflicting releases
/// remain distinct so the caller never loses Registry provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CognitiveCatalogSearchResult {
    pub source_revision: String,
    pub snapshots: Vec<PluginCatalogSnapshot>,
    pub plugins: Vec<VerifiedPluginCatalogRecord>,
    pub total_matches: u64,
    pub next_cursors: Vec<CognitiveCatalogPageCursor>,
}

/// Cursor for deterministic bounded traversal across configured Registries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CognitiveCatalogPageCursor {
    pub registry_name: String,
    pub cursor: String,
}

/// Exact A3S Use evidence needed to construct a Code cognitive binding.
///
/// This value deliberately contains no Code-owned schema. It binds one
/// promoted OKF projection to its managed scope, immutable package generation,
/// complete installed package lock, and current Registry capability snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CognitiveCapabilityEvidence {
    pub scope: PlanScope,
    pub package_id: String,
    pub package_version: String,
    pub lifecycle_generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
    pub package_lock: PluginPackageLock,
    pub package_lock_digest: String,
    pub capability_generation: u64,
    pub capability_revision: String,
    pub generation_digest: String,
    pub capability_snapshot_digest: String,
    pub projection: OkfCapabilityProjection,
}

/// Exact live capability plus its retained lifecycle-drain lease.
pub struct CognitiveCapabilityLease {
    evidence: CognitiveCapabilityEvidence,
    knowledge: OkfKnowledgeLease,
}

impl CognitiveCapabilityLease {
    pub fn evidence(&self) -> &CognitiveCapabilityEvidence {
        &self.evidence
    }

    pub fn knowledge(&self) -> &OkfKnowledgeLease {
        &self.knowledge
    }

    pub fn into_parts(self) -> (CognitiveCapabilityEvidence, OkfKnowledgeLease) {
        (self.evidence, self.knowledge)
    }
}

pub(super) async fn search_catalogs(
    registry_sources: &RegistrySourceStore,
    access: CognitiveRegistryAccess,
    selected_registry: Option<&str>,
    search: &PluginCatalogSearch,
) -> UseResult<CognitiveCatalogSearchResult> {
    let sources = registry_sources.resolve(selected_registry).await?;
    if selected_registry.is_none() && search.cursor.is_some() {
        return Err(embedded_error(
            "use.plugin.embedded_catalog_cursor_ambiguous",
            "A paginated catalog request must select the Registry that issued its cursor.",
        ));
    }
    let host = PluginCatalogHost::current()?;
    let mut pages = Vec::new();
    let registries = sources.all().take(if selected_registry.is_some() {
        1
    } else {
        usize::MAX
    });
    for registry in registries {
        pages.push(match access {
            CognitiveRegistryAccess::Refreshed => {
                search_remote_plugins(registry, &host, search).await?
            }
            CognitiveRegistryAccess::Cached => {
                search_cached_plugins(registry, &host, search).await?
            }
        });
    }
    merge_catalog_pages(sources.source_revision(), pages)
}

pub(super) async fn inspect_catalog(
    registry_sources: &RegistrySourceStore,
    access: CognitiveRegistryAccess,
    candidate: &VerifiedPluginCatalogRecord,
) -> UseResult<PluginCatalogInspection> {
    candidate.validate()?;
    let sources = registry_sources
        .resolve(Some(&candidate.provenance.registry_name))
        .await?;
    if !sources.root().matches_provenance(&candidate.provenance) {
        return Err(embedded_error(
            "use.plugin.embedded_registry_provenance_mismatch",
            "The configured Registry source no longer matches the selected catalog provenance.",
        ));
    }
    let host = PluginCatalogHost::current()?;
    let inspection = match access {
        CognitiveRegistryAccess::Refreshed => {
            inspect_remote_plugin(
                sources.root(),
                &host,
                &candidate.record.package_id,
                Some(&candidate.record.version),
                Some(candidate.record.channel),
            )
            .await?
        }
        CognitiveRegistryAccess::Cached => {
            inspect_cached_plugin(
                sources.root(),
                &host,
                &candidate.record.package_id,
                Some(&candidate.record.version),
                Some(candidate.record.channel),
            )
            .await?
        }
    };
    if inspection.plugin != *candidate {
        return Err(embedded_error(
            "use.plugin.embedded_catalog_drift",
            "The inspected catalog release changed after selection; review a fresh search result.",
        ));
    }
    Ok(inspection)
}

pub(super) async fn resolve_lock(
    registry_sources: &RegistrySourceStore,
    access: CognitiveRegistryAccess,
    candidate: &VerifiedPluginCatalogRecord,
) -> UseResult<PluginPackageLock> {
    candidate.validate()?;
    let sources = registry_sources
        .resolve(Some(&candidate.provenance.registry_name))
        .await?;
    if !sources.root().matches_provenance(&candidate.provenance) {
        return Err(embedded_error(
            "use.plugin.embedded_registry_provenance_mismatch",
            "The configured Registry source no longer matches the selected catalog provenance.",
        ));
    }
    let host = PluginPackageLockHost::new(current_host_target()?, env!("CARGO_PKG_VERSION"))?;
    let lock = match access {
        CognitiveRegistryAccess::Refreshed => {
            resolve_remote_package_lock(
                sources.root(),
                sources.dependencies(),
                &candidate.record.package_id,
                Some(&candidate.record.version),
                candidate.record.channel,
                host,
            )
            .await?
        }
        CognitiveRegistryAccess::Cached => {
            resolve_cached_remote_package_lock(
                sources.root(),
                sources.dependencies(),
                &candidate.record.package_id,
                Some(&candidate.record.version),
                candidate.record.channel,
                host,
            )
            .await?
        }
    };
    if lock
        .package(&candidate.record.package_id)
        .map(|package| &package.catalog)
        != Some(candidate)
    {
        return Err(embedded_error(
            "use.plugin.embedded_package_lock_drift",
            "The resolved package lock selected a different root release; review a fresh candidate.",
        ));
    }
    Ok(lock)
}

pub(super) async fn acquire_capability_lease(
    manager: &CognitivePackageManager,
    selected_scope: &PlanScope,
    package_id: &str,
    surface_id: &str,
) -> UseResult<Option<CognitiveCapabilityLease>> {
    if manager.scope() != selected_scope {
        return Err(embedded_error(
            "use.plugin.embedded_scope_mismatch",
            "The requested cognitive capability belongs to a different managed scope.",
        ));
    }
    let state = manager.observe_package(package_id).await?;
    if state.desired != a3s_use_core::PluginDesiredState::Enabled
        || !matches!(
            state.observed,
            a3s_use_core::PluginObservedState::Ready | a3s_use_core::PluginObservedState::Degraded
        )
    {
        return Ok(None);
    }
    let package_digest = state.package_digest.clone().ok_or_else(|| {
        embedded_error(
            "use.plugin.embedded_capability_evidence_invalid",
            "The enabled cognitive package omitted its package digest.",
        )
    })?;
    let manifest_digest = state.manifest_digest.clone().ok_or_else(|| {
        embedded_error(
            "use.plugin.embedded_capability_evidence_invalid",
            "The enabled cognitive package omitted its manifest digest.",
        )
    })?;
    let version = state.version.clone().ok_or_else(|| {
        embedded_error(
            "use.plugin.embedded_capability_evidence_invalid",
            "The enabled cognitive package omitted its version.",
        )
    })?;
    let surface = PluginSurfaceRef {
        kind: PluginSurfaceKind::Okf,
        id: surface_id.to_owned(),
    };
    if !state.selected_surfaces.contains(&surface) {
        return Err(embedded_error(
            "use.plugin.embedded_okf_surface_unselected",
            "The requested OKF surface is not part of the published selected surface set.",
        ));
    }

    let paths = manager.registry().paths();
    let snapshot = manager.registry().snapshot().await?;
    if snapshot.generation != state.capability_generation
        || snapshot.descriptor_digest()? != state.capability_revision
    {
        return Err(embedded_error(
            "use.plugin.embedded_capability_snapshot_drift",
            "The capability snapshot changed while the cognitive lease was being acquired.",
        ));
    }
    let route = snapshot
        .routes
        .iter()
        .find(|route| route.package_id == package_id && route.enabled)
        .ok_or_else(|| {
            embedded_error(
                "use.plugin.embedded_capability_hidden",
                "The exact cognitive package generation is no longer published.",
            )
        })?;
    let generation = route.lifecycle_generation.ok_or_else(|| {
        embedded_error(
            "use.plugin.embedded_capability_evidence_invalid",
            "The published cognitive package route omitted its lifecycle generation.",
        )
    })?;
    if route.version != version
        || route.package_sha256.as_deref() != package_digest.strip_prefix("sha256:")
        || route.manifest_sha256 != manifest_digest.trim_start_matches("sha256:")
    {
        return Err(embedded_error(
            "use.plugin.embedded_capability_snapshot_drift",
            "The capability snapshot route differs from the selected package generation.",
        ));
    }

    let qualified = a3s_use_core::PlanQualifiedSurfaceRef {
        package_id: package_id.to_owned(),
        surface,
    };
    let store = OkfKnowledgeBindingStore::from_extension_paths(paths);
    let binding = store
        .get(selected_scope, &qualified, generation)
        .await?
        .ok_or_else(|| {
            embedded_error(
                "use.plugin.embedded_okf_projection_missing",
                "The exact published package generation has no retained OKF Knowledge binding.",
            )
        })?;
    let projection =
        OkfCapabilityProjection::from_promoted(&binding.receipt, &binding.observation)?;
    if projection.scope != *selected_scope
        || projection.surface != qualified
        || projection.generation != generation
        || projection.package_digest != package_digest
        || projection.manifest_digest != manifest_digest
    {
        return Err(embedded_error(
            "use.plugin.embedded_okf_projection_drift",
            "The promoted OKF projection does not match the exact managed package generation.",
        ));
    }

    let package_lock = manager
        .installed_package_lock(package_id)
        .await?
        .ok_or_else(|| {
            embedded_error(
                "use.plugin.embedded_package_lock_missing",
                "The published cognitive package has no retained installed package lock.",
            )
        })?;
    let locked = package_lock.package(package_id).ok_or_else(|| {
        embedded_error(
            "use.plugin.embedded_package_lock_drift",
            "The installed package lock omits its published root package.",
        )
    })?;
    if locked.version() != version
        || locked.catalog.record.package.sha256.as_deref() != Some(&package_digest)
        || locked.catalog.record.package.manifest_sha256.as_deref() != Some(&manifest_digest)
    {
        return Err(embedded_error(
            "use.plugin.embedded_package_lock_drift",
            "The installed package lock differs from the published package generation.",
        ));
    }

    let package_lock_digest = package_lock.descriptor_digest()?;
    let generation_digest = generation_digest(
        package_id,
        selected_scope,
        &version,
        generation,
        &package_digest,
        &manifest_digest,
        &package_lock_digest,
        state.capability_generation,
        &state.capability_revision,
        &projection,
    )?;
    let capability_snapshot_digest = capability_snapshot_digest(
        package_id,
        &version,
        generation,
        &generation_digest,
        &projection,
    )?;

    let client = OkfKnowledgeClient::new(std::sync::Arc::new(
        SqliteOkfKnowledgeAdapter::from_extension_paths(paths),
    ));
    let provider = OkfKnowledgeLeaseProvider::new(manager.registry().clone(), client);
    let Some(knowledge) = provider.acquire(&projection).await? else {
        return Ok(None);
    };
    if knowledge.scope() != selected_scope || knowledge.projection() != &projection {
        return Err(embedded_error(
            "use.plugin.embedded_lease_drift",
            "The acquired Knowledge lease differs from the reviewed capability projection.",
        ));
    }
    let evidence = CognitiveCapabilityEvidence {
        scope: selected_scope.clone(),
        package_id: package_id.to_owned(),
        package_version: version,
        lifecycle_generation: generation,
        package_digest,
        manifest_digest,
        package_lock,
        package_lock_digest,
        capability_generation: state.capability_generation,
        capability_revision: state.capability_revision,
        generation_digest,
        capability_snapshot_digest,
        projection,
    };
    Ok(Some(CognitiveCapabilityLease {
        evidence,
        knowledge,
    }))
}

fn merge_catalog_pages(
    source_revision: &str,
    pages: Vec<PluginCatalogPage>,
) -> UseResult<CognitiveCatalogSearchResult> {
    let mut snapshots = Vec::with_capacity(pages.len());
    let mut plugins = BTreeMap::new();
    let mut total_matches = 0_u64;
    let mut next_cursors = Vec::new();
    for page in pages {
        total_matches = total_matches
            .checked_add(page.total_matches)
            .ok_or_else(|| {
                embedded_error(
                    "use.plugin.embedded_catalog_count_overflow",
                    "The multi-Registry result count exceeded its bounded integer.",
                )
            })?;
        if let Some(cursor) = page.next_cursor {
            next_cursors.push(CognitiveCatalogPageCursor {
                registry_name: page.snapshot.metadata.registry_name.clone(),
                cursor,
            });
        }
        snapshots.push(page.snapshot);
        for plugin in page.plugins {
            let digest = plugin.descriptor_digest()?;
            plugins.entry(digest).or_insert(plugin);
        }
    }
    snapshots.sort_by(|left, right| {
        left.metadata
            .registry_name
            .cmp(&right.metadata.registry_name)
    });
    next_cursors.sort_by(|left, right| left.registry_name.cmp(&right.registry_name));
    let mut plugins = plugins.into_values().collect::<Vec<_>>();
    plugins.sort_by(|left, right| {
        left.record
            .package_id
            .cmp(&right.record.package_id)
            .then_with(|| right.record.version.cmp(&left.record.version))
            .then_with(|| {
                left.provenance
                    .registry_name
                    .cmp(&right.provenance.registry_name)
            })
    });
    Ok(CognitiveCatalogSearchResult {
        source_revision: prefixed_sha256(source_revision),
        snapshots,
        plugins,
        total_matches,
        next_cursors,
    })
}

#[allow(clippy::too_many_arguments)]
fn generation_digest(
    package_id: &str,
    scope: &PlanScope,
    version: &str,
    lifecycle_generation: u64,
    package_digest: &str,
    manifest_digest: &str,
    package_lock_digest: &str,
    capability_generation: u64,
    capability_revision: &str,
    projection: &OkfCapabilityProjection,
) -> UseResult<String> {
    canonical_digest(
        GENERATION_DIGEST_DOMAIN,
        &(
            package_id,
            scope,
            version,
            lifecycle_generation,
            package_digest,
            manifest_digest,
            package_lock_digest,
            capability_generation,
            capability_revision,
            projection.descriptor_digest()?,
        ),
    )
}

fn capability_snapshot_digest(
    package_id: &str,
    package_version: &str,
    lifecycle_generation: u64,
    generation_digest: &str,
    projection: &OkfCapabilityProjection,
) -> UseResult<String> {
    canonical_digest(
        CAPABILITY_SNAPSHOT_DIGEST_DOMAIN,
        &(
            package_id,
            package_version,
            lifecycle_generation,
            generation_digest,
            projection.surface.surface.id.as_str(),
            projection.bundle.format_version.as_str(),
            projection.bundle.content_digest.as_str(),
        ),
    )
}

fn canonical_digest<T: Serialize + ?Sized>(domain: &str, value: &T) -> UseResult<String> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        embedded_error(
            "use.plugin.embedded_digest_invalid",
            format!("Failed to serialize canonical cognitive evidence: {error}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(CANONICAL_DIGEST_PREFIX);
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain.as_bytes());
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn prefixed_sha256(value: &str) -> String {
    if value.starts_with("sha256:") {
        value.to_owned()
    } else {
        format!("sha256:{value}")
    }
}

fn embedded_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_digest_uses_the_code_owned_canonical_domain_and_field_order() {
        let projection: OkfCapabilityProjection = serde_json::from_value(serde_json::json!({
            "schema": "a3s.use.okf-capability-projection.v2",
            "scope": { "kind": "workspace", "id": "workspace:test" },
            "surface": {
                "packageId": "acme/knowledge",
                "surface": { "kind": "okf", "id": "domain-knowledge" }
            },
            "generation": 7,
            "packageDigest": format!("sha256:{}", "1".repeat(64)),
            "manifestDigest": format!("sha256:{}", "2".repeat(64)),
            "bundle": {
                "schema": "a3s.use.okf-bundle.v1",
                "formatVersion": "0.2",
                "root": "domain-knowledge",
                "contentDigest": format!("sha256:{}", "3".repeat(64)),
                "conceptCount": 1,
                "fileCount": 1,
                "expandedBytes": 1,
                "limits": {
                    "maxFiles": 1,
                    "maxConcepts": 1,
                    "maxExpandedBytes": 1,
                    "maxDocumentBytes": 1,
                    "maxLinksPerDocument": 1
                }
            },
            "projectionId": "projection:test",
            "projectionReceiptDigest": format!("sha256:{}", "4".repeat(64)),
            "indexSchema": "index:v1",
            "indexBuildId": "build:v1",
            "indexDigest": format!("sha256:{}", "5".repeat(64)),
            "observationDigest": format!("sha256:{}", "6".repeat(64))
        }))
        .unwrap();
        projection.validate().unwrap();
        let digest = capability_snapshot_digest(
            "acme/knowledge",
            "1.0.0",
            7,
            &format!("sha256:{}", "7".repeat(64)),
            &projection,
        )
        .unwrap();
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), 71);
    }

    #[test]
    fn catalog_merge_retains_verified_records_from_partial_registry_pages() {
        let page: PluginCatalogPage = serde_json::from_value(serde_json::json!({
            "snapshot": {
                "metadata": {
                    "registryName": "fixture",
                    "registryUrl": "https://registry.example/",
                    "rootSha256": format!("sha256:{}", "1".repeat(64)),
                    "rootVersion": 1,
                    "timestampVersion": 1,
                    "snapshotVersion": 1,
                    "targetsVersion": 1,
                    "packageTargets": 1
                },
                "source": "refreshed",
                "hostTarget": "aarch64-apple-darwin",
                "useVersion": "0.3.0",
                "catalogRecords": 1,
                "verifiedAtUnixSeconds": 1,
                "ageSeconds": 0,
                "snapshotDigest": format!("sha256:{}", "2".repeat(64))
            },
            "totalMatches": 2,
            "plugins": [],
            "nextCursor": "v1.cursor"
        }))
        .unwrap();
        let result = merge_catalog_pages(&"3".repeat(64), vec![page]).unwrap();
        assert_eq!(result.total_matches, 2);
        assert!(result.plugins.is_empty());
        assert_eq!(result.snapshots.len(), 1);
        assert_eq!(result.next_cursors[0].registry_name, "fixture");
    }
}
