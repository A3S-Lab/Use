use std::collections::BTreeMap;

use a3s_use_core::{
    LockedPluginPackage, PlanAuthority, PlanPackageChangeKind, PlanScope, PluginDesiredState,
    PluginHostPackageState, PluginOperationAction, PluginOperationPlan, PluginOperationPlanBinding,
    PluginOperationPlanDraft, PluginOperationPlanEnvelope, PluginPlanningBundle,
    PluginWorkspaceGrantSnapshot, UseError, UseResult, PLUGIN_OPERATION_PLAN_SCHEMA_V4,
};
use a3s_use_extension::ExtensionManifest;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::enablement::{
    project_installed_state, reconcile_state, CognitivePackageEnablementRequest,
    CognitivePackageEnablementResult,
};
use super::enablement_store::operation_conflict;
use super::plan::{
    enablement_draft, enablement_operation, now_ms, package_state_revision, PLAN_LIFETIME_MS,
};
use super::{
    package_manager_error, CognitivePackageAuthorizationProvider, CognitivePackageManager,
};

pub const COGNITIVE_PACKAGE_ENABLEMENT_PLAN_RESULT_SCHEMA: &str =
    "a3s.use.cognitive-package-enablement-plan-result.v1";

/// Provider-neutral evidence for one exact installed enablement transition.
///
/// The signed planning bundle and Grant snapshot are captured from Use-owned
/// installed state. A trusted host binds Runtime providers and policy around
/// this draft, persists those same inputs, and reconstructs them at apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CognitivePackageEnablementDraft {
    pub request: CognitivePackageEnablementRequest,
    pub planned_at_ms: u64,
    pub expires_at_ms: u64,
    pub scope: PlanScope,
    pub state: PluginHostPackageState,
    pub draft: PluginOperationPlanDraft,
    pub grant_snapshot: PluginWorkspaceGrantSnapshot,
    pub planning_bundles: BTreeMap<String, PluginPlanningBundle>,
    pub installed_generations: BTreeMap<String, u64>,
    selected_surfaces: Vec<a3s_use_core::PluginSurfaceRef>,
    package: LockedPluginPackage,
    manifest: ExtensionManifest,
    receipt_digest: String,
    registry_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CognitivePackageEnablementPreparation {
    Outcome(Box<CognitivePackageEnablementPlanResult>),
    Draft(Box<CognitivePackageEnablementDraft>),
}

impl CognitivePackageEnablementDraft {
    pub fn provisional_binding(&self, authority: PlanAuthority) -> PluginOperationPlanBinding {
        PluginOperationPlanBinding {
            operation_id: self.request.operation_id.clone(),
            created_at_ms: self.planned_at_ms,
            expires_at_ms: self.expires_at_ms,
            scope: self.scope.clone(),
            authority,
        }
    }

    pub fn bind(
        self,
        plan: PluginOperationPlan,
    ) -> UseResult<CognitivePackageEnablementPlanResult> {
        plan.validate()?;
        if plan.operation_id != self.request.operation_id
            || plan.created_at_ms != self.planned_at_ms
            || plan.expires_at_ms != self.expires_at_ms
            || plan.scope != self.scope
            || plan.action != self.draft.action
            || plan.package_id != self.draft.package_id
            || plan.component_id != self.draft.component_id
            || plan.package_lock_digest != self.draft.package_lock_digest
            || plan.prior_package_lock_digest.is_some()
            || plan.packages != self.draft.packages
            || plan.impact != self.draft.impact
            || plan.state != self.draft.state
        {
            return Err(plan_result_error());
        }
        CognitivePackageEnablementPlanResult::planned(
            self.request,
            self.planned_at_ms,
            self.state,
            PluginOperationPlanEnvelope::new(plan)?,
        )
    }

    fn bind_standalone(
        self,
        authorization: &dyn CognitivePackageAuthorizationProvider,
    ) -> UseResult<CognitivePackageEnablementPlanResult> {
        let generated = enablement_operation(
            &self.request,
            &self.package,
            &self.selected_surfaces,
            &self.manifest,
            self.receipt_digest,
            self.registry_generation,
            &self.scope,
            self.planned_at_ms,
            &self.grant_snapshot,
            authorization,
        )?;
        CognitivePackageEnablementPlanResult::planned(
            self.request,
            self.planned_at_ms,
            self.state,
            generated.envelope,
        )
    }
}

/// Exact outcome of planning one desired cognitive-package enablement state.
///
/// `Planned` carries the immutable plan-v4 envelope that a trusted host may
/// review and later reproduce through
/// `ReviewedCognitivePackageAuthorizationProvider`. `NoChange` is a terminal
/// planning outcome and deliberately carries no synthetic mutation plan.
/// `Completed` returns the exact durable result for an operation ID that Use
/// has already applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CognitivePackageEnablementPlanStatus {
    NoChange,
    Planned,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitivePackageEnablementPlanResult {
    pub schema: String,
    pub request: CognitivePackageEnablementRequest,
    pub planned_at_ms: u64,
    pub status: CognitivePackageEnablementPlanStatus,
    pub state: PluginHostPackageState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PluginOperationPlanEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<CognitivePackageEnablementResult>,
}

impl CognitivePackageEnablementPlanResult {
    fn no_change(
        request: CognitivePackageEnablementRequest,
        planned_at_ms: u64,
        state: PluginHostPackageState,
    ) -> UseResult<Self> {
        Self::new(
            request,
            planned_at_ms,
            CognitivePackageEnablementPlanStatus::NoChange,
            state,
            None,
            None,
        )
    }

    fn planned(
        request: CognitivePackageEnablementRequest,
        planned_at_ms: u64,
        state: PluginHostPackageState,
        plan: PluginOperationPlanEnvelope,
    ) -> UseResult<Self> {
        Self::new(
            request,
            planned_at_ms,
            CognitivePackageEnablementPlanStatus::Planned,
            state,
            Some(plan),
            None,
        )
    }

    fn completed(
        request: CognitivePackageEnablementRequest,
        planned_at_ms: u64,
        plan: PluginOperationPlanEnvelope,
        result: CognitivePackageEnablementResult,
    ) -> UseResult<Self> {
        let state = result.state.clone();
        Self::new(
            request,
            planned_at_ms,
            CognitivePackageEnablementPlanStatus::Completed,
            state,
            Some(plan),
            Some(result),
        )
    }

    fn new(
        request: CognitivePackageEnablementRequest,
        planned_at_ms: u64,
        status: CognitivePackageEnablementPlanStatus,
        state: PluginHostPackageState,
        plan: Option<PluginOperationPlanEnvelope>,
        result: Option<CognitivePackageEnablementResult>,
    ) -> UseResult<Self> {
        let planned = Self {
            schema: COGNITIVE_PACKAGE_ENABLEMENT_PLAN_RESULT_SCHEMA.to_string(),
            request,
            planned_at_ms,
            status,
            state,
            plan,
            result,
        };
        planned.validate()?;
        Ok(planned)
    }

    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        let planned: Self = serde_json::from_slice(input).map_err(|_| plan_result_error())?;
        planned.validate()?;
        Ok(planned)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.request.validate()?;
        self.state.validate()?;
        if self.schema != COGNITIVE_PACKAGE_ENABLEMENT_PLAN_RESULT_SCHEMA
            || self.planned_at_ms == 0
            || self.state.desired == PluginDesiredState::Absent
        {
            return Err(plan_result_error());
        }

        match self.status {
            CognitivePackageEnablementPlanStatus::NoChange => {
                if self.plan.is_some()
                    || self.result.is_some()
                    || !self.state_matches_request(false)
                {
                    return Err(plan_result_error());
                }
            }
            CognitivePackageEnablementPlanStatus::Planned => {
                let plan = self.plan.as_ref().ok_or_else(plan_result_error)?;
                if self.result.is_some()
                    || !self.state_matches_request(true)
                    || self.validate_plan_binding(plan).is_err()
                    || self.validate_plan_precondition(plan).is_err()
                {
                    return Err(plan_result_error());
                }
            }
            CognitivePackageEnablementPlanStatus::Completed => {
                let result = self.result.as_ref().ok_or_else(plan_result_error)?;
                let plan = self.plan.as_ref().ok_or_else(plan_result_error)?;
                result.validate_for(&self.request)?;
                if result.state != self.state || self.validate_plan_binding(plan).is_err() {
                    return Err(plan_result_error());
                }
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer)
            .map_err(|_| plan_result_error())?;
        Ok(bytes)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }

    fn state_matches_request(&self, state_must_differ: bool) -> bool {
        let expected_desired = if self.request.enabled {
            PluginDesiredState::Enabled
        } else {
            PluginDesiredState::InstalledDisabled
        };
        self.state.package_generation == Some(self.request.expected_package_generation)
            && (self.state.desired != expected_desired) == state_must_differ
    }

    fn validate_plan_binding(&self, envelope: &PluginOperationPlanEnvelope) -> UseResult<()> {
        envelope.validate()?;
        let expected_action = if self.request.enabled {
            PluginOperationAction::Enable
        } else {
            PluginOperationAction::Disable
        };
        let root = envelope
            .plan
            .packages
            .iter()
            .find(|package| package.package_id == self.request.package_id.as_str())
            .ok_or_else(plan_result_error)?;
        if envelope.plan.schema != PLUGIN_OPERATION_PLAN_SCHEMA_V4
            || envelope.plan.operation_id != self.request.operation_id
            || envelope.plan.package_id != self.request.package_id.as_str()
            || envelope.plan.action != expected_action
            || root.change != PlanPackageChangeKind::Retain
            || root.before != root.after
        {
            return Err(plan_result_error());
        }
        Ok(())
    }

    fn validate_plan_precondition(&self, envelope: &PluginOperationPlanEnvelope) -> UseResult<()> {
        if envelope.plan.state.receipt_digest != self.state.receipt_digest
            || envelope.plan.state.capability_generation != self.state.capability_generation
        {
            return Err(plan_result_error());
        }
        Ok(())
    }
}

impl CognitivePackageManager {
    /// Plan an idempotent desired enablement change without authorizing or
    /// starting this request's lifecycle mutation.
    ///
    /// Recovery of an older pending operation may still run before observation.
    /// A changed result contains the exact plan-v4 envelope; callers must store
    /// and review it, then apply through a manager composed with
    /// `ReviewedCognitivePackageAuthorizationProvider`.
    pub async fn plan_enablement(
        &self,
        request: &CognitivePackageEnablementRequest,
    ) -> UseResult<CognitivePackageEnablementPlanResult> {
        match self.prepare_enablement(request).await? {
            CognitivePackageEnablementPreparation::Outcome(result) => Ok(*result),
            CognitivePackageEnablementPreparation::Draft(draft) => {
                (*draft).bind_standalone(self.authorization.as_ref())
            }
        }
    }

    /// Capture an exact provider-neutral enablement draft from installed
    /// Use-owned evidence without opening a Runtime provider or mutating package
    /// lifecycle state.
    pub async fn prepare_enablement(
        &self,
        request: &CognitivePackageEnablementRequest,
    ) -> UseResult<CognitivePackageEnablementPreparation> {
        request.validate()?;
        let store = self.enablement_store();
        let _operation_guard = store
            .lock_operation(&self.scope, &request.operation_id)
            .await?;
        let _package_guard = store.lock_package(&self.scope, &request.package_id).await?;
        let planned_at_ms = now_ms()?;

        if let Some(operation) = store
            .get_operation(&self.scope, &request.operation_id)
            .await?
        {
            if operation.request != *request {
                return Err(operation_conflict());
            }
            let plan = operation.envelope.clone();
            let result = self
                .replay_enablement_operation(&store, request, operation)
                .await?;
            return CognitivePackageEnablementPlanResult::completed(
                request.clone(),
                planned_at_ms,
                plan,
                result,
            )
            .map(Box::new)
            .map(CognitivePackageEnablementPreparation::Outcome);
        }

        let mut current = store.get_state(&self.scope, &request.package_id).await?;
        if let Some(pending) = current
            .as_ref()
            .filter(|state| state.active.is_some())
            .cloned()
        {
            let completed = self.complete_pending_enablement(&store, &pending).await?;
            current = Some(completed.state_after.clone());
            if completed.request.operation_id == request.operation_id {
                if completed.request != *request {
                    return Err(operation_conflict());
                }
                return CognitivePackageEnablementPlanResult::completed(
                    request.clone(),
                    planned_at_ms,
                    completed.envelope,
                    completed.result,
                )
                .map(Box::new)
                .map(CognitivePackageEnablementPreparation::Outcome);
            }
        }

        if let Some(operation) = store
            .get_operation(&self.scope, &request.operation_id)
            .await?
        {
            if operation.request != *request {
                return Err(operation_conflict());
            }
            let plan = operation.envelope.clone();
            let result = self
                .replay_enablement_operation(&store, request, operation)
                .await?;
            return CognitivePackageEnablementPlanResult::completed(
                request.clone(),
                planned_at_ms,
                plan,
                result,
            )
            .map(Box::new)
            .map(CognitivePackageEnablementPreparation::Outcome);
        }

        let (extension, locked_package) = self
            .required_enablement_extension(&request.package_id)
            .await?;
        self.lifecycle
            .validate_manifest_for_planning(&extension.manifest)?;
        let reconciled = reconcile_state(
            &self.scope,
            &request.package_id,
            current.as_ref(),
            &extension,
            planned_at_ms,
        )?;
        if reconciled.state_generation != request.expected_package_generation {
            return Err(package_manager_error(
                "use.plugin.package_generation_changed",
                format!(
                    "Cognitive package '{}' changed state generation before enablement planning.",
                    request.package_id
                ),
            )
            .with_detail(
                "expectedPackageGeneration",
                serde_json::json!(request.expected_package_generation),
            )
            .with_detail(
                "actualPackageGeneration",
                serde_json::json!(reconciled.state_generation),
            ));
        }

        let snapshot = self.registry.snapshot().await?;
        let state =
            project_installed_state(&extension, reconciled.state_generation, &snapshot, None)?;
        if current.as_ref() != Some(&reconciled) {
            store.put_state(&reconciled).await?;
        }
        if reconciled.enabled == request.enabled {
            return CognitivePackageEnablementPlanResult::no_change(
                request.clone(),
                planned_at_ms,
                state,
            )
            .map(Box::new)
            .map(CognitivePackageEnablementPreparation::Outcome);
        }

        let grant_snapshot = self
            .grant_store()
            .snapshot_scope(&self.scope.id, package_state_revision(snapshot.generation)?)
            .await?;
        let receipt_digest = extension.receipt.descriptor_digest()?;
        let draft = enablement_draft(
            request,
            &locked_package,
            &extension.selected_surfaces()?,
            receipt_digest.clone(),
            snapshot.generation,
        )?;
        let expires_at_ms = planned_at_ms.checked_add(PLAN_LIFETIME_MS).ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_clock_invalid",
                "The package-plan expiration time overflowed.",
            )
        })?;
        let lifecycle_generation = extension.receipt.lifecycle_generation.ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_enablement_state_invalid",
                "The installed cognitive package omitted its lifecycle generation.",
            )
        })?;
        let installed_generations =
            BTreeMap::from([(request.package_id.to_string(), lifecycle_generation)]);
        let mut planning_bundles = BTreeMap::new();
        if request.enabled {
            if let Some(bundle) = extension.plan_ready_planning_bundle()? {
                planning_bundles.insert(request.package_id.to_string(), bundle.clone());
            }
        }
        Ok(CognitivePackageEnablementPreparation::Draft(Box::new(
            CognitivePackageEnablementDraft {
                request: request.clone(),
                planned_at_ms,
                expires_at_ms,
                scope: self.scope.clone(),
                state,
                draft,
                grant_snapshot,
                planning_bundles,
                installed_generations,
                selected_surfaces: extension.selected_surfaces()?,
                package: locked_package,
                manifest: extension.manifest,
                receipt_digest,
                registry_generation: snapshot.generation,
            },
        )))
    }
}

fn plan_result_error() -> UseError {
    UseError::new(
        "use.plugin.package_enablement_plan_result_invalid",
        "The cognitive-package enablement planning result is invalid.",
    )
}
