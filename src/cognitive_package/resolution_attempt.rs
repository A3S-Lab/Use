use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use a3s_use_core::{
    PlanScope, PluginOperationAction, PluginPackageId, PluginPackageLock, PluginReleaseChannel,
    UseResult, MAX_PLUGIN_PLAN_ITEMS,
};
use a3s_use_extension::{
    PackageRegistryResolutionObserver, TrustedRegistry, VerifiedRegistryMetadata,
    MAX_CONFIGURED_REGISTRY_SOURCES,
};
use async_trait::async_trait;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use super::download_attempt::{
    ActivePackageDownloadAttempt, PackageDownloadAttemptStore, PendingPackageDownloadAttempt,
};
use super::plan::now_ms;
use super::planning_attempt_io::{
    acquire_package_lock, package_relative_path, read_optional_json, remove_file,
    validate_existing_directory_chain, write_json, PackagePlanningLock, PlanningAttemptKind,
};

const RESOLUTION_ATTEMPT_SCHEMA: &str = "a3s.use.plugin-resolution-attempt.v1";
const MAX_RESOLUTION_ATTEMPT_BYTES: u64 = 256 * 1024;
const MAX_REGISTRY_PACKAGE_TARGETS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PackageResolutionAccess {
    Refreshed,
    Cached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PackageResolutionAttemptStatus {
    Resolving,
    Resolved,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PackageRegistryResolutionRole {
    Root,
    Dependency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PackageRegistryResolutionStatus {
    Pending,
    Verifying,
    Verified,
    Failed,
}

/// Non-authoritative, path-free evidence for the Registry/TUF work that starts
/// before an exact package lock exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PendingPackageResolutionAttempt {
    pub schema: String,
    pub scope: PlanScope,
    pub action: PluginOperationAction,
    pub root_package_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_version: Option<String>,
    pub channel: PluginReleaseChannel,
    pub access: PackageResolutionAccess,
    pub started_at_ms: u64,
    pub status: PackageResolutionAttemptStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_lock_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub registries: Vec<PendingRegistryResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PendingRegistryResolution {
    pub registry_name: String,
    pub role: PackageRegistryResolutionRole,
    pub source_identity_digest: String,
    pub trust_root_digest: String,
    pub status: PackageRegistryResolutionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_targets: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct PackageResolutionAttemptStore {
    state_root: PathBuf,
    root: PathBuf,
}

#[derive(Debug)]
pub(super) struct ActivePackageResolutionAttempt {
    state_root: PathBuf,
    path: PathBuf,
    root_package_id: String,
    action: PluginOperationAction,
    lock: Option<PackagePlanningLock>,
}

impl PendingPackageResolutionAttempt {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        scope: PlanScope,
        action: PluginOperationAction,
        root_package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        access: PackageResolutionAccess,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        started_at_ms: u64,
    ) -> UseResult<Self> {
        let mut registries =
            BTreeMap::<String, (&TrustedRegistry, PackageRegistryResolutionRole)>::new();
        insert_registry(
            &mut registries,
            root_registry,
            PackageRegistryResolutionRole::Root,
        )?;
        for registry in dependency_registries {
            insert_registry(
                &mut registries,
                registry,
                PackageRegistryResolutionRole::Dependency,
            )?;
        }
        let record = Self {
            schema: RESOLUTION_ATTEMPT_SCHEMA.to_owned(),
            scope,
            action,
            root_package_id: root_package_id.to_owned(),
            requested_version: requested_version.map(str::to_owned),
            channel,
            access,
            started_at_ms,
            status: PackageResolutionAttemptStatus::Resolving,
            completed_at_ms: None,
            package_lock_digest: None,
            package_count: None,
            error_code: None,
            registries: registries
                .into_iter()
                .map(
                    |(registry_name, (registry, role))| PendingRegistryResolution {
                        registry_name,
                        role,
                        source_identity_digest: format!("sha256:{}", registry.source_identity()),
                        trust_root_digest: format!("sha256:{}", registry.root_sha256()),
                        status: PackageRegistryResolutionStatus::Pending,
                        root_version: None,
                        timestamp_version: None,
                        snapshot_version: None,
                        targets_version: None,
                        package_targets: None,
                        observed_at_ms: None,
                        error_code: None,
                    },
                )
                .collect(),
        };
        record.validate()?;
        Ok(record)
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        PluginPackageId::parse(self.root_package_id.clone()).map_err(|_| store_invalid())?;
        if self.schema != RESOLUTION_ATTEMPT_SCHEMA
            || !matches!(
                self.action,
                PluginOperationAction::Install | PluginOperationAction::Upgrade
            )
            || !valid_machine_id(&self.scope.id)
            || self.started_at_ms == 0
            || self
                .requested_version
                .as_deref()
                .is_some_and(|selector| !valid_version_selector(selector))
            || self.registries.is_empty()
            || self.registries.len() > MAX_CONFIGURED_REGISTRY_SOURCES
            || self
                .registries
                .windows(2)
                .any(|pair| pair[0].registry_name >= pair[1].registry_name)
            || self
                .registries
                .iter()
                .filter(|registry| registry.role == PackageRegistryResolutionRole::Root)
                .count()
                != 1
        {
            return Err(store_invalid());
        }
        for registry in &self.registries {
            registry.validate()?;
        }
        let states = self
            .registries
            .iter()
            .map(|registry| registry.status)
            .collect::<Vec<_>>();
        match self.status {
            PackageResolutionAttemptStatus::Resolving => {
                if self.completed_at_ms.is_some()
                    || self.package_lock_digest.is_some()
                    || self.package_count.is_some()
                    || self.error_code.is_some()
                    || states.contains(&PackageRegistryResolutionStatus::Failed)
                    || !ordered_active_states(&states)
                {
                    return Err(store_invalid());
                }
            }
            PackageResolutionAttemptStatus::Resolved => {
                if !valid_terminal_time(self.started_at_ms, self.completed_at_ms)
                    || !self
                        .package_lock_digest
                        .as_deref()
                        .is_some_and(valid_sha256)
                    || !self
                        .package_count
                        .is_some_and(|count| count > 0 && count as usize <= MAX_PLUGIN_PLAN_ITEMS)
                    || self.error_code.is_some()
                    || states
                        .iter()
                        .any(|status| *status != PackageRegistryResolutionStatus::Verified)
                {
                    return Err(store_invalid());
                }
            }
            PackageResolutionAttemptStatus::Failed => {
                if !valid_terminal_time(self.started_at_ms, self.completed_at_ms)
                    || self.package_lock_digest.is_some()
                    || self.package_count.is_some()
                    || !self.error_code.as_deref().is_some_and(valid_error_code)
                    || !ordered_failed_states(&states)
                {
                    return Err(store_invalid());
                }
            }
        }
        Ok(())
    }
}

fn valid_version_selector(selector: &str) -> bool {
    Version::parse(selector).is_ok_and(|version| version.to_string() == selector)
        || VersionReq::parse(selector).is_ok_and(|requirement| requirement.to_string() == selector)
}

impl PendingRegistryResolution {
    fn validate(&self) -> UseResult<()> {
        if !valid_registry_name(&self.registry_name)
            || !valid_sha256(&self.source_identity_digest)
            || !valid_sha256(&self.trust_root_digest)
        {
            return Err(store_invalid());
        }
        let versions = [
            self.root_version,
            self.timestamp_version,
            self.snapshot_version,
            self.targets_version,
        ];
        match self.status {
            PackageRegistryResolutionStatus::Pending => {
                if versions.iter().any(Option::is_some)
                    || self.package_targets.is_some()
                    || self.observed_at_ms.is_some()
                    || self.error_code.is_some()
                {
                    return Err(store_invalid());
                }
            }
            PackageRegistryResolutionStatus::Verifying => {
                if versions.iter().any(Option::is_some)
                    || self.package_targets.is_some()
                    || self.observed_at_ms.is_none_or(|time| time == 0)
                    || self.error_code.is_some()
                {
                    return Err(store_invalid());
                }
            }
            PackageRegistryResolutionStatus::Verified => {
                if versions
                    .iter()
                    .any(|version| !version.is_some_and(|value| value > 0))
                    || self
                        .package_targets
                        .is_none_or(|count| count > MAX_REGISTRY_PACKAGE_TARGETS)
                    || self.observed_at_ms.is_none_or(|time| time == 0)
                    || self.error_code.is_some()
                {
                    return Err(store_invalid());
                }
            }
            PackageRegistryResolutionStatus::Failed => {
                if versions.iter().any(Option::is_some)
                    || self.package_targets.is_some()
                    || self.observed_at_ms.is_none_or(|time| time == 0)
                    || !self.error_code.as_deref().is_some_and(valid_error_code)
                {
                    return Err(store_invalid());
                }
            }
        }
        Ok(())
    }
}

impl PackageResolutionAttemptStore {
    pub(super) fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("operations/package-resolutions"),
            state_root,
        }
    }

    pub(super) async fn begin(
        &self,
        record: PendingPackageResolutionAttempt,
    ) -> UseResult<ActivePackageResolutionAttempt> {
        record.validate()?;
        let lock = acquire_package_lock(
            &self.state_root,
            &record.root_package_id,
            PlanningAttemptKind::Resolution,
        )
        .await?;
        // Validate both retained state families before deleting either one.
        // A damaged resolution record must not cause valid download evidence
        // to be reconciled away during a retry.
        let existing = self.existing_record_path(&record.root_package_id).await?;
        PackageDownloadAttemptStore::new(&self.state_root)
            .remove_for_package_locked(&record.root_package_id, &lock)
            .await?;

        let target = self.record_path(record.action, &record.root_package_id)?;
        if let Some(path) = existing.filter(|path| path != &target) {
            remove_file(&self.state_root, &path, PlanningAttemptKind::Resolution).await?;
        }
        write_json(
            &self.state_root,
            &target,
            &record,
            MAX_RESOLUTION_ATTEMPT_BYTES,
            PlanningAttemptKind::Resolution,
        )
        .await?;
        Ok(ActivePackageResolutionAttempt {
            state_root: self.state_root.clone(),
            path: target,
            root_package_id: record.root_package_id,
            action: record.action,
            lock: Some(lock),
        })
    }

    pub(super) async fn get_for_package(
        &self,
        package_id: &str,
    ) -> UseResult<Option<PendingPackageResolutionAttempt>> {
        PluginPackageId::parse(package_id.to_owned()).map_err(|_| store_invalid())?;
        let Some(path) = self.existing_record_path(package_id).await? else {
            return Ok(None);
        };
        let record = read_record(&path).await?.ok_or_else(store_invalid)?;
        record.validate()?;
        Ok(Some(record))
    }

    async fn existing_record_path(&self, package_id: &str) -> UseResult<Option<PathBuf>> {
        let mut found = None;
        for action in [
            PluginOperationAction::Install,
            PluginOperationAction::Upgrade,
        ] {
            let path = self.record_path(action, package_id)?;
            let parent = path.parent().ok_or_else(store_invalid)?;
            if !validate_existing_directory_chain(
                &self.state_root,
                parent,
                PlanningAttemptKind::Resolution,
            )
            .await?
            {
                continue;
            }
            let Some(record) = read_record(&path).await? else {
                continue;
            };
            record.validate()?;
            if record.action != action || record.root_package_id != package_id {
                return Err(store_invalid());
            }
            if found.replace(path).is_some() {
                return Err(store_invalid());
            }
        }
        Ok(found)
    }

    fn record_path(&self, action: PluginOperationAction, package_id: &str) -> UseResult<PathBuf> {
        let action = action_segment(action)?;
        Ok(self.root.join(action).join(package_relative_path(
            package_id,
            "json",
            PlanningAttemptKind::Resolution,
        )?))
    }
}

impl ActivePackageResolutionAttempt {
    pub(super) async fn mark_resolved(&self, lock: &PluginPackageLock) -> UseResult<()> {
        lock.validate().map_err(|_| store_invalid())?;
        if lock.root_package_id != self.root_package_id {
            return Err(store_invalid());
        }
        let mut record = self.current().await?;
        if record.status == PackageResolutionAttemptStatus::Resolved {
            if record.package_lock_digest.as_deref() == Some(&lock.descriptor_digest()?)
                && record.package_count == u32::try_from(lock.packages.len()).ok()
            {
                return Ok(());
            }
            return Err(store_invalid());
        }
        if record.status != PackageResolutionAttemptStatus::Resolving
            || record
                .registries
                .iter()
                .any(|registry| registry.status != PackageRegistryResolutionStatus::Verified)
        {
            return Err(store_invalid());
        }
        record.status = PackageResolutionAttemptStatus::Resolved;
        record.completed_at_ms = Some(now_ms()?);
        record.package_lock_digest = Some(lock.descriptor_digest()?);
        record.package_count =
            Some(u32::try_from(lock.packages.len()).map_err(|_| store_invalid())?);
        record.validate()?;
        self.write(&record).await
    }

    pub(super) async fn mark_failed(&self, error_code: &str) -> UseResult<()> {
        let mut record = self.current().await?;
        if record.status == PackageResolutionAttemptStatus::Failed {
            return if record.error_code.as_deref() == Some(error_code) {
                Ok(())
            } else {
                Err(store_invalid())
            };
        }
        if record.status != PackageResolutionAttemptStatus::Resolving
            || !valid_error_code(error_code)
        {
            return Err(store_invalid());
        }
        record.status = PackageResolutionAttemptStatus::Failed;
        record.completed_at_ms = Some(now_ms()?);
        record.error_code = Some(error_code.to_owned());
        record.validate()?;
        self.write(&record).await
    }

    pub(super) async fn into_download(
        mut self,
        store: &PackageDownloadAttemptStore,
        download: PendingPackageDownloadAttempt,
    ) -> UseResult<ActivePackageDownloadAttempt> {
        let current = self.current().await?;
        if current.status != PackageResolutionAttemptStatus::Resolved
            || current.scope != download.scope
            || current.action != download.action
            || current.root_package_id != download.root_package_id
            || current.package_lock_digest.as_deref() != Some(&download.package_lock_digest)
        {
            return Err(store_invalid());
        }
        let lock = self.lock.take().ok_or_else(store_invalid)?;
        let active = store.begin_locked(download, lock).await?;
        remove_file(
            &self.state_root,
            &self.path,
            PlanningAttemptKind::Resolution,
        )
        .await?;
        Ok(active)
    }

    pub(super) async fn finish(mut self) -> UseResult<()> {
        self.current().await?;
        remove_file(
            &self.state_root,
            &self.path,
            PlanningAttemptKind::Resolution,
        )
        .await?;
        self.lock.take().ok_or_else(store_invalid)?;
        Ok(())
    }

    pub(super) async fn current(&self) -> UseResult<PendingPackageResolutionAttempt> {
        let record = read_record(&self.path).await?.ok_or_else(store_invalid)?;
        record.validate()?;
        if record.root_package_id != self.root_package_id || record.action != self.action {
            return Err(store_invalid());
        }
        Ok(record)
    }

    async fn write(&self, record: &PendingPackageResolutionAttempt) -> UseResult<()> {
        record.validate()?;
        if record.root_package_id != self.root_package_id || record.action != self.action {
            return Err(store_invalid());
        }
        write_json(
            &self.state_root,
            &self.path,
            record,
            MAX_RESOLUTION_ATTEMPT_BYTES,
            PlanningAttemptKind::Resolution,
        )
        .await
    }
}

#[async_trait]
impl PackageRegistryResolutionObserver for ActivePackageResolutionAttempt {
    async fn registry_resolution_started(&self, registry_name: &str) -> UseResult<()> {
        let mut record = self.current().await?;
        if record.status != PackageResolutionAttemptStatus::Resolving {
            return Err(store_invalid());
        }
        let registry = registry_mut(&mut record, registry_name)?;
        match registry.status {
            PackageRegistryResolutionStatus::Pending => {
                registry.status = PackageRegistryResolutionStatus::Verifying;
                registry.observed_at_ms = Some(now_ms()?);
            }
            PackageRegistryResolutionStatus::Verifying => return Ok(()),
            PackageRegistryResolutionStatus::Verified | PackageRegistryResolutionStatus::Failed => {
                return Err(store_invalid())
            }
        }
        record.validate()?;
        self.write(&record).await
    }

    async fn registry_resolution_verified(
        &self,
        metadata: &VerifiedRegistryMetadata,
    ) -> UseResult<()> {
        let mut record = self.current().await?;
        if record.status != PackageResolutionAttemptStatus::Resolving {
            return Err(store_invalid());
        }
        let registry = registry_mut(&mut record, &metadata.registry_name)?;
        if registry.status != PackageRegistryResolutionStatus::Verifying
            || registry.trust_root_digest != format!("sha256:{}", metadata.root_sha256)
        {
            return Err(store_invalid());
        }
        registry.status = PackageRegistryResolutionStatus::Verified;
        registry.root_version = Some(metadata.root_version);
        registry.timestamp_version = Some(metadata.timestamp_version);
        registry.snapshot_version = Some(metadata.snapshot_version);
        registry.targets_version = Some(metadata.targets_version);
        registry.package_targets = Some(metadata.package_targets);
        registry.observed_at_ms = Some(now_ms()?);
        record.validate()?;
        self.write(&record).await
    }

    async fn registry_resolution_failed(
        &self,
        registry_name: &str,
        error_code: &str,
    ) -> UseResult<()> {
        let mut record = self.current().await?;
        if record.status != PackageResolutionAttemptStatus::Resolving
            || !valid_error_code(error_code)
        {
            return Err(store_invalid());
        }
        let observed_at_ms = now_ms()?;
        let registry = registry_mut(&mut record, registry_name)?;
        if registry.status != PackageRegistryResolutionStatus::Verifying {
            return Err(store_invalid());
        }
        registry.status = PackageRegistryResolutionStatus::Failed;
        registry.observed_at_ms = Some(observed_at_ms);
        registry.error_code = Some(error_code.to_owned());
        record.status = PackageResolutionAttemptStatus::Failed;
        record.completed_at_ms = Some(observed_at_ms);
        record.error_code = Some(error_code.to_owned());
        record.validate()?;
        self.write(&record).await
    }
}

fn insert_registry<'a>(
    registries: &mut BTreeMap<String, (&'a TrustedRegistry, PackageRegistryResolutionRole)>,
    registry: &'a TrustedRegistry,
    role: PackageRegistryResolutionRole,
) -> UseResult<()> {
    if let Some((existing, existing_role)) = registries.get_mut(registry.name()) {
        if existing.base_url() != registry.base_url()
            || existing.root_sha256() != registry.root_sha256()
        {
            return Err(store_invalid());
        }
        if role == PackageRegistryResolutionRole::Root {
            *existing_role = role;
        }
        return Ok(());
    }
    registries.insert(registry.name().to_owned(), (registry, role));
    Ok(())
}

fn registry_mut<'a>(
    record: &'a mut PendingPackageResolutionAttempt,
    registry_name: &str,
) -> UseResult<&'a mut PendingRegistryResolution> {
    record
        .registries
        .iter_mut()
        .find(|registry| registry.registry_name == registry_name)
        .ok_or_else(store_invalid)
}

fn action_segment(action: PluginOperationAction) -> UseResult<&'static str> {
    match action {
        PluginOperationAction::Install => Ok("install"),
        PluginOperationAction::Upgrade => Ok("upgrade"),
        PluginOperationAction::Uninstall
        | PluginOperationAction::Enable
        | PluginOperationAction::Disable => Err(store_invalid()),
    }
}

async fn read_record(path: &Path) -> UseResult<Option<PendingPackageResolutionAttempt>> {
    read_optional_json(
        path,
        MAX_RESOLUTION_ATTEMPT_BYTES,
        PlanningAttemptKind::Resolution,
    )
    .await
}

fn ordered_active_states(states: &[PackageRegistryResolutionStatus]) -> bool {
    states
        .windows(2)
        .all(|pair| active_rank(pair[0]) <= active_rank(pair[1]))
        && states
            .iter()
            .filter(|status| **status == PackageRegistryResolutionStatus::Verifying)
            .count()
            <= 1
}

fn ordered_failed_states(states: &[PackageRegistryResolutionStatus]) -> bool {
    states.iter().all(|status| {
        matches!(
            status,
            PackageRegistryResolutionStatus::Verified
                | PackageRegistryResolutionStatus::Failed
                | PackageRegistryResolutionStatus::Pending
        )
    }) && states
        .windows(2)
        .all(|pair| failed_rank(pair[0]) <= failed_rank(pair[1]))
        && states
            .iter()
            .filter(|status| **status == PackageRegistryResolutionStatus::Failed)
            .count()
            <= 1
}

fn active_rank(status: PackageRegistryResolutionStatus) -> u8 {
    match status {
        PackageRegistryResolutionStatus::Verified => 0,
        PackageRegistryResolutionStatus::Verifying => 1,
        PackageRegistryResolutionStatus::Pending => 2,
        PackageRegistryResolutionStatus::Failed => 3,
    }
}

fn failed_rank(status: PackageRegistryResolutionStatus) -> u8 {
    match status {
        PackageRegistryResolutionStatus::Verified => 0,
        PackageRegistryResolutionStatus::Failed => 1,
        PackageRegistryResolutionStatus::Pending => 2,
        PackageRegistryResolutionStatus::Verifying => 3,
    }
}

fn valid_terminal_time(started_at_ms: u64, completed_at_ms: Option<u64>) -> bool {
    completed_at_ms.is_some_and(|completed| completed >= started_at_ms)
}

fn valid_registry_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

fn valid_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn store_invalid() -> a3s_use_core::UseError {
    super::planning_attempt_io::store_invalid(PlanningAttemptKind::Resolution)
}
