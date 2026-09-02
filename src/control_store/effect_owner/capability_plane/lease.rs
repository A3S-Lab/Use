use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::{PluginPackageId, UseError, UseResult};
use fs2::FileExt;
use tokio::fs;

use crate::control_store::model::ControlPublishedCapabilityPackage;

const GENERATION_LEASE_DIRECTORY: &str = "generation-leases";

#[derive(Debug, Clone)]
pub(in crate::control_store) struct ControlGenerationLeaseStore {
    state_root: PathBuf,
    lease_root: PathBuf,
}

impl ControlGenerationLeaseStore {
    pub(in crate::control_store) fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            lease_root: state_root.join(GENERATION_LEASE_DIRECTORY),
            state_root,
        }
    }

    pub(in crate::control_store) async fn try_acquire_shared(
        &self,
        packages: &[ControlPublishedCapabilityPackage],
    ) -> UseResult<Option<Vec<ControlGenerationFileLease>>> {
        let mut leases = Vec::with_capacity(packages.len());
        for package in packages {
            let Some(lease) = self
                .try_acquire(
                    &package.package_id,
                    package.lifecycle_generation,
                    LeaseMode::Shared,
                )
                .await?
            else {
                return Ok(None);
            };
            leases.push(lease);
        }
        Ok(Some(leases))
    }

    pub(in crate::control_store) async fn try_acquire_exclusive(
        &self,
        package_id: &str,
        lifecycle_generation: u64,
    ) -> UseResult<Option<ControlGenerationFileLease>> {
        self.try_acquire(package_id, lifecycle_generation, LeaseMode::Exclusive)
            .await
    }

    async fn try_acquire(
        &self,
        package_id: &str,
        lifecycle_generation: u64,
        mode: LeaseMode,
    ) -> UseResult<Option<ControlGenerationFileLease>> {
        let path = self.lock_path(package_id, lifecycle_generation)?;
        let parent = path.parent().ok_or_else(lease_path_invalid)?;
        ensure_owned_directory_chain(&self.state_root, parent).await?;
        match fs::symlink_metadata(&path).await {
            Ok(metadata)
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    || !metadata.is_file() =>
            {
                return Err(lease_path_invalid())
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(lease_io("inspect Control generation lease", &path, error)),
        }
        let error_path = path.clone();
        let acquired = tokio::task::spawn_blocking(move || -> io::Result<std::fs::File> {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(false).read(true).write(true);
            configure_no_follow(&mut options);
            let file = options.open(path)?;
            match mode {
                LeaseMode::Shared => FileExt::try_lock_shared(&file)?,
                LeaseMode::Exclusive => FileExt::try_lock_exclusive(&file)?,
            }
            Ok(file)
        })
        .await
        .map_err(|error| {
            UseError::new(
                "use.control.invocation_lease_io",
                format!("Control generation lease task failed: {error}"),
            )
        })?;
        match acquired {
            Ok(file) => {
                validate_regular_file(&error_path).await?;
                Ok(Some(ControlGenerationFileLease(file)))
            }
            Err(error) if lock_is_contended(&error) => Ok(None),
            Err(error) => Err(lease_io(
                "acquire Control generation lease",
                &error_path,
                error,
            )),
        }
    }

    fn lock_path(&self, package_id: &str, lifecycle_generation: u64) -> UseResult<PathBuf> {
        let package = PluginPackageId::parse(package_id.to_owned())?;
        if lifecycle_generation == 0 {
            return Err(lease_path_invalid());
        }
        let (publisher, name) = package_id.split_once('/').ok_or_else(lease_path_invalid)?;
        if package.as_str() != package_id {
            return Err(lease_path_invalid());
        }
        Ok(self
            .lease_root
            .join(publisher)
            .join(name)
            .join(format!("{lifecycle_generation:020}.lock")))
    }
}

#[derive(Debug, Clone, Copy)]
enum LeaseMode {
    Shared,
    Exclusive,
}

pub(in crate::control_store) struct ControlGenerationFileLease(std::fs::File);

impl Drop for ControlGenerationFileLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

async fn ensure_owned_directory_chain(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(lease_path_invalid());
    }
    validate_owned_directory(root).await?;
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| lease_path_invalid())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(lease_path_invalid());
        };
        current.push(segment);
        match fs::symlink_metadata(&current).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => return Err(lease_path_invalid()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(&current).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(lease_io(
                            "create Control generation lease directory",
                            &current,
                            error,
                        ))
                    }
                }
                validate_owned_directory(&current).await?;
            }
            Err(error) => {
                return Err(lease_io(
                    "inspect Control generation lease directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(())
}

async fn validate_owned_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| lease_io("inspect Control generation lease directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(lease_path_invalid());
    }
    Ok(())
}

async fn validate_regular_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| lease_io("inspect Control generation lease file", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(lease_path_invalid());
    }
    Ok(())
}

fn lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    false
}

fn configure_no_follow(options: &mut std::fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
}

fn lease_path_invalid() -> UseError {
    UseError::new(
        "use.control.invocation_lease_path_invalid",
        "A Control generation lease path is outside its owned link-free layout.",
    )
}

fn lease_io(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.control.invocation_lease_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
