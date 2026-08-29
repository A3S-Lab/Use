use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::{ArtifactCollectionGuard, UsePaths};

pub const REGISTRY_ARTIFACT_REFERENCE_INVENTORY_SCHEMA: &str =
    "a3s.use.registry-artifact-reference-inventory.v1";

const REMOTE_REGISTRIES_DIRECTORY: &str = "remote-registries";
const SOURCES_DIRECTORY: &str = "sources";
const VERIFIED_TARGETS_DIRECTORY: &str = "verified-targets";
const CATALOG_METADATA_DIRECTORY: &str = "catalog-metadata";
const MAX_REGISTRY_DIRECTORIES: usize = 1_024;
const MAX_SOURCE_DATASTORES: usize = 4_096;
const MAX_DATASTORE_ENTRIES: usize = 32;
const MAX_CATALOG_METADATA_ENTRIES: usize = 32;
const MAX_REGISTRY_ARTIFACT_REFERENCES: usize = 100_000;

/// One path-free blob reference retained by an immutable Registry-source
/// datastore, including datastores no longer selected by current config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryArtifactReference {
    pub registry_name: String,
    pub source_identity: String,
    pub digest: String,
    pub expected_bytes: u64,
}

/// Deterministic evidence derived from every durable Registry target
/// observation under one global Use state root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistryArtifactReferenceInventory {
    pub schema: String,
    pub references: Vec<RegistryArtifactReference>,
}

pub(super) async fn inspect(
    paths: &UsePaths,
    collection: &ArtifactCollectionGuard,
) -> UseResult<RegistryArtifactReferenceInventory> {
    collection.ensure_store(&paths.artifact_store())?;
    let root = paths.state_root().join(REMOTE_REGISTRIES_DIRECTORY);
    if !optional_owned_directory(&root, "Registry datastore root").await? {
        return Ok(empty_inventory());
    }

    let mut registry_count = 0_usize;
    let mut source_count = 0_usize;
    let mut references = Vec::new();
    let mut registries = fs::read_dir(&root)
        .await
        .map_err(|error| reference_io("read Registry datastore root", &root, error))?;
    while let Some(entry) = registries
        .next_entry()
        .await
        .map_err(|error| reference_io("read Registry datastore entry", &root, error))?
    {
        registry_count = checked_count(
            registry_count,
            MAX_REGISTRY_DIRECTORIES,
            "The Registry datastore inventory exceeds its registry bound.",
        )?;
        let registry_name = entry_name(&entry, "Registry datastore root")?;
        crate::remote::validate_registry_name(&registry_name)?;
        let registry_root = entry.path();
        require_owned_directory(&registry_root, "Registry datastore namespace").await?;
        scan_registry_namespace(
            &registry_root,
            &registry_name,
            &mut source_count,
            &mut references,
        )
        .await?;
    }
    references.sort_by(|left, right| {
        left.registry_name
            .cmp(&right.registry_name)
            .then_with(|| left.source_identity.cmp(&right.source_identity))
            .then_with(|| left.digest.cmp(&right.digest))
    });
    Ok(RegistryArtifactReferenceInventory {
        schema: REGISTRY_ARTIFACT_REFERENCE_INVENTORY_SCHEMA.to_owned(),
        references,
    })
}

async fn scan_registry_namespace(
    root: &Path,
    registry_name: &str,
    source_count: &mut usize,
    references: &mut Vec<RegistryArtifactReference>,
) -> UseResult<()> {
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| reference_io("read Registry namespace", root, error))?;
    let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| reference_io("read Registry namespace entry", root, error))?
    else {
        return Err(reference_invalid(
            "A Registry datastore namespace omits its sources directory.",
        ));
    };
    if entry_name(&entry, "Registry datastore namespace")? != SOURCES_DIRECTORY {
        return Err(reference_invalid(
            "A Registry datastore namespace contains an unknown entry.",
        ));
    }
    let sources_root = entry.path();
    require_owned_directory(&sources_root, "Registry sources directory").await?;
    if entries
        .next_entry()
        .await
        .map_err(|error| reference_io("read Registry namespace entry", root, error))?
        .is_some()
    {
        return Err(reference_invalid(
            "A Registry datastore namespace contains more than its sources directory.",
        ));
    }

    let mut sources = fs::read_dir(&sources_root)
        .await
        .map_err(|error| reference_io("read Registry sources directory", &sources_root, error))?;
    while let Some(entry) = sources
        .next_entry()
        .await
        .map_err(|error| reference_io("read Registry source datastore", &sources_root, error))?
    {
        *source_count = checked_count(
            *source_count,
            MAX_SOURCE_DATASTORES,
            "The Registry datastore inventory exceeds its source bound.",
        )?;
        let source_identity = entry_name(&entry, "Registry sources directory")?;
        if !valid_sha256(&source_identity) {
            return Err(reference_invalid(
                "A Registry source datastore has a non-canonical identity.",
            ));
        }
        let datastore = entry.path();
        require_owned_directory(&datastore, "Registry source datastore").await?;
        scan_source_datastore(&datastore, registry_name, &source_identity, references).await?;
    }
    Ok(())
}

async fn scan_source_datastore(
    root: &Path,
    registry_name: &str,
    source_identity: &str,
    references: &mut Vec<RegistryArtifactReference>,
) -> UseResult<()> {
    let mut entry_count = 0_usize;
    let mut has_verified_targets = false;
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| reference_io("read Registry source datastore", root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| reference_io("read Registry source datastore entry", root, error))?
    {
        entry_count = checked_count(
            entry_count,
            MAX_DATASTORE_ENTRIES,
            "A Registry source datastore exceeds its entry bound.",
        )?;
        let name = entry_name(&entry, "Registry source datastore")?;
        let path = entry.path();
        match name.as_str() {
            ".metadata.lock" | ".target-cache.lock" | "root.json" => {
                require_owned_file(&path, "Registry datastore file").await?;
            }
            CATALOG_METADATA_DIRECTORY => {
                require_owned_directory(&path, "Registry catalog metadata directory").await?;
                scan_catalog_metadata(&path).await?;
            }
            VERIFIED_TARGETS_DIRECTORY => {
                require_owned_directory(&path, "Registry target reference directory").await?;
                has_verified_targets = true;
            }
            _ if valid_temporary_name(&name, ".root-") => {
                require_owned_file(&path, "Registry root temporary file").await?;
            }
            _ => {
                return Err(reference_invalid(
                    "A Registry source datastore contains an unknown entry.",
                ));
            }
        }
    }

    if has_verified_targets {
        for (digest, expected_bytes) in
            crate::remote::inspect_registry_artifact_references(root).await?
        {
            if references.len() >= MAX_REGISTRY_ARTIFACT_REFERENCES {
                return Err(reference_limit(
                    "The Registry artifact reference inventory exceeds its reference bound.",
                ));
            }
            references.push(RegistryArtifactReference {
                registry_name: registry_name.to_owned(),
                source_identity: source_identity.to_owned(),
                digest,
                expected_bytes,
            });
        }
    }
    Ok(())
}

async fn scan_catalog_metadata(root: &Path) -> UseResult<()> {
    let mut entry_count = 0_usize;
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| reference_io("read Registry catalog metadata", root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| reference_io("read Registry catalog metadata entry", root, error))?
    {
        entry_count = checked_count(
            entry_count,
            MAX_CATALOG_METADATA_ENTRIES,
            "A Registry catalog metadata directory exceeds its entry bound.",
        )?;
        let name = entry_name(&entry, "Registry catalog metadata directory")?;
        let known = matches!(
            name.as_str(),
            "root.json"
                | "timestamp.json"
                | "snapshot.json"
                | "targets.json"
                | "catalog-cache.json"
        ) || valid_temporary_name(&name, ".catalog-cache-");
        if !known {
            return Err(reference_invalid(
                "A Registry catalog metadata directory contains an unknown entry.",
            ));
        }
        require_owned_file(&entry.path(), "Registry catalog metadata file").await?;
    }
    Ok(())
}

async fn optional_owned_directory(path: &Path, label: &str) -> UseResult<bool> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(reference_io(&format!("inspect {label}"), path, error)),
    };
    validate_owned_directory(path, &metadata, label)?;
    Ok(true)
}

async fn require_owned_directory(path: &Path, label: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| reference_io(&format!("inspect {label}"), path, error))?;
    validate_owned_directory(path, &metadata, label)
}

async fn require_owned_file(path: &Path, label: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| reference_io(&format!("inspect {label}"), path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(
            reference_invalid(format!("The {label} must be an owned regular file."))
                .with_detail("path", path.display().to_string()),
        );
    }
    Ok(())
}

fn validate_owned_directory(
    path: &Path,
    metadata: &std::fs::Metadata,
    label: &str,
) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata) || !metadata.is_dir() {
        return Err(
            reference_invalid(format!("The {label} must be an owned directory."))
                .with_detail("path", path.display().to_string()),
        );
    }
    Ok(())
}

fn entry_name(entry: &fs::DirEntry, label: &str) -> UseResult<String> {
    entry.file_name().into_string().map_err(|_| {
        reference_invalid(format!("The {label} contains a non-UTF-8 entry name."))
            .with_detail("path", entry.path().display().to_string())
    })
}

fn checked_count(current: usize, limit: usize, message: &str) -> UseResult<usize> {
    let next = current
        .checked_add(1)
        .ok_or_else(|| reference_limit(message))?;
    if next > limit {
        return Err(reference_limit(message));
    }
    Ok(next)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_temporary_name(name: &str, prefix: &str) -> bool {
    let Some(suffix) = name
        .strip_prefix(prefix)
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    !suffix.is_empty()
        && suffix.len() <= 80
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'-')
}

fn empty_inventory() -> RegistryArtifactReferenceInventory {
    RegistryArtifactReferenceInventory {
        schema: REGISTRY_ARTIFACT_REFERENCE_INVENTORY_SCHEMA.to_owned(),
        references: Vec::new(),
    }
}

fn reference_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.extension.registry_artifact_references_invalid",
        message,
    )
}

fn reference_limit(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.extension.registry_artifact_references_limit_exceeded",
        message,
    )
}

fn reference_io(action: &str, path: &Path, error: std::io::Error) -> UseError {
    UseError::new(
        "use.extension.registry_artifact_references_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

#[cfg(test)]
mod tests;
