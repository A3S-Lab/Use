// Catalog restore prepare/activation helpers (included into restore).

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
    configure_no_follow_async(&mut options);
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
    let before = file_identity(&metadata);
    let mut options = fs::OpenOptions::new();
    options.read(true);
    configure_no_follow_async(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| restore_io("open catalog restore marker", error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| restore_io("inspect opened catalog restore marker", error))?;
    if !opened.is_file() || opened.len() != expected_length || file_identity(&opened) != before {
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
        || file_identity(&after) != before
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
