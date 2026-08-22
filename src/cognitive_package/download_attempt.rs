use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use a3s_use_core::{
    PlanScope, PluginOperationAction, PluginPackageId, PluginPackageLock, UseResult,
    MAX_PLUGIN_PLAN_ITEMS,
};
use serde::{Deserialize, Serialize};

use super::planning_attempt_io::{
    read_optional_json, remove_file, validate_existing_directory_chain, write_json,
    PackagePlanningLock, PlanningAttemptKind,
};

const DOWNLOAD_ATTEMPT_SCHEMA: &str = "a3s.use.plugin-download-attempt.v1";
const MAX_DOWNLOAD_ATTEMPT_BYTES: u64 = 2 * 1024 * 1024;

/// Exact non-authoritative target set retained before a reviewed graph exists.
///
/// The record deliberately contains the verified package lock so cache
/// observation remains bound to historical Registry provenance after source
/// replacement. It is never accepted as planning, apply, or recovery input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PendingPackageDownloadAttempt {
    pub schema: String,
    pub scope: PlanScope,
    pub action: PluginOperationAction,
    pub root_package_id: String,
    pub package_lock_digest: String,
    pub package_lock: PluginPackageLock,
    pub selected_package_ids: BTreeSet<String>,
    pub started_at_ms: u64,
}

impl PendingPackageDownloadAttempt {
    pub fn new(
        scope: PlanScope,
        action: PluginOperationAction,
        package_lock: PluginPackageLock,
        selected_package_ids: BTreeSet<String>,
        started_at_ms: u64,
    ) -> UseResult<Self> {
        let record = Self {
            schema: DOWNLOAD_ATTEMPT_SCHEMA.to_owned(),
            scope,
            action,
            root_package_id: package_lock.root_package_id.clone(),
            package_lock_digest: package_lock.descriptor_digest()?,
            package_lock,
            selected_package_ids,
            started_at_ms,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.package_lock.validate().map_err(|_| store_invalid())?;
        PluginPackageId::parse(self.root_package_id.clone()).map_err(|_| store_invalid())?;
        if self.schema != DOWNLOAD_ATTEMPT_SCHEMA
            || !matches!(
                self.action,
                PluginOperationAction::Install | PluginOperationAction::Upgrade
            )
            || !valid_scope_id(&self.scope.id)
            || self.root_package_id != self.package_lock.root_package_id
            || self.package_lock_digest
                != self
                    .package_lock
                    .descriptor_digest()
                    .map_err(|_| store_invalid())?
            || self.selected_package_ids.is_empty()
            || self.selected_package_ids.len() > MAX_PLUGIN_PLAN_ITEMS
            || self
                .selected_package_ids
                .iter()
                .any(|package_id| self.package_lock.package(package_id).is_none())
            || self.started_at_ms == 0
        {
            return Err(store_invalid());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(super) struct PackageDownloadAttemptStore {
    state_root: PathBuf,
    root: PathBuf,
}

#[derive(Debug)]
pub(super) struct ActivePackageDownloadAttempt {
    record: PendingPackageDownloadAttempt,
    state_root: PathBuf,
    path: PathBuf,
    _lock: PackagePlanningLock,
}

impl PackageDownloadAttemptStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("operations/package-downloads"),
            state_root,
        }
    }

    /// Start one exact pre-plan attempt while holding a process-recoverable
    /// package lock. A new process may replace a retained attempt only after
    /// the prior process exits and releases this lock.
    #[cfg(test)]
    pub async fn begin(
        &self,
        record: PendingPackageDownloadAttempt,
    ) -> UseResult<ActivePackageDownloadAttempt> {
        record.validate()?;
        let lock = super::planning_attempt_io::acquire_package_lock(
            &self.state_root,
            &record.root_package_id,
            PlanningAttemptKind::Download,
        )
        .await?;
        self.begin_locked(record, lock).await
    }

    pub(super) async fn begin_locked(
        &self,
        record: PendingPackageDownloadAttempt,
        lock: PackagePlanningLock,
    ) -> UseResult<ActivePackageDownloadAttempt> {
        record.validate()?;
        if !lock.validates(&record.root_package_id) {
            return Err(store_invalid());
        }

        let mut existing_paths = Vec::new();
        for action in [
            PluginOperationAction::Install,
            PluginOperationAction::Upgrade,
        ] {
            let path = attempt_record_path(&self.root, action, &record.root_package_id)?;
            let Some(parent) = path.parent() else {
                return Err(store_invalid());
            };
            if !validate_existing_directory_chain(
                &self.state_root,
                parent,
                PlanningAttemptKind::Download,
            )
            .await?
            {
                continue;
            }
            let Some(existing) = read_optional(&path).await? else {
                continue;
            };
            existing.validate()?;
            if existing.action != action
                || existing.root_package_id != record.root_package_id
                || path
                    != attempt_record_path(&self.root, existing.action, &existing.root_package_id)?
            {
                return Err(store_invalid());
            }
            existing_paths.push(path);
        }
        if existing_paths.len() > 1 {
            return Err(store_invalid());
        }
        let target = attempt_record_path(&self.root, record.action, &record.root_package_id)?;
        if let Some(path) = existing_paths
            .first()
            .filter(|path| path.as_path() != target)
        {
            remove_file(&self.state_root, path, PlanningAttemptKind::Download).await?;
        }

        let path = target;
        write_json(
            &self.state_root,
            &path,
            &record,
            MAX_DOWNLOAD_ATTEMPT_BYTES,
            PlanningAttemptKind::Download,
        )
        .await?;
        Ok(ActivePackageDownloadAttempt {
            record,
            state_root: self.state_root.clone(),
            path,
            _lock: lock,
        })
    }

    pub async fn get_for_package(
        &self,
        root_package_id: &str,
    ) -> UseResult<Option<PendingPackageDownloadAttempt>> {
        PluginPackageId::parse(root_package_id.to_owned()).map_err(|_| store_invalid())?;
        let mut found = None;
        for action in [
            PluginOperationAction::Install,
            PluginOperationAction::Upgrade,
        ] {
            let path = attempt_record_path(&self.root, action, root_package_id)?;
            let parent = path.parent().ok_or_else(store_invalid)?;
            if !validate_existing_directory_chain(
                &self.state_root,
                parent,
                PlanningAttemptKind::Download,
            )
            .await?
            {
                continue;
            }
            let Some(record) = read_optional(&path).await? else {
                continue;
            };
            record.validate()?;
            if record.action != action || record.root_package_id != root_package_id {
                return Err(store_invalid());
            }
            if found.replace(record).is_some() {
                return Err(store_invalid());
            }
        }
        Ok(found)
    }

    pub(super) async fn remove_for_package_locked(
        &self,
        root_package_id: &str,
        lock: &PackagePlanningLock,
    ) -> UseResult<()> {
        PluginPackageId::parse(root_package_id.to_owned()).map_err(|_| store_invalid())?;
        if !lock.validates(root_package_id) {
            return Err(store_invalid());
        }
        let mut found = None;
        for action in [
            PluginOperationAction::Install,
            PluginOperationAction::Upgrade,
        ] {
            let path = attempt_record_path(&self.root, action, root_package_id)?;
            let parent = path.parent().ok_or_else(store_invalid)?;
            if !validate_existing_directory_chain(
                &self.state_root,
                parent,
                PlanningAttemptKind::Download,
            )
            .await?
            {
                continue;
            }
            let Some(record) = read_optional(&path).await? else {
                continue;
            };
            record.validate()?;
            if record.action != action || record.root_package_id != root_package_id {
                return Err(store_invalid());
            }
            if found.replace(path).is_some() {
                return Err(store_invalid());
            }
        }
        if let Some(path) = found {
            remove_file(&self.state_root, &path, PlanningAttemptKind::Download).await?;
        }
        Ok(())
    }
}

impl ActivePackageDownloadAttempt {
    /// Remove the pre-plan record only after a reviewed pending graph is
    /// durable. Dropping the guard without finishing intentionally retains the
    /// observation after a failure or process exit.
    pub async fn finish(self) -> UseResult<()> {
        self.record.validate()?;
        let parent = self.path.parent().ok_or_else(store_invalid)?;
        if !validate_existing_directory_chain(
            &self.state_root,
            parent,
            PlanningAttemptKind::Download,
        )
        .await?
        {
            return Err(store_invalid());
        }
        let current = read_optional(&self.path).await?.ok_or_else(store_invalid)?;
        if current != self.record {
            return Err(store_invalid());
        }
        remove_file(&self.state_root, &self.path, PlanningAttemptKind::Download).await
    }
}

fn attempt_record_path(
    root: &Path,
    action: PluginOperationAction,
    package_id: &str,
) -> UseResult<PathBuf> {
    let action = match action {
        PluginOperationAction::Install => "install",
        PluginOperationAction::Upgrade => "upgrade",
        PluginOperationAction::Uninstall
        | PluginOperationAction::Enable
        | PluginOperationAction::Disable => return Err(store_invalid()),
    };
    Ok(root
        .join(action)
        .join(package_relative_path(package_id, "json")?))
}

fn package_relative_path(package_id: &str, extension: &str) -> UseResult<PathBuf> {
    super::planning_attempt_io::package_relative_path(
        package_id,
        extension,
        PlanningAttemptKind::Download,
    )
}

async fn read_optional(path: &Path) -> UseResult<Option<PendingPackageDownloadAttempt>> {
    read_optional_json(
        path,
        MAX_DOWNLOAD_ATTEMPT_BYTES,
        PlanningAttemptKind::Download,
    )
    .await
}

fn valid_scope_id(value: &str) -> bool {
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

fn store_invalid() -> a3s_use_core::UseError {
    super::planning_attempt_io::store_invalid(PlanningAttemptKind::Download)
}
