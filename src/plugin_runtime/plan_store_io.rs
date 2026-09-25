// Runtime surface plan store IO helpers (included into plan_store).

async fn scan_records_at(
    root: &Path,
    expected_installation: Option<&InstallationId>,
) -> UseResult<Vec<(PathBuf, RuntimeSurfacePlanStoreRecord)>> {
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| path_error("read Runtime plan store", root, error))?;
    let mut records = Vec::new();
    let mut entries_seen = 0_usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read Runtime plan store entry", root, error))?
    {
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_PLAN_STORE_DIRECTORY_ENTRIES {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "The Runtime plan store directory exceeds its entry bound.",
            ));
        }
        let file_name = entry.file_name();
        let name = file_name.to_str().ok_or_else(|| {
            store_error(PLAN_STORE_ERROR, "A Runtime plan filename is not UTF-8.")
        })?;
        if name == PLAN_STORE_LOCK {
            validate_regular_file(&entry.path()).await?;
            continue;
        }
        if is_temporary_name(name) {
            validate_temporary_file(&entry.path()).await?;
            continue;
        }
        if !is_record_name(name) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "The Runtime plan store contains an unknown entry.",
            ));
        }
        let path = entry.path();
        let record = read_record_at(&path).await?.ok_or_else(|| {
            store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan record disappeared during inventory.",
            )
        })?;
        record.key.validate()?;
        if let Some(installation) = expected_installation {
            installation.ensure_same(&record.key.scope)?;
        }
        validate_record(&record.key, &record.plan)?;
        if canonical_path_for(root, &record.key)? != path {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan record is not stored at its canonical key path.",
            ));
        }
        records.push((path, record));
    }
    records.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(records)
}

fn canonical_path_for(root: &Path, key: &RuntimeSurfacePlanKey) -> UseResult<PathBuf> {
    let digest = key.descriptor_digest()?;
    let hex = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| store_error(PLAN_STORE_ERROR, "A Runtime plan key digest is invalid."))?;
    Ok(root.join(format!("{hex}.json")))
}

async fn acquire_lock_for(state_root: &Path, root: &Path) -> UseResult<StdFile> {
    fs::create_dir_all(state_root)
        .await
        .map_err(|error| path_error("create Runtime plan state root", state_root, error))?;
    validate_directory(state_root).await?;
    ensure_owned_directory(state_root, root).await?;
    let path = root.join(PLAN_STORE_LOCK);
    let file = open_plan_lock(&path, true)?.ok_or_else(|| {
        store_error(
            PLAN_STORE_IO,
            "The Runtime plan store lock disappeared while it was opened.",
        )
    })?;
    lock_plan_file(file, &path, LockMode::Exclusive).await
}

/// Open and shared-lock an existing plan-store lock without creating any
/// filesystem entry.  A missing lock is valid for a restored owner root: the
/// first publisher will create the operational lock under global reference
/// admission, while a collector already holds the inverse boundary.
async fn acquire_existing_shared_lock_for(
    state_root: &Path,
    root: &Path,
) -> UseResult<Option<StdFile>> {
    validate_directory(state_root).await?;
    validate_directory(root).await?;
    let path = root.join(PLAN_STORE_LOCK);
    let Some(file) = open_plan_lock(&path, false)? else {
        return Ok(None);
    };
    lock_plan_file(file, &path, LockMode::Shared)
        .await
        .map(Some)
}

#[derive(Debug, Clone, Copy)]
enum LockMode {
    Shared,
    Exclusive,
}

async fn lock_plan_file(mut file: StdFile, path: &Path, mode: LockMode) -> UseResult<StdFile> {
    let deadline = tokio::time::Instant::now() + PLAN_STORE_LOCK_WAIT;
    loop {
        let attempt = tokio::task::spawn_blocking(move || {
            let result = match mode {
                LockMode::Shared => FileExt::try_lock_shared(&file),
                LockMode::Exclusive => FileExt::try_lock_exclusive(&file),
            };
            (file, result)
        })
        .await
        .map_err(|error| {
            store_error(
                PLAN_STORE_IO,
                format!("Failed to acquire the Runtime plan store lock: {error}"),
            )
        })?;
        let (returned, result) = attempt;
        match result {
            Ok(()) => return Ok(returned),
            Err(error) if plan_lock_is_contended(&error) => {
                file = returned;
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(store_error(
                        "use.plugin.runtime.plan_store_busy",
                        "Another process owns the Runtime plan store lock.",
                    ));
                }
                tokio::time::sleep(
                    PLAN_STORE_LOCK_RETRY_INTERVAL.min(deadline.saturating_duration_since(now)),
                )
                .await;
            }
            Err(error) => return Err(path_error("acquire Runtime plan store lock", path, error)),
        }
    }
}

fn open_plan_lock(path: &Path, create: bool) -> UseResult<Option<StdFile>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => validate_plan_lock_metadata(path, &metadata)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect Runtime plan store lock", path, error)),
    }
    let mut options = StdOpenOptions::new();
    options
        .create(create)
        .truncate(false)
        .read(true)
        .write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) => return Err(path_error("open Runtime plan store lock", path, error)),
    };
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| path_error("inspect Runtime plan store lock", path, error))?;
    validate_plan_lock_metadata(path, &metadata)?;
    Ok(Some(file))
}

fn validate_plan_lock_metadata(path: &Path, metadata: &std::fs::Metadata) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata) || !metadata.is_file() {
        return Err(store_error(
            PLAN_STORE_ERROR,
            format!(
                "The Runtime plan store lock '{}' is not an owned regular file.",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn plan_lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[async_trait]
impl RuntimeSurfacePlanSource for RuntimeSurfacePlanStore {
    async fn read_plan(&self, key: &RuntimeSurfacePlanKey) -> UseResult<Vec<u8>> {
        let Some(plan) = self.get(key).await? else {
            return Err(store_error(
                PLAN_NOT_FOUND,
                "The committed Runtime surface plan is not present in the host-owned store.",
            ));
        };
        plan.to_canonical_bytes()
    }
}

fn validate_record(key: &RuntimeSurfacePlanKey, plan: &RuntimeSurfacePlan) -> UseResult<()> {
    if !key.matches_plan(plan) {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan does not match its complete durable key.",
        ));
    }
    plan.validate().map_err(|error| {
        store_error(
            PLAN_STORE_ERROR,
            format!("The Runtime plan store record contains an invalid plan: {error}"),
        )
    })
}

fn encode_record(key: &RuntimeSurfacePlanKey, plan: &RuntimeSurfacePlan) -> UseResult<Vec<u8>> {
    let record = RuntimeSurfacePlanStoreRecord {
        schema: RUNTIME_SURFACE_PLAN_STORE_SCHEMA.to_owned(),
        key: key.clone(),
        plan: plan.clone(),
    };
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    record.serialize(&mut serializer).map_err(|error| {
        store_error(
            PLAN_STORE_ERROR,
            format!("Failed to encode the Runtime plan store record: {error}"),
        )
    })?;
    if bytes.is_empty() || bytes.len() > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan store record exceeds its size bound.",
        ));
    }
    Ok(bytes)
}

fn decode_record(bytes: &[u8]) -> UseResult<RuntimeSurfacePlanStoreRecord> {
    if bytes.is_empty() || bytes.len() > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan record exceeds its size bound.",
        ));
    }
    let record: RuntimeSurfacePlanStoreRecord = serde_json::from_slice(bytes).map_err(|error| {
        store_error(
            PLAN_STORE_ERROR,
            format!("A Runtime plan record is invalid JSON: {error}"),
        )
    })?;
    if record.schema != RUNTIME_SURFACE_PLAN_STORE_SCHEMA {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan store record schema is unsupported.",
        ));
    }
    validate_record(&record.key, &record.plan)?;
    if encode_record(&record.key, &record.plan)? != bytes {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan record is not canonical JSON.",
        ));
    }
    Ok(record)
}

async fn read_record_at(path: &Path) -> UseResult<Option<RuntimeSurfacePlanStoreRecord>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect Runtime plan record", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() as usize > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES
    {
        return Err(store_error(
            PLAN_STORE_ERROR,
            format!(
                "Runtime plan record '{}' is not a bounded regular file.",
                path.display()
            ),
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read Runtime plan record", path, error))?;
    if bytes.is_empty() || bytes.len() > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan record changed outside its size bound while reading.",
        ));
    }
    decode_record(&bytes).map(Some).map_err(|error| {
        store_error(
            PLAN_STORE_ERROR,
            format!(
                "Runtime plan record '{}' failed validation: {}",
                path.display(),
                error.message
            ),
        )
    })
}

fn compare_existing(existing: &RuntimeSurfacePlanStoreRecord, requested: &[u8]) -> UseResult<()> {
    let existing = encode_record(&existing.key, &existing.plan)?;
    if existing == requested {
        Ok(())
    } else {
        Err(store_error(
            PLAN_STORE_CONFLICT,
            "A Runtime plan key already contains different immutable content.",
        ))
    }
}

async fn write_new_record(root: &Path, path: &Path, bytes: &[u8]) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| {
        store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan record has no owned parent directory.",
        )
    })?;
    ensure_owned_directory(root, parent).await?;
    let temporary = parent.join(format!(".plan-{}.tmp", unique_suffix()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary Runtime plan record", &temporary, error))?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.sync_all().await?;
        Ok::<_, io::Error>(())
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error("write Runtime plan record", &temporary, error));
    }
    drop(file);
    let target = path.to_path_buf();
    let error_target = target.clone();
    let publish = tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(temporary, &target)
    })
    .await
    .map_err(|error| {
        store_error(
            PLAN_STORE_IO,
            format!(
                "Failed to publish Runtime plan record '{}': {error}",
                error_target.display()
            ),
        )
    })?;
    if let Err(error) = publish {
        if error.kind() == io::ErrorKind::AlreadyExists {
            return Err(store_error(
                PLAN_STORE_CONFLICT,
                "A Runtime plan key appeared during no-clobber publication.",
            ));
        }
        return Err(path_error(
            "publish Runtime plan record",
            &error_target,
            error,
        ));
    }
    sync_parent(parent).await
}

fn is_record_name(name: &str) -> bool {
    let Some(hex) = name.strip_suffix(".json") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_temporary_name(name: &str) -> bool {
    name.starts_with(".plan-") && name.ends_with(".tmp") && name.len() <= 256
}

async fn validate_temporary_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect temporary Runtime plan record", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES as u64
    {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A temporary Runtime plan record is not an owned bounded file.",
        ));
    }
    Ok(())
}

async fn ensure_owned_directory(root: &Path, target: &Path) -> UseResult<()> {
    if !target.starts_with(root) {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan path escapes its host-owned root.",
        ));
    }
    match fs::symlink_metadata(root).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(root)
                .await
                .map_err(|error| path_error("create Runtime plan store", root, error))?;
        }
        Err(error) => return Err(path_error("inspect Runtime plan store", root, error)),
    }
    validate_directory(root).await?;
    let relative = target
        .strip_prefix(root)
        .map_err(|_| store_error(PLAN_STORE_ERROR, "A Runtime plan path has no owned prefix."))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan path contains a non-portable component.",
            ));
        }
        current.push(component.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(path_error("create Runtime plan directory", &current, error)),
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_exact_record_directory(root: &Path) -> UseResult<()> {
    if !validate_existing_directory(root).await? {
        return Ok(());
    }
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| path_error("read exact Runtime plan candidate", root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read exact Runtime plan candidate entry", root, error))?
    {
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan candidate filename is not UTF-8.",
            )
        })?;
        if !is_record_name(name) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan candidate contains an operational or unknown entry.",
            ));
        }
        validate_regular_file(&entry.path()).await?;
    }
    Ok(())
}

async fn validate_existing_directory(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan store root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_error("inspect Runtime plan store", path, error)),
    }
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect Runtime plan directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(store_error(
            PLAN_STORE_ERROR,
            format!(
                "Runtime plan directory '{}' is not an owned directory.",
                path.display()
            ),
        ));
    }
    Ok(())
}

async fn validate_regular_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect Runtime plan store file", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan store entry is not an owned regular file.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_parent(parent: &Path) -> UseResult<()> {
    fs::File::open(parent)
        .await
        .map_err(|error| path_error("open Runtime plan directory for sync", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync Runtime plan directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: &Path) -> UseResult<()> {
    Ok(())
}

fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

fn store_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    store_error(
        PLAN_STORE_IO,
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<RuntimeSurfacePlanStore>();
};
