use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use tempfile::TempDir;
use tokio::fs;

use super::package::{io_error, MANIFEST_NAME, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};

const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_PATH_DEPTH: usize = 32;

#[derive(Clone, Copy)]
enum ArchiveKind {
    TarGz,
    Zip,
}

struct ExtractedEntry {
    relative: PathBuf,
    file: bool,
}

/// One validated local package source kept alive through installation.
#[derive(Debug)]
pub(crate) struct PreparedPackageSource {
    root: PathBuf,
    _temporary: Option<TempDir>,
}

impl PreparedPackageSource {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

pub(crate) async fn prepare_package_source(source: &Path) -> UseResult<PreparedPackageSource> {
    let source = fs::canonicalize(source)
        .await
        .map_err(|error| io_error("resolve extension package", source, error))?;
    let metadata = fs::metadata(&source)
        .await
        .map_err(|error| io_error("inspect extension package", &source, error))?;
    if metadata.is_dir() {
        return Ok(PreparedPackageSource {
            root: source,
            _temporary: None,
        });
    }
    if !metadata.is_file() {
        return Err(UseError::new(
            "use.extension.package_unsupported",
            "The local extension source must be a package directory, .tar.gz, .tgz, or .zip archive.",
        ));
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(UseError::new(
            "use.extension.package_too_large",
            format!(
                "The extension archive exceeds the {MAX_ARCHIVE_BYTES} byte compressed-size limit."
            ),
        ));
    }
    let kind = archive_kind(&source)?;
    let temporary = tokio::task::spawn_blocking(tempfile::tempdir)
        .await
        .map_err(|error| {
            UseError::new(
                "use.extension.io",
                format!("Failed to create extension archive staging task: {error}"),
            )
        })?
        .map_err(|error| io_error("create extension archive staging directory", &source, error))?;
    let extraction_root = temporary.path().join("package");
    let blocking_source = source.clone();
    let blocking_root = extraction_root.clone();
    let package_relative = tokio::task::spawn_blocking(move || {
        extract_archive(&blocking_source, &blocking_root, kind)
    })
    .await
    .map_err(|error| {
        UseError::new(
            "use.extension.package_archive_invalid",
            format!("Extension archive extraction task failed: {error}"),
        )
    })??;
    let extraction_root = fs::canonicalize(&extraction_root).await.map_err(|error| {
        io_error(
            "resolve extension archive staging directory",
            &extraction_root,
            error,
        )
    })?;
    let root = extraction_root.join(package_relative);
    let root = fs::canonicalize(&root)
        .await
        .map_err(|error| io_error("resolve extracted extension package", &root, error))?;
    if !root.starts_with(&extraction_root) {
        return Err(UseError::new(
            "use.extension.path_escape",
            "The extracted extension package root escapes its staging directory.",
        ));
    }
    Ok(PreparedPackageSource {
        root,
        _temporary: Some(temporary),
    })
}

fn archive_kind(path: &Path) -> UseResult<ArchiveKind> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Ok(ArchiveKind::TarGz)
    } else if name.ends_with(".zip") {
        Ok(ArchiveKind::Zip)
    } else {
        Err(UseError::new(
            "use.extension.package_unsupported",
            "Extension package archives must use .tar.gz, .tgz, or .zip.",
        ))
    }
}

fn extract_archive(source: &Path, target: &Path, kind: ArchiveKind) -> UseResult<PathBuf> {
    std::fs::create_dir_all(target)
        .map_err(|error| archive_io("create extraction directory", target, error))?;
    let entries = match kind {
        ArchiveKind::TarGz => extract_tar_gz(source, target)?,
        ArchiveKind::Zip => extract_zip(source, target)?,
    };
    resolve_package_root(&entries)
}

fn extract_tar_gz(source: &Path, target: &Path) -> UseResult<Vec<ExtractedEntry>> {
    let file = File::open(source).map_err(|error| archive_io("open", source, error))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut extracted = Vec::new();
    let mut seen = BTreeSet::new();
    let mut extracted_bytes = 0_u64;
    let entries = archive
        .entries()
        .map_err(|error| archive_invalid(format!("Failed to read tar entries: {error}")))?;
    for (entry_count, entry) in entries.enumerate() {
        if entry_count >= MAX_PACKAGE_FILES {
            return Err(package_limit_error());
        }
        let mut entry =
            entry.map_err(|error| archive_invalid(format!("Failed to read tar entry: {error}")))?;
        let entry_path = entry
            .path()
            .map_err(|error| archive_invalid(format!("Failed to read tar entry path: {error}")))?
            .into_owned();
        let entry_type = entry.header().entry_type();
        if ignored_macos_metadata_path(&entry_path)? {
            if entry_type.is_dir() {
                continue;
            }
            if entry_type.is_file() {
                let remaining = MAX_PACKAGE_BYTES.saturating_sub(extracted_bytes);
                extracted_bytes = extracted_bytes.saturating_add(copy_bounded(
                    &mut entry,
                    &mut io::sink(),
                    remaining,
                    &entry_path,
                )?);
                continue;
            }
        }
        let Some(relative) = sanitized_relative_path(&entry_path)? else {
            if entry_type.is_dir() {
                continue;
            }
            return Err(archive_invalid(
                "The archive contains a non-directory root entry.",
            ));
        };
        if !seen.insert(relative.clone()) {
            return Err(archive_invalid(format!(
                "The archive contains duplicate entry '{}'.",
                relative.display()
            )));
        }
        let output = target.join(&relative);
        if entry_type.is_dir() {
            std::fs::create_dir_all(&output)
                .map_err(|error| archive_io("create archive directory", &output, error))?;
            extracted.push(ExtractedEntry {
                relative,
                file: false,
            });
        } else if entry_type.is_file() {
            let remaining = MAX_PACKAGE_BYTES.saturating_sub(extracted_bytes);
            if entry.size() > remaining {
                return Err(package_limit_error());
            }
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| archive_io("create archive parent", parent, error))?;
            }
            let mut output_file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&output)
                .map_err(|error| archive_io("create archive file", &output, error))?;
            extracted_bytes = extracted_bytes.saturating_add(copy_bounded(
                &mut entry,
                &mut output_file,
                remaining,
                &output,
            )?);
            apply_unix_mode(&output, entry.header().mode().ok())?;
            extracted.push(ExtractedEntry {
                relative,
                file: true,
            });
        } else if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(UseError::new(
                "use.extension.package_symlink",
                format!(
                    "Extension archive entry '{}' is a link.",
                    relative.display()
                ),
            ));
        } else {
            return Err(UseError::new(
                "use.extension.package_entry_invalid",
                format!(
                    "Extension archive entry '{}' is not a regular file or directory.",
                    relative.display()
                ),
            ));
        }
    }
    Ok(extracted)
}

fn extract_zip(source: &Path, target: &Path) -> UseResult<Vec<ExtractedEntry>> {
    let file = File::open(source).map_err(|error| archive_io("open", source, error))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| archive_invalid(format!("Failed to read ZIP archive: {error}")))?;
    if archive.len() > MAX_PACKAGE_FILES {
        return Err(package_limit_error());
    }
    let mut extracted = Vec::new();
    let mut seen = BTreeSet::new();
    let mut extracted_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            archive_invalid(format!("Failed to read ZIP entry {index}: {error}"))
        })?;
        if entry.is_symlink() {
            return Err(UseError::new(
                "use.extension.package_symlink",
                format!("Extension archive entry '{}' is a link.", entry.name()),
            ));
        }
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            UseError::new(
                "use.extension.path_escape",
                format!(
                    "Extension archive entry '{}' escapes the package.",
                    entry.name()
                ),
            )
        })?;
        if ignored_macos_metadata_path(&enclosed)? {
            if entry.is_dir() {
                continue;
            }
            if entry.is_file() {
                let remaining = MAX_PACKAGE_BYTES.saturating_sub(extracted_bytes);
                extracted_bytes = extracted_bytes.saturating_add(copy_bounded(
                    &mut entry,
                    &mut io::sink(),
                    remaining,
                    &enclosed,
                )?);
                continue;
            }
        }
        let Some(relative) = sanitized_relative_path(&enclosed)? else {
            if entry.is_dir() {
                continue;
            }
            return Err(archive_invalid(
                "The ZIP archive contains a non-directory root entry.",
            ));
        };
        if !seen.insert(relative.clone()) {
            return Err(archive_invalid(format!(
                "The ZIP archive contains duplicate entry '{}'.",
                relative.display()
            )));
        }
        let output = target.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&output)
                .map_err(|error| archive_io("create ZIP directory", &output, error))?;
            extracted.push(ExtractedEntry {
                relative,
                file: false,
            });
        } else if entry.is_file() {
            let remaining = MAX_PACKAGE_BYTES.saturating_sub(extracted_bytes);
            if entry.size() > remaining {
                return Err(package_limit_error());
            }
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| archive_io("create ZIP parent", parent, error))?;
            }
            let mut output_file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&output)
                .map_err(|error| archive_io("create ZIP file", &output, error))?;
            extracted_bytes = extracted_bytes.saturating_add(copy_bounded(
                &mut entry,
                &mut output_file,
                remaining,
                &output,
            )?);
            apply_unix_mode(&output, entry.unix_mode())?;
            extracted.push(ExtractedEntry {
                relative,
                file: true,
            });
        } else {
            return Err(UseError::new(
                "use.extension.package_entry_invalid",
                format!("Extension ZIP entry '{}' is unsupported.", entry.name()),
            ));
        }
    }
    Ok(extracted)
}

fn ignored_macos_metadata_path(path: &Path) -> UseResult<bool> {
    if path.as_os_str().is_empty() {
        return Err(archive_invalid("The archive contains an empty entry path."));
    }
    let encoded = path.to_str().ok_or_else(|| {
        archive_invalid("Extension archive paths must be valid UTF-8 for portability.")
    })?;
    if encoded.len() > MAX_PATH_BYTES {
        return Err(archive_invalid(format!(
            "Extension archive path '{}' is not portable.",
            path.display()
        )));
    }

    let mut first = None;
    let mut last = None;
    let mut depth = 0_usize;
    for component in path.components() {
        match component {
            Component::Normal(segment) => {
                depth += 1;
                if depth > MAX_PATH_DEPTH {
                    return Err(archive_invalid(format!(
                        "Extension archive path '{}' exceeds the depth limit.",
                        path.display()
                    )));
                }
                let segment = segment.to_str().ok_or_else(|| {
                    archive_invalid("Extension archive paths must be valid UTF-8 for portability.")
                })?;
                first.get_or_insert(segment);
                last = Some(segment);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(UseError::new(
                    "use.extension.path_escape",
                    format!(
                        "Extension archive path '{}' escapes the package.",
                        path.display()
                    ),
                ));
            }
        }
    }

    Ok(first == Some("__MACOSX") || last.is_some_and(|segment| segment.starts_with("._")))
}

pub(crate) fn sanitized_relative_path(path: &Path) -> UseResult<Option<PathBuf>> {
    if path.as_os_str().is_empty() {
        return Err(archive_invalid("The archive contains an empty entry path."));
    }
    let encoded = path.to_str().ok_or_else(|| {
        archive_invalid("Extension archive paths must be valid UTF-8 for portability.")
    })?;
    if encoded.len() > MAX_PATH_BYTES {
        return Err(archive_invalid(format!(
            "Extension archive path '{}' is not portable.",
            path.display()
        )));
    }
    let mut sanitized = PathBuf::new();
    let mut depth = 0_usize;
    for component in path.components() {
        match component {
            Component::Normal(segment) => {
                depth += 1;
                if depth > MAX_PATH_DEPTH {
                    return Err(archive_invalid(format!(
                        "Extension archive path '{}' exceeds the depth limit.",
                        path.display()
                    )));
                }
                validate_portable_segment(segment, path)?;
                sanitized.push(segment);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(UseError::new(
                    "use.extension.path_escape",
                    format!(
                        "Extension archive path '{}' escapes the package.",
                        path.display()
                    ),
                ));
            }
        }
    }
    if sanitized.as_os_str().is_empty() {
        Ok(None)
    } else {
        Ok(Some(sanitized))
    }
}

fn validate_portable_segment(segment: &std::ffi::OsStr, path: &Path) -> UseResult<()> {
    let segment = segment.to_str().ok_or_else(|| {
        archive_invalid("Extension archive paths must be valid UTF-8 for portability.")
    })?;
    if segment.ends_with(['.', ' '])
        || segment
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character))
    {
        return Err(archive_invalid(format!(
            "Extension archive path '{}' is not portable.",
            path.display()
        )));
    }
    let device = segment
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || device
            .strip_prefix("COM")
            .or_else(|| device.strip_prefix("LPT"))
            .is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            });
    if reserved {
        return Err(archive_invalid(format!(
            "Extension archive path '{}' uses a reserved device name.",
            path.display()
        )));
    }
    Ok(())
}

fn copy_bounded(
    reader: &mut impl Read,
    writer: &mut impl Write,
    remaining: u64,
    path: &Path,
) -> UseResult<u64> {
    let mut bounded = reader.take(remaining.saturating_add(1));
    let copied = io::copy(&mut bounded, writer)
        .map_err(|error| archive_io("extract archive file", path, error))?;
    if copied > remaining {
        return Err(package_limit_error());
    }
    Ok(copied)
}

fn resolve_package_root(entries: &[ExtractedEntry]) -> UseResult<PathBuf> {
    let manifests = entries
        .iter()
        .filter(|entry| {
            entry.file
                && entry
                    .relative
                    .file_name()
                    .is_some_and(|name| name == MANIFEST_NAME)
        })
        .collect::<Vec<_>>();
    let [manifest] = manifests.as_slice() else {
        return Err(UseError::new(
            "use.extension.package_layout_invalid",
            format!("Extension archives must contain exactly one regular {MANIFEST_NAME} file."),
        ));
    };
    let root = manifest
        .relative
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if !root.as_os_str().is_empty()
        && entries
            .iter()
            .any(|entry| !entry.relative.starts_with(&root))
    {
        return Err(UseError::new(
            "use.extension.package_layout_invalid",
            "Extension archive entries must all belong to the directory containing its manifest.",
        ));
    }
    Ok(root)
}

#[cfg(unix)]
fn apply_unix_mode(path: &Path, mode: Option<u32>) -> UseResult<()> {
    use std::os::unix::fs::PermissionsExt;

    if let Some(mode) = mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))
            .map_err(|error| archive_io("set archive file permissions", path, error))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_unix_mode(_path: &Path, _mode: Option<u32>) -> UseResult<()> {
    Ok(())
}

fn archive_io(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.extension.package_archive_invalid",
        format!(
            "Failed to {action} extension archive entry '{}': {error}",
            path.display()
        ),
    )
}

fn archive_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.package_archive_invalid", message)
}

fn package_limit_error() -> UseError {
    UseError::new(
        "use.extension.package_too_large",
        "The extension package exceeds the local installation limits.",
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[tokio::test]
    async fn tar_package_accepts_an_explicit_current_directory_root() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("package.tar.gz");
        {
            let file = File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);

            let mut root = tar::Header::new_gnu();
            root.set_path(".").unwrap();
            root.set_entry_type(tar::EntryType::Directory);
            root.set_size(0);
            root.set_mode(0o755);
            root.set_cksum();
            builder.append(&root, io::empty()).unwrap();

            let manifest = b"extension fixture";
            let mut header = tar::Header::new_gnu();
            header.set_path(format!("./{MANIFEST_NAME}")).unwrap();
            header.set_size(manifest.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, &manifest[..]).unwrap();
            builder.finish().unwrap();
        }

        let prepared = prepare_package_source(&archive_path).await.unwrap();
        assert_eq!(
            std::fs::read(prepared.root().join(MANIFEST_NAME)).unwrap(),
            b"extension fixture"
        );
    }

    #[tokio::test]
    async fn tar_package_ignores_bounded_macos_appledouble_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("package.tar.gz");
        {
            let file = File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);

            let metadata = b"appledouble";
            let mut metadata_header = tar::Header::new_gnu();
            metadata_header.set_path("./._.").unwrap();
            metadata_header.set_size(metadata.len() as u64);
            metadata_header.set_mode(0o644);
            metadata_header.set_cksum();
            builder.append(&metadata_header, &metadata[..]).unwrap();

            let manifest = b"extension fixture";
            let mut manifest_header = tar::Header::new_gnu();
            manifest_header
                .set_path(format!("./{MANIFEST_NAME}"))
                .unwrap();
            manifest_header.set_size(manifest.len() as u64);
            manifest_header.set_mode(0o644);
            manifest_header.set_cksum();
            builder.append(&manifest_header, &manifest[..]).unwrap();
            builder.finish().unwrap();
        }

        let prepared = prepare_package_source(&archive_path).await.unwrap();
        assert_eq!(
            std::fs::read(prepared.root().join(MANIFEST_NAME)).unwrap(),
            b"extension fixture"
        );
        assert!(!prepared.root().join("._.").exists());
    }

    #[tokio::test]
    async fn zip_package_rejects_parent_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("escape.zip");
        {
            let file = File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file(
                    "../a3s-use-extension.acl",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(b"escape").unwrap();
            writer.finish().unwrap();
        }

        let error = prepare_package_source(&archive_path).await.unwrap_err();
        assert_eq!(error.code, "use.extension.path_escape");
    }

    #[tokio::test]
    async fn tar_package_rejects_symbolic_links() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("link.tar.gz");
        {
            let file = File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);

            let manifest = b"extension fixture";
            let mut manifest_header = tar::Header::new_gnu();
            manifest_header
                .set_path(format!("package/{MANIFEST_NAME}"))
                .unwrap();
            manifest_header.set_size(manifest.len() as u64);
            manifest_header.set_mode(0o644);
            manifest_header.set_cksum();
            builder.append(&manifest_header, &manifest[..]).unwrap();

            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_path("package/escape").unwrap();
            link.set_link_name("../../outside").unwrap();
            link.set_size(0);
            link.set_cksum();
            builder.append(&link, io::empty()).unwrap();
            builder.finish().unwrap();
        }

        let error = prepare_package_source(&archive_path).await.unwrap_err();
        assert_eq!(error.code, "use.extension.package_symlink");
    }

    #[test]
    fn archive_paths_reject_cross_platform_escapes_and_device_names() {
        for path in ["C:/escape", "..\\escape", "package/CON", "package/name. "] {
            assert!(
                sanitized_relative_path(Path::new(path)).is_err(),
                "accepted unsafe path {path}"
            );
        }
        assert_eq!(
            sanitized_relative_path(Path::new("./package/bin/tool")).unwrap(),
            Some(PathBuf::from("package/bin/tool"))
        );
    }
}
