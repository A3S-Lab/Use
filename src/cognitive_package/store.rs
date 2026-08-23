use std::collections::BTreeMap;
use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    PlanPackageChangeKind, PluginOperationAction, PluginOperationPlanEnvelope, PluginPackageId,
    PluginPackageLock, UseError, UseResult, MAX_PLUGIN_PLAN_ITEMS,
};
use a3s_use_extension::{validate_catalog_manifest_binding, ExtensionManifest};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::grant::PackageGraphAuthorization;
use super::package_manager_error;

const INSTALLED_GRAPH_SCHEMA: &str = "a3s.use.installed-package-graph.v1";
const PENDING_GRAPH_SCHEMA: &str = "a3s.use.pending-package-graph-operation.v4";
const MAX_GRAPH_RECORD_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct InstalledPackageGraph {
    pub schema: String,
    pub package_lock_digest: String,
    pub package_lock: PluginPackageLock,
    pub installed_at_ms: u64,
}

impl InstalledPackageGraph {
    fn new(package_lock: PluginPackageLock, installed_at_ms: u64) -> UseResult<Self> {
        let graph = Self {
            schema: INSTALLED_GRAPH_SCHEMA.to_string(),
            package_lock_digest: package_lock.descriptor_digest()?,
            package_lock,
            installed_at_ms,
        };
        graph.validate()?;
        Ok(graph)
    }

    fn validate(&self) -> UseResult<()> {
        self.package_lock.validate()?;
        if self.schema != INSTALLED_GRAPH_SCHEMA
            || self.package_lock_digest != self.package_lock.descriptor_digest()?
            || self.installed_at_ms == 0
        {
            return Err(store_error(
                "An installed cognitive-package graph record is invalid.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// One current v4 graph operation from reviewed planning through admission or
/// pre-admission cancellation. Superseded preview records fail closed.
pub(super) struct PendingPackageGraphOperation {
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
pub(super) enum PackageGraphOperationPhase {
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
pub(super) struct InstalledPackageGraphStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl InstalledPackageGraphStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("package-graphs"),
            state_root,
        }
    }

    pub async fn put(&self, lock: &PluginPackageLock, installed_at_ms: u64) -> UseResult<bool> {
        let record = InstalledPackageGraph::new(lock.clone(), installed_at_ms)?;
        let _guard = acquire_lock(&self.state_root).await?;
        let path = package_record_path(&self.root, &lock.root_package_id)?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        let current = if validate_existing_directory_chain(&self.state_root, parent).await? {
            read_optional::<InstalledPackageGraph>(&path).await?
        } else {
            None
        };
        if let Some(current) = current {
            current.validate()?;
            if current.package_lock == record.package_lock {
                return Ok(false);
            }
            return Err(package_manager_error(
                "use.plugin.package_graph_reconcile_required",
                format!(
                    "Cognitive package '{}' already owns a different installed dependency lock.",
                    lock.root_package_id
                ),
            ));
        }
        write_new(&self.state_root, &path, &record).await?;
        Ok(true)
    }

    pub async fn get(&self, root_package_id: &str) -> UseResult<Option<InstalledPackageGraph>> {
        let path = package_record_path(&self.root, root_package_id)?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Ok(None);
        }
        let value: Option<InstalledPackageGraph> = read_optional(&path).await?;
        if let Some(value) = &value {
            value.validate()?;
            if value.package_lock.root_package_id != root_package_id {
                return Err(store_error(
                    "An installed graph record does not match its root package path.",
                ));
            }
        }
        Ok(value)
    }

    pub async fn replace(
        &self,
        root_package_id: &str,
        expected_digest: &str,
        replacement: &PluginPackageLock,
        installed_at_ms: u64,
    ) -> UseResult<bool> {
        if replacement.root_package_id != root_package_id {
            return Err(store_error(
                "A replacement graph does not own the requested root package.",
            ));
        }
        let record = InstalledPackageGraph::new(replacement.clone(), installed_at_ms)?;
        let _guard = acquire_lock(&self.state_root).await?;
        let path = package_record_path(&self.root, root_package_id)?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Err(store_error(
                "The installed package graph disappeared before replacement.",
            ));
        }
        let current = read_optional::<InstalledPackageGraph>(&path)
            .await?
            .ok_or_else(|| {
                store_error("The installed package graph disappeared before replacement.")
            })?;
        current.validate()?;
        if current.package_lock == record.package_lock {
            return Ok(false);
        }
        if current.package_lock_digest != expected_digest {
            return Err(store_error(
                "The installed package graph changed before replacement.",
            ));
        }
        write_new(&self.state_root, &path, &record).await?;
        Ok(true)
    }

    pub async fn list(&self) -> UseResult<Vec<InstalledPackageGraph>> {
        let mut records = Vec::new();
        if !validate_existing_directory_chain(&self.state_root, &self.root).await? {
            return Ok(records);
        }
        let mut publishers = match fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(error) => return Err(path_error("read installed graph store", &self.root, error)),
        };
        while let Some(publisher) = publishers
            .next_entry()
            .await
            .map_err(|error| path_error("read installed graph publisher", &self.root, error))?
        {
            let publisher_path = publisher.path();
            let publisher_metadata = fs::symlink_metadata(&publisher_path)
                .await
                .map_err(|error| path_error("inspect graph publisher", &publisher_path, error))?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&publisher_metadata)
                || !publisher_metadata.is_dir()
            {
                return Err(store_error(
                    "The installed graph store contains an invalid publisher entry.",
                ));
            }
            let mut packages = fs::read_dir(&publisher_path).await.map_err(|error| {
                path_error("read installed graph packages", &publisher_path, error)
            })?;
            while let Some(package) = packages.next_entry().await.map_err(|error| {
                path_error("read installed graph package", &publisher_path, error)
            })? {
                let package_path = package.path();
                let package_metadata =
                    fs::symlink_metadata(&package_path).await.map_err(|error| {
                        path_error("inspect installed graph record", &package_path, error)
                    })?;
                if records.len() >= MAX_PLUGIN_PLAN_ITEMS
                    || a3s_use_core::metadata_is_link_or_reparse_point(&package_metadata)
                    || !package_metadata.is_file()
                    || package_path.extension().and_then(|value| value.to_str()) != Some("json")
                {
                    return Err(store_error(
                        "The installed graph store contains an invalid or oversized record set.",
                    ));
                }
                let record = read_required::<InstalledPackageGraph>(&package_path).await?;
                record.validate()?;
                records.push(record);
            }
        }
        records.sort_by(|left, right| {
            left.package_lock
                .root_package_id
                .cmp(&right.package_lock.root_package_id)
        });
        Ok(records)
    }

    pub async fn remove(&self, root_package_id: &str, expected_digest: &str) -> UseResult<bool> {
        let _guard = acquire_lock(&self.state_root).await?;
        let path = package_record_path(&self.root, root_package_id)?;
        let parent = path.parent().ok_or_else(path_identity_error)?;
        if !validate_existing_directory_chain(&self.state_root, parent).await? {
            return Ok(false);
        }
        let Some(current) = read_optional::<InstalledPackageGraph>(&path).await? else {
            return Ok(false);
        };
        current.validate()?;
        if current.package_lock_digest != expected_digest {
            return Err(store_error(
                "The installed package graph changed before removal.",
            ));
        }
        fs::remove_file(&path)
            .await
            .map_err(|error| path_error("remove installed package graph", &path, error))?;
        sync_parent(path.parent().ok_or_else(path_identity_error)?).await?;
        Ok(true)
    }
}

#[derive(Debug, Clone)]
pub(super) struct PendingPackageGraphStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl PendingPackageGraphStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("operations").join("package-graphs"),
            state_root,
        }
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

    pub async fn put(&self, value: &PendingPackageGraphOperation) -> UseResult<bool> {
        value.validate()?;
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

fn package_record_path(root: &Path, package_id: &str) -> UseResult<PathBuf> {
    PluginPackageId::parse(package_id.to_string())
        .map_err(|_| store_error("A package graph path contains an invalid package identity."))?;
    let (publisher, package) = package_id
        .split_once('/')
        .ok_or_else(|| store_error("A package graph path is incomplete."))?;
    Ok(root.join(publisher).join(format!("{package}.json")))
}

fn pending_record_path(
    root: &Path,
    action: PluginOperationAction,
    package_id: &str,
) -> UseResult<PathBuf> {
    Ok(root
        .join(action_name(action))
        .join(package_record_path(Path::new(""), package_id)?))
}

fn action_name(action: PluginOperationAction) -> &'static str {
    match action {
        PluginOperationAction::Install => "install",
        PluginOperationAction::Uninstall => "uninstall",
        PluginOperationAction::Upgrade => "upgrade",
        PluginOperationAction::Enable => "enable",
        PluginOperationAction::Disable => "disable",
    }
}

async fn read_optional<T>(path: &Path) -> UseResult<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file()
                && metadata.len() <= MAX_GRAPH_RECORD_BYTES => {}
        Ok(_) => return Err(store_error("A package graph record path is invalid.")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect package graph record", path, error)),
    }
    read_required(path).await.map(Some)
}

async fn read_required<T>(path: &Path) -> UseResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect package graph record", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_GRAPH_RECORD_BYTES
    {
        return Err(store_error("A package graph record path is invalid."));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read package graph record", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_GRAPH_RECORD_BYTES {
        return Err(store_error(
            "A package graph record exceeds its size bound.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| store_error("A package graph record contains invalid JSON."))
}

async fn write_new<T: Serialize>(state_root: &Path, path: &Path, value: &T) -> UseResult<()> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(path_identity_error());
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| store_error("Failed to encode a package graph record."))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_GRAPH_RECORD_BYTES {
        return Err(store_error(
            "A package graph record exceeds its size bound.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| store_error("A package graph record has no owned parent."))?;
    ensure_owned_directory(state_root, parent).await?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{suffix}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary package graph record", &temporary, error))?;
    if let Err(error) = async {
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await?;
        Ok::<_, std::io::Error>(())
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error("commit package graph record", path, error));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(temporary.clone(), path.to_path_buf()).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent(parent).await
}

async fn activate_temporary_file(temporary: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_replace_blocking(temporary, &target)
    })
    .await
    .map_err(|error| {
        package_manager_error(
            "use.plugin.package_graph_io",
            format!(
                "Failed to commit package graph record '{}': atomic replacement task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| path_error("commit package graph record", &error_target, error))
}

async fn acquire_lock(state_root: &Path) -> UseResult<StdFile> {
    ensure_owned_directory(state_root, state_root).await?;
    let path = state_root.join(".package-graph.lock");
    match fs::symlink_metadata(&path).await {
        Ok(metadata)
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                || !metadata.is_file() =>
        {
            return Err(path_identity_error())
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect package graph lock", &path, error)),
    }
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| path_error("open package graph lock", &path, error))?;
        file.lock_exclusive()
            .map_err(|error| path_error("lock package graph store", &path, error))?;
        Ok(file)
    })
    .await
    .map_err(|error| {
        store_error(format!(
            "Failed to join the package graph lock task: {error}"
        ))
    })?
}

async fn ensure_owned_directory(state_root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(state_root) {
        return Err(path_identity_error());
    }
    fs::create_dir_all(state_root)
        .await
        .map_err(|error| path_error("create package graph state root", state_root, error))?;
    validate_directory(state_root).await?;
    let relative = directory
        .strip_prefix(state_root)
        .map_err(|_| path_identity_error())?;
    let mut current = state_root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create package graph directory",
                    &current,
                    error,
                ))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory_chain(state_root: &Path, directory: &Path) -> UseResult<bool> {
    if !directory.starts_with(state_root) {
        return Err(path_identity_error());
    }
    let relative = directory
        .strip_prefix(state_root)
        .map_err(|_| path_identity_error())?;
    let mut current = state_root.to_path_buf();
    for segment in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(segment) = segment {
            current.push(segment.as_os_str());
        }
        match fs::symlink_metadata(&current).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => return Err(path_identity_error()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    "inspect package graph directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(true)
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect package graph directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(path_identity_error());
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_parent(parent: &Path) -> UseResult<()> {
    fs::File::open(parent)
        .await
        .map_err(|error| path_error("open package graph directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync package graph directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: &Path) -> UseResult<()> {
    Ok(())
}

fn path_identity_error() -> UseError {
    store_error("A package graph record escaped or traversed its configured state root.")
}

fn path_error(operation: &str, path: &Path, error: std::io::Error) -> UseError {
    package_manager_error(
        "use.plugin.package_graph_io",
        format!("Failed to {operation} '{}': {error}", path.display()),
    )
}

fn store_error(message: impl Into<String>) -> UseError {
    package_manager_error("use.plugin.package_graph_store_invalid", message)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::cognitive_package::{InstallDisposition, UninstallDisposition, UpgradeDisposition};
    use a3s_use_core::{
        CatalogAvailability, PlanScope, PlanScopeKind, PluginCatalogRecord, PluginPackageLockHost,
        PluginPackageResolver, PluginWorkspaceGrantSnapshot, SurfaceChangeKind,
        VerifiedCatalogProvenance, VerifiedPluginCatalogRecord, PLUGIN_CATALOG_SCHEMA_V3,
        PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
    };

    const CATALOG: &[u8] =
        include_bytes!("../../crates/core/fixtures/plugins/catalog-record-okf-v3.json");
    const MANIFEST: &str =
        include_str!("../../crates/extension/fixtures/manifests/plugin-v3-okf.acl");

    fn digest(seed: char) -> String {
        format!("sha256:{}", seed.to_string().repeat(64))
    }

    fn manifest(version: &str) -> ExtensionManifest {
        let mut manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
        manifest.version = version.to_string();
        manifest
    }

    fn package_lock(version: &str, seed: char) -> PluginPackageLock {
        let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
        record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
        record.version = version.to_string();
        record.archive.target_name = format!(
            "extensions/acme/knowledge/{version}/stable/linux-x86_64/acme-knowledge-{version}.tar.gz"
        );
        record.archive.sha256 = digest(seed);
        record.package.sha256 = Some(digest(seed));
        record.package.manifest_sha256 = Some(digest(seed));
        record.availability = CatalogAvailability::Available;
        record.validate().unwrap();
        let provenance = VerifiedCatalogProvenance {
            registry_name: "packages".to_string(),
            registry_url: "https://packages.example.test/a3s/".to_string(),
            root_sha256: digest('f'),
            root_version: 1,
            timestamp_version: 1,
            snapshot_version: 1,
            targets_version: 1,
            catalog_record_digest: record.descriptor_digest().unwrap(),
        };
        let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
        PluginPackageResolver::new(
            PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
        )
        .resolve(verified, Vec::new())
        .unwrap()
    }

    fn grant_snapshot(state_revision: u64) -> PluginWorkspaceGrantSnapshot {
        PluginWorkspaceGrantSnapshot {
            schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
            scope_id: "current".to_string(),
            state_revision,
            grants: Vec::new(),
        }
    }

    fn scope() -> PlanScope {
        PlanScope {
            kind: PlanScopeKind::User,
            id: "current".to_string(),
        }
    }

    fn install_pending(lock: &PluginPackageLock) -> PendingPackageGraphOperation {
        let package_id = lock.root_package_id.clone();
        let manifests =
            BTreeMap::from([(package_id.clone(), manifest(lock.packages[0].version()))]);
        let dispositions = BTreeMap::from([(package_id, InstallDisposition::Add)]);
        let generated = crate::cognitive_package::plan::install_operation(
            lock,
            &dispositions,
            &crate::cognitive_package::plan::all_surface_selections(lock),
            &manifests,
            1,
            &scope(),
            10,
            &grant_snapshot(2),
            &crate::cognitive_package::StandaloneCognitivePackageAuthorizationProvider,
        )
        .unwrap();
        PendingPackageGraphOperation::planned(
            generated.envelope,
            10,
            generated.generations,
            manifests,
        )
        .unwrap()
        .admit(10, PackageGraphAuthorization::default())
        .unwrap()
    }

    fn uninstall_pending(lock: &PluginPackageLock) -> PendingPackageGraphOperation {
        let package_id = lock.root_package_id.clone();
        let manifests =
            BTreeMap::from([(package_id.clone(), manifest(lock.packages[0].version()))]);
        let dispositions = BTreeMap::from([(package_id.clone(), UninstallDisposition::Remove)]);
        let generations = BTreeMap::from([(package_id, 7)]);
        let generated = crate::cognitive_package::plan::uninstall_operation(
            lock,
            &dispositions,
            &crate::cognitive_package::plan::all_surface_selections(lock),
            generations,
            digest('9'),
            1,
            &scope(),
            10,
            &grant_snapshot(2),
            &crate::cognitive_package::StandaloneCognitivePackageAuthorizationProvider,
        )
        .unwrap();
        PendingPackageGraphOperation::planned(
            generated.envelope,
            10,
            generated.generations,
            manifests,
        )
        .unwrap()
        .admit(10, PackageGraphAuthorization::default())
        .unwrap()
    }

    fn upgrade_pending(
        prior: &PluginPackageLock,
        candidate: &PluginPackageLock,
    ) -> PendingPackageGraphOperation {
        let package_id = candidate.root_package_id.clone();
        let manifests = BTreeMap::from([(
            package_id.clone(),
            manifest(candidate.packages[0].version()),
        )]);
        let prior_manifests =
            BTreeMap::from([(package_id.clone(), manifest(prior.packages[0].version()))]);
        let dispositions = BTreeMap::from([(package_id.clone(), UpgradeDisposition::Replace)]);
        let prior_generations = BTreeMap::from([(package_id, 7)]);
        let generated = crate::cognitive_package::plan::upgrade_operation(
            candidate,
            prior,
            &dispositions,
            &crate::cognitive_package::plan::all_surface_selections(prior),
            &crate::cognitive_package::plan::all_surface_selections(candidate),
            &manifests,
            &prior_generations,
            digest('9'),
            8,
            &scope(),
            10,
            &grant_snapshot(9),
            &crate::cognitive_package::StandaloneCognitivePackageAuthorizationProvider,
        )
        .unwrap();
        PendingPackageGraphOperation::planned_upgrade(
            generated.envelope,
            10,
            generated.generations,
            manifests,
            prior.clone(),
            prior_generations,
            prior_manifests,
        )
        .unwrap()
        .admit(10, PackageGraphAuthorization::default())
        .unwrap()
    }

    mod records;
    #[cfg(unix)]
    mod symlinks;
}
