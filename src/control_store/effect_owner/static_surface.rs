use a3s_use_core::{PluginOperationAction, PluginSurfaceKind, UseError, UseResult};
use a3s_use_extension::ArtifactStore;
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::super::effect_port::{
    ControlEffectFailure, ControlEffectPortOutcome, ControlSkillEffectPort,
    ControlSurfaceApplication, ControlSurfaceEffectAction, ControlSurfaceEffectRequest,
    ControlUiEffectPort,
};
use super::super::model::{valid_error_code, ControlEffectOwner};
use crate::plugin_lifecycle::PluginLifecycleAction;

const STATIC_SURFACE_RECEIPT_DOMAIN: &[u8] = b"a3s.use.control-static-surface-receipt.v1\0";
const STATIC_SURFACE_FAILURE_DOMAIN: &[u8] = b"a3s.use.control-static-surface-failure.v1\0";
const STATIC_AUTHORITY_ERROR: &str = "use.control_store.static_authority_invalid";
const STATIC_OWNER_ERROR: &str = "use.control_store.static_owner_failed";

/// Inactive Control owner for immutable Skill and UI package contributions.
///
/// Preparation derives content evidence only through a verified Artifact
/// Store lease. Stop and remove are projection-owned no-ops whose portable
/// receipts depend only on committed Control authority. This adapter never
/// reads the legacy lifecycle Registry and never exposes a package path.
#[derive(Debug, Clone)]
pub(in crate::control_store) struct ControlStaticSurfaceEffectPort {
    artifact_store: ArtifactStore,
}

impl ControlStaticSurfaceEffectPort {
    pub(in crate::control_store) fn new(artifact_store: ArtifactStore) -> Self {
        Self { artifact_store }
    }

    async fn apply(
        &self,
        request: &ControlSurfaceEffectRequest,
        expected_kind: PluginSurfaceKind,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        let expected_owner = match expected_kind {
            PluginSurfaceKind::Skill => ControlEffectOwner::SkillHost,
            PluginSurfaceKind::Ui => ControlEffectOwner::UiHost,
            PluginSurfaceKind::Flow
            | PluginSurfaceKind::Mcp
            | PluginSurfaceKind::Okf
            | PluginSurfaceKind::Tool => return rejected(request, STATIC_AUTHORITY_ERROR),
        };
        if request
            .validate_for_owner(expected_kind, expected_owner)
            .is_err()
        {
            return rejected(request, STATIC_AUTHORITY_ERROR);
        }

        let materialization_digest = match request.action {
            ControlSurfaceEffectAction::Prepare => {
                let catalog = &request.authority.package.package.catalog;
                let package = match self.artifact_store.acquire_verified_package(catalog).await {
                    Ok(package) => package,
                    Err(error) => return failed(request, error),
                };
                let evidence = match expected_kind {
                    PluginSurfaceKind::Skill => {
                        package.inspect_skill_surface(&request.surface.id).await
                    }
                    PluginSurfaceKind::Ui => package.inspect_ui_surface(&request.surface.id).await,
                    PluginSurfaceKind::Flow
                    | PluginSurfaceKind::Mcp
                    | PluginSurfaceKind::Okf
                    | PluginSurfaceKind::Tool => return rejected(request, STATIC_AUTHORITY_ERROR),
                };
                match evidence {
                    Ok(evidence) => Some(evidence.digest().to_string()),
                    Err(error) => return failed(request, error),
                }
            }
            ControlSurfaceEffectAction::Stop | ControlSurfaceEffectAction::Remove => None,
        };
        let receipt_digest = match receipt_digest(request, materialization_digest.as_deref()) {
            Ok(digest) => digest,
            Err(error) => return failed(request, error),
        };
        match ControlSurfaceApplication::new(request, receipt_digest, materialization_digest) {
            Ok(application) => ControlEffectPortOutcome::applied(application),
            Err(_) => rejected(request, STATIC_AUTHORITY_ERROR),
        }
    }
}

#[async_trait]
impl ControlSkillEffectPort for ControlStaticSurfaceEffectPort {
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        self.apply(request, PluginSurfaceKind::Skill).await
    }
}

#[async_trait]
impl ControlUiEffectPort for ControlStaticSurfaceEffectPort {
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        self.apply(request, PluginSurfaceKind::Ui).await
    }
}

fn receipt_digest(
    request: &ControlSurfaceEffectRequest,
    materialization_digest: Option<&str>,
) -> UseResult<String> {
    let catalog_digest = request
        .authority
        .package
        .package
        .catalog
        .descriptor_digest()
        .map_err(|_| authority_error())?;
    let mut hasher = Sha256::new();
    hasher.update(STATIC_SURFACE_RECEIPT_DOMAIN);
    hash_field(&mut hasher, surface_kind(request.surface.kind));
    hash_field(&mut hasher, surface_action(request.action));
    hash_field(&mut hasher, &request.identity.operation_id);
    hash_field(&mut hasher, request.identity.installation.kind.as_str());
    hash_field(&mut hasher, &request.identity.installation.id);
    hash_field(&mut hasher, &request.identity.plan_digest);
    hash_field(
        &mut hasher,
        operation_action(request.identity.operation_action),
    );
    hash_u64(&mut hasher, request.identity.installation_generation);
    hash_u64(&mut hasher, u64::from(request.identity.sequence));
    hash_field(&mut hasher, &request.identity.idempotency_key);
    hash_field(&mut hasher, &request.authority.generation_operation_id);
    hash_field(&mut hasher, &request.authority.snapshot_digest);
    hash_u64(&mut hasher, request.authority.committed_at_ms);
    hash_field(&mut hasher, &request.authority.host.target);
    hash_field(&mut hasher, &request.authority.host.use_version);
    hash_field(&mut hasher, &catalog_digest);
    hash_field(&mut hasher, &request.package_id);
    hash_u64(&mut hasher, request.lifecycle_generation);
    hash_field(&mut hasher, &request.package_digest);
    hash_field(&mut hasher, &request.manifest_digest);
    hash_field(&mut hasher, lifecycle_action(request.lifecycle_action));
    hash_field(&mut hasher, &request.surface.id);
    hash_field(
        &mut hasher,
        request
            .authority
            .grant
            .as_ref()
            .map(|grant| grant.grant_digest.as_str())
            .unwrap_or("none"),
    );
    hash_u64(
        &mut hasher,
        request
            .authority
            .grant
            .as_ref()
            .map_or(0, |grant| grant.receipt_revision),
    );
    hash_field(&mut hasher, materialization_digest.unwrap_or("none"));
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn failed(
    request: &ControlSurfaceEffectRequest,
    error: UseError,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    let error_code = if valid_error_code(&error.code) {
        error.code
    } else {
        STATIC_OWNER_ERROR.to_string()
    };
    let failure = failure(request, &error_code);
    if matches!(
        error_code.as_str(),
        "use.artifact_store.busy" | "use.artifact_store.io" | "use.extension.io"
    ) {
        ControlEffectPortOutcome::deferred(failure)
    } else {
        ControlEffectPortOutcome::rejected(failure)
    }
}

fn rejected(
    request: &ControlSurfaceEffectRequest,
    error_code: &str,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    ControlEffectPortOutcome::rejected(failure(request, error_code))
}

fn failure(request: &ControlSurfaceEffectRequest, error_code: &str) -> ControlEffectFailure {
    let mut hasher = Sha256::new();
    hasher.update(STATIC_SURFACE_FAILURE_DOMAIN);
    hash_field(&mut hasher, &request.identity.idempotency_key);
    hash_field(&mut hasher, error_code);
    ControlEffectFailure {
        evidence_digest: format!("sha256:{:x}", hasher.finalize()),
        error_code: error_code.to_string(),
    }
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn hash_u64(hasher: &mut Sha256, value: u64) {
    hasher.update(value.to_be_bytes());
}

const fn operation_action(action: PluginOperationAction) -> &'static str {
    match action {
        PluginOperationAction::Install => "install",
        PluginOperationAction::Uninstall => "uninstall",
        PluginOperationAction::Upgrade => "upgrade",
        PluginOperationAction::Enable => "enable",
        PluginOperationAction::Disable => "disable",
    }
}

const fn lifecycle_action(action: PluginLifecycleAction) -> &'static str {
    match action {
        PluginLifecycleAction::Install => "install",
        PluginLifecycleAction::Upgrade => "upgrade",
        PluginLifecycleAction::Enable => "enable",
        PluginLifecycleAction::Disable => "disable",
        PluginLifecycleAction::Uninstall => "uninstall",
    }
}

const fn surface_action(action: ControlSurfaceEffectAction) -> &'static str {
    match action {
        ControlSurfaceEffectAction::Prepare => "prepare",
        ControlSurfaceEffectAction::Stop => "stop",
        ControlSurfaceEffectAction::Remove => "remove",
    }
}

const fn surface_kind(kind: PluginSurfaceKind) -> &'static str {
    match kind {
        PluginSurfaceKind::Flow => "flow",
        PluginSurfaceKind::Mcp => "mcp",
        PluginSurfaceKind::Okf => "okf",
        PluginSurfaceKind::Skill => "skill",
        PluginSurfaceKind::Tool => "tool",
        PluginSurfaceKind::Ui => "ui",
    }
}

fn authority_error() -> UseError {
    UseError::new(
        STATIC_AUTHORITY_ERROR,
        "Static surface execution requires one exact committed package authority.",
    )
}
