use a3s_use_core::{
    CapabilityGatewayCatalog, InstallationId, InstallationKind, PluginOperationAction,
    PluginSurfaceKind, PluginSurfaceRef, UseResult,
};
use async_trait::async_trait;
use semver::{Version, VersionReq};

use crate::plugin_lifecycle::PluginLifecycleAction;

use super::model::{
    input_error, valid_error_code, valid_machine_id, valid_sha256, ControlCapabilityCatalogBinding,
    ControlCapabilityEffectAuthority, ControlEffectIntent, ControlEffectKind, ControlEffectOwner,
    ControlEffectSubject, ControlPackageEffectAuthority, ControlRuntimeBindingObservation,
    ControlRuntimeEffectAuthority, ControlRuntimeSchemaAttestation,
};

/// Classification returned by an external effect owner.
///
/// `Deferred` and `Rejected` both mean the owner can prove that it accepted no
/// effect. A deferral is transient and becomes eligible for bounded same-key
/// retry; rejection is terminal under the committed policy. `Unknown` means
/// acceptance is ambiguous and therefore requires explicit same-key
/// reconciliation. Provider ports return this enum directly so an ordinary
/// transport error can never be mistaken for a safe no-effect result.
pub(in crate::control_store) enum ControlEffectPortOutcome<T> {
    Applied(T),
    /// The owner proves that it accepted no effect, but a bounded same-key
    /// retry may succeed after transient contention or unavailability.
    Deferred(ControlEffectFailure),
    Rejected(ControlEffectFailure),
    Unknown(ControlEffectFailure),
}

impl<T> ControlEffectPortOutcome<T> {
    pub(in crate::control_store) fn applied(application: T) -> Self {
        Self::Applied(application)
    }

    pub(in crate::control_store) fn rejected(failure: ControlEffectFailure) -> Self {
        Self::Rejected(failure)
    }

    pub(in crate::control_store) fn deferred(failure: ControlEffectFailure) -> Self {
        Self::Deferred(failure)
    }

    pub(in crate::control_store) fn unknown(failure: ControlEffectFailure) -> Self {
        Self::Unknown(failure)
    }

    pub(in crate::control_store) fn map<U>(
        self,
        map: impl FnOnce(T) -> U,
    ) -> ControlEffectPortOutcome<U> {
        match self {
            Self::Applied(application) => ControlEffectPortOutcome::Applied(map(application)),
            Self::Deferred(failure) => ControlEffectPortOutcome::Deferred(failure),
            Self::Rejected(failure) => ControlEffectPortOutcome::Rejected(failure),
            Self::Unknown(failure) => ControlEffectPortOutcome::Unknown(failure),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlEffectFailure {
    pub(in crate::control_store) evidence_digest: String,
    pub(in crate::control_store) error_code: String,
}

impl ControlEffectFailure {
    pub(in crate::control_store) fn new(
        evidence_digest: impl Into<String>,
        error_code: impl Into<String>,
    ) -> UseResult<Self> {
        let failure = Self {
            evidence_digest: evidence_digest.into(),
            error_code: error_code.into(),
        };
        if !valid_sha256(&failure.evidence_digest) || !valid_error_code(&failure.error_code) {
            return Err(input_error(
                "Control effect failure evidence is invalid or unbounded.",
            ));
        }
        Ok(failure)
    }
}

/// Identity shared by every typed owner request.
///
/// The deadline is claim metadata, not desired state. Owners must finish
/// before it, while every durable identity remains the exact committed outbox
/// identity across retries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlEffectRequestIdentity {
    pub(in crate::control_store) operation_id: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) operation_action: PluginOperationAction,
    pub(in crate::control_store) installation_generation: u64,
    pub(in crate::control_store) sequence: u32,
    pub(in crate::control_store) idempotency_key: String,
    pub(in crate::control_store) required: bool,
    pub(in crate::control_store) attempt: u32,
    pub(in crate::control_store) deadline_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityCutoverRequest {
    pub(in crate::control_store) identity: ControlEffectRequestIdentity,
    pub(in crate::control_store) authority: ControlCapabilityEffectAuthority,
    pub(in crate::control_store) expected_capability_generation: u64,
    pub(in crate::control_store) capability_generation: u64,
    pub(in crate::control_store) descriptor_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlInvocationDrainRequest {
    pub(in crate::control_store) identity: ControlEffectRequestIdentity,
    pub(in crate::control_store) authority: ControlPackageEffectAuthority,
    pub(in crate::control_store) package_id: String,
    pub(in crate::control_store) lifecycle_generation: u64,
    pub(in crate::control_store) package_digest: String,
    pub(in crate::control_store) manifest_digest: String,
    pub(in crate::control_store) lifecycle_action: PluginLifecycleAction,
}

impl ControlInvocationDrainRequest {
    /// Revalidate one exact prior package incarnation at the invocation owner
    /// boundary before attempting to exclude active calls.
    pub(in crate::control_store) fn validate_for_owner(&self) -> UseResult<()> {
        let authority = &self.authority;
        let catalog = &authority.package.package.catalog;
        let selected = catalog
            .selected_state(&authority.package.selected_surfaces)
            .map_err(|_| invocation_authority_error())?;
        let grant_required = authority.package.enabled && !selected.permissions.surfaces.is_empty();
        let grant_matches = match authority.grant.as_ref() {
            None => !grant_required,
            Some(selection) => {
                grant_required
                    && self.identity.installation.kind == InstallationKind::Workspace
                    && selection.grant.scope_id == self.identity.installation.id
                    && selection.receipt_revision > 0
                    && selection.receipt_revision
                        <= authority.installation_generation.saturating_add(1)
                    && selection.package_id() == self.package_id
                    && selection.grant.package_digest == self.package_digest
                    && selection.grant.descriptor_digest().is_ok_and(|digest| {
                        digest == selection.grant_digest
                            && selection
                                .grant
                                .validate_active_against(
                                    &catalog.record.permission_ceiling,
                                    authority.committed_at_ms,
                                )
                                .is_ok()
                    })
            }
        };
        let subject = ControlEffectSubject::Package {
            package_id: self.package_id.clone(),
            lifecycle_generation: self.lifecycle_generation,
            package_digest: self.package_digest.clone(),
            manifest_digest: self.manifest_digest.clone(),
            action: self.lifecycle_action,
        };
        let idempotency_matches = ControlEffectIntent::new(
            self.identity.sequence,
            self.identity.installation.clone(),
            self.identity.plan_digest.clone(),
            self.identity.operation_action,
            self.identity.installation_generation,
            subject,
            ControlEffectOwner::InvocationLeases,
            ControlEffectKind::CallsDrain,
            self.identity.required,
        )
        .is_ok_and(|intent| intent.idempotency_key == self.identity.idempotency_key);
        let lifecycle_matches = matches!(
            (self.identity.operation_action, self.lifecycle_action),
            (
                PluginOperationAction::Upgrade,
                PluginLifecycleAction::Uninstall
            ) | (
                PluginOperationAction::Disable,
                PluginLifecycleAction::Disable
            ) | (
                PluginOperationAction::Uninstall,
                PluginLifecycleAction::Uninstall
            )
        );
        let host_matches =
            VersionReq::parse(&catalog.record.requires_use).is_ok_and(|requirement| {
                Version::parse(&authority.host.use_version)
                    .is_ok_and(|version| requirement.matches(&version))
            }) && (catalog.record.target == "any"
                || catalog.record.target == authority.host.target);
        if !valid_machine_id(&self.identity.operation_id)
            || !valid_machine_id(&authority.generation_operation_id)
            || self.identity.attempt == 0
            || self.identity.deadline_at_ms == 0
            || !self.identity.required
            || self.identity.installation_generation != authority.installation_generation
            || authority.installation_generation == 0
            || !valid_sha256(&authority.snapshot_digest)
            || authority.committed_at_ms == 0
            || authority.host.validate().is_err()
            || !host_matches
            || authority.package.validate().is_err()
            || authority.package.package_id() != self.package_id
            || authority.lifecycle_generation != self.lifecycle_generation
            || catalog.record.package.sha256.as_deref() != Some(self.package_digest.as_str())
            || catalog.record.package.manifest_sha256.as_deref()
                != Some(self.manifest_digest.as_str())
            || !grant_matches
            || !lifecycle_matches
            || !idempotency_matches
        {
            return Err(invocation_authority_error());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::control_store) enum ControlSurfaceEffectAction {
    Prepare,
    Stop,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlSurfaceEffectRequest {
    pub(in crate::control_store) identity: ControlEffectRequestIdentity,
    pub(in crate::control_store) authority: ControlPackageEffectAuthority,
    pub(in crate::control_store) package_id: String,
    pub(in crate::control_store) lifecycle_generation: u64,
    pub(in crate::control_store) package_digest: String,
    pub(in crate::control_store) manifest_digest: String,
    pub(in crate::control_store) lifecycle_action: PluginLifecycleAction,
    pub(in crate::control_store) surface: PluginSurfaceRef,
    pub(in crate::control_store) action: ControlSurfaceEffectAction,
}

impl ControlSurfaceEffectRequest {
    /// Revalidate the portable request at an external owner boundary.
    ///
    /// The dispatcher derives this value from a committed effect and its
    /// owner-shaped authority. Keeping the binding rules on the shared request
    /// prevents Runtime, Flow, Knowledge, Skill, and UI adapters from growing
    /// subtly different copies of the same authority checks.
    pub(in crate::control_store) fn validate_for_owner(
        &self,
        expected_kind: PluginSurfaceKind,
        expected_owner: ControlEffectOwner,
    ) -> UseResult<()> {
        let authority = &self.authority;
        let catalog = &authority.package.package.catalog;
        let catalog_record = &catalog.record;
        let catalog_package = &catalog.record.package;
        let selected = catalog
            .selected_state(&authority.package.selected_surfaces)
            .map_err(|_| surface_authority_error())?;
        let grant_required = authority.package.enabled && !selected.permissions.surfaces.is_empty();
        let grant_matches = match authority.grant.as_ref() {
            None => !grant_required,
            Some(selection) => {
                grant_required
                    && self.identity.installation.kind == InstallationKind::Workspace
                    && selection.grant.scope_id == self.identity.installation.id
                    && selection.receipt_revision > 0
                    && selection.receipt_revision
                        <= authority.installation_generation.saturating_add(1)
                    && selection.package_id() == self.package_id
                    && selection.grant.package_digest == self.package_digest
                    && selection.grant.descriptor_digest().is_ok_and(|digest| {
                        digest == selection.grant_digest
                            && selection
                                .grant
                                .validate_active_against(
                                    &catalog.record.permission_ceiling,
                                    authority.committed_at_ms,
                                )
                                .is_ok()
                    })
            }
        };
        let effect_kind = match self.action {
            ControlSurfaceEffectAction::Prepare => ControlEffectKind::SurfacePrepare,
            ControlSurfaceEffectAction::Stop => ControlEffectKind::SurfaceStop,
            ControlSurfaceEffectAction::Remove => ControlEffectKind::SurfaceRemove,
        };
        let subject = ControlEffectSubject::Surface {
            package_id: self.package_id.clone(),
            lifecycle_generation: self.lifecycle_generation,
            package_digest: self.package_digest.clone(),
            manifest_digest: self.manifest_digest.clone(),
            action: self.lifecycle_action,
            surface: self.surface.clone(),
        };
        let idempotency_matches = ControlEffectIntent::new(
            self.identity.sequence,
            self.identity.installation.clone(),
            self.identity.plan_digest.clone(),
            self.identity.operation_action,
            self.identity.installation_generation,
            subject,
            expected_owner,
            effect_kind,
            self.identity.required,
        )
        .is_ok_and(|intent| intent.idempotency_key == self.identity.idempotency_key);
        let host_matches =
            VersionReq::parse(&catalog_record.requires_use).is_ok_and(|requirement| {
                Version::parse(&authority.host.use_version)
                    .is_ok_and(|version| requirement.matches(&version))
            }) && (catalog_record.target == "any"
                || catalog_record.target == authority.host.target);
        if !valid_machine_id(&self.identity.operation_id)
            || !valid_machine_id(&authority.generation_operation_id)
            || self.identity.attempt == 0
            || self.identity.deadline_at_ms == 0
            || self.identity.installation_generation != authority.installation_generation
            || authority.installation_generation == 0
            || !valid_sha256(&authority.snapshot_digest)
            || authority.committed_at_ms == 0
            || authority.host.validate().is_err()
            || !host_matches
            || authority.package.validate().is_err()
            || (!authority.package.enabled && self.action == ControlSurfaceEffectAction::Prepare)
            || authority.package.package_id() != self.package_id
            || authority.lifecycle_generation != self.lifecycle_generation
            || catalog_package.sha256.as_deref() != Some(self.package_digest.as_str())
            || catalog_package.manifest_sha256.as_deref() != Some(self.manifest_digest.as_str())
            || self.surface.kind != expected_kind
            || !authority.package.selected_surfaces.contains(&self.surface)
            || !grant_matches
            || !idempotency_matches
        {
            return Err(surface_authority_error());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlRuntimeEffectRequest {
    pub(in crate::control_store) surface: ControlSurfaceEffectRequest,
    pub(in crate::control_store) authority: ControlRuntimeEffectAuthority,
    pub(in crate::control_store) provider_id: String,
    pub(in crate::control_store) selection_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlReceiptApplication {
    pub(in crate::control_store) receipt_digest: String,
}

/// Evidence produced by the concrete Capability Plane after both immutable
/// payloads have reached durable storage.
///
/// The dispatcher records this value in the later observation transaction;
/// that transaction is the only point at which the catalog binding becomes a
/// published Control cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityCutoverApplication {
    pub(in crate::control_store) receipt_digest: String,
    pub(in crate::control_store) catalog: ControlCapabilityCatalogBinding,
}

impl ControlCapabilityCutoverApplication {
    pub(in crate::control_store) fn new(
        request: &ControlCapabilityCutoverRequest,
        receipt_digest: impl Into<String>,
        catalog: ControlCapabilityCatalogBinding,
    ) -> UseResult<Self> {
        let application = Self {
            receipt_digest: receipt_digest.into(),
            catalog,
        };
        if !valid_sha256(&application.receipt_digest)
            || application.catalog.validate().is_err()
            || !application.catalog.matches_generation(
                &request.identity.installation,
                request.capability_generation,
            )
        {
            return Err(input_error(
                "Capability cutover evidence does not bind its committed generation.",
            ));
        }
        Ok(application)
    }
}

impl ControlReceiptApplication {
    pub(in crate::control_store) fn new(receipt_digest: impl Into<String>) -> UseResult<Self> {
        let application = Self {
            receipt_digest: receipt_digest.into(),
        };
        if !valid_sha256(&application.receipt_digest) {
            return Err(input_error(
                "Control effect receipt evidence must be a canonical SHA-256 digest.",
            ));
        }
        Ok(application)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlRuntimeApplication {
    pub(in crate::control_store) receipt_digest: String,
    pub(in crate::control_store) binding: Option<ControlRuntimeBindingObservation>,
    pub(in crate::control_store) schema_attestation: Option<ControlRuntimeSchemaAttestation>,
}

impl ControlRuntimeApplication {
    pub(in crate::control_store) fn new(
        request: &ControlRuntimeEffectRequest,
        receipt_digest: impl Into<String>,
        binding: Option<ControlRuntimeBindingObservation>,
    ) -> UseResult<Self> {
        Self::new_with_schema_attestation(request, receipt_digest, binding, None)
    }

    pub(in crate::control_store) fn new_with_schema_attestation(
        request: &ControlRuntimeEffectRequest,
        receipt_digest: impl Into<String>,
        binding: Option<ControlRuntimeBindingObservation>,
        schema_attestation: Option<ControlRuntimeSchemaAttestation>,
    ) -> UseResult<Self> {
        let application = Self {
            receipt_digest: receipt_digest.into(),
            binding,
            schema_attestation,
        };
        let binding_matches = match request.surface.action {
            ControlSurfaceEffectAction::Prepare => application
                .binding
                .as_ref()
                .is_some_and(ControlRuntimeBindingObservation::validate),
            ControlSurfaceEffectAction::Stop | ControlSurfaceEffectAction::Remove => {
                application.binding.is_none() && application.schema_attestation.is_none()
            }
        };
        let attestation_matches = application
            .schema_attestation
            .as_ref()
            .is_none_or(|attestation| attestation.validate().is_ok());
        let attestation_kind_matches = application.schema_attestation.is_none()
            || request.surface.surface.kind == PluginSurfaceKind::Tool;
        if !valid_sha256(&application.receipt_digest)
            || !binding_matches
            || !attestation_matches
            || !attestation_kind_matches
        {
            return Err(input_error(
                "Runtime effect application evidence does not match its typed request.",
            ));
        }
        Ok(application)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlSurfaceApplication {
    pub(in crate::control_store) receipt_digest: String,
    pub(in crate::control_store) materialization_digest: Option<String>,
}

impl ControlSurfaceApplication {
    pub(in crate::control_store) fn new(
        request: &ControlSurfaceEffectRequest,
        receipt_digest: impl Into<String>,
        materialization_digest: Option<String>,
    ) -> UseResult<Self> {
        let application = Self {
            receipt_digest: receipt_digest.into(),
            materialization_digest,
        };
        let materialization_matches = match request.action {
            ControlSurfaceEffectAction::Prepare => application
                .materialization_digest
                .as_deref()
                .is_some_and(valid_sha256),
            ControlSurfaceEffectAction::Stop | ControlSurfaceEffectAction::Remove => {
                application.materialization_digest.is_none()
            }
        };
        if !valid_sha256(&application.receipt_digest) || !materialization_matches {
            return Err(input_error(
                "Surface effect application evidence does not match its typed request.",
            ));
        }
        Ok(application)
    }
}

#[async_trait]
pub(in crate::control_store) trait ControlCapabilityIndexEffectPort:
    Send + Sync
{
    async fn cutover(
        &self,
        request: &ControlCapabilityCutoverRequest,
    ) -> ControlEffectPortOutcome<ControlCapabilityCutoverApplication>;
}

/// Host-owned, side-effect-free and deterministic projection of one committed
/// Control generation into its Agent-facing catalog.
///
/// Implementations must derive the result only from the supplied committed
/// authority and immutable signed package/provider evidence. For a given
/// authority and immutable evidence, retries with the same effect identity
/// must produce byte-identical catalog data; nondeterministic host state must
/// first be committed into that authority. `Applied` means projection
/// completed; `Deferred` and `Rejected` must prove that no payload mutation
/// occurred. The concrete Capability Plane owns durable publication after
/// this port returns.
#[async_trait]
pub(in crate::control_store) trait ControlCapabilityCatalogProjectionPort:
    Send + Sync
{
    async fn project(
        &self,
        authority: &ControlCapabilityEffectAuthority,
    ) -> ControlEffectPortOutcome<CapabilityGatewayCatalog>;
}

#[async_trait]
pub(in crate::control_store) trait ControlInvocationLeaseEffectPort:
    Send + Sync
{
    async fn drain(
        &self,
        request: &ControlInvocationDrainRequest,
    ) -> ControlEffectPortOutcome<ControlReceiptApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlRuntimeEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlRuntimeEffectRequest,
    ) -> ControlEffectPortOutcome<ControlRuntimeApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlFlowEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlKnowledgeEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlSkillEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication>;
}

#[async_trait]
pub(in crate::control_store) trait ControlUiEffectPort:
    Send + Sync
{
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication>;
}

fn surface_authority_error() -> a3s_use_core::UseError {
    a3s_use_core::UseError::new(
        "use.control_store.surface_authority_invalid",
        "Surface execution requires one exact committed package authority.",
    )
}

fn invocation_authority_error() -> a3s_use_core::UseError {
    a3s_use_core::UseError::new(
        "use.control.invocation_authority_invalid",
        "Invocation drain requires one exact committed prior package authority.",
    )
}
