//! Deterministic package packing and fingerprinting.

use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::ExtensionManifest;

use crate::tools_error;

/// Read and parse one package's `a3s-use-extension.acl` manifest.
pub(crate) fn read_manifest(package_directory: &Path) -> UseResult<ExtensionManifest> {
    let manifest_path = package_directory.join("a3s-use-extension.acl");
    let input = std::fs::read_to_string(&manifest_path).map_err(|error| {
        tools_error(
            "registry_tools.package_read_failed",
            &format!(
                "Failed to read package manifest '{}': {error}",
                manifest_path.display()
            ),
        )
    })?;
    ExtensionManifest::parse_acl(&input)
}

/// Build the deterministic `package/`-prefixed tar.gz for one package root.
///
/// Byte-for-byte reproducible for identical content: fixed zero timestamps,
/// zero uid/gid, sorted paths, best compression, and mode derived only from
/// the source executable bit.
pub(crate) fn deterministic_package_archive(root: &Path) -> UseResult<Vec<u8>> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let encoder = flate2::GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), flate2::Compression::best());
    let mut archive = tar::Builder::new(encoder);
    for (relative, path) in files {
        let body = std::fs::read(&path).map_err(|error| read_error(&path, &error.to_string()))?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| read_error(&path, &error.to_string()))?;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        let mode = if executable { 0o755 } else { 0o644 };
        let archive_path = format!("package/{relative}");
        let mut header = tar::Header::new_gnu();
        header.set_path(&archive_path).map_err(|error| {
            tools_error(
                "registry_tools.package_invalid",
                &format!("Package path '{archive_path}' cannot be encoded: {error}"),
            )
        })?;
        header.set_size(body.len() as u64);
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        archive.append(&header, body.as_slice()).map_err(|error| {
            tools_error(
                "registry_tools.pack_failed",
                &format!("Failed to append '{archive_path}' to the archive: {error}"),
            )
        })?;
    }
    archive
        .into_inner()
        .map_err(|error| {
            tools_error(
                "registry_tools.pack_failed",
                &format!("Failed to finish the package archive: {error}"),
            )
        })?
        .finish()
        .map_err(|error| {
            tools_error(
                "registry_tools.pack_failed",
                &format!("Failed to compress the package archive: {error}"),
            )
        })
}

fn read_error(path: &Path, error: &str) -> UseError {
    tools_error(
        "registry_tools.package_read_failed",
        &format!("Failed to read '{}': {error}", path.display()),
    )
}

fn collect_files<'a>(
    root: &'a Path,
    directory: &'a Path,
    output: &mut Vec<(String, std::path::PathBuf)>,
) -> UseResult<()> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        tools_error(
            "registry_tools.package_read_failed",
            &format!(
                "Failed to read package directory '{}': {error}",
                directory.display()
            ),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| read_error(directory, &error.to_string()))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| read_error(&path, &error.to_string()))?;
        if metadata.is_dir() {
            collect_files(root, &path, output)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(tools_error(
                "registry_tools.package_invalid",
                &format!(
                    "Package entry '{}' is not a regular file or directory.",
                    path.display()
                ),
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| {
                tools_error(
                    "registry_tools.package_invalid",
                    &format!("Package entry '{}' escapes its root.", path.display()),
                )
            })?
            .iter()
            .map(|segment| segment.to_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("/");
        if relative.is_empty() {
            return Err(tools_error(
                "registry_tools.package_invalid",
                "The package root itself cannot be a file.",
            ));
        }
        output.push((relative, path));
    }
    Ok(())
}

/// CLI entry: pack one package directory and report its digest evidence.
pub(crate) fn pack_command(options: &crate::Options) -> UseResult<()> {
    let package_directory = Path::new(&options.require("package-dir")?)
        .canonicalize()
        .map_err(|error| {
            tools_error(
                "registry_tools.package_read_failed",
                &format!("Failed to resolve the package directory: {error}"),
            )
        })?;
    let output = options.require("out")?;
    read_manifest(&package_directory)?;
    let archive = deterministic_package_archive(&package_directory)?;
    std::fs::write(&output, &archive).map_err(|error| {
        tools_error(
            "registry_tools.pack_failed",
            &format!("Failed to write archive '{}': {error}", output),
        )
    })?;
    println!(
        "{}",
        serde_json::json!({
            "archive": output,
            "length": archive.len(),
            "sha256": format!("sha256:{}", a3s_use_extension::sha256_hex(&archive)),
        })
    );
    Ok(())
}
