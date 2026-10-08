use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::error::ToolAcquireError;

pub(crate) struct InstallLock {
    file: File,
}

impl InstallLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self, ToolAcquireError> {
        let path = root.join(".install.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| ToolAcquireError::io("open tool install lock", &path, error))?;
        file.lock_exclusive()
            .map_err(|error| ToolAcquireError::io("lock tool install", &path, error))?;
        Ok(Self { file })
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        // Call the fs2 trait method. Inherent File::unlock requires Rust 1.89.
        let _ = FileExt::unlock(&self.file);
    }
}

pub(crate) fn join_under(root: &Path, parts: &[&str]) -> Result<PathBuf, ToolAcquireError> {
    let mut path = root.to_path_buf();
    for part in parts {
        if !is_single_component(part) {
            return Err(ToolAcquireError::InvalidName((*part).to_string()));
        }
        path.push(part);
    }
    Ok(path)
}

fn is_single_component(part: &str) -> bool {
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.contains('/')
        && !part.contains('\\')
        && !part.contains('\0')
}
