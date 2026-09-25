use std::sync::Arc;

use a3s_use_core::{
    InstallationPackageSelection, InstallationSnapshot, PlanScope, PluginDesiredState,
    PluginHostPackageState, PluginObservedState, PluginOperationAction,
    PluginOperationConfirmation, PluginOperationPlan, PluginOperationPlanEnvelope, PluginPackageId,
    UseError, UseResult,
};
use a3s_use_extension::{
    ExtensionRegistrySnapshot, InstalledExtension, EXTENSION_RECEIPT_SCHEMA_VERSION,
};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin_lifecycle::{PluginLifecycleCheckpointOutcome, PluginLifecycleOperationRecord};

use super::enablement_store::{
    operation_conflict, CognitivePackageArtifactState, StoredCognitivePackageEnablement,
};
use super::grant::authorize_planned_operation;
use super::plan::now_ms;
use super::plan::{enablement_operation, package_state_revision};
use super::reviewed_authorization::ReviewedCognitivePackageAuthorizationProvider;
use super::{package_manager_error, CognitivePackageManager};

pub const COGNITIVE_PACKAGE_ENABLEMENT_REQUEST_SCHEMA: &str =
    "a3s.use.cognitive-package-enablement-request.v1";
pub const COGNITIVE_PACKAGE_ENABLEMENT_RESULT_SCHEMA: &str =
    "a3s.use.cognitive-package-enablement-result.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitivePackageEnablementRequest {
    pub schema: String,
    pub operation_id: String,
    pub package_id: PluginPackageId,
    pub expected_package_generation: u64,
    pub enabled: bool,
}

impl CognitivePackageEnablementRequest {
    pub fn new(
        operation_id: impl Into<String>,
        package_id: impl Into<String>,
        expected_package_generation: u64,
        enabled: bool,
    ) -> UseResult<Self> {
        let request = Self {
            schema: COGNITIVE_PACKAGE_ENABLEMENT_REQUEST_SCHEMA.to_string(),
            operation_id: operation_id.into(),
            package_id: PluginPackageId::parse(package_id.into())?,
            expected_package_generation,
            enabled,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != COGNITIVE_PACKAGE_ENABLEMENT_REQUEST_SCHEMA
            || self.expected_package_generation == 0
        {
            return Err(enablement_error(
                "use.plugin.package_enablement_request_invalid",
                "The cognitive-package enablement schema or expected state generation is invalid.",
            ));
        }
        Self::validate_operation_id(&self.operation_id)
    }

    pub(crate) fn validate_operation_id(operation_id: &str) -> UseResult<()> {
        PluginOperationPlan::validate_operation_id(operation_id).map_err(|_| {
            enablement_error(
                "use.plugin.package_enablement_request_invalid",
                "The cognitive-package enablement operation identity is invalid.",
            )
        })
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_bytes(
            self,
            "Failed to canonicalize the cognitive-package enablement request.",
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(digest(&self.canonical_bytes()?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitivePackageEnablementResult {
    pub schema: String,
    pub operation_id: String,
    pub package_id: PluginPackageId,
    pub completed_at_ms: u64,
    pub operation_result_digest: String,
    pub changed: bool,
    pub state: PluginHostPackageState,
    pub replayed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CognitivePackageEnablementOutcome<'a> {
    schema: &'a str,
    operation_id: &'a str,
    package_id: &'a PluginPackageId,
    completed_at_ms: u64,
    changed: bool,
    state: &'a PluginHostPackageState,
}

impl CognitivePackageEnablementResult {
    fn new(
        request: &CognitivePackageEnablementRequest,
        completed_at_ms: u64,
        changed: bool,
        state: PluginHostPackageState,
    ) -> UseResult<Self> {
        let operation_result_digest = outcome_digest(
            &request.operation_id,
            &request.package_id,
            completed_at_ms,
            changed,
            &state,
        )?;
        let result = Self {
            schema: COGNITIVE_PACKAGE_ENABLEMENT_RESULT_SCHEMA.to_string(),
            operation_id: request.operation_id.clone(),
            package_id: request.package_id.clone(),
            completed_at_ms,
            operation_result_digest,
            changed,
            state,
            replayed: false,
        };
        result.validate_for(request)?;
        Ok(result)
    }

    pub fn validate(&self) -> UseResult<()> {
        Self::validate_operation_id(&self.operation_id)?;
        self.state.validate()?;
        if self.schema != COGNITIVE_PACKAGE_ENABLEMENT_RESULT_SCHEMA
            || self.completed_at_ms == 0
            || self.state.desired == PluginDesiredState::Absent
            || self.operation_result_digest
                != outcome_digest(
                    &self.operation_id,
                    &self.package_id,
                    self.completed_at_ms,
                    self.changed,
                    &self.state,
                )?
        {
            return Err(enablement_error(
                "use.plugin.package_enablement_result_invalid",
                "The cognitive-package enablement result is invalid.",
            ));
        }
        Ok(())
    }

    pub fn validate_for(&self, request: &CognitivePackageEnablementRequest) -> UseResult<()> {
        self.validate()?;
        request.validate()?;
        let expected_desired = if request.enabled {
            PluginDesiredState::Enabled
        } else {
            PluginDesiredState::InstalledDisabled
        };
        let generation = self.state.package_generation.ok_or_else(|| {
            enablement_error(
                "use.plugin.package_enablement_result_invalid",
                "The cognitive-package enablement result omitted its state generation.",
            )
        })?;
        let generation_matches = if self.changed {
            generation > request.expected_package_generation
        } else {
            generation == request.expected_package_generation
        };
        if self.operation_id != request.operation_id
            || self.package_id != request.package_id
            || self.state.desired != expected_desired
            || !generation_matches
        {
            return Err(enablement_error(
                "use.plugin.package_enablement_result_mismatch",
                "The cognitive-package enablement result does not bind the exact request and state generation.",
            ));
        }
        Ok(())
    }

    fn validate_operation_id(operation_id: &str) -> UseResult<()> {
        CognitivePackageEnablementRequest::validate_operation_id(operation_id).map_err(|_| {
            enablement_error(
                "use.plugin.package_enablement_result_invalid",
                "The cognitive-package enablement result operation identity is invalid.",
            )
        })
    }
}

impl CognitivePackageManager {
    /// Apply one exact host-reviewed enablement plan without replacing its
    /// immutable artifact generation or installed dependency graph.
    ///
    /// The package state generation is owned by the Installation Snapshot and
    /// is distinct from the immutable receipt lifecycle generation. The
    /// operation ID and complete result are durable, so a host restart resumes
    /// checkpoints or replays the exact prior result.
    pub async fn apply_enablement(
        &self,
        request: &CognitivePackageEnablementRequest,
        reviewed_plan: PluginOperationPlanEnvelope,
        confirmation: Option<PluginOperationConfirmation>,
    ) -> UseResult<CognitivePackageEnablementResult> {
        request.validate()?;
        let authorization =
            ReviewedCognitivePackageAuthorizationProvider::new(reviewed_plan, confirmation)?;
        let reviewed = Self::with_plan_scope_lifecycle_and_authorization(
            self.registry.clone(),
            self.scope().clone(),
            self.lifecycle.clone(),
            Arc::new(authorization),
        )?;
        reviewed.apply_reviewed_enablement(request).await
    }

    async fn apply_reviewed_enablement(
        &self,
        request: &CognitivePackageEnablementRequest,
    ) -> UseResult<CognitivePackageEnablementResult> {
        let maintenance = Arc::new(self.maintenance_lock().acquire_shared().await?);
        let _mutation = self.installation_mutation_lock().acquire().await?;
        request.validate()?;
        let reviewed_plan = self.authorization.reviewed_plan().ok_or_else(|| {
            enablement_error(
                "use.plugin.package_reviewed_plan_required",
                "Enablement apply requires the exact plan returned by plan_enablement.",
            )
        })?;
        let expected_action = if request.enabled {
            a3s_use_core::PluginOperationAction::Enable
        } else {
            a3s_use_core::PluginOperationAction::Disable
        };
        if reviewed_plan.plan.operation_id != request.operation_id
            || reviewed_plan.plan.package_id != request.package_id.as_str()
            || reviewed_plan.plan.scope != *self.scope()
            || reviewed_plan.plan.action != expected_action
        {
            return Err(enablement_error(
                "use.plugin.package_reviewed_plan_mismatch",
                "The reviewed enablement plan does not bind the exact request and scope.",
            ));
        }
        self.apply_enablement_through_control(request, maintenance)
            .await
    }

    /// Apply enable/disable through Control as sole mutable authority.
    ///
    /// Never creates `package-enablement/`, `installation-snapshot.json`, or
    /// writes Grant/Registry leaves. Surface effects drain inside Control.
    async fn apply_enablement_through_control(
        &self,
        request: &CognitivePackageEnablementRequest,
        maintenance: Arc<a3s_use_extension::StateMaintenanceGuard>,
    ) -> UseResult<CognitivePackageEnablementResult> {
        let reviewed_plan = self.authorization.reviewed_plan().ok_or_else(|| {
            enablement_error(
                "use.plugin.package_reviewed_plan_required",
                "Enablement apply requires the exact plan returned by plan_enablement.",
            )
        })?;
        if let Some(replayed) = self
            .control_enablement_replay(request, Some(reviewed_plan))
            .await?
        {
            return Ok(replayed);
        }
        let (extension, package_selection, installation_snapshot) = self
            .required_enablement_extension(&request.package_id)
            .await?;
        require_materialized_enablement(&extension, &package_selection)?;
        if request.enabled {
            self.lifecycle.validate_manifest(&extension.manifest)?;
        } else {
            self.lifecycle
                .validate_manifest_for_retirement(&extension.manifest)?;
        }
        if package_selection.state_generation != request.expected_package_generation {
            return Err(package_manager_error(
                "use.plugin.package_generation_changed",
                format!(
                    "Cognitive package '{}' changed state generation before enablement.",
                    request.package_id
                ),
            )
            .with_detail(
                "expectedPackageGeneration",
                serde_json::json!(request.expected_package_generation),
            )
            .with_detail(
                "actualPackageGeneration",
                serde_json::json!(package_selection.state_generation),
            ));
        }
        if package_selection.enabled == request.enabled {
            return Err(enablement_error(
                "use.plugin.package_enablement_plan_stale",
                "The reviewed enablement plan is no longer applicable; plan the current state again.",
            ));
        }
        installation_snapshot
            .transition_package_enablement(
                request.package_id.as_str(),
                request.expected_package_generation,
                request.enabled,
            )?
            .ok_or_else(|| {
                enablement_error(
                    "use.plugin.package_enablement_plan_stale",
                    "The reviewed enablement plan no longer changes installation intent.",
                )
            })?;

        let admitted_at_ms = now_ms()?;
        let capability_generation = installation_snapshot.generation;
        let grant_snapshot = self
            .planned_grant_snapshot(package_state_revision(capability_generation)?)
            .await?;
        let generated = enablement_operation(
            request,
            &package_selection.package,
            &package_selection.selected_surfaces,
            &extension.manifest,
            extension.receipt.descriptor_digest()?,
            capability_generation,
            self.scope(),
            admitted_at_ms,
            &grant_snapshot,
            self.authorization.as_ref(),
        )?;
        self.authorization.verify_plan(&generated.envelope)?;
        let authorization = authorize_planned_operation(
            self.authorization.as_ref(),
            &generated.envelope,
            generated.grants.as_ref(),
            admitted_at_ms,
        )
        .await?;
        let (evidence, grants) =
            super::control_authority::control_admission_from_authorization(&authorization)?;
        let control = self.ensure_control().await?;
        let committed_at_ms = now_ms()?;
        let snapshot = control
            .apply_reviewed_operation(
                &generated.envelope,
                &evidence,
                grants.as_ref(),
                admitted_at_ms,
                committed_at_ms,
                &[],
                maintenance,
            )
            .await?;
        let completed_at_ms = control
            .observe_operation(&request.operation_id)
            .await?
            .and_then(|observed| observed.completed_at_ms)
            .unwrap_or(committed_at_ms);
        let selection = snapshot
            .package_selection(request.package_id.as_str())
            .ok_or_else(|| {
                enablement_error(
                    "use.plugin.package_enablement_state_invalid",
                    "The package disappeared from Control after enablement commit.",
                )
            })?;
        if selection.enabled != request.enabled {
            return Err(enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "Control did not materialize the reviewed enablement intent.",
            ));
        }
        let mut extension = extension;
        extension.receipt.enabled = selection.enabled;
        extension.receipt.lifecycle_generation = Some(selection.state_generation);
        let state = project_installed_state_control(&extension, selection, &snapshot)?;
        CognitivePackageEnablementResult::new(request, completed_at_ms, true, state)
    }

    /// Observe the exact current package and capability evidence while using
    /// the snapshot-owned package state generation for optimistic concurrency.
    pub async fn observe_package(&self, package_id: &str) -> UseResult<PluginHostPackageState> {
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
        let _mutation = self.installation_mutation_lock().acquire().await?;
        let package_id = PluginPackageId::parse(package_id.to_string())?;
        self.observe_package_through_control(&package_id).await
    }

    async fn observe_package_through_control(
        &self,
        package_id: &PluginPackageId,
    ) -> UseResult<PluginHostPackageState> {
        let control = self.ensure_control().await?;
        let Some(installation_snapshot) = control.current_snapshot().await? else {
            return project_absent_state(&ExtensionRegistrySnapshot::empty(self.scope().clone())?);
        };
        if installation_snapshot
            .package_selection(package_id.as_str())
            .is_none()
        {
            return project_absent_state_from_installation(&installation_snapshot);
        }
        let (extension, selection, snapshot) =
            self.required_enablement_extension(package_id).await?;
        project_installed_state_control(&extension, &selection, &snapshot)
    }

    pub(super) async fn required_enablement_extension(
        &self,
        package_id: &PluginPackageId,
    ) -> UseResult<(
        InstalledExtension,
        InstallationPackageSelection,
        InstallationSnapshot,
    )> {
        self.required_enablement_extension_control(package_id).await
    }

    async fn required_enablement_extension_control(
        &self,
        package_id: &PluginPackageId,
    ) -> UseResult<(
        InstalledExtension,
        InstallationPackageSelection,
        InstallationSnapshot,
    )> {
        let control = self.ensure_control().await?;
        let snapshot = control.current_snapshot().await?.ok_or_else(|| {
            enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "Control Store has no installation snapshot for an installed package.",
            )
        })?;
        let selection = snapshot
            .package_selection(package_id.as_str())
            .cloned()
            .ok_or_else(|| {
                package_manager_error(
                    "use.extension.not_installed",
                    format!("Cognitive package '{package_id}' is not installed."),
                )
            })?;
        let extension = self
            .registry
            .load_control_package_selection(&selection)
            .await?;
        Ok((extension, selection, snapshot))
    }

    /// Replay a completed Control enablement by exact operation identity.
    ///
    /// Returns `Ok(None)` when Control has no record for this operation ID.
    /// Same ID with different reviewed evidence fails closed as an operation
    /// conflict. In-flight records must resume through Control, not re-plan.
    pub(super) async fn control_enablement_replay(
        &self,
        request: &CognitivePackageEnablementRequest,
        reviewed_plan: Option<&PluginOperationPlanEnvelope>,
    ) -> UseResult<Option<CognitivePackageEnablementResult>> {
        let control = self.ensure_control().await?;
        let Some(observed) = control.observe_operation(&request.operation_id).await? else {
            return Ok(None);
        };
        let expected_action = if request.enabled {
            PluginOperationAction::Enable
        } else {
            PluginOperationAction::Disable
        };
        let binds_request = observed.envelope.plan.operation_id == request.operation_id
            && observed.envelope.plan.package_id == request.package_id.as_str()
            && observed.envelope.plan.action == expected_action
            && observed.envelope.plan.scope == *self.scope();
        if let Some(reviewed) = reviewed_plan {
            if !observed.matches_envelope(reviewed) {
                return Err(operation_conflict());
            }
        } else if !binds_request {
            return Err(operation_conflict());
        }
        match observed.phase {
            crate::control_store::ControlObservedOperationPhase::Completed => {
                let completed_at_ms = observed.completed_at_ms.ok_or_else(|| {
                    enablement_error(
                        "use.plugin.package_enablement_state_invalid",
                        "A completed Control enablement omitted its completion time.",
                    )
                })?;
                let (extension, selection, snapshot) = self
                    .required_enablement_extension(&request.package_id)
                    .await?;
                if selection.enabled != request.enabled {
                    return Err(enablement_error(
                        "use.plugin.package_enablement_state_invalid",
                        "Control enablement completion does not match the installed selection.",
                    ));
                }
                let state = project_installed_state_control(&extension, &selection, &snapshot)?;
                let mut result =
                    CognitivePackageEnablementResult::new(request, completed_at_ms, true, state)?;
                result.replayed = true;
                Ok(Some(result))
            }
            crate::control_store::ControlObservedOperationPhase::InFlight => Err(enablement_error(
                "use.plugin.package_enablement_in_flight",
                "Control still holds an in-flight enablement for this operation identity.",
            )),
            crate::control_store::ControlObservedOperationPhase::Cancelled
            | crate::control_store::ControlObservedOperationPhase::Rejected => {
                Err(operation_conflict())
            }
        }
    }
}

pub(super) fn reconcile_state(
    scope: &PlanScope,
    package_id: &PluginPackageId,
    current: Option<&StoredCognitivePackageEnablement>,
    extension: &InstalledExtension,
    installation_snapshot: &InstallationSnapshot,
    selection: &InstallationPackageSelection,
    updated_at_ms: u64,
) -> UseResult<StoredCognitivePackageEnablement> {
    if installation_snapshot.package_selection(package_id.as_str()) != Some(selection)
        || selection.package_id() != package_id.as_str()
        || selection.package.catalog != *extension.plan_ready_catalog()?
        || selection.selected_surfaces != extension.selected_surfaces()?
    {
        return Err(enablement_error(
            "use.plugin.package_enablement_state_invalid",
            "The lifecycle receipt does not materialize the authoritative installation snapshot intent.",
        ));
    }
    let artifact = artifact_state(extension)?;
    if let Some(current) = current {
        current.validate()?;
        if current.scope != *scope || current.package_id != package_id.as_str() {
            return Err(enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "The stored package enablement projection has different ownership.",
            ));
        }
        if current.active.is_some() {
            return Err(enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "A pending package enablement operation was not recovered before projection.",
            ));
        }
        if current.artifact.as_ref() == Some(&artifact)
            && current.installation_generation == installation_snapshot.generation
            && current.installation_snapshot_digest == installation_snapshot.descriptor_digest()?
            && current.state_generation == selection.state_generation
            && current.enabled == selection.enabled
        {
            return Ok(current.clone());
        }
    }
    StoredCognitivePackageEnablement::new(
        scope.clone(),
        package_id.to_string(),
        installation_snapshot,
        selection.state_generation,
        Some(artifact),
        selection.enabled,
        updated_at_ms,
    )
}

fn artifact_state(extension: &InstalledExtension) -> UseResult<CognitivePackageArtifactState> {
    if extension.receipt.schema_version != EXTENSION_RECEIPT_SCHEMA_VERSION {
        return Err(enablement_error(
            "use.plugin.package_enablement_unsupported",
            "Enablement requires a schema-v4 cognitive-package receipt.",
        ));
    }
    let generation = extension.receipt.lifecycle_generation.ok_or_else(|| {
        enablement_error(
            "use.plugin.package_enablement_state_invalid",
            "The cognitive-package receipt omitted its immutable lifecycle generation.",
        )
    })?;
    let package_sha256 = extension.receipt.package_sha256.as_deref().ok_or_else(|| {
        enablement_error(
            "use.plugin.package_enablement_state_invalid",
            "The cognitive-package receipt omitted its package digest.",
        )
    })?;
    let artifact = CognitivePackageArtifactState {
        version: extension.receipt.version.clone(),
        generation,
        package_digest: prefixed_digest(package_sha256)?,
        manifest_digest: prefixed_digest(&extension.receipt.manifest_sha256)?,
    };
    artifact.validate()?;
    Ok(artifact)
}

pub(super) fn project_installed_state(
    extension: &InstalledExtension,
    selection: &InstallationPackageSelection,
    snapshot: &ExtensionRegistrySnapshot,
    lifecycle: Option<&PluginLifecycleOperationRecord>,
) -> UseResult<PluginHostPackageState> {
    let artifact = artifact_state(extension)?;
    if selection.package_id() != extension.receipt.package_id
        || selection.package.catalog != *extension.plan_ready_catalog()?
        || selection.selected_surfaces != extension.selected_surfaces()?
    {
        return Err(enablement_error(
            "use.plugin.package_enablement_state_invalid",
            "The package receipt does not materialize its installation snapshot selection.",
        ));
    }
    let bindings = snapshot
        .packages
        .iter()
        .filter(|binding| binding.package_id == extension.receipt.package_id)
        .collect::<Vec<_>>();
    if bindings.len() != 1 {
        return Err(enablement_error(
            "use.plugin.package_enablement_state_invalid",
            "The capability snapshot does not contain one exact package projection.",
        ));
    }
    let binding = bindings[0];
    if binding.enabled != extension.receipt.enabled
        || binding.version != extension.receipt.version
        || binding.lifecycle_generation != extension.receipt.lifecycle_generation
        || binding.package_sha256 != extension.receipt.package_sha256
        || binding.manifest_sha256 != extension.receipt.manifest_sha256
    {
        return Err(enablement_error(
            "use.plugin.package_enablement_state_invalid",
            "The package receipt and capability snapshot projection disagree.",
        ));
    }
    extension.plan_ready_catalog()?;
    let desired = if selection.enabled {
        PluginDesiredState::Enabled
    } else {
        PluginDesiredState::InstalledDisabled
    };
    let observed = if selection.enabled != extension.receipt.enabled {
        PluginObservedState::Reconciling
    } else if desired == PluginDesiredState::InstalledDisabled {
        PluginObservedState::Installed
    } else if lifecycle.is_some_and(|record| {
        record
            .receipts
            .iter()
            .any(|receipt| receipt.outcome == PluginLifecycleCheckpointOutcome::OptionalFailed)
    }) {
        PluginObservedState::Degraded
    } else {
        PluginObservedState::Ready
    };
    let state = PluginHostPackageState {
        version: Some(artifact.version),
        package_generation: Some(selection.state_generation),
        package_digest: Some(artifact.package_digest),
        manifest_digest: Some(artifact.manifest_digest),
        receipt_digest: Some(extension.receipt.descriptor_digest()?),
        capability_generation: snapshot.generation,
        capability_revision: snapshot.descriptor_digest()?,
        desired,
        observed,
        selected_surfaces: selection.selected_surfaces.clone(),
    };
    state.validate()?;
    Ok(state)
}

/// Project host package state from Control snapshot authority (no Registry).
pub(super) fn project_installed_state_control(
    extension: &InstalledExtension,
    selection: &InstallationPackageSelection,
    installation_snapshot: &InstallationSnapshot,
) -> UseResult<PluginHostPackageState> {
    let artifact = artifact_state(extension)?;
    if selection.package_id() != extension.receipt.package_id
        || selection.package.catalog != *extension.plan_ready_catalog()?
        || selection.selected_surfaces != extension.selected_surfaces()?
        || selection.enabled != extension.receipt.enabled
        || extension.receipt.lifecycle_generation != Some(selection.state_generation)
    {
        return Err(enablement_error(
            "use.plugin.package_enablement_state_invalid",
            "The package receipt does not materialize its Control installation selection.",
        ));
    }
    let desired = if selection.enabled {
        PluginDesiredState::Enabled
    } else {
        PluginDesiredState::InstalledDisabled
    };
    let observed = if desired == PluginDesiredState::InstalledDisabled {
        PluginObservedState::Installed
    } else {
        PluginObservedState::Ready
    };
    let state = PluginHostPackageState {
        version: Some(artifact.version),
        package_generation: Some(selection.state_generation),
        package_digest: Some(artifact.package_digest),
        manifest_digest: Some(artifact.manifest_digest),
        receipt_digest: Some(extension.receipt.descriptor_digest()?),
        capability_generation: installation_snapshot.generation,
        capability_revision: installation_snapshot.descriptor_digest()?,
        desired,
        observed,
        selected_surfaces: selection.selected_surfaces.clone(),
    };
    state.validate()?;
    Ok(state)
}

pub(super) fn require_materialized_enablement(
    extension: &InstalledExtension,
    selection: &InstallationPackageSelection,
) -> UseResult<()> {
    if selection.enabled != extension.receipt.enabled {
        return Err(package_manager_error(
            "use.plugin.package_graph_reconcile_required",
            "The package receipt has not materialized the current installation enablement intent.",
        ));
    }
    Ok(())
}

fn project_absent_state(snapshot: &ExtensionRegistrySnapshot) -> UseResult<PluginHostPackageState> {
    let state = PluginHostPackageState {
        version: None,
        package_generation: None,
        package_digest: None,
        manifest_digest: None,
        receipt_digest: None,
        capability_generation: snapshot.generation,
        capability_revision: snapshot.descriptor_digest()?,
        desired: PluginDesiredState::Absent,
        observed: PluginObservedState::Removed,
        selected_surfaces: Vec::new(),
    };
    state.validate()?;
    Ok(state)
}

fn project_absent_state_from_installation(
    snapshot: &InstallationSnapshot,
) -> UseResult<PluginHostPackageState> {
    let state = PluginHostPackageState {
        version: None,
        package_generation: None,
        package_digest: None,
        manifest_digest: None,
        receipt_digest: None,
        capability_generation: snapshot.generation,
        capability_revision: snapshot.descriptor_digest()?,
        desired: PluginDesiredState::Absent,
        observed: PluginObservedState::Removed,
        selected_surfaces: Vec::new(),
    };
    state.validate()?;
    Ok(state)
}

fn prefixed_digest(value: &str) -> UseResult<String> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(enablement_error(
            "use.plugin.package_enablement_state_invalid",
            "The cognitive-package enablement evidence contains an invalid SHA-256 digest.",
        ));
    }
    Ok(format!("sha256:{value}"))
}

fn outcome_digest(
    operation_id: &str,
    package_id: &PluginPackageId,
    completed_at_ms: u64,
    changed: bool,
    state: &PluginHostPackageState,
) -> UseResult<String> {
    let outcome = CognitivePackageEnablementOutcome {
        schema: COGNITIVE_PACKAGE_ENABLEMENT_RESULT_SCHEMA,
        operation_id,
        package_id,
        completed_at_ms,
        changed,
        state,
    };
    Ok(digest(&canonical_bytes(
        &outcome,
        "Failed to canonicalize the cognitive-package enablement outcome.",
    )?))
}

fn canonical_bytes(value: &impl Serialize, message: &'static str) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value
        .serialize(&mut serializer)
        .map_err(|_| enablement_error("use.plugin.package_enablement_contract_invalid", message))?;
    Ok(bytes)
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn generation_exhausted() -> UseError {
    package_manager_error(
        "use.plugin.package_generation_exhausted",
        "The cognitive-package enablement state generation is exhausted.",
    )
}

fn enablement_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
