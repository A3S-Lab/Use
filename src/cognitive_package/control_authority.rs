//! Control Store authority selection for cognitive-package lifecycle.
//!
//! Production installs one mutable authority: Control Store. Clean roots
//! activate Control. Any legacy authority leaf fails closed (wipe and
//! reinstall). Dual authority is never selected.

use a3s_runtime::RuntimeClientRegistry;
use a3s_use_core::{InstallationSnapshot, UseResult};
use a3s_use_extension::{
    load_cached_capability_description_trust_store, load_capability_description_trust_store,
    ExtensionPaths, RegistrySourceStore, VerifiedCapabilityDescriptionTrustStore,
};
use std::sync::Arc;

use crate::control_store::{
    control_database_present, legacy_authority_present, ProductionControlHostDependencies,
    ProductionControlLifecycle,
};
use crate::plugin_runtime::RuntimeSurfacePlanPublication;

use super::embedded::CognitiveRegistryAccess;
use super::grant::{
    CognitivePackageAuthorizationEvidence, PackageGraphAuthorization,
    PlannedWorkspaceGrantOperation,
};
use super::package_manager_error;
use super::plan::now_ms;
use super::store::PendingPackageGraphOperation;
use super::ControlRuntimeServiceReadinessPort;

/// Mutable authority selected for one installation state root.
///
/// Production cutover admits only Control. Legacy selection is rejected at
/// `select_installation_authority`; remaining call sites must not branch on a
/// second writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallationAuthorityKind {
    Control,
}

/// Inspect one installation state root and select sole Control authority.
pub(crate) fn select_installation_authority(
    state_root: &std::path::Path,
) -> UseResult<InstallationAuthorityKind> {
    let has_control = control_database_present(state_root);
    let has_legacy = legacy_authority_present(state_root);
    match (has_control, has_legacy) {
        (_, true) => Err(package_manager_error(
            "use.control_store.legacy_state_unsupported",
            "Legacy mutable authority is present; wipe the installation state root and reinstall under Control Store.",
        )),
        (true, false) | (false, false) => Ok(InstallationAuthorityKind::Control),
    }
}

/// Open or create the production Control lifecycle for one installation.
///
/// When `signed_trust` is set, the signed descriptor-snapshot projector is
/// used and only a Registry/TUF-verified trust store is accepted. Preview and
/// unsigned installations keep the unsigned projector via `None`.
pub(crate) async fn open_control_lifecycle(
    paths: &ExtensionPaths,
    runtime_registry: Arc<RuntimeClientRegistry>,
    flow_compiler: Option<&std::path::Path>,
) -> UseResult<ProductionControlLifecycle> {
    open_control_lifecycle_with_signed_trust(paths, runtime_registry, flow_compiler, None).await
}

/// Open Control with an optional Registry/TUF-verified description trust store.
pub(crate) async fn open_control_lifecycle_with_signed_trust(
    paths: &ExtensionPaths,
    runtime_registry: Arc<RuntimeClientRegistry>,
    flow_compiler: Option<&std::path::Path>,
    signed_trust: Option<VerifiedCapabilityDescriptionTrustStore>,
) -> UseResult<ProductionControlLifecycle> {
    open_control_lifecycle_with_host_ports(
        paths,
        runtime_registry,
        flow_compiler,
        signed_trust,
        None,
    )
    .await
}

/// Open Control with optional signed trust and an optional injected Runtime
/// Service readiness port for managed hosts.
pub(crate) async fn open_control_lifecycle_with_host_ports(
    paths: &ExtensionPaths,
    runtime_registry: Arc<RuntimeClientRegistry>,
    flow_compiler: Option<&std::path::Path>,
    signed_trust: Option<VerifiedCapabilityDescriptionTrustStore>,
    runtime_readiness: Option<Arc<dyn crate::control_store::ControlRuntimeServiceReadinessPort>>,
) -> UseResult<ProductionControlLifecycle> {
    let dependencies = match (signed_trust, runtime_readiness) {
        (Some(trust), Some(readiness)) => {
            let mut deps =
                ProductionControlHostDependencies::standalone_with_signed_catalog_from_registry(
                    paths,
                    runtime_registry,
                    trust,
                    flow_compiler,
                )?;
            deps.runtime_readiness = readiness;
            deps
        }
        (Some(trust), None) => {
            ProductionControlHostDependencies::standalone_with_signed_catalog_from_registry(
                paths,
                runtime_registry,
                trust,
                flow_compiler,
            )?
        }
        (None, Some(readiness)) => {
            ProductionControlHostDependencies::with_injected_runtime_readiness(
                paths,
                runtime_registry,
                readiness,
                flow_compiler,
            )?
        }
        (None, None) => {
            ProductionControlHostDependencies::standalone(paths, runtime_registry, flow_compiler)?
        }
    };
    let lifecycle = ProductionControlLifecycle::from_extension_paths(paths, dependencies)?;
    if !control_database_present(&paths.installation_state_root()) {
        lifecycle.initialize().await?;
    }
    Ok(lifecycle)
}

/// Reject managed Runtime plan publications when Control has no live readiness.
///
/// Opaque `gateway:` minting is valid only for skill/native-only roots that
/// admit no Runtime Service publications. Managed Tool/MCP publications must
/// bind through an injected [`ControlRuntimeServiceReadinessPort`].
pub(crate) fn require_control_runtime_readiness_for_publications(
    readiness: Option<&Arc<dyn ControlRuntimeServiceReadinessPort>>,
    publications: &[RuntimeSurfacePlanPublication],
) -> UseResult<()> {
    if publications.is_empty() {
        return Ok(());
    }
    if readiness.is_none() {
        return Err(package_manager_error(
            "use.control_store.runtime_readiness_required",
            "Managed Runtime surface publications require an injected Control Runtime Service readiness port; opaque gateway: minting is rejected.",
        ));
    }
    Ok(())
}

/// Load the signed description trust store from a configured TrustedRegistry.
///
/// Product Control open must not inject fixture keys. When no Registry sources
/// are configured and no name is selected, returns `None` so preview roots keep
/// the unsigned projector. When sources exist, resolves the selected or default
/// TrustedRegistry and loads `capability/description-trust-store-v1.json`
/// through the Registry/TUF path (fail closed).
pub(crate) async fn load_signed_description_trust_for_control(
    paths: &ExtensionPaths,
    registry_name: Option<&str>,
    access: CognitiveRegistryAccess,
) -> UseResult<Option<VerifiedCapabilityDescriptionTrustStore>> {
    let store = RegistrySourceStore::new(paths.use_paths().clone());
    let snapshot = store.snapshot().await?;
    if registry_name.is_none() && snapshot.sources.is_empty() {
        return Ok(None);
    }
    let sources = store.resolve(registry_name).await?;
    let trust = match access {
        CognitiveRegistryAccess::Refreshed => {
            load_capability_description_trust_store(sources.root()).await?
        }
        CognitiveRegistryAccess::Cached => {
            load_cached_capability_description_trust_store(sources.root()).await?
        }
    };
    trust.validate()?;
    Ok(Some(trust))
}

/// Convert durable pending authorization into Control admission inputs.
pub(crate) fn control_admission_from_pending(
    pending: &PendingPackageGraphOperation,
) -> UseResult<(
    CognitivePackageAuthorizationEvidence,
    Option<PlannedWorkspaceGrantOperation>,
)> {
    control_admission_from_authorization(&pending.authorization)
}

/// Convert in-memory authorization (enablement) into Control admission inputs.
pub(crate) fn control_admission_from_authorization(
    authorization: &PackageGraphAuthorization,
) -> UseResult<(
    CognitivePackageAuthorizationEvidence,
    Option<PlannedWorkspaceGrantOperation>,
)> {
    let evidence = CognitivePackageAuthorizationEvidence {
        operation_confirmation: authorization.operation_confirmation.clone(),
        grant_confirmations: authorization.grant_confirmations.clone(),
    };
    let grants = planned_grants_from_authorization(authorization)?;
    Ok((evidence, grants))
}

fn planned_grants_from_authorization(
    authorization: &PackageGraphAuthorization,
) -> UseResult<Option<PlannedWorkspaceGrantOperation>> {
    match (
        &authorization.grant_snapshot,
        &authorization.grant_change_set,
    ) {
        (None, None) => Ok(None),
        (Some(snapshot), Some(change_set)) => Ok(Some(PlannedWorkspaceGrantOperation {
            snapshot: snapshot.clone(),
            change_set: change_set.clone(),
            ceilings: authorization.grant_ceilings.clone(),
        })),
        _ => Err(package_manager_error(
            "use.plugin.authorization_invalid",
            "Persisted Grant authorization is incomplete for Control admission.",
        )),
    }
}

/// Apply one admitted graph operation through Control as sole authority.
pub(crate) async fn apply_pending_through_control(
    lifecycle: &ProductionControlLifecycle,
    pending: &PendingPackageGraphOperation,
    publications: &[RuntimeSurfacePlanPublication],
    maintenance: std::sync::Arc<a3s_use_extension::StateMaintenanceGuard>,
) -> UseResult<InstallationSnapshot> {
    let (evidence, grants) = control_admission_from_pending(pending)?;
    let reviewed_at_ms = if pending.admitted_at_ms == 0 {
        pending.planned_at_ms
    } else {
        pending.admitted_at_ms
    };
    let committed_at_ms = now_ms()?;
    lifecycle
        .apply_reviewed_operation(
            &pending.envelope,
            &evidence,
            grants.as_ref(),
            reviewed_at_ms,
            committed_at_ms,
            publications,
            maintenance,
        )
        .await
}

type ArcRuntimeRegistry = Arc<RuntimeClientRegistry>;

#[cfg(test)]
mod tests {
    use super::*;
    use a3s_use_core::{InstallationId, InstallationKind};
    use a3s_use_extension::ExtensionPaths;

    #[test]
    fn empty_publications_allow_absent_control_readiness() {
        require_control_runtime_readiness_for_publications(None, &[]).unwrap();
    }

    #[test]
    fn clean_root_selects_control() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        assert_eq!(
            select_installation_authority(&state).unwrap(),
            InstallationAuthorityKind::Control
        );
    }

    #[test]
    fn legacy_snapshot_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("installation-snapshot.json"), "{}").unwrap();
        let error = select_installation_authority(&state).unwrap_err();
        assert_eq!(error.code, "use.control_store.legacy_state_unsupported");
    }

    #[test]
    fn dual_authority_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("installation-snapshot.json"), "{}").unwrap();
        std::fs::write(state.join("control.sqlite3"), []).unwrap();
        let error = select_installation_authority(&state).unwrap_err();
        assert_eq!(error.code, "use.control_store.legacy_state_unsupported");
    }

    #[tokio::test]
    async fn open_control_lifecycle_initializes_clean_root() {
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            InstallationId::new(InstallationKind::User, "user/control-authority").unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation,
        )
        .unwrap();
        let lifecycle = open_control_lifecycle(
            &paths,
            std::sync::Arc::new(RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        assert!(control_database_present(&paths.installation_state_root()));
        assert!(lifecycle.current_snapshot().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn control_registry_diagnostic_face_binds_empty_control_without_published_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            InstallationId::new(InstallationKind::User, "user/control-diagnostic-face").unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation,
        )
        .unwrap();
        open_control_lifecycle(
            &paths,
            std::sync::Arc::new(RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        let manager = crate::cognitive_package::CognitivePackageManager::with_lifecycle(
            a3s_use_extension::ExtensionRegistry::new(paths),
            std::sync::Arc::new(
                crate::cognitive_package::StandaloneCognitivePackageLifecycleFactory::default(),
            ),
        )
        .unwrap();
        let (generation, digest, pending) =
            manager.control_registry_diagnostic_face().await.unwrap();
        assert_eq!(generation, 0);
        assert!(pending.is_empty());
        assert!(digest.starts_with("sha256:"));
        let empty = a3s_use_extension::ExtensionRegistrySnapshot::empty(
            manager.registry().installation().clone(),
        )
        .unwrap();
        assert_eq!(digest, empty.descriptor_digest().unwrap());
    }

    #[tokio::test]
    async fn signed_trust_load_stays_optional_without_registry_sources() {
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            InstallationId::new(InstallationKind::User, "user/signed-trust-optional").unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation,
        )
        .unwrap();
        let trust = load_signed_description_trust_for_control(
            &paths,
            None,
            super::CognitiveRegistryAccess::Cached,
        )
        .await
        .expect("empty Registry configuration must keep unsigned Control open");
        assert!(trust.is_none());
    }

    #[tokio::test]
    async fn signed_trust_load_fails_closed_for_unknown_registry_name() {
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            InstallationId::new(InstallationKind::User, "user/signed-trust-missing").unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation,
        )
        .unwrap();
        let error = load_signed_description_trust_for_control(
            &paths,
            Some("missing-official"),
            super::CognitiveRegistryAccess::Refreshed,
        )
        .await
        .expect_err("explicit Registry selection must fail closed");
        assert_eq!(error.code, "use.extension.registry_source_not_found");
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn published_gateway_serve_fails_closed_without_catalog() {
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            InstallationId::new(InstallationKind::User, "user/gateway-serve").unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation,
        )
        .unwrap();
        let lifecycle = open_control_lifecycle(
            &paths,
            std::sync::Arc::new(RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        let opened = lifecycle
            .open_published_capability_gateway(
                crate::capability_gateway::CapabilityGatewayCompositionOptions::default(),
            )
            .await
            .unwrap();
        assert!(opened.is_none());
        let error = lifecycle
            .serve_published_capability_gateway_stdio(
                crate::capability_gateway::CapabilityGatewayCompositionOptions::default(),
            )
            .await
            .expect_err("stdio serve requires a published Control catalog");
        assert_eq!(
            error.code,
            "use.control.capability_gateway_publication_missing"
        );
    }

    #[cfg(feature = "mcp")]
    #[tokio::test]
    async fn production_gateway_cutover_activation_available_after_open() {
        // Product face exposes gateway_cutover_activation for long-lived hosts.
        // Empty roots have no publication, so open returns None; the HTTP serve
        // path attaches reconcile+drain only after a catalog is published.
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            InstallationId::new(InstallationKind::User, "user/gateway-cutover").unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation,
        )
        .unwrap();
        let lifecycle = open_control_lifecycle(
            &paths,
            std::sync::Arc::new(RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        let opened = lifecycle
            .open_published_capability_gateway(
                crate::capability_gateway::CapabilityGatewayCompositionOptions::default(),
            )
            .await
            .unwrap();
        assert!(opened.is_none());
        let error = lifecycle
            .serve_published_capability_gateway_streamable_http(
                tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
                crate::capability_gateway::CapabilityGatewayHttpConfig::for_principal(
                    "unused-token",
                    "agent/cli",
                )
                .unwrap(),
                tokio_util::sync::CancellationToken::new(),
                crate::capability_gateway::CapabilityGatewayCompositionOptions::default(),
            )
            .await
            .expect_err("HTTP serve requires a published Control catalog");
        assert_eq!(
            error.code,
            "use.control.capability_gateway_publication_missing"
        );
    }

    #[tokio::test]
    async fn control_authority_knowledge_grants_observe_via_control_only() {
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            InstallationId::new(InstallationKind::User, "user/knowledge-control").unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation,
        )
        .unwrap();
        let _manager =
            crate::okf_knowledge::OkfKnowledgeRecoveryManager::for_control_authority(&paths);
        assert!(!control_database_present(&paths.installation_state_root()));
        let lifecycle = open_control_lifecycle(
            &paths,
            std::sync::Arc::new(RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        assert!(control_database_present(&paths.installation_state_root()));
        assert!(lifecycle
            .observe_stored_workspace_grant(
                "workspace",
                "acme/knowledge",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn planned_grant_snapshot_does_not_create_legacy_grants_leaf() {
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            InstallationId::new(InstallationKind::User, "user/grant-snapshot").unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation,
        )
        .unwrap();
        let lifecycle = open_control_lifecycle(
            &paths,
            std::sync::Arc::new(RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        let snapshot = lifecycle
            .planned_grant_snapshot("workspace", 1)
            .await
            .unwrap();
        assert_eq!(snapshot.scope_id, "workspace");
        assert_eq!(snapshot.state_revision, 1);
        assert!(snapshot.grants.is_empty());
        assert!(
            !paths.installation_state_root().join("grants").exists(),
            "Control planned_grant_snapshot must not materialize legacy grants/"
        );
    }
}
