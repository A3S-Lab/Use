use std::fmt;

use a3s_use_core::{InstallationId, UseError, UseResult, MAX_PLUGIN_PLAN_ITEMS};
use serde::{Deserialize, Serialize};

use super::{
    ExtensionGenerationLease, ExtensionLifecycleIdentity, ExtensionRegistry,
    ExtensionRegistrySnapshot, InstalledExtension,
};

pub const EXTENSION_SNAPSHOT_CURSOR_SCHEMA: &str = "a3s.use.extension-snapshot-cursor.v3";

/// Exact immutable package generation selected by one Registry publication.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionSnapshotPackage {
    pub package_id: String,
    pub lifecycle_generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
}

impl ExtensionSnapshotPackage {
    fn lifecycle_identity(&self) -> UseResult<ExtensionLifecycleIdentity> {
        ExtensionLifecycleIdentity::new(
            &self.package_id,
            self.package_digest.clone(),
            self.manifest_digest.clone(),
            self.lifecycle_generation,
        )
        .map_err(|error| {
            snapshot_cursor_error(format!(
                "A snapshot package has invalid lifecycle identity: {}",
                error.message
            ))
        })
    }

    fn matches_extension(&self, extension: &InstalledExtension) -> bool {
        let receipt = &extension.receipt;
        receipt.package_id == self.package_id
            && receipt.lifecycle_generation == Some(self.lifecycle_generation)
            && receipt.package_sha256.as_deref() == self.package_digest.strip_prefix("sha256:")
            && receipt.manifest_sha256
                == self
                    .manifest_digest
                    .strip_prefix("sha256:")
                    .unwrap_or_default()
    }
}

/// Stable resume and lease-acquisition cursor for one Registry publication.
///
/// `revision` binds the complete Registry projection, including disabled
/// packages and human aliases. `packages` is the sorted set of callable immutable generations that
/// must all be leased before a host may admit work against this cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionSnapshotCursor {
    pub schema: String,
    pub installation: InstallationId,
    pub generation: u64,
    pub revision: String,
    pub packages: Vec<ExtensionSnapshotPackage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unleasable_packages: Vec<String>,
}

impl ExtensionSnapshotCursor {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != EXTENSION_SNAPSHOT_CURSOR_SCHEMA
            || self.installation.validate().is_err()
            || !valid_canonical_sha256(&self.revision)
            || self.packages.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.unleasable_packages.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.packages.windows(2).any(|pair| pair[0] >= pair[1])
            || self.unleasable_packages.iter().any(|package_id| {
                !matches!(
                    super::normalize_package_id(package_id),
                    Ok(normalized) if normalized == *package_id
                )
            })
            || self
                .unleasable_packages
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(snapshot_cursor_error(
                "The extension snapshot cursor has invalid schema, revision, bounds, or ordering.",
            ));
        }
        for package in &self.packages {
            package.lifecycle_identity()?;
        }
        if self
            .packages
            .iter()
            .map(|package| package.package_id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != self.packages.len()
        {
            return Err(snapshot_cursor_error(
                "The extension snapshot cursor contains duplicate package identities.",
            ));
        }
        if self.packages.iter().any(|package| {
            self.unleasable_packages
                .binary_search(&package.package_id)
                .is_ok()
        }) {
            return Err(snapshot_cursor_error(
                "The extension snapshot cursor contains overlapping leasable and unleasable package identities.",
            ));
        }
        Ok(())
    }

    pub fn is_fully_leasable(&self) -> bool {
        self.unleasable_packages.is_empty()
    }
}

impl ExtensionRegistrySnapshot {
    /// Derive the canonical exact-generation cursor for this immutable
    /// publication without reading mutable receipts.
    pub fn cursor(&self) -> UseResult<ExtensionSnapshotCursor> {
        self.validate()?;
        let mut packages = Vec::new();
        let mut unleasable_packages = Vec::new();
        for binding in self.packages.iter().filter(|binding| binding.enabled) {
            let (Some(lifecycle_generation), Some(package_sha256)) = (
                binding.lifecycle_generation,
                binding.package_sha256.as_deref(),
            ) else {
                unleasable_packages.push(binding.package_id.clone());
                continue;
            };
            let package = ExtensionSnapshotPackage {
                package_id: binding.package_id.clone(),
                lifecycle_generation,
                package_digest: format!("sha256:{package_sha256}"),
                manifest_digest: format!("sha256:{}", binding.manifest_sha256),
            };
            package.lifecycle_identity()?;
            packages.push(package);
        }
        packages.sort();
        unleasable_packages.sort();
        let cursor = ExtensionSnapshotCursor {
            schema: EXTENSION_SNAPSHOT_CURSOR_SCHEMA.to_owned(),
            installation: self.installation.clone(),
            generation: self.generation,
            revision: self.descriptor_digest()?,
            packages,
            unleasable_packages,
        };
        cursor.validate()?;
        Ok(cursor)
    }
}

/// All-or-nothing RAII lease for every callable package in one publication.
///
/// Dropping the value synchronously releases its shared generation locks. Lifecycle
/// cleanup remains explicit and asynchronous in its owning coordinator.
pub struct ExtensionSnapshotLease {
    cursor: ExtensionSnapshotCursor,
    leases: Vec<ExtensionGenerationLease>,
}

impl ExtensionSnapshotLease {
    pub fn cursor(&self) -> &ExtensionSnapshotCursor {
        &self.cursor
    }

    pub fn len(&self) -> usize {
        self.leases.len()
    }

    pub fn is_empty(&self) -> bool {
        self.leases.is_empty()
    }

    pub fn packages(&self) -> impl ExactSizeIterator<Item = &InstalledExtension> {
        self.leases.iter().map(ExtensionGenerationLease::extension)
    }

    pub async fn verify_integrity(&self) -> UseResult<()> {
        for lease in &self.leases {
            lease.verify_integrity().await?;
        }
        Ok(())
    }
}

impl fmt::Debug for ExtensionSnapshotLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionSnapshotLease")
            .field("cursor", &self.cursor)
            .field("lease_count", &self.leases.len())
            .finish()
    }
}

impl ExtensionRegistry {
    /// Acquire every callable package generation selected by `expected`.
    ///
    /// Acquisition is deterministic and all-or-nothing. The Registry
    /// publication is checked before and after all generation locks are held, so a
    /// concurrent cutover can never return a mixed-generation lease. `None`
    /// means the cursor is stale, a package was hidden, or drain already owns a
    /// required generation.
    pub async fn acquire_published_snapshot(
        &self,
        expected: &ExtensionSnapshotCursor,
    ) -> UseResult<Option<ExtensionSnapshotLease>> {
        expected.validate()?;
        if expected.installation != *self.installation() {
            return Err(UseError::new(
                "use.extension.snapshot_scope_mismatch",
                "The extension snapshot cursor belongs to a different installation.",
            ));
        }
        if !expected.is_fully_leasable() {
            return Err(UseError::new(
                "use.extension.snapshot_unleasable",
                "The published extension snapshot contains a callable package without immutable lifecycle generation evidence.",
            )
            .with_detail("packageIds", expected.unleasable_packages.clone())
            .with_suggestion(
                "Reinstall the package before using exact-generation admission.",
            ));
        }

        let before = self.published_snapshot().await?.cursor()?;
        if before != *expected {
            return Ok(None);
        }

        let mut leases = Vec::with_capacity(expected.packages.len());
        for package in &expected.packages {
            let identity = package.lifecycle_identity()?;
            let Some(lease) = self
                .acquire_published_lifecycle_generation(&identity)
                .await?
            else {
                return Ok(None);
            };
            if !package.matches_extension(lease.extension()) {
                return Err(UseError::new(
                    "use.extension.snapshot_lease_mismatch",
                    "An acquired extension lease differs from its exact snapshot package identity.",
                ));
            }
            leases.push(lease);
        }

        let after = self.published_snapshot().await?.cursor()?;
        if after != *expected {
            return Ok(None);
        }
        Ok(Some(ExtensionSnapshotLease {
            cursor: expected.clone(),
            leases,
        }))
    }
}

fn snapshot_cursor_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.snapshot_cursor_invalid", message)
}

fn valid_canonical_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}
