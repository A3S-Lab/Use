use std::collections::BTreeMap;
use std::path::PathBuf;

use a3s_use_core::{
    PlanPackageChangeKind, PluginOperationAction, PluginOperationPlanEnvelope, PluginPackageId,
    PluginPackageLock, UseError, UseResult, MAX_PLUGIN_PLAN_ITEMS,
};
use a3s_use_extension::{
    validate_catalog_manifest_binding, ArtifactStore, ExtensionManifest, ExtensionPaths,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

use super::super::{grant::PackageGraphAuthorization, package_manager_error};
use super::inventory::read_pending_operations_locked;
#[cfg(test)]
use super::test_artifact_store;
use super::{
    acquire_lock, action_name, path_error, path_identity_error, pending_record_path, read_optional,
    store_error, sync_parent, validate_existing_directory_chain, write_new,
};

const PENDING_GRAPH_SCHEMA: &str = "a3s.use.pending-package-graph-operation.v4";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One current v4 graph operation from reviewed planning through admission or
/// pre-admission cancellation. Superseded preview records fail closed.
pub(in crate::cognitive_package) struct PendingPackageGraphOperation {
    pub schema: String,
    pub envelope: PluginOperationPlanEnvelope,
    pub phase: PackageGraphOperationPhase,
    pub planned_at_ms: u64,
    pub admitted_at_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cancelled_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_request_id: Option<String>,
    pub authorization: PackageGraphAuthorization,
    pub generations: BTreeMap<String, u64>,
    pub manifests: BTreeMap<String, ExtensionManifest>,
    pub manifest_digests: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior_package_lock: Option<PluginPackageLock>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prior_generations: BTreeMap<String, u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prior_manifests: BTreeMap<String, ExtensionManifest>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prior_manifest_digests: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::cognitive_package) enum PackageGraphOperationPhase {
    Planned,
    Admitted,
    Cancelled,
}

const fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl PendingPackageGraphOperation {
    pub fn planned(
        envelope: PluginOperationPlanEnvelope,
        planned_at_ms: u64,
        generations: BTreeMap<String, u64>,
        manifests: BTreeMap<String, ExtensionManifest>,
    ) -> UseResult<Self> {
        let manifest_digests = manifest_record_digests(&manifests)?;
        let operation = Self {
            schema: PENDING_GRAPH_SCHEMA.to_string(),
            envelope,
            phase: PackageGraphOperationPhase::Planned,
            planned_at_ms,
            admitted_at_ms: 0,
            cancelled_at_ms: 0,
            cancellation_request_id: None,
            authorization: PackageGraphAuthorization::default(),
            generations,
            manifests,
            manifest_digests,
            prior_package_lock: None,
            prior_generations: BTreeMap::new(),
            prior_manifests: BTreeMap::new(),
            prior_manifest_digests: BTreeMap::new(),
        };
        operation.validate()?;
        Ok(operation)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn planned_upgrade(
        envelope: PluginOperationPlanEnvelope,
        planned_at_ms: u64,
        generations: BTreeMap<String, u64>,
        manifests: BTreeMap<String, ExtensionManifest>,
        prior_package_lock: PluginPackageLock,
        prior_generations: BTreeMap<String, u64>,
        prior_manifests: BTreeMap<String, ExtensionManifest>,
    ) -> UseResult<Self> {
        let manifest_digests = manifest_record_digests(&manifests)?;
        let prior_manifest_digests = manifest_record_digests(&prior_manifests)?;
        let operation = Self {
            schema: PENDING_GRAPH_SCHEMA.to_string(),
            envelope,
            phase: PackageGraphOperationPhase::Planned,
            planned_at_ms,
            admitted_at_ms: 0,
            cancelled_at_ms: 0,
            cancellation_request_id: None,
            authorization: PackageGraphAuthorization::default(),
            generations,
            manifests,
            manifest_digests,
            prior_package_lock: Some(prior_package_lock),
            prior_generations,
            prior_manifests,
            prior_manifest_digests,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn admit(
        &self,
        admitted_at_ms: u64,
        authorization: PackageGraphAuthorization,
    ) -> UseResult<Self> {
        self.validate()?;
        if self.phase != PackageGraphOperationPhase::Planned || admitted_at_ms < self.planned_at_ms
        {
            return Err(store_error(
                "Only an exact planned package graph operation can be admitted.",
            ));
        }
        let mut admitted = self.clone();
        admitted.schema = PENDING_GRAPH_SCHEMA.to_string();
        admitted.phase = PackageGraphOperationPhase::Admitted;
        admitted.admitted_at_ms = admitted_at_ms;
        admitted.authorization = authorization;
        admitted.validate()?;
        Ok(admitted)
    }

    pub fn cancel(&self, cancellation_request_id: &str, cancelled_at_ms: u64) -> UseResult<Self> {
        self.validate()?;
        if self.phase != PackageGraphOperationPhase::Planned
            || cancellation_request_id.is_empty()
            || cancellation_request_id.len() > 256
            || cancelled_at_ms < self.planned_at_ms
        {
            return Err(store_error(
                "Only an exact planned package graph operation can be cancelled.",
            ));
        }
        let mut cancelled = self.clone();
        cancelled.schema = PENDING_GRAPH_SCHEMA.to_string();
        cancelled.phase = PackageGraphOperationPhase::Cancelled;
        cancelled.cancelled_at_ms = cancelled_at_ms;
        cancelled.cancellation_request_id = Some(cancellation_request_id.to_owned());
        cancelled.validate()?;
        Ok(cancelled)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.envelope.validate()?;
        if self.schema != PENDING_GRAPH_SCHEMA {
            return Err(store_error(
                "A pending cognitive-package graph operation has an unsupported schema.",
            ));
        }
        let phase_valid = match self.phase {
            PackageGraphOperationPhase::Planned => {
                self.planned_at_ms > 0
                    && self.admitted_at_ms == 0
                    && self.cancelled_at_ms == 0
                    && self.cancellation_request_id.is_none()
                    && self.authorization == PackageGraphAuthorization::default()
            }
            PackageGraphOperationPhase::Admitted => {
                self.planned_at_ms > 0
                    && self.admitted_at_ms >= self.planned_at_ms
                    && self.cancelled_at_ms == 0
                    && self.cancellation_request_id.is_none()
            }
            PackageGraphOperationPhase::Cancelled => {
                self.planned_at_ms > 0
                    && self.admitted_at_ms == 0
                    && self.cancelled_at_ms >= self.planned_at_ms
                    && self
                        .cancellation_request_id
                        .as_deref()
                        .is_some_and(|request_id| !request_id.is_empty() && request_id.len() <= 256)
                    && self.authorization == PackageGraphAuthorization::default()
            }
        };
        if !phase_valid {
            return Err(store_error(
                "A pending cognitive-package graph operation has an invalid phase.",
            ));
        }
        if self.phase == PackageGraphOperationPhase::Admitted {
            self.authorization
                .validate_against(&self.envelope, self.admitted_at_ms)?;
        }
        let changed = self
            .envelope
            .plan
            .packages
            .iter()
            .filter(|package| match self.envelope.plan.action {
                PluginOperationAction::Install => package.change == PlanPackageChangeKind::Add,
                PluginOperationAction::Upgrade => matches!(
                    package.change,
                    PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
                ),
                PluginOperationAction::Uninstall => package.change == PlanPackageChangeKind::Remove,
                PluginOperationAction::Enable | PluginOperationAction::Disable => false,
            })
            .map(|package| package.package_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let generations = self
            .generations
            .iter()
            .filter_map(|(package_id, generation)| (*generation > 0).then_some(package_id.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        let manifests = self
            .manifests
            .iter()
            .filter_map(|(package_id, manifest)| {
                (manifest.schema_version == 3 && manifest.package_id == *package_id)
                    .then_some(package_id.as_str())
            })
            .collect::<std::collections::BTreeSet<_>>();
        let retired = self
            .envelope
            .plan
            .packages
            .iter()
            .filter(|package| {
                matches!(
                    package.change,
                    PlanPackageChangeKind::Replace | PlanPackageChangeKind::Remove
                )
            })
            .map(|package| package.package_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let replaced = self
            .envelope
            .plan
            .packages
            .iter()
            .filter(|package| package.change == PlanPackageChangeKind::Replace)
            .map(|package| package.package_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let prior_generations = self
            .prior_generations
            .iter()
            .filter_map(|(package_id, generation)| (*generation > 0).then_some(package_id.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        let prior_manifests = self
            .prior_manifests
            .iter()
            .filter_map(|(package_id, manifest)| {
                (manifest.schema_version == 3 && manifest.package_id == *package_id)
                    .then_some(package_id.as_str())
            })
            .collect::<std::collections::BTreeSet<_>>();
        let upgrade_evidence_valid = match self.envelope.plan.action {
            PluginOperationAction::Upgrade => {
                self.prior_package_lock.as_ref().is_some_and(|prior| {
                    prior.validate().is_ok()
                        && self
                            .envelope
                            .package_lock
                            .as_ref()
                            .is_some_and(|candidate| {
                                prior.root_package_id == candidate.root_package_id
                                    && prior.host == candidate.host
                            })
                        && self
                            .envelope
                            .prior_package_lock
                            .as_ref()
                            .is_none_or(|bound| bound == prior)
                        && (!self
                            .envelope
                            .plan
                            .packages
                            .iter()
                            .any(|package| package.change == PlanPackageChangeKind::Remove)
                            || self.envelope.prior_package_lock.as_ref() == Some(prior))
                        && retired == prior_generations
                        && retired == prior_manifests
                })
            }
            _ => {
                self.prior_package_lock.is_none()
                    && self.prior_generations.is_empty()
                    && self.prior_manifests.is_empty()
                    && self.envelope.prior_package_lock.is_none()
                    && replaced.is_empty()
            }
        };
        let candidate_manifests_valid = self
            .envelope
            .package_lock
            .as_ref()
            .is_some_and(|lock| manifests_match_lock(&self.manifests, lock));
        let prior_manifests_valid = self
            .prior_package_lock
            .as_ref()
            .map_or(self.prior_manifests.is_empty(), |lock| {
                manifests_match_lock(&self.prior_manifests, lock)
            });
        let replacement_generations_advance = replaced.iter().all(|package_id| {
            self.generations
                .get(*package_id)
                .zip(self.prior_generations.get(*package_id))
                .is_some_and(|(candidate, prior)| candidate > prior)
        });
        let manifest_digests_valid = manifest_record_digests(&self.manifests)
            .is_ok_and(|digests| digests == self.manifest_digests);
        let prior_manifest_digests_valid = manifest_record_digests(&self.prior_manifests)
            .is_ok_and(|digests| digests == self.prior_manifest_digests);
        if changed != generations
            || changed != manifests
            || !upgrade_evidence_valid
            || !candidate_manifests_valid
            || !prior_manifests_valid
            || !replacement_generations_advance
            || !manifest_digests_valid
            || !prior_manifest_digests_valid
            || self.generations.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.prior_generations.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.manifest_digests.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.prior_manifest_digests.len() > MAX_PLUGIN_PLAN_ITEMS
        {
            return Err(store_error(
                "A pending cognitive-package graph operation is invalid.",
            ));
        }
        Ok(())
    }

    pub fn phase(&self) -> PackageGraphOperationPhase {
        self.phase
    }

    pub fn action(&self) -> PluginOperationAction {
        self.envelope.plan.action
    }

    pub fn root_package_id(&self) -> &str {
        &self.envelope.plan.package_id
    }
}

fn manifests_match_lock(
    manifests: &BTreeMap<String, ExtensionManifest>,
    package_lock: &PluginPackageLock,
) -> bool {
    manifests.iter().all(|(package_id, manifest)| {
        package_lock.package(package_id).is_some_and(|package| {
            validate_catalog_manifest_binding(&package.catalog.record, manifest).is_ok()
        })
    })
}

fn manifest_record_digests(
    manifests: &BTreeMap<String, ExtensionManifest>,
) -> UseResult<BTreeMap<String, String>> {
    manifests
        .iter()
        .map(|(package_id, manifest)| {
            let bytes = serde_json::to_vec(manifest)
                .map_err(|_| store_error("Failed to encode a pending package manifest."))?;
            Ok((
                package_id.clone(),
                format!("sha256:{:x}", Sha256::digest(bytes)),
            ))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub(in crate::cognitive_package) struct PendingPackageGraphStore {
    artifact_store: ArtifactStore,
    state_root: PathBuf,
    root: PathBuf,
}

impl PendingPackageGraphStore {
    #[cfg(test)]
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        let artifact_store = test_artifact_store(&state_root);
        Self::from_parts(state_root, artifact_store)
    }

    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::from_parts(paths.installation_state_root(), paths.artifact_store())
    }

    fn from_parts(state_root: PathBuf, artifact_store: ArtifactStore) -> Self {
        Self {
            artifact_store,
            root: state_root.join("operations").join("package-graphs"),
            state_root,
        }
    }

    #[cfg(test)]
    pub(super) fn artifact_store(&self) -> &ArtifactStore {
        &self.artifact_store
    }

    pub async fn get(
        &self,
        action: PluginOperationAction,
        root_package_id: &str,
    ) -> UseResult<Option<PendingPackageGraphOperation>> {
        let path = pending_record_path(&self.root, action, root_package_id)?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Ok(None);
        }
        let _guard = acquire_lock(&self.state_root).await?;
        let value: Option<PendingPackageGraphOperation> = read_optional(&path).await?;
        if let Some(value) = &value {
            value.validate()?;
            if value.action() != action || value.root_package_id() != root_package_id {
                return Err(store_error(
                    "A pending graph operation does not match its owned path.",
                ));
            }
        }
        Ok(value)
    }

    /// Read the single pending graph operation owned by one root package.
    ///
    /// All action namespaces are inspected under the same graph-store lock so
    /// diagnostics cannot combine records from different mutation instants.
    pub async fn get_for_package(
        &self,
        root_package_id: &str,
    ) -> UseResult<Option<PendingPackageGraphOperation>> {
        PluginPackageId::parse(root_package_id.to_owned())
            .map_err(|_| store_error("A pending graph diagnostic package identity is invalid."))?;
        let actions = [
            PluginOperationAction::Install,
            PluginOperationAction::Upgrade,
            PluginOperationAction::Uninstall,
        ];
        let mut has_namespace = false;
        for action in actions {
            let path = pending_record_path(&self.root, action, root_package_id)?;
            let parent = path.parent().ok_or_else(path_identity_error)?;
            has_namespace |= validate_existing_directory_chain(&self.state_root, parent).await?;
        }
        if !has_namespace {
            return Ok(None);
        }

        let _guard = acquire_lock(&self.state_root).await?;
        let mut found = None;
        for action in actions {
            let path = pending_record_path(&self.root, action, root_package_id)?;
            let parent = path.parent().ok_or_else(path_identity_error)?;
            if !validate_existing_directory_chain(&self.state_root, parent).await? {
                continue;
            }
            let Some(value) = read_optional::<PendingPackageGraphOperation>(&path).await? else {
                continue;
            };
            value.validate()?;
            if value.action() != action || value.root_package_id() != root_package_id {
                return Err(store_error(
                    "A pending graph operation does not match its owned path.",
                ));
            }
            if found.replace(value).is_some() {
                return Err(store_error(
                    "A cognitive package has more than one pending graph operation.",
                ));
            }
        }
        Ok(found)
    }

    /// Return the sole durable admitted writer, if any, across all root and
    /// action namespaces in the current conservative installation domain.
    pub async fn admitted_operation(&self) -> UseResult<Option<PendingPackageGraphOperation>> {
        let _guard = acquire_lock(&self.state_root).await?;
        let mut admitted = read_pending_operations_locked(&self.root)
            .await?
            .into_iter()
            .filter(|operation| operation.phase() == PackageGraphOperationPhase::Admitted);
        let active = admitted.next();
        if admitted.next().is_some() {
            return Err(store_error(
                "More than one graph operation owns the installation mutation domain.",
            ));
        }
        Ok(active)
    }

    pub async fn put(&self, value: &PendingPackageGraphOperation) -> UseResult<bool> {
        value.validate()?;
        let _artifact_admission = self.artifact_store.acquire_reference_admission().await?;
        let _guard = acquire_lock(&self.state_root).await?;
        for action in [
            PluginOperationAction::Install,
            PluginOperationAction::Upgrade,
            PluginOperationAction::Uninstall,
        ] {
            let path = pending_record_path(&self.root, action, value.root_package_id())?;
            let parent = path.parent().ok_or_else(path_identity_error)?;
            if !validate_existing_directory_chain(&self.state_root, parent).await? {
                continue;
            }
            let Some(current) = read_optional::<PendingPackageGraphOperation>(&path).await? else {
                continue;
            };
            current.validate()?;
            if current.action() != action || current.root_package_id() != value.root_package_id() {
                return Err(store_error(
                    "A pending graph operation does not match its owned path.",
                ));
            }
            if current == *value {
                return Ok(false);
            }
            return Err(package_manager_error(
                "use.plugin.package_graph_busy",
                format!(
                    "Another '{}' graph operation is pending for cognitive package '{}'.",
                    action_name(current.action()),
                    value.root_package_id()
                ),
            ));
        }
        let path = pending_record_path(&self.root, value.action(), value.root_package_id())?;
        write_new(&self.state_root, &path, value).await?;
        Ok(true)
    }

    /// Fail before authorization when another crash-recoverable graph
    /// operation already owns this installation's mutation domain.
    pub async fn require_admission_available(
        &self,
        expected: &PendingPackageGraphOperation,
    ) -> UseResult<()> {
        expected.validate()?;
        if expected.phase() != PackageGraphOperationPhase::Planned {
            return Err(store_error(
                "Only a planned package graph operation can check admission availability.",
            ));
        }
        let _guard = acquire_lock(&self.state_root).await?;
        if let Some(active) = read_pending_operations_locked(&self.root)
            .await?
            .into_iter()
            .find(|operation| operation.phase() == PackageGraphOperationPhase::Admitted)
        {
            return Err(admission_busy(&active));
        }
        Ok(())
    }

    /// Atomically advance one exact reviewed plan to durable authorization.
    ///
    /// This is the only planned-to-admitted transition for package graphs.
    /// A replay of the exact admitted value is idempotent; changed plan,
    /// manifest, generation, or authorization identity fails closed.
    pub async fn admit(
        &self,
        expected: &PendingPackageGraphOperation,
        admitted_at_ms: u64,
        authorization: PackageGraphAuthorization,
    ) -> UseResult<(PendingPackageGraphOperation, bool)> {
        if expected.phase() != PackageGraphOperationPhase::Planned {
            return Err(store_error(
                "Only a planned package graph operation can enter admission.",
            ));
        }
        let admitted = expected.admit(admitted_at_ms, authorization)?;
        let _guard = acquire_lock(&self.state_root).await?;
        for active in read_pending_operations_locked(&self.root).await? {
            if active.phase() == PackageGraphOperationPhase::Admitted
                && (active.action() != expected.action()
                    || active.root_package_id() != expected.root_package_id())
            {
                return Err(admission_busy(&active));
            }
        }
        let path = pending_record_path(&self.root, expected.action(), expected.root_package_id())?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Err(store_error(
                "The reviewed package graph plan disappeared before admission.",
            ));
        }
        let current = read_optional::<PendingPackageGraphOperation>(&path)
            .await?
            .ok_or_else(|| {
                store_error("The reviewed package graph plan disappeared before admission.")
            })?;
        current.validate()?;
        if current == admitted {
            return Ok((admitted, false));
        }
        if current != *expected {
            return Err(store_error(
                "The reviewed package graph plan changed before admission.",
            ));
        }
        write_new(&self.state_root, &path, &admitted).await?;
        Ok((admitted, true))
    }

    pub async fn remove(&self, expected: &PendingPackageGraphOperation) -> UseResult<bool> {
        expected.validate()?;
        let _guard = acquire_lock(&self.state_root).await?;
        let path = pending_record_path(&self.root, expected.action(), expected.root_package_id())?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Ok(false);
        }
        let Some(current) = read_optional::<PendingPackageGraphOperation>(&path).await? else {
            return Ok(false);
        };
        if current != *expected {
            return Err(store_error(
                "The pending package graph changed before completion.",
            ));
        }
        fs::remove_file(&path)
            .await
            .map_err(|error| path_error("remove pending package graph", &path, error))?;
        sync_parent(path.parent().ok_or_else(path_identity_error)?).await?;
        Ok(true)
    }

    /// Cancel an exact reviewed graph before it crosses durable admission.
    ///
    /// The package-graph store lock serializes this check with admission, so a
    /// successful return proves that no lifecycle mutation can later start
    /// from the cancelled plan. Admitted operations deliberately fail closed.
    pub async fn cancel_planned(
        &self,
        expected: &PendingPackageGraphOperation,
        cancellation_request_id: &str,
        cancelled_at_ms: u64,
    ) -> UseResult<(PendingPackageGraphOperation, bool)> {
        expected.validate()?;
        if expected.phase() != PackageGraphOperationPhase::Planned {
            return Err(package_manager_error(
                "use.plugin.package_graph_cancel_too_late",
                "The cognitive-package operation already crossed durable admission.",
            ));
        }
        let _guard = acquire_lock(&self.state_root).await?;
        let cancelled = expected.cancel(cancellation_request_id, cancelled_at_ms)?;
        let path = pending_record_path(&self.root, expected.action(), expected.root_package_id())?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Err(store_error(
                "The reviewed package graph plan disappeared before cancellation.",
            ));
        }
        let current = read_optional::<PendingPackageGraphOperation>(&path)
            .await?
            .ok_or_else(|| {
                store_error("The reviewed package graph plan disappeared before cancellation.")
            })?;
        current.validate()?;
        if current == cancelled {
            return Ok((cancelled, false));
        }
        if current != *expected {
            return Err(store_error(
                "The reviewed package graph changed before cancellation.",
            ));
        }
        if current.phase() != PackageGraphOperationPhase::Planned {
            return Err(package_manager_error(
                "use.plugin.package_graph_cancel_too_late",
                "The cognitive-package operation crossed durable admission before cancellation.",
            ));
        }
        write_new(&self.state_root, &path, &cancelled).await?;
        Ok((cancelled, true))
    }
}

fn admission_busy(active: &PendingPackageGraphOperation) -> UseError {
    package_manager_error(
        "use.plugin.package_graph_busy",
        format!(
            "Admitted '{}' graph operation for cognitive package '{}' owns the installation mutation domain.",
            action_name(active.action()),
            active.root_package_id()
        ),
    )
    .with_detail(
        "activeOperationId",
        active.envelope.plan.operation_id.clone(),
    )
    .with_detail("activePlanDigest", active.envelope.plan_digest.clone())
}
