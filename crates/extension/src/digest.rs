use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use sha2::{Digest, Sha256};

use super::package::{io_error, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};
use super::source::sanitized_relative_path;

struct PackageFile {
    normalized: String,
    path: PathBuf,
    size: u64,
}

pub(crate) async fn package_sha256(root: &Path) -> UseResult<String> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || hash_package(&root))
        .await
        .map_err(|error| {
            UseError::new(
                "use.extension.io",
                format!("Failed to hash extension package: blocking task failed: {error}"),
            )
        })?
}

fn hash_package(root: &Path) -> UseResult<String> {
    let mut files = Vec::new();
    let mut entries = 0_usize;
    let mut bytes = 0_u64;
    collect_files(root, root, &mut files, &mut entries, &mut bytes)?;
    files.sort_by(|left, right| left.normalized.cmp(&right.normalized));

    let mut digest = Sha256::new();
    digest.update(b"a3s-use-expanded-package-v1\0");
    for package_file in files {
        let path_bytes = package_file.normalized.as_bytes();
        digest.update((path_bytes.len() as u64).to_be_bytes());
        digest.update(path_bytes);
        digest.update(package_file.size.to_be_bytes());

        let file = File::open(&package_file.path)
            .map_err(|error| io_error("open extension package file", &package_file.path, error))?;
        let mut reader = BufReader::new(file);
        let mut buffer = [0_u8; 64 * 1024];
        let mut read_bytes = 0_u64;
        loop {
            let count = reader.read(&mut buffer).map_err(|error| {
                io_error("hash extension package file", &package_file.path, error)
            })?;
            if count == 0 {
                break;
            }
            read_bytes = read_bytes.saturating_add(count as u64);
            if read_bytes > package_file.size {
                return Err(package_changed(&package_file.path));
            }
            digest.update(&buffer[..count]);
        }
        if read_bytes != package_file.size {
            return Err(package_changed(&package_file.path));
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PackageFile>,
    entries: &mut usize,
    bytes: &mut u64,
) -> UseResult<()> {
    let children = std::fs::read_dir(directory)
        .map_err(|error| io_error("read extension package directory", directory, error))?;
    for child in children {
        let child =
            child.map_err(|error| io_error("read extension package entry", directory, error))?;
        *entries = entries.saturating_add(1);
        if *entries > MAX_PACKAGE_FILES {
            return Err(package_limit_error());
        }
        let path = child.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| io_error("inspect extension package entry", &path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(UseError::new(
                "use.extension.package_symlink",
                format!(
                    "Extension package entry '{}' is a symbolic link.",
                    path.display()
                ),
            ));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files, entries, bytes)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(UseError::new(
                "use.extension.package_entry_invalid",
                format!(
                    "Extension package entry '{}' is not a regular file or directory.",
                    path.display()
                ),
            ));
        }
        *bytes = bytes.saturating_add(metadata.len());
        if *bytes > MAX_PACKAGE_BYTES {
            return Err(package_limit_error());
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            UseError::new(
                "use.extension.path_escape",
                format!(
                    "Extension package entry '{}' escapes its root.",
                    path.display()
                ),
            )
        })?;
        let relative = sanitized_relative_path(relative)?.ok_or_else(|| {
            UseError::new(
                "use.extension.package_entry_invalid",
                "Extension package contains an empty file path.",
            )
        })?;
        let normalized = relative
            .iter()
            .map(|segment| {
                segment.to_str().ok_or_else(|| {
                    UseError::new(
                        "use.extension.package_entry_invalid",
                        format!(
                            "Extension package path '{}' is not valid UTF-8.",
                            relative.display()
                        ),
                    )
                })
            })
            .collect::<UseResult<Vec<_>>>()?
            .join("/");
        files.push(PackageFile {
            normalized,
            path,
            size: metadata.len(),
        });
    }
    Ok(())
}

fn package_changed(path: &Path) -> UseError {
    UseError::new(
        "use.extension.package_changed",
        format!(
            "Extension package file '{}' changed while it was hashed.",
            path.display()
        ),
    )
}

fn package_limit_error() -> UseError {
    UseError::new(
        "use.extension.package_too_large",
        "The extension package exceeds the local installation limits.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn package_digest_is_order_independent_and_content_sensitive() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir_all(first.join("bin")).unwrap();
        std::fs::create_dir_all(second.join("bin")).unwrap();
        std::fs::write(first.join("z.txt"), b"z").unwrap();
        std::fs::write(first.join("bin/tool"), b"tool").unwrap();
        std::fs::write(second.join("bin/tool"), b"tool").unwrap();
        std::fs::write(second.join("z.txt"), b"z").unwrap();

        let first_digest = package_sha256(&first).await.unwrap();
        let second_digest = package_sha256(&second).await.unwrap();
        assert_eq!(first_digest, second_digest);
        assert_eq!(first_digest.len(), 64);

        std::fs::write(second.join("bin/tool"), b"changed").unwrap();
        assert_ne!(first_digest, package_sha256(&second).await.unwrap());
    }
}
