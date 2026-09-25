// Plugin Host protocol store filesystem helpers (included into host_store).

async fn acquire_lock(lock_path: PathBuf) -> UseResult<StdFile> {
    match fs::symlink_metadata(&lock_path).await {
        Ok(metadata)
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                || !metadata.is_file() =>
        {
            return Err(store_invalid("A Plugin Host lock path is invalid."))
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect Plugin Host lock", &lock_path, error)),
    }
    let error_path = lock_path.clone();
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        Ok::<_, io::Error>(file)
    })
    .await
    .map_err(|error| {
        store_io(format!(
            "Failed to join the Plugin Host lock task '{}': {error}",
            error_path.display()
        ))
    })?
    .map_err(|error| path_error("acquire Plugin Host lock", &error_path, error))
}

async fn read_optional<T: DeserializeOwned>(
    state_root: &Path,
    path: &Path,
) -> UseResult<Option<T>> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(store_invalid(
            "A Plugin Host record path escapes its state root.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| store_invalid("A Plugin Host record path is incomplete."))?;
    if !validate_existing_directory_chain(state_root, parent).await? {
        return Ok(None);
    }
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect Plugin Host record", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_HOST_RECORD_BYTES
    {
        return Err(store_invalid(
            "A Plugin Host record is not a bounded regular file.",
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read Plugin Host record", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_HOST_RECORD_BYTES {
        return Err(store_invalid(
            "A Plugin Host record changed outside its size bound.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| store_invalid(format!("A Plugin Host record is invalid JSON: {error}")))
}

async fn write_new<T: Serialize>(state_root: &Path, path: &Path, value: &T) -> UseResult<()> {
    write_record(state_root, path, value, false).await
}

async fn write_replace<T: Serialize>(state_root: &Path, path: &Path, value: &T) -> UseResult<()> {
    write_record(state_root, path, value, true).await
}

async fn write_record<T: Serialize>(
    state_root: &Path,
    path: &Path,
    value: &T,
    replace: bool,
) -> UseResult<()> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(store_invalid(
            "A Plugin Host record path escapes its state root.",
        ));
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        store_invalid(format!("Failed to encode a Plugin Host record: {error}"))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_HOST_RECORD_BYTES {
        return Err(store_invalid(
            "A Plugin Host record exceeds its storage bound.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| store_invalid("A Plugin Host record path is incomplete."))?;
    ensure_owned_directory(state_root, parent).await?;
    let temporary = parent.join(format!(".plugin-host-{}.tmp", unique_suffix()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary Plugin Host record", &temporary, error))?;
    if let Err(error) = async {
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "commit temporary Plugin Host record",
            path,
            error,
        ));
    }
    drop(file);
    let error_target = path.to_path_buf();
    let activation_target = error_target.clone();
    let activation = tokio::task::spawn_blocking(move || {
        if replace {
            a3s_use_extension::persist_temporary_replace_blocking(temporary, &activation_target)
        } else {
            a3s_use_extension::persist_temporary_noclobber_blocking(temporary, &activation_target)
        }
    })
    .await
    .map_err(|error| {
        store_io(format!(
            "Failed to join Plugin Host activation for '{}': {error}",
            error_target.display()
        ))
    })?;
    activation.map_err(|error| path_error("activate Plugin Host record", &error_target, error))?;
    sync_parent(parent).await
}

async fn ensure_owned_directory(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(store_invalid(
            "A Plugin Host directory escapes its state root.",
        ));
    }
    fs::create_dir_all(root)
        .await
        .map_err(|error| path_error("create Plugin Host state root", root, error))?;
    validate_directory(root).await?;
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| store_invalid("A Plugin Host directory has invalid ownership."))?;
    let mut current = root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create Plugin Host state directory",
                    &current,
                    error,
                ))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory_chain(root: &Path, directory: &Path) -> UseResult<bool> {
    if !directory.starts_with(root) {
        return Err(store_invalid(
            "A Plugin Host directory escapes its state root.",
        ));
    }
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| store_invalid("A Plugin Host directory has invalid ownership."))?;
    let mut current = root.to_path_buf();
    match fs::symlink_metadata(&current).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() => {
        }
        Ok(_) => return Err(store_invalid("The Plugin Host state root is invalid.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(path_error(
                "inspect Plugin Host state root",
                &current,
                error,
            ))
        }
    }
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::symlink_metadata(&current).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => return Err(store_invalid("A Plugin Host state directory is invalid.")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    "inspect Plugin Host state directory",
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
        .map_err(|error| path_error("inspect Plugin Host directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(store_invalid("A Plugin Host directory is invalid."));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_parent(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| path_error("open Plugin Host directory", path, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync Plugin Host directory", path, error))
}

#[cfg(not(unix))]
async fn sync_parent(_path: &Path) -> UseResult<()> {
    Ok(())
}

pub(super) fn digest_value<T: Serialize>(value: &T) -> UseResult<String> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value
        .serialize(&mut serializer)
        .map_err(|error| store_invalid(format!("Failed to canonicalize Host state: {error}")))?;
    Ok(format!("sha256:{}", sha256_hex(&bytes)))
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn operation_binding_digest(operation_id: &str, plan_digest: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.use.plugin-host-operation-binding.v1\0");
    hasher.update(operation_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(plan_digest.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

fn store_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.host_store_invalid", message)
}

fn store_conflict(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.host_store_conflict", message)
}

fn store_io(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.host_store_io", message)
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    store_io(format!("Failed to {action} '{}': {error}", path.display()))
}
