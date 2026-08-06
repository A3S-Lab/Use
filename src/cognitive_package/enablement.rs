use a3s_use_core::{
    LockedPluginPackage, PlanScope, PluginDesiredState, PluginHostPackageState,
    PluginObservedState, PluginOperationPlan, PluginPackageId, PluginSurfaceRef, UseError,
    UseResult,
};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, ExtensionRegistrySnapshot, InstalledExtension,
};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin_lifecycle::{
    PluginLifecycleAction, PluginLifecycleCheckpointOutcome, PluginLifecycleIntent,
    PluginLifecycleIntentSpec, PluginLifecycleOperationRecord,
};

use super::enablement_store::{
    operation_conflict, CognitivePackageArtifactState, CognitivePackageEnablementStore,
    PendingCognitivePackageEnablement, StoredCognitivePackageEnablement,
    StoredCognitivePackageEnablementOperation, ENABLEMENT_STATE_SCHEMA_V2,
};
use super::grant::authorize_planned_operation;
use super::plan::now_ms;
use super::plan::{enablement_operation, package_state_revision};
use super::{package_manager_error, CognitivePackageManager};

pub const COGNITIVE_PACKAGE_ENABLEMENT_REQUEST_SCHEMA: &str =
    "a3s.use.cognitive-package-enablement-request.v1";
pub const COGNITIVE_PACKAGE_ENABLEMENT_RESULT_SCHEMA: &str =
    "a3s.use.cognitive-package-enablement-result.v1";
const COGNITIVE_PACKAGE_RECEIPT_SCHEMA_VERSION: u32 = 3;

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
    /// Change one schema-v3 package's desired visibility without replacing its
    /// immutable artifact generation or installed dependency graph.
    ///
    /// The package state generation is Use-owned and distinct from the
    /// immutable receipt lifecycle generation. The operation ID and complete
    /// result are durable, so a host restart resumes checkpoints or replays the
    /// exact prior result.
    pub async fn set_enablement(
        &self,
        request: &CognitivePackageEnablementRequest,
    ) -> UseResult<CognitivePackageEnablementResult> {
        request.validate()?;
        let store = self.enablement_store();
        let _operation_guard = store
            .lock_operation(&self.scope, &request.operation_id)
            .await?;
        let _package_guard = store.lock_package(&self.scope, &request.package_id).await?;

        if let Some(operation) = store
            .get_operation(&self.scope, &request.operation_id)
            .await?
        {
            return self
                .replay_enablement_operation(&store, request, operation)
                .await;
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
                return Ok(completed.result);
            }
        }

        if let Some(operation) = store
            .get_operation(&self.scope, &request.operation_id)
            .await?
        {
            return self
                .replay_enablement_operation(&store, request, operation)
                .await;
        }

        let (extension, locked_package) = self
            .required_enablement_extension(&request.package_id)
            .await?;
        self.lifecycle.validate_manifest(&extension.manifest)?;
        let admitted_at_ms = now_ms()?;
        let reconciled = reconcile_state(
            &self.scope,
            &request.package_id,
            current.as_ref(),
            &extension,
            admitted_at_ms,
        )?;
        if reconciled.state_generation != request.expected_package_generation {
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
                serde_json::json!(reconciled.state_generation),
            ));
        }

        if reconciled.enabled == request.enabled {
            let snapshot = self.registry.snapshot().await?;
            let state =
                project_installed_state(&extension, reconciled.state_generation, &snapshot, None)?;
            let result =
                CognitivePackageEnablementResult::new(request, admitted_at_ms, false, state)?;
            let mut state_after = reconciled;
            state_after.updated_at_ms = admitted_at_ms;
            state_after.validate()?;
            let operation = StoredCognitivePackageEnablementOperation::new(
                self.scope.clone(),
                request.clone(),
                None,
                None,
                None,
                result.clone(),
                state_after.clone(),
            )?;
            store.put_operation(&operation).await?;
            if current.as_ref() != Some(&state_after) {
                store.put_state(&state_after).await?;
            }
            return Ok(result);
        }

        let state_generation_after = reconciled
            .state_generation
            .checked_add(1)
            .ok_or_else(generation_exhausted)?;
        let artifact = reconciled.artifact.as_ref().ok_or_else(|| {
            enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "An installed cognitive package has no immutable artifact identity.",
            )
        })?;
        let snapshot = self.registry.snapshot().await?;
        let grant_snapshot = self
            .grant_store()
            .snapshot_scope(&self.scope.id, package_state_revision(snapshot.generation)?)
            .await?;
        let generated = enablement_operation(
            request,
            &locked_package,
            &extension.manifest,
            extension.receipt.descriptor_digest()?,
            snapshot.generation,
            &self.scope,
            admitted_at_ms,
            &grant_snapshot,
            self.authorization.as_ref(),
        )?;
        let authorization = authorize_planned_operation(
            self.authorization.as_ref(),
            &generated.envelope,
            generated.grants.as_ref(),
            admitted_at_ms,
        )
        .await?;
        let request_digest = request.descriptor_digest()?;
        let action = if request.enabled {
            PluginLifecycleAction::Enable
        } else {
            PluginLifecycleAction::Disable
        };
        let intent = PluginLifecycleIntent::from_manifest(
            PluginLifecycleIntentSpec {
                operation_id: request.operation_id.clone(),
                plan_digest: generated.envelope.plan_digest.clone(),
                scope_id: self.scope.id.clone(),
                package_id: request.package_id.to_string(),
                package_digest: artifact.package_digest.clone(),
                manifest_digest: artifact.manifest_digest.clone(),
                generation: artifact.generation,
                action,
            },
            &extension.manifest,
        )?;
        let mut pending = reconciled;
        pending.schema = ENABLEMENT_STATE_SCHEMA_V2.to_string();
        pending.active = Some(PendingCognitivePackageEnablement {
            request_digest,
            request: request.clone(),
            intent,
            envelope: Some(generated.envelope),
            authorization: Some(authorization),
            state_generation_after,
            started_at_ms: admitted_at_ms,
        });
        pending.validate()?;
        store.put_state(&pending).await?;

        let completed = self.complete_pending_enablement(&store, &pending).await?;
        Ok(completed.result)
    }

    /// Observe the exact current package and capability evidence while using
    /// the durable Use-owned state generation for optimistic concurrency.
    pub async fn observe_package(&self, package_id: &str) -> UseResult<PluginHostPackageState> {
        let package_id = PluginPackageId::parse(package_id.to_string())?;
        let store = self.enablement_store();
        let _guard = store.lock_package(&self.scope, &package_id).await?;
        let mut current = store.get_state(&self.scope, &package_id).await?;
        if let Some(pending) = current
            .as_ref()
            .filter(|state| state.active.is_some())
            .cloned()
        {
            current = Some(
                self.complete_pending_enablement(&store, &pending)
                    .await?
                    .state_after,
            );
        }

        let observed_at_ms = now_ms()?;
        let snapshot = self.registry.snapshot().await?;
        let Some(extension) = self.registry.get(package_id.as_str()).await? else {
            if let Some(state) = current.as_ref() {
                if state.artifact.is_some() {
                    let state_generation = state
                        .state_generation
                        .checked_add(1)
                        .ok_or_else(generation_exhausted)?;
                    let tombstone = StoredCognitivePackageEnablement::new(
                        self.scope.clone(),
                        package_id.to_string(),
                        state_generation,
                        None,
                        false,
                        observed_at_ms,
                    )?;
                    store.put_state(&tombstone).await?;
                }
            }
            return project_absent_state(&snapshot);
        };
        self.validate_enablement_extension(&package_id, &extension)
            .await?;
        let reconciled = reconcile_state(
            &self.scope,
            &package_id,
            current.as_ref(),
            &extension,
            observed_at_ms,
        )?;
        if current.as_ref() != Some(&reconciled) {
            store.put_state(&reconciled).await?;
        }
        project_installed_state(&extension, reconciled.state_generation, &snapshot, None)
    }

    pub(super) async fn replay_enablement_operation(
        &self,
        store: &CognitivePackageEnablementStore,
        request: &CognitivePackageEnablementRequest,
        operation: StoredCognitivePackageEnablementOperation,
    ) -> UseResult<CognitivePackageEnablementResult> {
        operation.validate()?;
        if operation.request != *request {
            return Err(operation_conflict());
        }
        self.repair_completed_enablement_state(store, &operation)
            .await?;
        let mut result = operation.result;
        result.replayed = true;
        result.validate_for(request)?;
        Ok(result)
    }

    async fn repair_completed_enablement_state(
        &self,
        store: &CognitivePackageEnablementStore,
        operation: &StoredCognitivePackageEnablementOperation,
    ) -> UseResult<()> {
        let package_id = &operation.request.package_id;
        let current = store.get_state(&self.scope, package_id).await?;
        if current
            .as_ref()
            .is_some_and(|state| state.state_generation > operation.state_after.state_generation)
        {
            return Ok(());
        }
        if let Some(current) = &current {
            if current.state_generation == operation.state_after.state_generation
                && current == &operation.state_after
            {
                return Ok(());
            }
            if current
                .active
                .as_ref()
                .is_some_and(|active| active.request.operation_id != operation.request.operation_id)
            {
                return Err(operation_conflict());
            }
        }
        let Some(extension) = self.registry.get(package_id.as_str()).await? else {
            return Ok(());
        };
        let artifact = artifact_state(&extension)?;
        if operation.state_after.artifact.as_ref() == Some(&artifact)
            && extension.receipt.enabled == operation.state_after.enabled
        {
            store.put_state(&operation.state_after).await?;
        }
        Ok(())
    }

    pub(super) async fn complete_pending_enablement(
        &self,
        store: &CognitivePackageEnablementStore,
        current: &StoredCognitivePackageEnablement,
    ) -> UseResult<StoredCognitivePackageEnablementOperation> {
        current.validate()?;
        let active = current.active.as_ref().ok_or_else(|| {
            enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "The cognitive-package enablement recovery record has no active operation.",
            )
        })?;
        if let Some(operation) = store
            .get_operation(&self.scope, &active.request.operation_id)
            .await?
        {
            if operation.request != active.request {
                return Err(operation_conflict());
            }
            self.repair_completed_enablement_state(store, &operation)
                .await?;
            return Ok(operation);
        }

        let (extension, _) = self
            .required_enablement_extension(&active.request.package_id)
            .await?;
        self.lifecycle.validate_manifest(&extension.manifest)?;
        let artifact = artifact_state(&extension)?;
        if current.artifact.as_ref() != Some(&artifact) {
            return Err(package_manager_error(
                "use.plugin.package_generation_changed",
                "The immutable package generation changed while enablement was pending.",
            ));
        }
        if let Some(envelope) = &active.envelope {
            let mut prior_receipt = extension.receipt.clone();
            prior_receipt.enabled = !active.request.enabled;
            if envelope.plan.state.receipt_digest.as_deref()
                != Some(prior_receipt.descriptor_digest()?.as_str())
            {
                return Err(package_manager_error(
                    "use.plugin.package_generation_changed",
                    "The exact installed receipt changed after enablement planning.",
                ));
            }
        }
        let completed_at_fallback = now_ms()?;
        let identity = ExtensionLifecycleIdentity::new(
            active.request.package_id.as_str(),
            artifact.package_digest.clone(),
            artifact.manifest_digest.clone(),
            artifact.generation,
        )?;
        let coordinator = self.lifecycle.enablement_coordinator(
            self.registry.clone(),
            self.registry.lifecycle_package_root(&identity),
        )?;
        if active.requires_authority_revalidation() {
            let envelope = active.envelope.as_ref().ok_or_else(|| {
                enablement_error(
                    "use.plugin.package_enablement_state_invalid",
                    "A plan-bound enablement operation omitted its immutable envelope.",
                )
            })?;
            self.authorization.verify_plan(envelope)?;
        }
        let grants = active
            .authorization
            .as_ref()
            .zip(active.envelope.as_ref())
            .map(|(authorization, envelope)| {
                authorization.lifecycle_unit(self.grant_store(), envelope)
            })
            .transpose()?
            .flatten();
        let lifecycle = match (&active.envelope, grants.as_ref()) {
            (Some(envelope), Some(grants)) if active.request.enabled => {
                coordinator
                    .apply_enable_with_grants(
                        envelope,
                        &active.intent,
                        &extension.manifest,
                        grants,
                        || now_ms().unwrap_or(completed_at_fallback),
                    )
                    .await?
            }
            (Some(envelope), Some(grants)) => {
                coordinator
                    .apply_disable_with_grants(
                        envelope,
                        &active.intent,
                        &extension.manifest,
                        grants,
                        || now_ms().unwrap_or(completed_at_fallback),
                    )
                    .await?
            }
            _ => {
                coordinator
                    .apply(&active.intent, &extension.manifest, || {
                        now_ms().unwrap_or(completed_at_fallback)
                    })
                    .await?
            }
        };
        let completed_at_ms = lifecycle.completed_at_ms.ok_or_else(|| {
            enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "A completed enablement lifecycle omitted its completion time.",
            )
        })?;
        let selected = self
            .registry
            .get(active.request.package_id.as_str())
            .await?
            .ok_or_else(|| {
                package_manager_error(
                    "use.extension.not_installed",
                    "The cognitive package disappeared during enablement.",
                )
            })?;
        if artifact_state(&selected)? != artifact
            || selected.receipt.enabled != active.request.enabled
        {
            return Err(enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "The package receipt does not reflect the completed enablement lifecycle.",
            ));
        }
        let snapshot = self.registry.snapshot().await?;
        let state = project_installed_state(
            &selected,
            active.state_generation_after,
            &snapshot,
            Some(&lifecycle),
        )?;
        let result =
            CognitivePackageEnablementResult::new(&active.request, completed_at_ms, true, state)?;
        let state_after = StoredCognitivePackageEnablement::new(
            self.scope.clone(),
            active.request.package_id.to_string(),
            active.state_generation_after,
            Some(artifact),
            active.request.enabled,
            completed_at_ms,
        )?;
        let operation = StoredCognitivePackageEnablementOperation::new(
            self.scope.clone(),
            active.request.clone(),
            active.envelope.clone(),
            active.authorization.clone(),
            active.envelope.as_ref().map(|_| active.started_at_ms),
            result,
            state_after,
        )?;
        store.put_operation(&operation).await?;
        store.put_state(&operation.state_after).await?;
        Ok(operation)
    }

    pub(super) async fn required_enablement_extension(
        &self,
        package_id: &PluginPackageId,
    ) -> UseResult<(InstalledExtension, LockedPluginPackage)> {
        let extension = self
            .registry
            .get(package_id.as_str())
            .await?
            .ok_or_else(|| {
                package_manager_error(
                    "use.extension.not_installed",
                    format!("Cognitive package '{}' is not installed.", package_id),
                )
            })?;
        let package = self
            .validate_enablement_extension(package_id, &extension)
            .await?;
        Ok((extension, package))
    }

    async fn validate_enablement_extension(
        &self,
        package_id: &PluginPackageId,
        extension: &InstalledExtension,
    ) -> UseResult<LockedPluginPackage> {
        if extension.receipt.schema_version != COGNITIVE_PACKAGE_RECEIPT_SCHEMA_VERSION
            || extension.receipt.package_id != package_id.as_str()
            || extension.receipt.lifecycle_generation.is_none()
            || extension.receipt.package_sha256.is_none()
        {
            return Err(enablement_error(
                "use.plugin.package_enablement_unsupported",
                "Enablement requires an exact schema-v3 cognitive-package receipt.",
            ));
        }
        let catalog = extension.plan_ready_catalog()?;
        let mut owned = None;
        for graph in self.graph_store().list().await? {
            if let Some(locked) = graph.package_lock.package(package_id.as_str()) {
                if &locked.catalog != catalog {
                    return Err(enablement_error(
                        "use.plugin.package_enablement_state_invalid",
                        "An installed graph disagrees with the selected package catalog evidence.",
                    ));
                }
                owned.get_or_insert_with(|| locked.clone());
            }
        }
        owned.ok_or_else(|| {
            enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "The schema-v3 package is not owned by an installed dependency graph.",
            )
        })
    }
}

pub(super) fn reconcile_state(
    scope: &PlanScope,
    package_id: &PluginPackageId,
    current: Option<&StoredCognitivePackageEnablement>,
    extension: &InstalledExtension,
    updated_at_ms: u64,
) -> UseResult<StoredCognitivePackageEnablement> {
    let artifact = artifact_state(extension)?;
    if let Some(current) = current {
        current.validate()?;
        if current.scope != *scope || current.package_id != package_id.as_str() {
            return Err(enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "The stored package enablement state has different ownership.",
            ));
        }
        if current.active.is_some() {
            return Err(enablement_error(
                "use.plugin.package_enablement_state_invalid",
                "A pending package enablement operation was not recovered before reconciliation.",
            ));
        }
        if current.artifact.as_ref() == Some(&artifact)
            && current.enabled == extension.receipt.enabled
        {
            return Ok(current.clone());
        }
        let state_generation = current
            .state_generation
            .checked_add(1)
            .ok_or_else(generation_exhausted)?
            .max(artifact.generation);
        return StoredCognitivePackageEnablement::new(
            scope.clone(),
            package_id.to_string(),
            state_generation,
            Some(artifact),
            extension.receipt.enabled,
            updated_at_ms,
        );
    }
    StoredCognitivePackageEnablement::new(
        scope.clone(),
        package_id.to_string(),
        artifact.generation,
        Some(artifact),
        extension.receipt.enabled,
        updated_at_ms,
    )
}

fn artifact_state(extension: &InstalledExtension) -> UseResult<CognitivePackageArtifactState> {
    if extension.receipt.schema_version != COGNITIVE_PACKAGE_RECEIPT_SCHEMA_VERSION {
        return Err(enablement_error(
            "use.plugin.package_enablement_unsupported",
            "Enablement requires a schema-v3 cognitive-package receipt.",
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
    state_generation: u64,
    snapshot: &ExtensionRegistrySnapshot,
    lifecycle: Option<&PluginLifecycleOperationRecord>,
) -> UseResult<PluginHostPackageState> {
    let artifact = artifact_state(extension)?;
    let bindings = snapshot
        .routes
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
    let catalog = extension.plan_ready_catalog()?;
    let mut selected_surfaces = catalog
        .record
        .surfaces
        .iter()
        .map(a3s_use_core::CatalogSurface::reference)
        .collect::<Vec<PluginSurfaceRef>>();
    selected_surfaces.sort();
    selected_surfaces.dedup();
    let desired = if extension.receipt.enabled {
        PluginDesiredState::Enabled
    } else {
        PluginDesiredState::InstalledDisabled
    };
    let observed = if desired == PluginDesiredState::InstalledDisabled {
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
        package_generation: Some(state_generation),
        package_digest: Some(artifact.package_digest),
        manifest_digest: Some(artifact.manifest_digest),
        receipt_digest: Some(extension.receipt.descriptor_digest()?),
        capability_generation: snapshot.generation,
        capability_revision: snapshot.descriptor_digest()?,
        desired,
        observed,
        selected_surfaces,
    };
    state.validate()?;
    Ok(state)
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
