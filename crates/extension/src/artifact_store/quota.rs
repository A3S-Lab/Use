use a3s_use_core::{UseError, UseResult};
use serde::Serialize;

use super::{
    ArtifactKind, ArtifactPhysicalState, ArtifactReferenceAdmission, ArtifactStore,
    MAX_ARTIFACT_STORE_INVENTORY_ENTRIES,
};

mod lock;
mod policy;

use lock::{acquire_storage_quota_lock, StorageQuotaLock, StorageQuotaLockMode};
use policy::{load_policy, remove_optional_temporary, remove_policy, write_policy};

pub(super) use lock::STORAGE_QUOTA_LOCK;
pub(super) use policy::{
    validate_policy_metadata, STORAGE_QUOTA_POLICY_FILE, STORAGE_QUOTA_TEMPORARY_FILE,
};

pub const ARTIFACT_STORAGE_QUOTA_POLICY_SCHEMA_VERSION: u32 = 1;
pub const MAX_ARTIFACT_STORAGE_QUOTA_ARTIFACTS: u64 = MAX_ARTIFACT_STORE_INVENTORY_ENTRIES as u64;

/// Global logical-byte and digest-container ceiling for the Artifact Store.
/// Logical bytes are file lengths, not allocated filesystem blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactStorageQuotaPolicy {
    max_physical_bytes: u64,
    max_physical_artifacts: u64,
}

impl ArtifactStorageQuotaPolicy {
    pub fn new(max_physical_bytes: u64, max_physical_artifacts: u64) -> UseResult<Self> {
        if max_physical_bytes == 0
            || max_physical_artifacts == 0
            || max_physical_artifacts > MAX_ARTIFACT_STORAGE_QUOTA_ARTIFACTS
        {
            return Err(quota_policy_invalid(
                "Artifact storage quota bounds are zero or the artifact limit exceeds the inventory ceiling.",
            ));
        }
        Ok(Self {
            max_physical_bytes,
            max_physical_artifacts,
        })
    }

    pub const fn max_physical_bytes(&self) -> u64 {
        self.max_physical_bytes
    }

    pub const fn max_physical_artifacts(&self) -> u64 {
        self.max_physical_artifacts
    }
}

/// Immutable view of the active durable Artifact Store quota policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactStorageQuotaSnapshot {
    pub schema_version: u32,
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<ArtifactStorageQuotaPolicy>,
}

/// Operator action applied with revision compare-and-swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactStorageQuotaAction {
    Set,
    Clear,
}

/// Result of one durable quota-policy mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactStorageQuotaMutation {
    pub action: ArtifactStorageQuotaAction,
    pub changed: bool,
    pub previous_revision: String,
    pub snapshot: ArtifactStorageQuotaSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct ArtifactStorageWrite {
    kind: ArtifactKind,
    digest: String,
    content_bytes: u64,
    content_files: u64,
}

impl ArtifactStorageWrite {
    pub(crate) fn blob(sha256: &str, content_bytes: u64) -> UseResult<Self> {
        Self::new(
            ArtifactKind::Blob,
            format!("sha256:{sha256}"),
            content_bytes,
            1,
        )
    }

    pub(crate) fn expanded(
        digest: &str,
        content_bytes: u64,
        content_files: u64,
    ) -> UseResult<Self> {
        Self::new(
            ArtifactKind::ExpandedPackage,
            digest.to_owned(),
            content_bytes,
            content_files,
        )
    }

    fn new(
        kind: ArtifactKind,
        digest: String,
        content_bytes: u64,
        content_files: u64,
    ) -> UseResult<Self> {
        let valid_digest = digest.strip_prefix("sha256:").is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        });
        if !valid_digest
            || content_bytes == 0
            || content_files == 0
            || (kind == ArtifactKind::Blob && content_files != 1)
        {
            return Err(quota_policy_invalid(
                "An Artifact Store write has invalid identity or physical expectations.",
            ));
        }
        Ok(Self {
            kind,
            digest,
            content_bytes,
            content_files,
        })
    }
}

#[derive(Debug)]
#[must_use = "dropping storage admission releases the global quota boundary"]
pub(crate) struct ArtifactStorageAdmission {
    lock: StorageQuotaLock,
}

impl ArtifactStorageAdmission {
    pub(crate) fn ensure_store(&self, store: &ArtifactStore) -> UseResult<()> {
        self.lock.ensure_store(store)
    }
}

impl ArtifactStore {
    /// Read the active durable global storage policy and its CAS revision.
    pub async fn storage_quota(&self) -> UseResult<ArtifactStorageQuotaSnapshot> {
        let _reference = self.acquire_reference_admission().await?;
        let _lock = acquire_storage_quota_lock(self, StorageQuotaLockMode::Shared).await?;
        Ok(snapshot(load_policy(self.root()).await?))
    }

    /// Set the durable global storage policy if its reviewed revision is current.
    pub async fn set_storage_quota(
        &self,
        expected_revision: &str,
        policy: ArtifactStorageQuotaPolicy,
    ) -> UseResult<ArtifactStorageQuotaMutation> {
        let _reference = self.acquire_reference_admission().await?;
        let _lock = acquire_storage_quota_lock(self, StorageQuotaLockMode::Exclusive).await?;
        let current = snapshot(load_policy(self.root()).await?);
        require_revision(&current, expected_revision)?;
        let changed = current.policy != Some(policy);
        if changed {
            write_policy(self.root(), policy).await?;
        }
        Ok(ArtifactStorageQuotaMutation {
            action: ArtifactStorageQuotaAction::Set,
            changed,
            previous_revision: current.revision,
            snapshot: snapshot(Some(policy)),
        })
    }

    /// Disable the global storage policy if its reviewed revision is current.
    pub async fn clear_storage_quota(
        &self,
        expected_revision: &str,
    ) -> UseResult<ArtifactStorageQuotaMutation> {
        let _reference = self.acquire_reference_admission().await?;
        let _lock = acquire_storage_quota_lock(self, StorageQuotaLockMode::Exclusive).await?;
        let current = snapshot(load_policy(self.root()).await?);
        require_revision(&current, expected_revision)?;
        let changed = current.policy.is_some();
        remove_optional_temporary(&self.root().join(STORAGE_QUOTA_TEMPORARY_FILE)).await?;
        if changed {
            remove_policy(self.root()).await?;
        }
        Ok(ArtifactStorageQuotaMutation {
            action: ArtifactStorageQuotaAction::Clear,
            changed,
            previous_revision: current.revision,
            snapshot: snapshot(None),
        })
    }

    pub(crate) async fn acquire_storage_admission(
        &self,
        reference: &ArtifactReferenceAdmission,
        write: ArtifactStorageWrite,
    ) -> UseResult<ArtifactStorageAdmission> {
        reference.ensure_store(self)?;
        let shared = acquire_storage_quota_lock(self, StorageQuotaLockMode::Shared).await?;
        if load_policy(self.root()).await?.is_none() {
            return Ok(ArtifactStorageAdmission { lock: shared });
        }
        drop(shared);

        let exclusive = acquire_storage_quota_lock(self, StorageQuotaLockMode::Exclusive).await?;
        if let Some(policy) = load_policy(self.root()).await? {
            self.admit_storage_write(policy, &write).await?;
        }
        Ok(ArtifactStorageAdmission { lock: exclusive })
    }

    async fn admit_storage_write(
        &self,
        policy: ArtifactStorageQuotaPolicy,
        write: &ArtifactStorageWrite,
    ) -> UseResult<()> {
        let inventory = self.scan_inventory_under_global_guard().await?;
        let mut current_bytes = 0_u64;
        for entry in &inventory.entries {
            current_bytes = current_bytes
                .checked_add(entry.content_bytes)
                .and_then(|value| value.checked_add(entry.staging_bytes))
                .ok_or_else(quota_accounting_overflow)?;
        }
        let current_artifacts =
            u64::try_from(inventory.entries.len()).map_err(|_| quota_accounting_overflow())?;
        let existing = inventory
            .entries
            .iter()
            .find(|entry| entry.kind == write.kind && entry.digest == write.digest);
        let replaced_staging = existing.map_or(0, |entry| entry.staging_bytes);
        let projected_content = match existing {
            Some(entry) if entry.state == ArtifactPhysicalState::Complete => 0,
            _ => write.content_bytes,
        };
        let projected_bytes = current_bytes
            .checked_sub(replaced_staging)
            .and_then(|value| value.checked_add(projected_content))
            .ok_or_else(quota_accounting_overflow)?;
        let projected_artifacts = current_artifacts
            .checked_add(u64::from(existing.is_none()))
            .ok_or_else(quota_accounting_overflow)?;

        let bytes_worsen_excess =
            projected_bytes > policy.max_physical_bytes() && projected_bytes > current_bytes;
        let artifacts_worsen_excess = projected_artifacts > policy.max_physical_artifacts()
            && projected_artifacts > current_artifacts;
        if bytes_worsen_excess || artifacts_worsen_excess {
            return Err(UseError::new(
                "use.artifact_store.quota_exceeded",
                "The Artifact Store write would exceed the configured global storage quota.",
            )
            .with_detail(
                "kind",
                match write.kind {
                    ArtifactKind::Blob => "blob",
                    ArtifactKind::ExpandedPackage => "expanded-package",
                },
            )
            .with_detail("digest", write.digest.clone())
            .with_detail("expectedContentBytes", write.content_bytes.to_string())
            .with_detail("expectedContentFiles", write.content_files.to_string())
            .with_detail("replacedStagingBytes", replaced_staging.to_string())
            .with_detail("currentPhysicalBytes", current_bytes.to_string())
            .with_detail("projectedPhysicalBytes", projected_bytes.to_string())
            .with_detail("maxPhysicalBytes", policy.max_physical_bytes().to_string())
            .with_detail("currentPhysicalArtifacts", current_artifacts.to_string())
            .with_detail(
                "projectedPhysicalArtifacts",
                projected_artifacts.to_string(),
            )
            .with_detail(
                "maxPhysicalArtifacts",
                policy.max_physical_artifacts().to_string(),
            )
            .with_suggestion(
                "Increase the reviewed global quota or complete confirmed Artifact Store maintenance before retrying.",
            ));
        }
        Ok(())
    }
}

fn snapshot(policy: Option<ArtifactStorageQuotaPolicy>) -> ArtifactStorageQuotaSnapshot {
    ArtifactStorageQuotaSnapshot {
        schema_version: ARTIFACT_STORAGE_QUOTA_POLICY_SCHEMA_VERSION,
        revision: policy::revision(policy),
        policy,
    }
}

fn require_revision(
    current: &ArtifactStorageQuotaSnapshot,
    expected_revision: &str,
) -> UseResult<()> {
    if current.revision == expected_revision {
        return Ok(());
    }
    Err(UseError::new(
        "use.artifact_store.quota_revision_conflict",
        "Artifact Store quota policy changed after it was reviewed.",
    )
    .with_detail("expectedRevision", expected_revision.to_owned())
    .with_detail("actualRevision", current.revision.clone())
    .with_suggestion("Inspect the current quota revision and review the mutation again."))
}

fn quota_policy_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.artifact_store.quota_policy_invalid", message)
}

pub(super) fn quota_config_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.artifact_store.quota_config_invalid", message)
}

fn quota_accounting_overflow() -> UseError {
    UseError::new(
        "use.artifact_store.quota_accounting_overflow",
        "Artifact Store quota accounting exceeds its numeric bounds.",
    )
}
