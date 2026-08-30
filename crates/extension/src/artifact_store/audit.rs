use std::path::Path;

use a3s_use_core::UseResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::blob::blob_open_options;
use super::{
    artifact_store_error, ArtifactCollectionGuard, ArtifactInventoryEntry, ArtifactKind,
    ArtifactPhysicalState, ArtifactStore,
};
use crate::package::io_error;

/// Stable schema for deterministic, path-free Artifact Store digest evidence.
pub const ARTIFACT_STORE_DIGEST_AUDIT_SCHEMA: &str = "a3s.use.artifact-store-digest-audit.v1";

/// Integrity outcome for one physical artifact container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactDigestAuditStatus {
    /// Canonical content was present and matched the digest in its path.
    Verified,
    /// Canonical content was present but did not match the digest in its path.
    Mismatch,
    /// Canonical content was absent, so no digest could be computed.
    Incomplete,
}

/// One path-free digest result with the physical measurements observed before
/// hashing. A mismatch is evidence only and grants no mutation authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactDigestAuditEntry {
    pub kind: ArtifactKind,
    pub digest: String,
    pub content_bytes: u64,
    pub content_files: u64,
    pub staging_entries: u64,
    pub staging_bytes: u64,
    pub status: ArtifactDigestAuditStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_digest: Option<String>,
}

/// Deterministic digest evidence for one stable physical Artifact Store scan.
///
/// The report is deliberately path-free and has no deletion, quarantine,
/// repair, or rehydration authority. `audited_bytes` and `audited_files` cover
/// both verified and mismatched complete content; incomplete containers are
/// retained as evidence but are not hashed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactStoreDigestAudit {
    pub schema: String,
    pub entries: Vec<ArtifactDigestAuditEntry>,
    pub verified_artifacts: u64,
    pub mismatched_artifacts: u64,
    pub incomplete_artifacts: u64,
    pub audited_bytes: u64,
    pub audited_files: u64,
}

impl ArtifactStore {
    /// Recompute every complete artifact digest while the exact store-bound
    /// collection guard prevents admitted publication.
    ///
    /// This is an explicit maintenance operation: hashing is sequential to
    /// avoid turning one full-store verification pass into unbounded disk
    /// contention. Unsafe layouts and physical drift fail closed. Digest
    /// mismatches remain reportable forensic evidence and are never replaced.
    pub async fn audit_digests(
        &self,
        collection: &ArtifactCollectionGuard,
    ) -> UseResult<ArtifactStoreDigestAudit> {
        collection.ensure_store(self)?;
        let inventory = self.scan_inventory_under_global_guard().await?;
        let mut report = ArtifactStoreDigestAudit {
            schema: ARTIFACT_STORE_DIGEST_AUDIT_SCHEMA.to_owned(),
            entries: Vec::with_capacity(inventory.entries.len()),
            verified_artifacts: 0,
            mismatched_artifacts: 0,
            incomplete_artifacts: 0,
            audited_bytes: 0,
            audited_files: 0,
        };

        for physical in &inventory.entries {
            let (status, observed_digest) = match physical.state {
                ArtifactPhysicalState::Incomplete => {
                    checked_increment(
                        &mut report.incomplete_artifacts,
                        "The incomplete artifact count overflowed during digest audit.",
                    )?;
                    (ArtifactDigestAuditStatus::Incomplete, None)
                }
                ArtifactPhysicalState::Complete => {
                    let observed = self.audit_complete_artifact(physical).await?;
                    report.audited_bytes = checked_add(
                        report.audited_bytes,
                        physical.content_bytes,
                        "The audited artifact byte count overflowed.",
                    )?;
                    report.audited_files = checked_add(
                        report.audited_files,
                        physical.content_files,
                        "The audited artifact file count overflowed.",
                    )?;
                    let status = if observed == physical.digest {
                        checked_increment(
                            &mut report.verified_artifacts,
                            "The verified artifact count overflowed during digest audit.",
                        )?;
                        ArtifactDigestAuditStatus::Verified
                    } else {
                        checked_increment(
                            &mut report.mismatched_artifacts,
                            "The mismatched artifact count overflowed during digest audit.",
                        )?;
                        ArtifactDigestAuditStatus::Mismatch
                    };
                    (status, Some(observed))
                }
            };
            report.entries.push(ArtifactDigestAuditEntry {
                kind: physical.kind,
                digest: physical.digest.clone(),
                content_bytes: physical.content_bytes,
                content_files: physical.content_files,
                staging_entries: physical.staging_entries,
                staging_bytes: physical.staging_bytes,
                status,
                observed_digest,
            });
        }

        let final_inventory = self.scan_inventory_under_global_guard().await?;
        if final_inventory != inventory {
            return Err(artifact_store_error(
                "use.artifact_store.audit_unstable",
                "The physical Artifact Store changed while its digests were audited.",
            ));
        }
        Ok(report)
    }

    async fn audit_complete_artifact(
        &self,
        physical: &ArtifactInventoryEntry,
    ) -> UseResult<String> {
        match physical.kind {
            ArtifactKind::Blob => {
                let path = self.blob_path(&physical.digest)?;
                hash_blob(&path, physical).await
            }
            ArtifactKind::ExpandedPackage => {
                let path = self.expanded_package_path(&physical.digest)?;
                let fingerprint = crate::digest::package_fingerprint(&path).await?;
                if fingerprint.byte_count != physical.content_bytes
                    || fingerprint.file_count != physical.content_files
                {
                    return Err(unstable_artifact(
                        physical,
                        "An expanded-package artifact changed after physical inventory.",
                    ));
                }
                Ok(format!("sha256:{}", fingerprint.sha256))
            }
        }
    }
}

async fn hash_blob(path: &Path, physical: &ArtifactInventoryEntry) -> UseResult<String> {
    let mut file = blob_open_options()
        .open(path)
        .await
        .map_err(|error| io_error("open artifact blob for digest audit", path, error))?;
    validate_opened_blob(&file, physical, path).await?;

    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| io_error("read artifact blob for digest audit", path, error))?;
        if read == 0 {
            break;
        }
        bytes = bytes.checked_add(read as u64).ok_or_else(|| {
            unstable_artifact(
                physical,
                "An artifact blob length overflowed during digest audit.",
            )
        })?;
        if bytes > physical.content_bytes {
            return Err(unstable_artifact(
                physical,
                "An artifact blob grew after physical inventory.",
            ));
        }
        digest.update(&buffer[..read]);
    }
    if bytes != physical.content_bytes {
        return Err(unstable_artifact(
            physical,
            "An artifact blob changed length during digest audit.",
        ));
    }
    validate_opened_blob(&file, physical, path).await?;
    Ok(format!("sha256:{:x}", digest.finalize()))
}

async fn validate_opened_blob(
    file: &tokio::fs::File,
    physical: &ArtifactInventoryEntry,
    path: &Path,
) -> UseResult<()> {
    let metadata = file
        .metadata()
        .await
        .map_err(|error| io_error("inspect artifact blob during digest audit", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() != physical.content_bytes
    {
        return Err(unstable_artifact(
            physical,
            "An artifact blob changed after physical inventory.",
        ));
    }
    Ok(())
}

fn checked_increment(value: &mut u64, message: &str) -> UseResult<()> {
    *value = checked_add(*value, 1, message)?;
    Ok(())
}

fn checked_add(left: u64, right: u64, message: &str) -> UseResult<u64> {
    left.checked_add(right)
        .ok_or_else(|| artifact_store_error("use.artifact_store.audit_limit_exceeded", message))
}

fn unstable_artifact(physical: &ArtifactInventoryEntry, message: &str) -> a3s_use_core::UseError {
    artifact_store_error("use.artifact_store.audit_unstable", message)
        .with_detail("kind", artifact_kind_name(physical.kind))
        .with_detail("digest", physical.digest.clone())
}

fn artifact_kind_name(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Blob => "blob",
        ArtifactKind::ExpandedPackage => "expanded-package",
    }
}
