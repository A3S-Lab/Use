//! Plan-bound clean-target restore for immutable Gateway catalogs.
//!
//! Catalog records are projections, not lifecycle authority. Restore therefore
//! accepts an exact caller-supplied record set, binds it to a canonical review
//! digest, and publishes the complete owner directory with one no-clobber
//! rename. Existing live owner state is never merged or replaced.

use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, UseError, UseResult};
use a3s_use_extension::StateMaintenanceLock;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    canonical_catalog_bytes, ensure_directory_exists, ensure_owned_directory_chain,
    metadata_is_link_or_reparse_point, path_for_digest, scan_records, sync_directory,
    validate_existing_directory, validate_regular_file, validate_store_layout, write_new_record,
    CapabilityGatewayCatalogStore, CATALOG_LOCK, CATALOG_RETENTION_JOURNAL, CATALOG_STAGING,
    MAX_CAPABILITY_GATEWAY_CATALOG_BYTES, MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS,
};

/// Canonical schema for one reviewed clean-target catalog restore.
pub const CAPABILITY_GATEWAY_CATALOG_RESTORE_PLAN_SCHEMA: &str =
    "a3s.use.capability-gateway-catalog-restore-plan.v1";
/// Canonical schema for one completed clean-target catalog restore.
pub const CAPABILITY_GATEWAY_CATALOG_RESTORE_RESULT_SCHEMA: &str =
    "a3s.use.capability-gateway-catalog-restore-result.v1";

const INVENTORY_DOMAIN: &[u8] = b"a3s.use.capability-gateway-catalog-restore-inventory.v1\0";
const STAGING_PREFIX: &str = ".catalog-restore-";
const CANDIDATE_DIRECTORY: &str = "candidate";
const ACTIVATION_FILE: &str = "activation.json";
const ACTIVATION_PARTIAL_FILE: &str = "activation.json.partial";
const ACTIVATION_SCHEMA: &str = "a3s.use.capability-gateway-catalog-restore-activation.v1";
const MAX_PLAN_BYTES: usize = 4 * 1024 * 1024;
const MAX_ACTIVATION_BYTES: usize = 64 * 1024;
const MAX_RESTORE_BYTES: u64 = MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
    .saturating_mul(MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS as u64);
const ERROR_INVALID: &str = "use.plugin.capability_gateway_catalog_restore_invalid";
const ERROR_TARGET_NOT_EMPTY: &str =
    "use.plugin.capability_gateway_catalog_restore_target_not_empty";

mod layout;

use layout::{reject_foreign_staging, validate_candidate_layout, validate_staging_layout};

/// One immutable catalog named by a clean-target restore plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalogRestoreEntry {
    pub digest: String,
    pub generation: u64,
    pub revision: String,
    pub byte_count: u64,
}

impl CapabilityGatewayCatalogRestoreEntry {
    fn validate(&self) -> UseResult<()> {
        valid_digest(&self.digest)?;
        super::validate_revision(&self.revision).map_err(|_| {
            restore_invalid("A catalog restore entry contains an invalid revision.")
        })?;
        if self.byte_count == 0 || self.byte_count > MAX_CAPABILITY_GATEWAY_CATALOG_BYTES {
            return Err(restore_invalid(
                "A catalog restore entry exceeds its canonical byte bound.",
            ));
        }
        Ok(())
    }
}

/// Exact path-free record set approved for a clean owner restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalogRestorePlan {
    pub schema: String,
    pub installation: InstallationId,
    pub record_count: u64,
    pub byte_count: u64,
    pub inventory_digest: String,
    pub records: Vec<CapabilityGatewayCatalogRestoreEntry>,
}

impl CapabilityGatewayCatalogRestorePlan {
    /// Validate the immutable plan without consulting live state.
    pub fn validate(&self) -> UseResult<()> {
        self.installation
            .validate()
            .map_err(|_| restore_invalid("The catalog restore plan installation is invalid."))?;
        if self.schema != CAPABILITY_GATEWAY_CATALOG_RESTORE_PLAN_SCHEMA
            || self.records.len() > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS
            || self.record_count != u64::try_from(self.records.len()).unwrap_or(u64::MAX)
            || !valid_sha256(&self.inventory_digest)
        {
            return Err(restore_invalid(
                "The catalog restore plan identity or record count is invalid.",
            ));
        }
        let mut byte_count = 0_u64;
        let mut previous = None;
        for record in &self.records {
            record.validate()?;
            if previous.is_some_and(|digest| digest >= record.digest.as_str()) {
                return Err(restore_invalid(
                    "Catalog restore records are duplicated or not canonically ordered.",
                ));
            }
            previous = Some(record.digest.as_str());
            byte_count = byte_count
                .checked_add(record.byte_count)
                .ok_or_else(|| restore_invalid("Catalog restore byte accounting overflowed."))?;
        }
        if self.byte_count != byte_count
            || self.byte_count > MAX_RESTORE_BYTES
            || self.inventory_digest != inventory_digest(&self.records)?
        {
            return Err(restore_invalid(
                "The catalog restore plan inventory accounting is invalid.",
            ));
        }
        let bytes = canonical_json(self, "catalog restore plan")?;
        if bytes.is_empty() || bytes.len() > MAX_PLAN_BYTES {
            return Err(restore_invalid(
                "The catalog restore plan exceeds its canonical byte bound.",
            ));
        }
        Ok(())
    }

    /// Return the canonical digest that must be confirmed at apply time.
    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        Ok(digest(&canonical_json(self, "catalog restore plan")?))
    }
}

/// Bounded evidence for a completed or terminally replayed restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalogRestoreResult {
    pub schema: String,
    pub installation: InstallationId,
    pub plan_digest: String,
    pub inventory_digest: String,
    pub changed: bool,
    pub restored_record_count: u64,
    pub restored_byte_count: u64,
}

impl CapabilityGatewayCatalogRestoreResult {
    pub fn validate(&self) -> UseResult<()> {
        self.installation
            .validate()
            .map_err(|_| restore_invalid("The catalog restore result installation is invalid."))?;
        if self.schema != CAPABILITY_GATEWAY_CATALOG_RESTORE_RESULT_SCHEMA
            || !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.inventory_digest)
            || self.restored_record_count > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS as u64
            || self.restored_byte_count > MAX_RESTORE_BYTES
            || (self.restored_record_count == 0 && self.restored_byte_count != 0)
        {
            return Err(restore_invalid(
                "The catalog restore result identity or accounting is invalid.",
            ));
        }
        Ok(())
    }
}

impl CapabilityGatewayCatalogStore {
    /// Build a path-free plan for an exact catalog record set.
    ///
    /// This is pure review evidence. Apply always re-derives the entries from
    /// the supplied canonical catalogs and refuses to merge them with any
    /// existing live owner directory.
    pub fn plan_clean_restore(
        &self,
        catalogs: &[CapabilityGatewayCatalog],
    ) -> UseResult<CapabilityGatewayCatalogRestorePlan> {
        let prepared = prepare_catalogs(self, catalogs)?;
        let records = prepared
            .iter()
            .map(|record| record.entry.clone())
            .collect::<Vec<_>>();
        let byte_count = records.iter().try_fold(0_u64, |total, record| {
            total
                .checked_add(record.byte_count)
                .ok_or_else(|| restore_invalid("Catalog restore byte accounting overflowed."))
        })?;
        let plan = CapabilityGatewayCatalogRestorePlan {
            schema: CAPABILITY_GATEWAY_CATALOG_RESTORE_PLAN_SCHEMA.to_owned(),
            installation: self.installation.clone(),
            record_count: u64::try_from(records.len()).map_err(|_| {
                restore_invalid("The catalog restore record count exceeds the platform range.")
            })?,
            byte_count,
            inventory_digest: inventory_digest(&records)?,
            records,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Publish one reviewed catalog set only into a clean owner target.
    ///
    /// The complete candidate directory is verified before an activation
    /// marker is persisted, then moved into place with a no-clobber rename.
    /// A retry can recover both sides of that single publication boundary.
    pub async fn apply_clean_restore(
        &self,
        plan: &CapabilityGatewayCatalogRestorePlan,
        catalogs: &[CapabilityGatewayCatalog],
        expected_plan_digest: &str,
    ) -> UseResult<CapabilityGatewayCatalogRestoreResult> {
        plan.validate()?;
        valid_digest(expected_plan_digest)?;
        if plan.installation != self.installation {
            return Err(restore_invalid(
                "The catalog restore plan belongs to another installation.",
            ));
        }
        let plan_digest = plan.descriptor_digest()?;
        if plan_digest != expected_plan_digest {
            return Err(restore_invalid(
                "The confirmed catalog restore plan digest does not match its payload.",
            ));
        }
        let prepared = prepare_catalogs(self, catalogs)?;
        if prepared
            .iter()
            .map(|record| &record.entry)
            .ne(plan.records.iter())
        {
            return Err(restore_invalid(
                "The supplied catalog set differs from the reviewed restore plan.",
            ));
        }

        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_exclusive()
            .await?;
        ensure_directory_exists(&self.state_root).await?;
        let (state_root, root) = self.physical_paths().await?;
        let parent = root.parent().ok_or_else(|| {
            restore_invalid("The catalog restore target has no owned parent directory.")
        })?;
        ensure_owned_directory_chain(&state_root, parent).await?;
        let staging = staging_directory(parent, &plan_digest)?;
        reject_foreign_staging(parent, &staging).await?;

        match inspect_live(self, &root).await? {
            LiveCatalogRoot::Absent => {}
            LiveCatalogRoot::Owned(current) if current == plan.records => {
                retire_completed_staging(self, &staging, plan, &plan_digest).await?;
                return restore_result(plan, plan_digest, false);
            }
            LiveCatalogRoot::Owned(_) => return Err(restore_target_not_empty()),
        }
        if plan.records.is_empty() {
            reject_unexpected_staging(&staging).await?;
            return restore_result(plan, plan_digest, false);
        }

        prepare_staging(self, &state_root, &staging, &prepared, plan, &plan_digest).await?;
        let candidate = staging.join(CANDIDATE_DIRECTORY);
        validate_candidate(self, &candidate, &plan.records).await?;
        if !recover_activation_marker(&staging, plan, &plan_digest).await? {
            create_activation_marker(&staging, plan, &plan_digest).await?;
        }
        validate_candidate(self, &candidate, &plan.records).await?;
        if !matches!(inspect_live(self, &root).await?, LiveCatalogRoot::Absent) {
            return Err(restore_target_not_empty());
        }
        publish_candidate(candidate, root.clone()).await?;
        let LiveCatalogRoot::Owned(current) = inspect_live(self, &root).await? else {
            return Err(restore_invalid(
                "The activated catalog owner directory is missing.",
            ));
        };
        if current != plan.records {
            return Err(restore_invalid(
                "The activated catalog owner inventory differs from its reviewed plan.",
            ));
        }
        retire_staging(&staging, plan, &plan_digest).await?;
        restore_result(plan, plan_digest, true)
    }
}

#[derive(Debug)]
struct PreparedCatalog {
    entry: CapabilityGatewayCatalogRestoreEntry,
    bytes: Vec<u8>,
}

fn prepare_catalogs(
    store: &CapabilityGatewayCatalogStore,
    catalogs: &[CapabilityGatewayCatalog],
) -> UseResult<Vec<PreparedCatalog>> {
    if catalogs.len() > MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS {
        return Err(restore_invalid(
            "The catalog restore source exceeds its record bound.",
        ));
    }
    let mut prepared = catalogs
        .iter()
        .map(|catalog| {
            store.validate_catalog(catalog).map_err(|error| {
                restore_invalid(format!(
                    "A catalog restore source record is invalid: {}",
                    error.message
                ))
            })?;
            let bytes = canonical_catalog_bytes(catalog).map_err(|error| {
                restore_invalid(format!(
                    "A catalog restore source record is not canonical: {}",
                    error.message
                ))
            })?;
            let entry = CapabilityGatewayCatalogRestoreEntry {
                digest: catalog.descriptor_digest()?,
                generation: catalog.generation(),
                revision: catalog.revision().to_owned(),
                byte_count: u64::try_from(bytes.len()).map_err(|_| {
                    restore_invalid("A catalog restore record byte count overflowed.")
                })?,
            };
            entry.validate()?;
            if digest(&bytes) != entry.digest {
                return Err(restore_invalid(
                    "A catalog restore source digest does not match its canonical bytes.",
                ));
            }
            Ok(PreparedCatalog { entry, bytes })
        })
        .collect::<UseResult<Vec<_>>>()?;
    prepared.sort_by(|left, right| left.entry.digest.cmp(&right.entry.digest));
    if prepared
        .windows(2)
        .any(|pair| pair[0].entry.digest == pair[1].entry.digest)
    {
        return Err(restore_invalid(
            "The catalog restore source contains duplicate records.",
        ));
    }
    let total = prepared.iter().try_fold(0_u64, |total, record| {
        total
            .checked_add(record.entry.byte_count)
            .ok_or_else(|| restore_invalid("Catalog restore byte accounting overflowed."))
    })?;
    if total > MAX_RESTORE_BYTES {
        return Err(restore_invalid(
            "The catalog restore source exceeds its total byte bound.",
        ));
    }
    Ok(prepared)
}

enum LiveCatalogRoot {
    Absent,
    Owned(Vec<CapabilityGatewayCatalogRestoreEntry>),
}

async fn inspect_live(
    store: &CapabilityGatewayCatalogStore,
    root: &Path,
) -> UseResult<LiveCatalogRoot> {
    if !validate_existing_directory(root).await? {
        return Ok(LiveCatalogRoot::Absent);
    }
    validate_store_layout(root).await?;
    super::retention::ensure_no_pending_journal(root).await?;
    let records = scan_records(store, root).await?;
    Ok(LiveCatalogRoot::Owned(entries_from_records(&records)?))
}

fn entries_from_records(
    records: &[(String, CapabilityGatewayCatalog)],
) -> UseResult<Vec<CapabilityGatewayCatalogRestoreEntry>> {
    let mut entries = records
        .iter()
        .map(|(digest, catalog)| {
            let bytes = canonical_catalog_bytes(catalog)?;
            Ok(CapabilityGatewayCatalogRestoreEntry {
                digest: digest.clone(),
                generation: catalog.generation(),
                revision: catalog.revision().to_owned(),
                byte_count: u64::try_from(bytes.len()).map_err(|_| {
                    restore_invalid("A live catalog restore byte count overflowed.")
                })?,
            })
        })
        .collect::<UseResult<Vec<_>>>()?;
    entries.sort_by(|left, right| left.digest.cmp(&right.digest));
    Ok(entries)
}

async fn prepare_staging(
    store: &CapabilityGatewayCatalogStore,
    state_root: &Path,
    staging: &Path,
    records: &[PreparedCatalog],
    plan: &CapabilityGatewayCatalogRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    ensure_owned_directory_chain(state_root, staging).await?;
    validate_staging_layout(staging).await?;
    let candidate = staging.join(CANDIDATE_DIRECTORY);
    let activation_started = recover_activation_marker(staging, plan, plan_digest).await?;
    if activation_started {
        if !validate_existing_directory(&candidate).await? {
            return Err(restore_invalid(
                "The catalog restore candidate disappeared after activation began.",
            ));
        }
        return validate_candidate(store, &candidate, &plan.records).await;
    }

    if validate_existing_directory(&candidate).await? {
        if validate_candidate(store, &candidate, &plan.records)
            .await
            .is_ok()
        {
            return Ok(());
        }
        remove_unactivated_candidate(staging, &candidate).await?;
    }
    ensure_owned_directory_chain(staging, &candidate).await?;
    for record in records {
        let target = path_for_digest(&candidate, &record.entry.digest)?;
        write_new_record(&candidate, &target, &record.bytes).await?;
    }
    validate_candidate(store, &candidate, &plan.records).await
}

async fn validate_candidate(
    store: &CapabilityGatewayCatalogStore,
    candidate: &Path,
    expected: &[CapabilityGatewayCatalogRestoreEntry],
) -> UseResult<()> {
    validate_candidate_layout(candidate).await?;
    let records = scan_records(store, candidate)
        .await
        .map_err(wrap_restore_error)?;
    if entries_from_records(&records)? != expected {
        return Err(restore_invalid(
            "The staged catalog restore inventory differs from its reviewed plan.",
        ));
    }
    Ok(())
}

async fn remove_unactivated_candidate(staging: &Path, candidate: &Path) -> UseResult<()> {
    if candidate.parent() != Some(staging)
        || candidate.file_name().and_then(|name| name.to_str()) != Some(CANDIDATE_DIRECTORY)
    {
        return Err(restore_invalid(
            "The catalog restore candidate path is outside its exact staging directory.",
        ));
    }
    let metadata = fs::symlink_metadata(candidate)
        .await
        .map_err(|error| restore_io("inspect incomplete catalog restore candidate", error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "The incomplete catalog restore candidate is not an owned directory.",
        ));
    }
    let candidate = candidate.to_path_buf();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::remove_dir_all_with_windows_retry_blocking(&candidate)
    })
    .await
    .map_err(|error| {
        restore_invalid(format!(
            "The incomplete catalog restore cleanup worker did not complete: {error}"
        ))
    })?
    .map_err(|error| restore_io("remove incomplete catalog restore candidate", error))?;
    sync_directory(staging).await
}

async fn recover_activation_marker(
    staging: &Path,
    plan: &CapabilityGatewayCatalogRestorePlan,
    plan_digest: &str,
) -> UseResult<bool> {
    let expected = activation_bytes(plan, plan_digest)?;
    let marker = staging.join(ACTIVATION_FILE);
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker_length = optional_regular_file_length(&marker).await?;
    let partial_length = optional_regular_file_length(&partial).await?;
    if marker_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "The catalog restore activation marker state is ambiguous.",
        ));
    }
    if let Some(length) = marker_length {
        if length != expected.len() as u64 || read_exact_owned(&marker, length).await? != expected {
            return Err(restore_invalid(
                "The catalog restore activation marker differs from its plan.",
            ));
        }
        return Ok(true);
    }
    let Some(length) = partial_length else {
        return Ok(false);
    };
    if length < expected.len() as u64 {
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_io("remove incomplete catalog restore marker", error))?;
        sync_directory(staging).await?;
        return Ok(false);
    }
    if length != expected.len() as u64 || read_exact_owned(&partial, length).await? != expected {
        return Err(restore_invalid(
            "A complete catalog restore marker partial has unexpected bytes.",
        ));
    }
    publish_noclobber(partial, marker, "publish catalog restore activation marker").await?;
    sync_directory(staging).await?;
    Ok(true)
}

async fn create_activation_marker(
    staging: &Path,
    plan: &CapabilityGatewayCatalogRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    if recover_activation_marker(staging, plan, plan_digest).await? {
        return Ok(());
    }
    let bytes = activation_bytes(plan, plan_digest)?;
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker = staging.join(ACTIVATION_FILE);
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    super::configure_no_follow_async(&mut options);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&partial)
        .await
        .map_err(|error| restore_io("create catalog restore activation marker", error))?;
    file.write_all(&bytes)
        .await
        .map_err(|error| restore_io("write catalog restore activation marker", error))?;
    file.flush()
        .await
        .map_err(|error| restore_io("flush catalog restore activation marker", error))?;
    file.sync_all()
        .await
        .map_err(|error| restore_io("sync catalog restore activation marker", error))?;
    drop(file);
    if read_exact_owned(&partial, bytes.len() as u64).await? != bytes {
        return Err(restore_invalid(
            "The catalog restore activation marker changed before publication.",
        ));
    }
    publish_noclobber(partial, marker, "publish catalog restore activation marker").await?;
    sync_directory(staging).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Activation<'a> {
    schema: &'static str,
    installation: &'a InstallationId,
    plan_digest: &'a str,
    inventory_digest: &'a str,
    record_count: u64,
    byte_count: u64,
}

fn activation_bytes(
    plan: &CapabilityGatewayCatalogRestorePlan,
    plan_digest: &str,
) -> UseResult<Vec<u8>> {
    plan.validate()?;
    valid_digest(plan_digest)?;
    if plan.descriptor_digest()? != plan_digest {
        return Err(restore_invalid(
            "The catalog restore activation digest differs from its plan.",
        ));
    }
    let bytes = canonical_json(
        &Activation {
            schema: ACTIVATION_SCHEMA,
            installation: &plan.installation,
            plan_digest,
            inventory_digest: &plan.inventory_digest,
            record_count: plan.record_count,
            byte_count: plan.byte_count,
        },
        "catalog restore activation",
    )?;
    if bytes.is_empty() || bytes.len() > MAX_ACTIVATION_BYTES {
        return Err(restore_invalid(
            "The catalog restore activation marker exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

async fn publish_candidate(candidate: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_retain_blocking(candidate, &target)
    })
    .await
    .map_err(|error| {
        restore_invalid(format!(
            "The catalog restore publication worker did not complete: {error}"
        ))
    })?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            restore_target_not_empty()
        } else {
            restore_io(
                &format!(
                    "atomically publish catalog restore target '{}'",
                    error_target.display()
                ),
                error,
            )
        }
    })?;
    sync_directory(error_target.parent().ok_or_else(|| {
        restore_invalid("The published catalog restore target has no parent directory.")
    })?)
    .await
}

async fn retire_completed_staging(
    store: &CapabilityGatewayCatalogStore,
    staging: &Path,
    plan: &CapabilityGatewayCatalogRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    match fs::symlink_metadata(staging).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(restore_io(
            "inspect completed catalog restore staging directory",
            error,
        )),
        Ok(metadata) => {
            if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
                return Err(restore_invalid(
                    "The completed catalog restore staging path is not an owned directory.",
                ));
            }
            validate_staging_layout(staging).await?;
            let activation_started = recover_activation_marker(staging, plan, plan_digest).await?;
            let candidate = staging.join(CANDIDATE_DIRECTORY);
            let candidate_exists = validate_existing_directory(&candidate).await?;
            if candidate_exists {
                // A no-clobber publication can lose its process immediately
                // after the target move (or after observing an equivalent
                // competing target). Validate and retire the retained
                // candidate instead of making a safe replay permanently
                // unrecoverable.
                validate_candidate(store, &candidate, &plan.records).await?;
                remove_unactivated_candidate(staging, &candidate).await?;
            }
            if activation_started {
                return retire_staging(staging, plan, plan_digest).await;
            }
            if candidate_exists {
                return retire_unmarked_staging(staging).await;
            }
            Err(restore_invalid(
                "The completed catalog restore has ambiguous staged evidence.",
            ))
        }
    }
}

async fn retire_unmarked_staging(staging: &Path) -> UseResult<()> {
    validate_staging_layout(staging).await?;
    let mut entries = fs::read_dir(staging)
        .await
        .map_err(|error| restore_io("read unmarked catalog restore staging", error))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| restore_io("finish unmarked catalog restore staging", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "The unmarked catalog restore staging directory contains residual evidence.",
        ));
    }
    fs::remove_dir(staging)
        .await
        .map_err(|error| restore_io("retire unmarked catalog restore staging", error))?;
    sync_directory(staging.parent().ok_or_else(|| {
        restore_invalid("The catalog restore staging directory has no owned parent.")
    })?)
    .await
}

async fn retire_staging(
    staging: &Path,
    plan: &CapabilityGatewayCatalogRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    validate_staging_layout(staging).await?;
    if !recover_activation_marker(staging, plan, plan_digest).await?
        || validate_existing_directory(&staging.join(CANDIDATE_DIRECTORY)).await?
    {
        return Err(restore_invalid(
            "The catalog restore staging directory cannot be retired before activation.",
        ));
    }
    let marker = staging.join(ACTIVATION_FILE);
    fs::remove_file(&marker)
        .await
        .map_err(|error| restore_io("retire catalog restore activation marker", error))?;
    sync_directory(staging).await?;
    let mut entries = fs::read_dir(staging)
        .await
        .map_err(|error| restore_io("read retired catalog restore staging directory", error))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| restore_io("finish catalog restore staging directory", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "The catalog restore staging directory contains residual evidence.",
        ));
    }
    fs::remove_dir(staging)
        .await
        .map_err(|error| restore_io("retire catalog restore staging directory", error))?;
    sync_directory(staging.parent().ok_or_else(|| {
        restore_invalid("The catalog restore staging directory has no owned parent.")
    })?)
    .await
}

async fn reject_unexpected_staging(staging: &Path) -> UseResult<()> {
    match fs::symlink_metadata(staging).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(restore_io("inspect empty catalog restore staging", error)),
        Ok(_) => Err(restore_invalid(
            "An empty catalog restore plan has unexpected staged evidence.",
        )),
    }
}

async fn optional_regular_file_length(path: &Path) -> UseResult<Option<u64>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata_is_link_or_reparse_point(&metadata) && metadata.is_file() => {
            if metadata.len() == 0 || metadata.len() > MAX_ACTIVATION_BYTES as u64 {
                return Err(restore_invalid(
                    "A catalog restore marker exceeds its byte bound.",
                ));
            }
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_invalid(
            "A catalog restore marker is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_io("inspect catalog restore marker", error)),
    }
}

async fn read_exact_owned(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    validate_regular_file(path)
        .await
        .map_err(wrap_restore_error)?;
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("inspect catalog restore marker", error))?;
    if metadata.len() != expected_length || expected_length > MAX_ACTIVATION_BYTES as u64 {
        return Err(restore_invalid(
            "A catalog restore marker changed before it was read.",
        ));
    }
    let before = super::file_identity(&metadata);
    let mut options = fs::OpenOptions::new();
    options.read(true);
    super::configure_no_follow_async(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| restore_io("open catalog restore marker", error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| restore_io("inspect opened catalog restore marker", error))?;
    if !opened.is_file()
        || opened.len() != expected_length
        || super::file_identity(&opened) != before
    {
        return Err(restore_invalid(
            "A catalog restore marker changed while it was opened.",
        ));
    }
    let mut bytes = Vec::with_capacity(expected_length as usize);
    (&mut file)
        .take(expected_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| restore_io("read catalog restore marker", error))?;
    if bytes.len() as u64 != expected_length {
        return Err(restore_invalid(
            "A catalog restore marker changed while it was read.",
        ));
    }
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("reinspect catalog restore marker", error))?;
    if metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || super::file_identity(&after) != before
        || after.len() != expected_length
    {
        return Err(restore_invalid(
            "A catalog restore marker changed after it was read.",
        ));
    }
    Ok(bytes)
}

async fn publish_noclobber(
    source: PathBuf,
    target: PathBuf,
    action: &'static str,
) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target)
    })
    .await
    .map_err(|error| restore_invalid(format!("Failed to join {action}: {error}")))?
    .map_err(|error| restore_io(&format!("{action} '{}'", error_target.display()), error))
}

fn staging_directory(parent: &Path, plan_digest: &str) -> UseResult<PathBuf> {
    let hex = plan_digest
        .strip_prefix("sha256:")
        .filter(|value| valid_hex(value, 64))
        .ok_or_else(|| restore_invalid("The catalog restore plan digest is invalid."))?;
    Ok(parent.join(format!("{STAGING_PREFIX}{hex}")))
}

fn restore_result(
    plan: &CapabilityGatewayCatalogRestorePlan,
    plan_digest: String,
    changed: bool,
) -> UseResult<CapabilityGatewayCatalogRestoreResult> {
    let result = CapabilityGatewayCatalogRestoreResult {
        schema: CAPABILITY_GATEWAY_CATALOG_RESTORE_RESULT_SCHEMA.to_owned(),
        installation: plan.installation.clone(),
        plan_digest,
        inventory_digest: plan.inventory_digest.clone(),
        changed,
        restored_record_count: plan.record_count,
        restored_byte_count: plan.byte_count,
    };
    result.validate()?;
    Ok(result)
}

fn inventory_digest(records: &[CapabilityGatewayCatalogRestoreEntry]) -> UseResult<String> {
    let bytes = canonical_json(records, "catalog restore inventory")?;
    let mut hasher = Sha256::new();
    hasher.update(INVENTORY_DOMAIN);
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn canonical_json<T: Serialize + ?Sized>(value: &T, label: &str) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value
        .serialize(&mut serializer)
        .map_err(|error| restore_invalid(format!("Failed to encode canonical {label}: {error}")))?;
    Ok(bytes)
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> UseResult<()> {
    if valid_sha256(value) {
        Ok(())
    } else {
        Err(restore_invalid("A catalog restore digest is invalid."))
    }
}

fn valid_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| valid_hex(hex, 64))
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn wrap_restore_error(error: UseError) -> UseError {
    restore_invalid(format!(
        "Catalog restore owner verification failed: {}",
        error.message
    ))
}

fn restore_target_not_empty() -> UseError {
    UseError::new(
        ERROR_TARGET_NOT_EMPTY,
        "The clean-target catalog restore refuses to merge or replace an existing owner directory.",
    )
}

fn restore_io(action: &str, error: io::Error) -> UseError {
    restore_invalid(format!("Failed to {action}: {error}"))
}

fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_INVALID, message)
}

#[cfg(test)]
mod tests;
