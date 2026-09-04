use std::io::ErrorKind;
use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::package::{activate_temporary_file, io_error, sync_parent_directory, unique_suffix};
use super::registry::ExtensionRegistrySnapshot;
use super::ExtensionPaths;

pub(crate) const MAX_EXTENSION_REGISTRY_SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;

pub(super) async fn read_registry_snapshot(
    paths: &ExtensionPaths,
) -> UseResult<ExtensionRegistrySnapshot> {
    let path = paths.registry_snapshot_path();
    let parent = path.parent().ok_or_else(|| {
        UseError::new(
            "use.extension.registry_invalid",
            "The extension registry snapshot has no parent directory.",
        )
    })?;
    if !validate_existing_owned_directory_chain(paths, parent).await? {
        return ExtensionRegistrySnapshot::empty(paths.installation().clone());
    }

    let metadata = match fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return ExtensionRegistrySnapshot::empty(paths.installation().clone())
        }
        Err(error) => return Err(io_error("read extension registry snapshot", &path, error)),
    };
    validate_registry_file_metadata(&path, &metadata)?;

    let mut file = open_registry_file(&path)
        .await
        .map_err(|error| io_error("open extension registry snapshot", &path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| io_error("inspect opened extension registry snapshot", &path, error))?;
    validate_registry_file_metadata(&path, &opened)?;
    if !same_file_identity(&metadata, &opened) {
        return Err(registry_changed(
            "The extension registry snapshot changed before it was read.",
        ));
    }
    let capacity = usize::try_from(opened.len()).map_err(|_| {
        registry_invalid("The extension registry snapshot size cannot fit in memory.")
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(MAX_EXTENSION_REGISTRY_SNAPSHOT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| io_error("read extension registry snapshot", &path, error))?;
    if bytes.len() as u64 != opened.len() {
        return Err(registry_changed(
            "The extension registry snapshot changed while it was read.",
        ));
    }
    let observed = fs::symlink_metadata(&path)
        .await
        .map_err(|error| io_error("reinspect extension registry snapshot", &path, error))?;
    validate_registry_file_metadata(&path, &observed)?;
    if observed.len() != opened.len() || !same_file_identity(&opened, &observed) {
        return Err(registry_changed(
            "The extension registry snapshot changed while it was read.",
        ));
    }

    let snapshot: ExtensionRegistrySnapshot = serde_json::from_slice(&bytes).map_err(|error| {
        UseError::new(
            "use.extension.registry_invalid",
            format!(
                "Invalid extension registry snapshot '{}': {error}",
                path.display()
            ),
        )
    })?;
    snapshot.validate()?;
    if snapshot.installation != *paths.installation() {
        return Err(UseError::new(
            "use.extension.registry_scope_mismatch",
            "The extension Registry snapshot belongs to a different installation.",
        ));
    }
    Ok(snapshot)
}

pub(super) async fn write_registry_snapshot(
    paths: &ExtensionPaths,
    snapshot: &ExtensionRegistrySnapshot,
) -> UseResult<()> {
    snapshot.validate()?;
    if snapshot.installation != *paths.installation() {
        return Err(UseError::new(
            "use.extension.registry_scope_mismatch",
            "The extension Registry snapshot belongs to a different installation.",
        ));
    }
    let path = paths.registry_snapshot_path();
    let parent = path.parent().ok_or_else(|| {
        UseError::new(
            "use.extension.registry_invalid",
            "The extension registry snapshot has no parent directory.",
        )
    })?;
    ensure_owned_directory_chain(paths, parent).await?;
    let temporary = parent.join(format!(".registry-{}.tmp", unique_suffix()));
    let bytes = serde_json::to_vec_pretty(snapshot).map_err(|error| {
        UseError::new(
            "use.extension.registry_invalid",
            format!("Failed to encode extension registry snapshot: {error}"),
        )
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_EXTENSION_REGISTRY_SNAPSHOT_BYTES {
        return Err(registry_invalid(format!(
            "The encoded extension registry snapshot must contain between 1 byte and {MAX_EXTENSION_REGISTRY_SNAPSHOT_BYTES} bytes."
        )));
    }
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    configure_registry_file_options(&mut options);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| io_error("create temporary extension registry", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "write extension registry snapshot",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.flush().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "flush extension registry snapshot",
            &temporary,
            error,
        ));
    }
    let written = match file.metadata().await {
        Ok(metadata) => metadata,
        Err(error) => {
            let _ = fs::remove_file(&temporary).await;
            return Err(io_error(
                "inspect temporary extension registry",
                &temporary,
                error,
            ));
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&written)
        || !written.is_file()
        || written.len() != bytes.len() as u64
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(registry_changed(
            "The temporary extension registry snapshot changed while it was written.",
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(
            "sync extension registry snapshot",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(
        temporary.clone(),
        path.to_path_buf(),
        "activate extension registry snapshot",
    )
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(parent, "extension registry").await
}

async fn open_registry_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    configure_registry_file_options(&mut options);
    options.open(path).await
}

fn configure_registry_file_options(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
}

async fn validate_existing_owned_directory_chain(
    paths: &ExtensionPaths,
    directory: &Path,
) -> UseResult<bool> {
    let root = paths.use_paths().state_root();
    if !directory.starts_with(root) {
        return Err(registry_invalid(
            "The extension registry snapshot escapes the configured Use state root.",
        ));
    }
    let relative = directory.strip_prefix(root).map_err(|_| {
        registry_invalid("The extension registry snapshot escapes the configured Use state root.")
    })?;
    let mut current = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            current.push(component.as_os_str());
        }
        let metadata = match fs::symlink_metadata(&current).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(io_error(
                    "inspect extension registry directory",
                    &current,
                    error,
                ))
            }
        };
        validate_owned_directory(&current, &metadata)?;
    }
    Ok(true)
}

async fn ensure_owned_directory_chain(paths: &ExtensionPaths, directory: &Path) -> UseResult<()> {
    let root = paths.use_paths().state_root();
    if !directory.starts_with(root) {
        return Err(registry_invalid(
            "The extension registry snapshot escapes the configured Use state root.",
        ));
    }

    let mut missing = Vec::new();
    let mut candidate = directory.to_path_buf();
    loop {
        match fs::symlink_metadata(&candidate).await {
            Ok(metadata) => {
                validate_owned_directory(&candidate, &metadata)?;
                break;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                missing.push(candidate.clone());
                if candidate == root {
                    break;
                }
                candidate = candidate.parent().map(Path::to_path_buf).ok_or_else(|| {
                    registry_invalid(
                        "No existing parent exists for the extension registry directory.",
                    )
                })?;
                if !candidate.starts_with(root) {
                    return Err(registry_invalid(
                        "The extension registry directory has no existing parent inside the configured Use state root.",
                    ));
                }
            }
            Err(error) => {
                return Err(io_error(
                    "inspect extension registry directory",
                    &candidate,
                    error,
                ));
            }
        }
    }

    while let Some(path) = missing.pop() {
        match fs::create_dir(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(io_error(
                    "create extension registry directory",
                    &path,
                    error,
                ));
            }
        }
        let metadata = fs::symlink_metadata(&path)
            .await
            .map_err(|error| io_error("inspect extension registry directory", &path, error))?;
        validate_owned_directory(&path, &metadata)?;
    }
    Ok(())
}

fn validate_owned_directory(path: &Path, metadata: &std::fs::Metadata) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata) || !metadata.is_dir() {
        return Err(registry_invalid(format!(
            "Extension registry directory '{}' must be an owned regular directory.",
            path.display()
        )));
    }
    Ok(())
}

fn validate_registry_file_metadata(path: &Path, metadata: &std::fs::Metadata) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_EXTENSION_REGISTRY_SNAPSHOT_BYTES
    {
        return Err(registry_invalid(format!(
            "Extension registry snapshot '{}' must be a bounded owned regular file.",
            path.display()
        )));
    }
    Ok(())
}

fn same_file_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(windows)]
    {
        // The stable Windows Metadata API does not expose a portable file
        // identity. Length plus last-write time is the strongest path-level
        // identity available here; the no-reparse handle open above still
        // prevents the path from redirecting through a link.
        left.len() == right.len() && left.modified().ok() == right.modified().ok()
    }
    #[cfg(not(any(unix, windows)))]
    {
        left.len() == right.len() && left.modified().ok() == right.modified().ok()
    }
}

fn registry_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_invalid", message)
}

fn registry_changed(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_changed", message)
}
