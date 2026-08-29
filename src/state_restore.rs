//! Reviewed, authority-bound recovery for a complete A3S Use installation.
//!
//! The coordinated archive is integrity evidence, never its own restore
//! authority. Planning and apply require the live Registry projection,
//! installed receipts, Registry-owned files, and Grant files to remain byte
//! identical to the independently retained authority captured by the backup.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceLock};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state_backup::{
    scan_state_for_restore, validate_owned_roots, validate_state_backup_entry_path,
    StateBackupEntry, StateBackupFamily, StateBackupManager, StateBackupManifest, StateBackupRoot,
    MAX_STATE_BACKUP_BYTES, MAX_STATE_BACKUP_FILES,
};

mod authority;
mod filesystem;
mod journal;

use authority::{authority_entries, validate_live_authority};
pub use journal::{
    ActiveStateRestoreDiagnostic, StateRestoreDiagnostic, StateRestoreDiagnosticStatus,
    StateRestoreOperationDiagnostic, StateRestoreResult, A3S_USE_STATE_RESTORE_DIAGNOSTIC_SCHEMA,
    A3S_USE_STATE_RESTORE_OPERATION_SCHEMA, A3S_USE_STATE_RESTORE_RESULT_SCHEMA,
};
use journal::{StateRestoreOperation, StateRestoreOperationStatus, StateRestoreOperationStore};

pub const A3S_USE_STATE_RESTORE_PLAN_SCHEMA: &str = "a3s.use.state-restore-plan.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateRestorePlanStatus {
    Required,
    NoChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateRestoreActionKind {
    Add,
    Replace,
    Remove,
    Retain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRestoreFileEvidence {
    pub length: u64,
    pub sha256: String,
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_mode: Option<u32>,
}

impl StateRestoreFileEvidence {
    fn from_entry(entry: &StateBackupEntry) -> Self {
        Self {
            length: entry.length,
            sha256: entry.sha256.clone(),
            read_only: entry.read_only,
            unix_mode: entry.unix_mode,
        }
    }

    fn validate(&self) -> bool {
        self.length <= crate::state_backup::MAX_STATE_BACKUP_FILE_BYTES
            && valid_sha256(&self.sha256)
            && self.unix_mode.is_none_or(|mode| mode <= 0o7777)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRestoreAction {
    pub action: StateRestoreActionKind,
    pub root: StateBackupRoot,
    pub path: String,
    pub family: StateBackupFamily,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<StateRestoreFileEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<StateRestoreFileEvidence>,
}

impl StateRestoreAction {
    fn before_entry(&self) -> Option<StateBackupEntry> {
        self.before.as_ref().map(|evidence| StateBackupEntry {
            root: self.root,
            path: self.path.clone(),
            family: self.family,
            length: evidence.length,
            sha256: evidence.sha256.clone(),
            read_only: evidence.read_only,
            unix_mode: evidence.unix_mode,
        })
    }

    fn after_entry(&self) -> Option<StateBackupEntry> {
        self.after.as_ref().map(|evidence| StateBackupEntry {
            root: self.root,
            path: self.path.clone(),
            family: self.family,
            length: evidence.length,
            sha256: evidence.sha256.clone(),
            read_only: evidence.read_only,
            unix_mode: evidence.unix_mode,
        })
    }

    fn validate(&self) -> UseResult<()> {
        if validate_state_backup_entry_path(self.root, &self.path)? != self.family
            || self.before.as_ref().is_some_and(|value| !value.validate())
            || self.after.as_ref().is_some_and(|value| !value.validate())
        {
            return Err(plan_invalid(
                "A state restore action is invalid or exceeds its evidence bounds.",
            ));
        }
        let expected = match (&self.before, &self.after) {
            (None, Some(_)) => StateRestoreActionKind::Add,
            (Some(_), None) => StateRestoreActionKind::Remove,
            (Some(before), Some(after)) if before == after => StateRestoreActionKind::Retain,
            (Some(_), Some(_)) => StateRestoreActionKind::Replace,
            (None, None) => {
                return Err(plan_invalid(
                    "A state restore action has neither prior nor candidate evidence.",
                ))
            }
        };
        if self.action != expected {
            return Err(plan_invalid(
                "A state restore action does not match its prior and candidate evidence.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRestoreActionSummary {
    pub add_files: u64,
    pub add_bytes: u64,
    pub replace_files: u64,
    pub replace_bytes: u64,
    pub remove_files: u64,
    pub remove_bytes: u64,
    pub retain_files: u64,
    pub retain_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRestorePlan {
    pub schema: String,
    pub status: StateRestorePlanStatus,
    pub backup: StateBackupManifest,
    pub backup_manifest_digest: String,
    pub before_inventory_digest: String,
    pub authority_digest: String,
    pub summary: StateRestoreActionSummary,
    pub actions: Vec<StateRestoreAction>,
}

impl StateRestorePlan {
    pub fn validate(&self) -> UseResult<()> {
        self.backup.validate()?;
        if self.schema != A3S_USE_STATE_RESTORE_PLAN_SCHEMA
            || self.backup.use_version != env!("CARGO_PKG_VERSION")
            || self.backup.os != std::env::consts::OS
            || self.backup.architecture != std::env::consts::ARCH
            || !valid_sha256(&self.backup_manifest_digest)
            || !valid_sha256(&self.before_inventory_digest)
            || !valid_sha256(&self.authority_digest)
            || self.backup.descriptor_digest()? != self.backup_manifest_digest
        {
            return Err(plan_invalid(
                "The state restore plan identity or platform is invalid.",
            ));
        }

        let mut prior_key = None;
        let mut portable = BTreeSet::new();
        let mut before = Vec::new();
        let mut after = Vec::new();
        for action in &self.actions {
            action.validate()?;
            let key = (action.root, action.path.as_str());
            if prior_key.is_some_and(|prior| prior >= key)
                || !portable.insert((action.root, action.path.to_ascii_lowercase()))
            {
                return Err(plan_invalid(
                    "State restore actions are not uniquely and portably ordered.",
                ));
            }
            prior_key = Some(key);
            if let Some(entry) = action.before_entry() {
                before.push(entry);
            }
            if let Some(entry) = action.after_entry() {
                after.push(entry);
            }
        }
        validate_entries(&before)?;
        validate_entries(&after)?;
        if after != self.backup.entries
            || digest_entries(&before)? != self.before_inventory_digest
            || summarize_actions(&self.actions)? != self.summary
        {
            return Err(plan_invalid(
                "The state restore action inventory or accounting is inconsistent.",
            ));
        }
        let before_authority = authority_entries(&before);
        let after_authority = authority_entries(&after);
        if before_authority != after_authority
            || digest_entries(&after_authority)? != self.authority_digest
            || self.actions.iter().any(|action| {
                matches!(
                    action.family,
                    StateBackupFamily::Registry | StateBackupFamily::Grants
                ) && action.action != StateRestoreActionKind::Retain
            })
        {
            return Err(plan_invalid(
                "The state restore plan does not preserve exact independent Registry and Grant authority.",
            ));
        }
        let expected_status = if self
            .actions
            .iter()
            .all(|action| action.action == StateRestoreActionKind::Retain)
        {
            StateRestorePlanStatus::NoChange
        } else {
            StateRestorePlanStatus::Required
        };
        if self.status != expected_status {
            return Err(plan_invalid(
                "The state restore plan status does not match its actions.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "state restore plan")
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(sha256(&self.canonical_bytes()?))
    }
}

#[derive(Debug, Clone)]
pub struct StateRestoreManager {
    paths: ExtensionPaths,
    maintenance: StateMaintenanceLock,
    operations: StateRestoreOperationStore,
}

impl StateRestoreManager {
    pub fn new(paths: ExtensionPaths) -> Self {
        Self {
            maintenance: StateMaintenanceLock::new(paths.state_root()),
            operations: StateRestoreOperationStore::new(paths.clone()),
            paths,
        }
    }

    /// Build a path-free, immutable review. This changes no live state and
    /// requires independently retained live Registry and Grant authority.
    pub async fn plan_restore(&self, backup_path: impl AsRef<Path>) -> UseResult<StateRestorePlan> {
        validate_owned_roots(&self.paths)?;
        let backup_path = resolve_external_path(backup_path.as_ref(), &self.paths)?;
        let backup = StateBackupManager::verify_backup(backup_path).await?;
        validate_backup_platform(&backup)?;
        require_backup_installation(&self.paths, &backup)?;
        let _maintenance = self.maintenance.acquire_exclusive().await?;
        let live = scan_state_for_restore(&self.paths, None)?;
        let authority_digest = validate_live_authority(&self.paths, &backup, &live).await?;
        build_plan(backup, live, authority_digest)
    }

    /// Project bounded, path-free restore progress without taking the
    /// maintenance lock or changing marker, journal, or filesystem evidence.
    pub async fn diagnose_restore(&self) -> UseResult<StateRestoreDiagnostic> {
        validate_owned_roots(&self.paths)?;
        self.operations.diagnose().await
    }

    /// Apply exactly one reviewed whole-installation restore and durably
    /// converge the same operation after interruption.
    pub async fn apply_restore(
        &self,
        backup_path: impl AsRef<Path>,
        rollback_backup_path: impl AsRef<Path>,
        reviewed_plan_digest: &str,
    ) -> UseResult<StateRestoreResult> {
        if !valid_sha256(reviewed_plan_digest) {
            return Err(restore_error(
                "use.state_restore_plan_mismatch",
                "Whole-installation restore requires an exact canonical SHA-256 plan digest.",
            ));
        }
        validate_owned_roots(&self.paths)?;
        let backup_path = resolve_external_path(backup_path.as_ref(), &self.paths)?;
        let rollback_backup_path =
            resolve_external_path(rollback_backup_path.as_ref(), &self.paths)?;
        if backup_path == rollback_backup_path {
            return Err(restore_error(
                "use.state_restore_path_invalid",
                "The reviewed backup and rollback backup must use different external paths.",
            ));
        }

        // Existing operations can resume from their exact staged candidates
        // after the external source archive is lost. A still-present source is
        // always reverified and must match the durable plan.
        let inspected = inspect_restore_backup(&self.paths, &backup_path).await;
        let rollback_inspected = inspect_optional_backup(&rollback_backup_path).await?;
        let maintenance = self.maintenance.acquire_exclusive().await?;
        let marker = self.operations.active().await?;
        if marker
            .as_ref()
            .is_some_and(|marker| marker.plan_digest != reviewed_plan_digest)
        {
            return Err(restore_in_progress(
                marker.as_ref().map(|marker| marker.plan_digest.as_str()),
            ));
        }

        if let Some(operation) = self.operations.load(reviewed_plan_digest).await? {
            if let Ok(backup) = &inspected {
                if *backup != operation.plan.backup {
                    return Err(restore_error(
                        "use.state_restore_backup_mismatch",
                        "The supplied backup differs from the backup bound by the durable restore operation.",
                    ));
                }
            }
            if let Some(rollback) = &rollback_inspected {
                validate_rollback_manifest(rollback, &operation.plan)?;
                if rollback.descriptor_digest()? != operation.rollback_backup_manifest_digest {
                    return Err(restore_error(
                        "use.state_restore_rollback_mismatch",
                        "The supplied rollback backup differs from the durable restore operation.",
                    ));
                }
            }
            self.operations.activate(&operation).await?;
            return self
                .resume_restore(operation, &backup_path, &maintenance)
                .await;
        }

        if let Some(marker) = marker {
            let backup = inspected.as_ref().map_err(|_| {
                restore_error(
                    "use.state_restore_backup_unavailable",
                    "The active restore marker has no journal and requires the exact reviewed backup to recover its handoff.",
                )
            })?;
            let rollback = rollback_inspected.as_ref().ok_or_else(|| {
                restore_error(
                    "use.state_restore_rollback_unavailable",
                    "The active restore marker has no journal and requires its exact external rollback backup.",
                )
            })?;
            let live = scan_state_for_restore(&self.paths, Some(reviewed_plan_digest))?;
            let authority_digest = validate_live_authority(&self.paths, backup, &live).await?;
            let plan = build_plan(backup.clone(), live, authority_digest)?;
            if plan.descriptor_digest()? != reviewed_plan_digest {
                return Err(restore_error(
                    "use.state_restore_plan_mismatch",
                    "Use state changed during the active-marker handoff; the durable restore cannot be reconstructed.",
                ));
            }
            validate_rollback_manifest(rollback, &plan)?;
            if rollback.descriptor_digest()? != marker.rollback_backup_manifest_digest {
                return Err(restore_error(
                    "use.state_restore_rollback_mismatch",
                    "The rollback backup differs from the active restore marker.",
                ));
            }
            let operation = marker.recover_operation(plan)?;
            self.operations.begin(&operation).await?;
            maybe_test_crash("journal-planned");
            return self
                .resume_restore(operation, &backup_path, &maintenance)
                .await;
        }

        if let Some(nonterminal) = self.operations.nonterminal().await? {
            return Err(restore_in_progress(Some(&nonterminal.plan_digest)));
        }

        let backup = inspected?;
        let live = scan_state_for_restore(&self.paths, None)?;
        let authority_digest = validate_live_authority(&self.paths, &backup, &live).await?;
        let plan = build_plan(backup, live, authority_digest)?;
        let actual_plan_digest = plan.descriptor_digest()?;
        if actual_plan_digest != reviewed_plan_digest {
            return Err(restore_error(
                "use.state_restore_plan_mismatch",
                "Use state or authority changed after review; create and confirm a new restore plan.",
            )
            .with_detail("actualPlanDigest", serde_json::json!(actual_plan_digest)));
        }
        if plan.status == StateRestorePlanStatus::NoChange {
            return StateRestoreResult::no_change(&plan, reviewed_plan_digest.to_owned());
        }

        let rollback = ensure_rollback_backup(&self.paths, &rollback_backup_path, &plan).await?;
        maybe_test_crash("rollback-captured");
        let operation = StateRestoreOperation::new(
            plan,
            reviewed_plan_digest.to_owned(),
            rollback.descriptor_digest()?,
            now_ms()?,
        )?;
        self.operations.activate(&operation).await?;
        maybe_test_crash("active-marker");
        self.operations.begin(&operation).await?;
        maybe_test_crash("journal-planned");
        self.resume_restore(operation, &backup_path, &maintenance)
            .await
    }

    async fn resume_restore(
        &self,
        mut operation: StateRestoreOperation,
        backup_path: &Path,
        _maintenance: &a3s_use_extension::StateMaintenanceGuard,
    ) -> UseResult<StateRestoreResult> {
        if operation.status == StateRestoreOperationStatus::Completed {
            self.validate_terminal(&operation).await?;
            self.operations.clear_active(&operation).await?;
            return operation.result();
        }
        self.operations.activate(&operation).await?;
        if operation.status == StateRestoreOperationStatus::Planned {
            filesystem::stage_candidates(&self.paths, backup_path, &operation).await?;
            self.advance(&mut operation, StateRestoreOperationStatus::Staged, None)
                .await?;
        }
        if operation.status == StateRestoreOperationStatus::Staged {
            let live = scan_state_for_restore(&self.paths, Some(&operation.plan_digest))?;
            if digest_entries(&live)? != operation.plan.before_inventory_digest {
                return Err(restore_error(
                    "use.state_restore_state_mismatch",
                    "Live state changed after review and before restore publication.",
                ));
            }
            let authority =
                validate_live_authority(&self.paths, &operation.plan.backup, &live).await?;
            if authority != operation.plan.authority_digest {
                return Err(authority_mismatch(
                    "Live Registry or Grant authority changed before restore publication.",
                ));
            }
            self.advance(
                &mut operation,
                StateRestoreOperationStatus::Publishing,
                None,
            )
            .await?;
        }
        if operation.status == StateRestoreOperationStatus::Publishing {
            filesystem::apply_actions(&self.paths, &operation).await?;
            self.advance(&mut operation, StateRestoreOperationStatus::Published, None)
                .await?;
        }
        if operation.status == StateRestoreOperationStatus::Published {
            filesystem::remove_candidates(&self.paths, &operation).await?;
            self.advance(
                &mut operation,
                StateRestoreOperationStatus::CandidatesRemoved,
                None,
            )
            .await?;
        }
        if operation.status == StateRestoreOperationStatus::CandidatesRemoved {
            self.validate_terminal(&operation).await?;
            self.advance(&mut operation, StateRestoreOperationStatus::Verified, None)
                .await?;
        }
        if operation.status == StateRestoreOperationStatus::Verified {
            self.advance(
                &mut operation,
                StateRestoreOperationStatus::Completed,
                Some(now_ms()?),
            )
            .await?;
        }
        if operation.status != StateRestoreOperationStatus::Completed {
            return Err(restore_error(
                "use.state_restore_operation_invalid",
                "The whole-installation restore did not reach a terminal state.",
            ));
        }
        self.validate_terminal(&operation).await?;
        self.operations.clear_active(&operation).await?;
        operation.result()
    }

    async fn advance(
        &self,
        operation: &mut StateRestoreOperation,
        status: StateRestoreOperationStatus,
        completed_at_ms: Option<u64>,
    ) -> UseResult<()> {
        operation.advance(status, completed_at_ms)?;
        self.operations.save(operation).await?;
        maybe_test_crash(status.checkpoint());
        Ok(())
    }

    async fn validate_terminal(&self, operation: &StateRestoreOperation) -> UseResult<()> {
        filesystem::validate_candidates_absent(&self.paths, operation)?;
        let live = scan_state_for_restore(&self.paths, Some(&operation.plan_digest))?;
        if live != operation.plan.backup.entries {
            return Err(restore_error(
                "use.state_restore_terminal_mismatch",
                "Published Use state does not exactly match the reviewed backup inventory.",
            ));
        }
        let authority = validate_live_authority(&self.paths, &operation.plan.backup, &live).await?;
        if authority != operation.plan.authority_digest {
            return Err(authority_mismatch(
                "Published Registry or Grant authority differs from the reviewed restore.",
            ));
        }
        Ok(())
    }
}

fn build_plan(
    backup: StateBackupManifest,
    live: Vec<StateBackupEntry>,
    authority_digest: String,
) -> UseResult<StateRestorePlan> {
    let actions = build_actions(&live, &backup.entries);
    let status = if actions
        .iter()
        .all(|action| action.action == StateRestoreActionKind::Retain)
    {
        StateRestorePlanStatus::NoChange
    } else {
        StateRestorePlanStatus::Required
    };
    let plan = StateRestorePlan {
        schema: A3S_USE_STATE_RESTORE_PLAN_SCHEMA.to_owned(),
        status,
        backup_manifest_digest: backup.descriptor_digest()?,
        before_inventory_digest: digest_entries(&live)?,
        authority_digest,
        summary: summarize_actions(&actions)?,
        backup,
        actions,
    };
    plan.validate()?;
    Ok(plan)
}

fn build_actions(
    before: &[StateBackupEntry],
    after: &[StateBackupEntry],
) -> Vec<StateRestoreAction> {
    let before = before
        .iter()
        .map(|entry| ((entry.root, entry.path.as_str()), entry))
        .collect::<BTreeMap<_, _>>();
    let after = after
        .iter()
        .map(|entry| ((entry.root, entry.path.as_str()), entry))
        .collect::<BTreeMap<_, _>>();
    before
        .keys()
        .chain(after.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|key| {
            let before = before.get(&key).copied();
            let after = after.get(&key).copied();
            let entry = after.or(before).expect("a merged restore key has evidence");
            let before_evidence = before.map(StateRestoreFileEvidence::from_entry);
            let after_evidence = after.map(StateRestoreFileEvidence::from_entry);
            let action = match (&before_evidence, &after_evidence) {
                (None, Some(_)) => StateRestoreActionKind::Add,
                (Some(_), None) => StateRestoreActionKind::Remove,
                (Some(left), Some(right)) if left == right => StateRestoreActionKind::Retain,
                (Some(_), Some(_)) => StateRestoreActionKind::Replace,
                (None, None) => unreachable!("a merged restore key has evidence"),
            };
            StateRestoreAction {
                action,
                root: entry.root,
                path: entry.path.clone(),
                family: entry.family,
                before: before_evidence,
                after: after_evidence,
            }
        })
        .collect()
}

fn summarize_actions(actions: &[StateRestoreAction]) -> UseResult<StateRestoreActionSummary> {
    let mut summary = StateRestoreActionSummary::default();
    for action in actions {
        let (files, bytes, evidence) = match action.action {
            StateRestoreActionKind::Add => (
                &mut summary.add_files,
                &mut summary.add_bytes,
                action.after.as_ref(),
            ),
            StateRestoreActionKind::Replace => (
                &mut summary.replace_files,
                &mut summary.replace_bytes,
                action.after.as_ref(),
            ),
            StateRestoreActionKind::Remove => (
                &mut summary.remove_files,
                &mut summary.remove_bytes,
                action.before.as_ref(),
            ),
            StateRestoreActionKind::Retain => (
                &mut summary.retain_files,
                &mut summary.retain_bytes,
                action.after.as_ref(),
            ),
        };
        *files = files
            .checked_add(1)
            .ok_or_else(|| plan_invalid("State restore action accounting overflowed."))?;
        *bytes = bytes
            .checked_add(evidence.map_or(0, |value| value.length))
            .ok_or_else(|| plan_invalid("State restore byte accounting overflowed."))?;
    }
    Ok(summary)
}

fn validate_entries(entries: &[StateBackupEntry]) -> UseResult<()> {
    if entries.len() as u64 > MAX_STATE_BACKUP_FILES {
        return Err(plan_invalid(
            "A state restore inventory exceeds its file-count bound.",
        ));
    }
    let mut bytes = 0u64;
    let mut prior = None;
    let mut portable = BTreeSet::new();
    for entry in entries {
        if validate_state_backup_entry_path(entry.root, &entry.path)? != entry.family
            || !valid_sha256(&entry.sha256)
            || entry.unix_mode.is_some_and(|mode| mode > 0o7777)
            || prior.is_some_and(|prior| prior >= (entry.root, entry.path.as_str()))
            || !portable.insert((entry.root, entry.path.to_ascii_lowercase()))
        {
            return Err(plan_invalid(
                "A state restore inventory entry is invalid or unsorted.",
            ));
        }
        bytes = bytes
            .checked_add(entry.length)
            .ok_or_else(|| plan_invalid("State restore inventory bytes overflowed."))?;
        if bytes > MAX_STATE_BACKUP_BYTES {
            return Err(plan_invalid(
                "A state restore inventory exceeds its byte bound.",
            ));
        }
        prior = Some((entry.root, entry.path.as_str()));
    }
    Ok(())
}

fn digest_entries(entries: &[StateBackupEntry]) -> UseResult<String> {
    validate_entries(entries)?;
    Ok(sha256(&canonical_json(entries, "state restore inventory")?))
}

fn validate_backup_platform(backup: &StateBackupManifest) -> UseResult<()> {
    backup.validate()?;
    if backup.use_version != env!("CARGO_PKG_VERSION")
        || backup.os != std::env::consts::OS
        || backup.architecture != std::env::consts::ARCH
    {
        return Err(restore_error(
            "use.state_restore_incompatible",
            "Whole-installation restore requires the exact current Use version, OS, and architecture.",
        ));
    }
    Ok(())
}

fn require_backup_installation(
    paths: &ExtensionPaths,
    backup: &StateBackupManifest,
) -> UseResult<()> {
    if backup.installation != *paths.installation() {
        return Err(restore_error(
            "use.state_restore_installation_mismatch",
            "The state backup belongs to a different installation.",
        ));
    }
    Ok(())
}

fn canonical_json<T: Serialize + ?Sized>(value: &T, label: &str) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        plan_invalid(format!("The canonical {label} cannot be encoded: {error}"))
    })?;
    Ok(bytes)
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

async fn inspect_restore_backup(
    paths: &ExtensionPaths,
    path: &Path,
) -> UseResult<StateBackupManifest> {
    let backup = StateBackupManager::verify_backup(path).await?;
    validate_backup_platform(&backup)?;
    require_backup_installation(paths, &backup)?;
    Ok(backup)
}

async fn inspect_optional_backup(path: &Path) -> UseResult<Option<StateBackupManifest>> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => StateBackupManager::verify_backup(path).await.map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_io(format!(
            "The rollback backup cannot be inspected: {error}"
        ))),
    }
}

async fn ensure_rollback_backup(
    paths: &ExtensionPaths,
    rollback_path: &Path,
    plan: &StateRestorePlan,
) -> UseResult<StateBackupManifest> {
    let manifest = match inspect_optional_backup(rollback_path).await? {
        Some(manifest) => manifest,
        None => {
            StateBackupManager::new(paths.clone())
                .backup_under_exclusive(rollback_path)
                .await?
        }
    };
    validate_rollback_manifest(&manifest, plan)?;
    Ok(manifest)
}

fn validate_rollback_manifest(
    manifest: &StateBackupManifest,
    plan: &StateRestorePlan,
) -> UseResult<()> {
    validate_backup_platform(manifest)?;
    if manifest.installation != plan.backup.installation {
        return Err(restore_error(
            "use.state_restore_rollback_mismatch",
            "The rollback backup belongs to a different installation.",
        ));
    }
    let before = plan
        .actions
        .iter()
        .filter_map(StateRestoreAction::before_entry)
        .collect::<Vec<_>>();
    if manifest.entries != before
        || manifest.inventory_digest != plan.before_inventory_digest
        || manifest.authority != plan.backup.authority
    {
        return Err(restore_error(
            "use.state_restore_rollback_mismatch",
            "The explicit rollback backup does not exactly preserve the reviewed live inventory and authority.",
        ));
    }
    Ok(())
}

fn resolve_external_path(path: &Path, paths: &ExtensionPaths) -> UseResult<PathBuf> {
    let file_name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            restore_error(
                "use.state_restore_path_invalid",
                "A whole-installation restore archive path has no file name.",
            )
        })?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent_metadata = std::fs::symlink_metadata(parent).map_err(|error| {
        restore_io(format!(
            "A whole-installation restore archive directory cannot be inspected: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&parent_metadata)
        || !parent_metadata.is_dir()
    {
        return Err(restore_error(
            "use.state_restore_path_invalid",
            "A whole-installation restore archive parent is not an owned directory.",
        ));
    }
    let parent = std::fs::canonicalize(parent).map_err(|error| {
        restore_io(format!(
            "A whole-installation restore archive directory cannot be resolved: {error}"
        ))
    })?;
    let resolved = parent.join(file_name);
    for root in [
        paths.use_paths().data_root(),
        paths.use_paths().state_root(),
    ] {
        let root = match std::fs::canonicalize(root) {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if root.is_absolute() {
                    root.to_path_buf()
                } else {
                    std::env::current_dir()
                        .map_err(|error| {
                            restore_io(format!("The current directory is unavailable: {error}"))
                        })?
                        .join(root)
                }
            }
            Err(error) => {
                return Err(restore_io(format!(
                    "A Use-owned root cannot be resolved: {error}"
                )))
            }
        };
        if resolved.starts_with(root) {
            return Err(restore_error(
                "use.state_restore_path_invalid",
                "Whole-installation restore and rollback archives must remain outside Use-owned roots.",
            ));
        }
    }
    Ok(resolved)
}

fn now_ms() -> UseResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            restore_io(format!(
                "The system clock is before the Unix epoch: {error}"
            ))
        })?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| restore_io("The system clock exceeds the supported millisecond range."))
}

fn restore_in_progress(plan_digest: Option<&str>) -> UseError {
    let mut error = restore_error(
        "use.state_restore_in_progress",
        "A durable whole-installation restore must reach its terminal result before another restore can start.",
    )
    .with_suggestion(
        "Resume the active restore with its exact backup, rollback destination, and reviewed plan digest.",
    );
    if let Some(plan_digest) = plan_digest {
        error = error.with_detail("activePlanDigest", serde_json::json!(plan_digest));
    }
    error
}

#[cfg(test)]
pub(super) const RESTORE_CRASH_CHECKPOINT_ENV: &str = "A3S_USE_TEST_STATE_RESTORE_CHECKPOINT";

#[cfg(test)]
pub(super) fn maybe_test_crash(checkpoint: &str) {
    if std::env::var(RESTORE_CRASH_CHECKPOINT_ENV).as_deref() == Ok(checkpoint) {
        std::process::exit(87);
    }
}

#[cfg(not(test))]
pub(super) fn maybe_test_crash(_checkpoint: &str) {}

fn plan_invalid(message: impl Into<String>) -> UseError {
    restore_error("use.state_restore_plan_invalid", message)
}

fn authority_mismatch(message: impl Into<String>) -> UseError {
    restore_error("use.state_restore_authority_mismatch", message).with_suggestion(
        "Recover the exact Registry and Grant authority independently before reviewing this backup again.",
    )
}

fn restore_io(message: impl Into<String>) -> UseError {
    restore_error("use.state_restore_io", message)
}

fn restore_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests;
