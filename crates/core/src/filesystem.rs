use std::fs::Metadata;
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::path::{Component, Path, PathBuf};

/// Returns whether host filesystem metadata identifies a link-like object.
///
/// Windows directory junctions and other reparse points are not guaranteed to
/// report themselves as symbolic links through [`std::fs::FileType`]. Trusted
/// paths must reject the broader reparse-point class before opening, reading,
/// or traversing them.
pub fn metadata_is_link_or_reparse_point(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink() || metadata_has_reparse_point(metadata)
}

/// Converts an ordinary Windows path into the extended-length namespace used
/// by native APIs that do not opt in to modern long-path behavior.
///
/// The returned path is absolute because the extended namespace does not
/// resolve relative components. Device and already-verbatim paths retain their
/// namespace; UNC paths are translated to `\\?\UNC\...`.
#[cfg(windows)]
#[doc(hidden)]
pub fn windows_extended_length_path(path: &Path) -> io::Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let absolute = std::path::absolute(path)?;
    if absolute
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "extended-length Windows paths cannot contain relative components",
        ));
    }

    let mut path: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    if path.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows paths cannot contain NUL characters",
        ));
    }
    for unit in &mut path {
        if *unit == b'/' as u16 {
            *unit = b'\\' as u16;
        }
    }

    const VERBATIM: &[u16] = &[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    const DEVICE: &[u16] = &[b'\\' as u16, b'\\' as u16, b'.' as u16, b'\\' as u16];
    const UNC: &[u16] = &[
        b'\\' as u16,
        b'\\' as u16,
        b'?' as u16,
        b'\\' as u16,
        b'U' as u16,
        b'N' as u16,
        b'C' as u16,
        b'\\' as u16,
    ];

    let extended = if path.starts_with(VERBATIM) || path.starts_with(DEVICE) {
        path
    } else if path.starts_with(&[b'\\' as u16, b'\\' as u16]) {
        let mut extended = Vec::with_capacity(UNC.len() + path.len() - 2);
        extended.extend_from_slice(UNC);
        extended.extend_from_slice(&path[2..]);
        extended
    } else {
        let mut extended = Vec::with_capacity(VERBATIM.len() + path.len());
        extended.extend_from_slice(VERBATIM);
        extended.extend_from_slice(&path);
        extended
    };
    Ok(PathBuf::from(OsString::from_wide(&extended)))
}

#[cfg(windows)]
fn metadata_has_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    // FILE_ATTRIBUTE_REPARSE_POINT from the Windows file attribute contract.
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_has_reparse_point(_metadata: &Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::metadata_is_link_or_reparse_point;

    #[test]
    fn regular_files_and_directories_are_not_links_or_reparse_points() {
        let temporary = tempdir().expect("create temporary directory");
        let directory = temporary.path().join("directory");
        let file = temporary.path().join("file.txt");
        fs::create_dir(&directory).expect("create regular directory");
        fs::write(&file, b"regular file").expect("create regular file");

        let directory_metadata =
            fs::symlink_metadata(&directory).expect("inspect regular directory");
        let file_metadata = fs::symlink_metadata(&file).expect("inspect regular file");

        assert!(!metadata_is_link_or_reparse_point(&directory_metadata));
        assert!(!metadata_is_link_or_reparse_point(&file_metadata));
    }

    #[cfg(unix)]
    #[test]
    fn unix_symbolic_links_are_detected() {
        use std::os::unix::fs::symlink;

        let temporary = tempdir().expect("create temporary directory");
        let target = temporary.path().join("target");
        let link = temporary.path().join("link");
        fs::create_dir(&target).expect("create symlink target");
        symlink(&target, &link).expect("create symbolic link");

        let metadata = fs::symlink_metadata(&link).expect("inspect symbolic link");
        assert!(metadata_is_link_or_reparse_point(&metadata));
    }

    #[cfg(windows)]
    #[test]
    fn windows_directory_junctions_are_detected() {
        use std::process::Command;

        let temporary = tempdir().expect("create temporary directory");
        let target = temporary.path().join("target");
        let junction = temporary.path().join("junction");
        fs::create_dir(&target).expect("create junction target");

        let output = Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(&target)
            .output()
            .expect("invoke mklink for directory junction");
        assert!(
            output.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let metadata = fs::symlink_metadata(&junction).expect("inspect directory junction");
        assert!(metadata_is_link_or_reparse_point(&metadata));
    }
}
