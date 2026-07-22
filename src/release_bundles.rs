use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{inspect_release_bundle, ReleaseBundlePackage};
use tokio::fs;

const RELEASE_BUNDLES_DIRECTORY: &str = "extensions";
const RELEASE_BUNDLES_ENV: &str = "A3S_USE_RELEASE_BUNDLES_DIR";
const MANIFEST_NAME: &str = "a3s-use-extension.acl";
const MAX_BUNDLE_DIRECTORIES: usize = 512;

pub(crate) async fn list() -> UseResult<Vec<ReleaseBundlePackage>> {
    list_at(&release_bundles_root()?).await
}

pub(crate) async fn resolve(package_id: &str) -> UseResult<(PathBuf, ReleaseBundlePackage)> {
    let segments = package_segments(package_id)?;
    let root = release_bundles_root()?;
    validate_root(&root, false).await?;
    let package_root = root.join(segments[0]).join(segments[1]);
    let package = inspect_release_bundle(&package_root)
        .await
        .map_err(|error| {
            UseError::new(
                "use.extension.release_bundle_unavailable",
                format!(
                    "Release bundle '{}' is not available from this A3S Use installation: {}",
                    package_id, error.message
                ),
            )
        })?;
    if package.package_id != package_id {
        return Err(UseError::new(
            "use.extension.release_bundle_invalid",
            format!(
                "Release bundle directory for '{}' declares '{}'.",
                package_id, package.package_id
            ),
        ));
    }
    Ok((package_root, package))
}

async fn list_at(root: &Path) -> UseResult<Vec<ReleaseBundlePackage>> {
    if !validate_root(root, true).await? {
        return Ok(Vec::new());
    }
    let mut publishers = fs::read_dir(root)
        .await
        .map_err(|error| io_error("read release bundle root", root, error))?;
    let mut packages = Vec::new();
    let mut directories = 0_usize;
    while let Some(publisher) = publishers
        .next_entry()
        .await
        .map_err(|error| io_error("read release bundle publisher", root, error))?
    {
        directories = directories.saturating_add(1);
        ensure_directory_limit(directories)?;
        let publisher_path = publisher.path();
        let metadata = publisher.file_type().await.map_err(|error| {
            io_error("inspect release bundle publisher", &publisher_path, error)
        })?;
        if metadata.is_symlink() || !metadata.is_dir() {
            continue;
        }
        let mut entries = fs::read_dir(&publisher_path)
            .await
            .map_err(|error| io_error("read release bundle packages", &publisher_path, error))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| io_error("read release bundle package", &publisher_path, error))?
        {
            directories = directories.saturating_add(1);
            ensure_directory_limit(directories)?;
            let package_root = entry.path();
            let metadata = entry.file_type().await.map_err(|error| {
                io_error("inspect release bundle package", &package_root, error)
            })?;
            if metadata.is_symlink() || !metadata.is_dir() {
                continue;
            }
            match fs::symlink_metadata(package_root.join(MANIFEST_NAME)).await {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(io_error(
                        "inspect release bundle manifest",
                        &package_root,
                        error,
                    ))
                }
            }
            let package = inspect_release_bundle(&package_root).await?;
            let expected = format!(
                "{}/{}",
                publisher.file_name().to_string_lossy(),
                entry.file_name().to_string_lossy()
            );
            if package.package_id != expected {
                return Err(UseError::new(
                    "use.extension.release_bundle_invalid",
                    format!(
                        "Release bundle directory '{}' declares package '{}'.",
                        package_root.display(),
                        package.package_id
                    ),
                ));
            }
            packages.push(package);
        }
    }
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    if packages
        .windows(2)
        .any(|pair| pair[0].package_id == pair[1].package_id)
    {
        return Err(UseError::new(
            "use.extension.release_bundle_invalid",
            "The A3S Use release contains duplicate extension bundle IDs.",
        ));
    }
    Ok(packages)
}

fn release_bundles_root() -> UseResult<PathBuf> {
    if let Some(value) = std::env::var_os(RELEASE_BUNDLES_ENV) {
        if value.is_empty() {
            return Err(UseError::new(
                "use.extension.release_bundle_path_invalid",
                format!("{RELEASE_BUNDLES_ENV} cannot be empty."),
            ));
        }
        let path = PathBuf::from(value);
        return if path.is_absolute() {
            Ok(path)
        } else {
            std::env::current_dir()
                .map(|directory| directory.join(path))
                .map_err(|error| {
                    UseError::new(
                        "use.extension.release_bundle_path_invalid",
                        format!("Failed to resolve {RELEASE_BUNDLES_ENV}: {error}"),
                    )
                })
        };
    }
    let executable = std::env::current_exe().map_err(|error| {
        UseError::new(
            "use.extension.release_bundle_path_invalid",
            format!("Failed to locate the A3S Use executable: {error}"),
        )
    })?;
    executable
        .parent()
        .map(|directory| directory.join(RELEASE_BUNDLES_DIRECTORY))
        .ok_or_else(|| {
            UseError::new(
                "use.extension.release_bundle_path_invalid",
                "The A3S Use executable has no containing directory.",
            )
        })
}

async fn validate_root(root: &Path, missing_is_empty: bool) -> UseResult<bool> {
    let metadata = match fs::symlink_metadata(root).await {
        Ok(metadata) => metadata,
        Err(error) if missing_is_empty && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(false)
        }
        Err(error) => return Err(io_error("inspect release bundle root", root, error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(UseError::new(
            "use.extension.release_bundle_path_invalid",
            format!(
                "Release bundle root '{}' must be a real directory.",
                root.display()
            ),
        ));
    }
    Ok(true)
}

fn package_segments(package_id: &str) -> UseResult<[&str; 2]> {
    let mut segments = package_id.split('/');
    match (segments.next(), segments.next(), segments.next()) {
        (Some(publisher), Some(name), None) if valid_segment(publisher) && valid_segment(name) => {
            Ok([publisher, name])
        }
        _ => Err(UseError::new(
            "use.extension.package_id_invalid",
            "Extension package IDs must be '<publisher>/<name>' lowercase identifiers.",
        )),
    }
}

fn valid_segment(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn ensure_directory_limit(count: usize) -> UseResult<()> {
    if count <= MAX_BUNDLE_DIRECTORIES {
        Ok(())
    } else {
        Err(UseError::new(
            "use.extension.release_bundle_invalid",
            "The A3S Use release contains too many extension bundle directories.",
        ))
    }
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> UseError {
    UseError::new(
        "use.extension.io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn catalogs_valid_release_bundles_and_ignores_unrelated_directories() {
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("a3s/science");
        fs::create_dir_all(package.join("skills/science"))
            .await
            .unwrap();
        fs::create_dir_all(temp.path().join("notes/drafts"))
            .await
            .unwrap();
        fs::write(
            package.join(MANIFEST_NAME),
            r#"extension "a3s/science" {
  schema_version = 1
  version = "1.2.3"
  route = "science"
  actions = ["read"]
  skill { path = "skills/science/SKILL.md" }
}"#,
        )
        .await
        .unwrap();
        fs::write(
            package.join("skills/science/SKILL.md"),
            "---\nname: science\ndescription: Research.\n---\n",
        )
        .await
        .unwrap();

        let catalog = list_at(temp.path()).await.unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].package_id, "a3s/science");
        assert_eq!(catalog[0].version, "1.2.3");
        assert_eq!(catalog[0].surfaces, ["skill"]);
        assert_eq!(catalog[0].package_sha256.len(), 64);
    }
}
