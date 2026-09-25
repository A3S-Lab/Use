use std::fmt;
use std::sync::Arc;

#[cfg(feature = "extensions")]
use a3s_use_core::InstallationSnapshot;
use a3s_use_core::{InstallationId, PluginPackageId, UseError, UseResult, MAX_PLUGIN_PLAN_ITEMS};
use serde::{Deserialize, Serialize};
#[cfg(not(feature = "extensions"))]
use sha2::{Digest, Sha256};

use super::CapabilityRegistrySnapshot;

pub const CAPABILITY_SNAPSHOT_CURSOR_SCHEMA: &str = "a3s.use.capability-snapshot-cursor.v4";

/// Exact Use-owned package generation projected into a capability snapshot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityPackageGeneration {
    pub package_id: String,
    pub lifecycle_generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
}

/// Stable resume cursor for one complete Use capability projection.
///
/// The capability `revision` includes built-ins, readiness, and projected
/// surfaces. `registry_revision` binds the authoritative package Registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilitySnapshotCursor {
    pub schema: String,
    pub installation: InstallationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_snapshot_digest: Option<String>,
    pub generation: u64,
    pub revision: String,
    pub registry_revision: String,
    pub packages: Vec<CapabilityPackageGeneration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unleasable_packages: Vec<String>,
}

impl CapabilitySnapshotCursor {
    pub(super) fn from_projection(
        revision: &str,
        upstream: CapabilityUpstreamEvidence,
    ) -> UseResult<Self> {
        let cursor = Self {
            schema: CAPABILITY_SNAPSHOT_CURSOR_SCHEMA.to_owned(),
            installation: upstream.installation,
            installation_generation: upstream.installation_generation,
            installation_snapshot_digest: upstream.installation_snapshot_digest,
            generation: upstream.generation,
            revision: revision.to_owned(),
            registry_revision: upstream.registry_revision,
            packages: upstream.packages,
            unleasable_packages: upstream.unleasable_packages,
        };
        cursor.validate()?;
        Ok(cursor)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.installation.validate().map_err(|_| {
            cursor_error("The capability snapshot cursor installation identity is invalid.")
        })?;
        if self.schema != CAPABILITY_SNAPSHOT_CURSOR_SCHEMA
            || self.installation_generation.is_some() != self.installation_snapshot_digest.is_some()
            || self.installation_generation == Some(0)
            || self
                .installation_snapshot_digest
                .as_ref()
                .is_some_and(|digest| !valid_canonical_sha256(digest))
            || !valid_lower_sha256(&self.revision)
            || !valid_canonical_sha256(&self.registry_revision)
            || self.packages.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.unleasable_packages.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.packages.windows(2).any(|pair| pair[0] >= pair[1])
            || self
                .unleasable_packages
                .iter()
                .any(|package_id| PluginPackageId::parse(package_id.clone()).is_err())
            || self
                .unleasable_packages
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return Err(cursor_error(
                "The capability snapshot cursor has invalid schema, digest, bounds, or ordering.",
            ));
        }
        let mut package_ids = std::collections::BTreeSet::new();
        for package in &self.packages {
            if package.lifecycle_generation == 0
                || PluginPackageId::parse(package.package_id.clone()).is_err()
                || !valid_canonical_sha256(&package.package_digest)
                || !valid_canonical_sha256(&package.manifest_digest)
                || !package_ids.insert(package.package_id.as_str())
            {
                return Err(cursor_error(
                    "The capability snapshot cursor contains an invalid or duplicate package generation.",
                ));
            }
        }
        if self.packages.iter().any(|package| {
            self.unleasable_packages
                .binary_search(&package.package_id)
                .is_ok()
        }) {
            return Err(cursor_error(
                "The capability snapshot cursor contains overlapping leasable and unleasable package identities.",
            ));
        }
        Ok(())
    }

    pub fn is_fully_leasable(&self) -> bool {
        self.unleasable_packages.is_empty()
    }
}

/// One immutable capability projection plus its complete upstream RAII lease.
pub struct CapabilitySnapshotLease {
    snapshot: Arc<CapabilityRegistrySnapshot>,
    #[cfg(feature = "extensions")]
    extension: a3s_use_extension::ExtensionSnapshotLease,
}

impl CapabilitySnapshotLease {
    pub fn snapshot(&self) -> &CapabilityRegistrySnapshot {
        &self.snapshot
    }

    pub fn cursor(&self) -> &CapabilitySnapshotCursor {
        self.snapshot.cursor()
    }

    pub fn package_count(&self) -> usize {
        #[cfg(feature = "extensions")]
        {
            self.extension.len()
        }
        #[cfg(not(feature = "extensions"))]
        {
            0
        }
    }

    #[cfg(feature = "extensions")]
    pub fn extension_lease(&self) -> &a3s_use_extension::ExtensionSnapshotLease {
        &self.extension
    }
}

impl fmt::Debug for CapabilitySnapshotLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilitySnapshotLease")
            .field("cursor", self.cursor())
            .field("package_count", &self.package_count())
            .finish()
    }
}

/// Acquire the exact complete Use generation selected by `expected`.
///
/// The projection is rebuilt before and after all package leases are held.
/// `None` means capability readiness changed, the Registry cursor became
/// stale, a generation was hidden, or lifecycle drain won the race.
pub async fn acquire_snapshot_lease(
    installation: InstallationId,
    expected: &CapabilitySnapshotCursor,
) -> UseResult<Option<CapabilitySnapshotLease>> {
    let registry = super::CapabilityRegistry::from_env(installation)?;
    acquire_snapshot_lease_from(&registry, expected).await
}

pub(super) async fn acquire_snapshot_lease_from(
    registry: &super::CapabilityRegistry,
    expected: &CapabilitySnapshotCursor,
) -> UseResult<Option<CapabilitySnapshotLease>> {
    expected.validate()?;
    if expected.installation != *registry.installation() {
        return Err(UseError::new(
            "use.capability.snapshot_scope_mismatch",
            "The capability snapshot cursor belongs to a different installation.",
        ));
    }
    let observed = registry.snapshot().await?;
    if observed.cursor() != expected {
        return Ok(None);
    }

    #[cfg(feature = "extensions")]
    let extension = {
        let extension_registry = registry.extension_registry();
        let paths = extension_registry.paths();
        let installation = crate::control_store::read_current_installation_snapshot(
            &paths.installation_state_root(),
            paths.installation(),
        )
        .await?;
        match installation.as_ref() {
            Some(installation) => {
                let extension_cursor = a3s_use_extension::ExtensionSnapshotCursor {
                    schema: a3s_use_extension::EXTENSION_SNAPSHOT_CURSOR_SCHEMA.to_owned(),
                    installation: expected.installation.clone(),
                    generation: expected.generation,
                    revision: expected.registry_revision.clone(),
                    packages: expected
                        .packages
                        .iter()
                        .map(|package| a3s_use_extension::ExtensionSnapshotPackage {
                            package_id: package.package_id.clone(),
                            lifecycle_generation: package.lifecycle_generation,
                            package_digest: package.package_digest.clone(),
                            manifest_digest: package.manifest_digest.clone(),
                        })
                        .collect(),
                    unleasable_packages: expected.unleasable_packages.clone(),
                };
                let Some(lease) = extension_registry
                    .acquire_control_snapshot(&extension_cursor, installation)
                    .await?
                else {
                    return Ok(None);
                };
                let confirmed_installation =
                    crate::control_store::read_current_installation_snapshot(
                        &paths.installation_state_root(),
                        paths.installation(),
                    )
                    .await?;
                if confirmed_installation.as_ref() != Some(installation) {
                    return Ok(None);
                }
                lease
            }
            None => {
                let state_root = paths.installation_state_root();
                if !crate::control_store::control_database_present(&state_root) {
                    return Err(UseError::new(
                        "use.capability.control_required",
                        "Capability snapshot leasing requires an initialized Control Store.",
                    )
                    .with_suggestion(
                        "Initialize Control Store for this installation before acquiring a capability lease.",
                    ));
                }
                crate::control_store::reject_legacy_authority_paths(&state_root)?;
                if !expected.packages.is_empty() {
                    return Ok(None);
                }
                let extension_cursor = a3s_use_extension::ExtensionSnapshotCursor {
                    schema: a3s_use_extension::EXTENSION_SNAPSHOT_CURSOR_SCHEMA.to_owned(),
                    installation: expected.installation.clone(),
                    generation: expected.generation,
                    revision: expected.registry_revision.clone(),
                    packages: Vec::new(),
                    unleasable_packages: expected.unleasable_packages.clone(),
                };
                let Some(lease) = extension_registry
                    .acquire_empty_control_snapshot(&extension_cursor)
                    .await?
                else {
                    return Ok(None);
                };
                let confirmed = crate::control_store::read_current_installation_snapshot(
                    &state_root,
                    paths.installation(),
                )
                .await?;
                if confirmed.is_some() {
                    return Ok(None);
                }
                lease
            }
        }
    };

    let confirmed = registry.snapshot().await?;
    if confirmed.cursor() != expected {
        return Ok(None);
    }
    Ok(Some(CapabilitySnapshotLease {
        snapshot: Arc::new(confirmed),
        #[cfg(feature = "extensions")]
        extension,
    }))
}

#[derive(Debug, Clone)]
pub(super) struct CapabilityUpstreamEvidence {
    installation: InstallationId,
    installation_generation: Option<u64>,
    installation_snapshot_digest: Option<String>,
    generation: u64,
    registry_revision: String,
    packages: Vec<CapabilityPackageGeneration>,
    unleasable_packages: Vec<String>,
}

impl CapabilityUpstreamEvidence {
    pub(super) fn installation_generation(&self) -> Option<u64> {
        self.installation_generation
    }

    pub(super) fn installation_snapshot_digest(&self) -> Option<&str> {
        self.installation_snapshot_digest.as_deref()
    }

    #[cfg(feature = "extensions")]
    pub(super) fn from_snapshot(
        snapshot: &a3s_use_extension::ExtensionRegistrySnapshot,
        installation: Option<&InstallationSnapshot>,
    ) -> UseResult<Self> {
        let cursor = snapshot.cursor()?;
        let (packages, unleasable_packages, generation, registry_revision) = match installation {
            Some(installation) => (
                control_leasable_packages(installation)?,
                Vec::new(),
                installation.generation,
                installation.descriptor_digest()?,
            ),
            None => (
                cursor
                    .packages
                    .iter()
                    .map(CapabilityPackageGeneration::from)
                    .collect(),
                cursor.unleasable_packages,
                cursor.generation,
                cursor.revision,
            ),
        };
        Ok(Self {
            installation: cursor.installation,
            installation_generation: installation.map(|snapshot| snapshot.generation),
            installation_snapshot_digest: installation
                .map(InstallationSnapshot::descriptor_digest)
                .transpose()?,
            generation,
            registry_revision,
            packages,
            unleasable_packages,
        })
    }

    #[cfg(not(feature = "extensions"))]
    pub(super) fn empty(installation: InstallationId) -> Self {
        Self {
            installation,
            installation_generation: None,
            installation_snapshot_digest: None,
            generation: 0,
            registry_revision: format!(
                "sha256:{:x}",
                Sha256::digest(b"a3s.use.extension-snapshot.empty.v1\0")
            ),
            packages: Vec::new(),
            unleasable_packages: Vec::new(),
        }
    }
}

#[cfg(feature = "extensions")]
impl From<&a3s_use_extension::ExtensionSnapshotPackage> for CapabilityPackageGeneration {
    fn from(package: &a3s_use_extension::ExtensionSnapshotPackage) -> Self {
        Self {
            package_id: package.package_id.clone(),
            lifecycle_generation: package.lifecycle_generation,
            package_digest: package.package_digest.clone(),
            manifest_digest: package.manifest_digest.clone(),
        }
    }
}

#[cfg(feature = "extensions")]
fn control_leasable_packages(
    installation: &InstallationSnapshot,
) -> UseResult<Vec<CapabilityPackageGeneration>> {
    let mut packages = Vec::new();
    for selection in installation
        .packages
        .iter()
        .filter(|selection| selection.enabled)
    {
        let record = &selection.package.catalog.record.package;
        let (Some(package_sha256), Some(manifest_sha256)) =
            (record.sha256.as_deref(), record.manifest_sha256.as_deref())
        else {
            return Err(UseError::new(
                "use.capability.snapshot_unleasable",
                "A Control-selected package is missing immutable digest evidence for capability leasing.",
            ));
        };
        packages.push(CapabilityPackageGeneration {
            package_id: selection.package_id().to_owned(),
            lifecycle_generation: selection.state_generation,
            package_digest: if package_sha256.starts_with("sha256:") {
                package_sha256.to_owned()
            } else {
                format!("sha256:{package_sha256}")
            },
            manifest_digest: if manifest_sha256.starts_with("sha256:") {
                manifest_sha256.to_owned()
            } else {
                format!("sha256:{manifest_sha256}")
            },
        });
    }
    packages.sort();
    Ok(packages)
}

fn cursor_error(message: impl Into<String>) -> UseError {
    UseError::new("use.capability.snapshot_cursor_invalid", message)
}

fn valid_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_canonical_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(valid_lower_sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor() -> CapabilitySnapshotCursor {
        CapabilitySnapshotCursor {
            schema: CAPABILITY_SNAPSHOT_CURSOR_SCHEMA.to_owned(),
            installation: InstallationId::new(
                a3s_use_core::InstallationKind::User,
                "capability-tests",
            )
            .unwrap(),
            installation_generation: Some(5),
            installation_snapshot_digest: Some(format!("sha256:{}", "e".repeat(64))),
            generation: 7,
            revision: "a".repeat(64),
            registry_revision: format!("sha256:{}", "b".repeat(64)),
            packages: vec![CapabilityPackageGeneration {
                package_id: "acme/guide".to_owned(),
                lifecycle_generation: 11,
                package_digest: format!("sha256:{}", "c".repeat(64)),
                manifest_digest: format!("sha256:{}", "d".repeat(64)),
            }],
            unleasable_packages: Vec::new(),
        }
    }

    #[test]
    fn capability_cursor_is_strictly_validated() {
        let canonical = cursor();
        canonical.validate().unwrap();
        assert!(canonical.is_fully_leasable());

        let mut duplicate = canonical.clone();
        duplicate.packages.push(duplicate.packages[0].clone());
        let error = duplicate.validate().unwrap_err();
        assert_eq!(error.code, "use.capability.snapshot_cursor_invalid");

        let mut malformed = canonical;
        malformed.registry_revision = "sha256:ABC".to_owned();
        assert_eq!(
            malformed.validate().unwrap_err().code,
            "use.capability.snapshot_cursor_invalid"
        );

        let mut missing_installation_digest = cursor();
        missing_installation_digest.installation_snapshot_digest = None;
        assert_eq!(
            missing_installation_digest.validate().unwrap_err().code,
            "use.capability.snapshot_cursor_invalid"
        );

        let mut malformed_installation_digest = cursor();
        malformed_installation_digest.installation_snapshot_digest = Some("sha256:ABC".to_owned());
        assert_eq!(
            malformed_installation_digest.validate().unwrap_err().code,
            "use.capability.snapshot_cursor_invalid"
        );

        let mut empty_package = cursor();
        empty_package.unleasable_packages.push(String::new());
        assert_eq!(
            empty_package.validate().unwrap_err().code,
            "use.capability.snapshot_cursor_invalid"
        );

        let mut duplicate_package = cursor();
        duplicate_package.unleasable_packages =
            vec!["acme/legacy".to_owned(), "acme/legacy".to_owned()];
        assert_eq!(
            duplicate_package.validate().unwrap_err().code,
            "use.capability.snapshot_cursor_invalid"
        );

        let mut unbounded_packages = cursor();
        unbounded_packages.unleasable_packages = (0..=MAX_PLUGIN_PLAN_ITEMS)
            .map(|index| format!("acme/package-{index:05}"))
            .collect();
        assert_eq!(
            unbounded_packages.validate().unwrap_err().code,
            "use.capability.snapshot_cursor_invalid"
        );
    }

    #[test]
    fn internal_cursor_is_omitted_from_capability_snapshot_v5_json() {
        let cursor = cursor();
        let snapshot = CapabilityRegistrySnapshot {
            schema_version: super::super::CAPABILITY_REGISTRY_SCHEMA_VERSION,
            installation: cursor.installation.clone(),
            installation_generation: cursor.installation_generation,
            installation_snapshot_digest: cursor.installation_snapshot_digest.clone(),
            generation: cursor.generation,
            revision: cursor.revision.clone(),
            capabilities: vec![super::super::product_seeds::box_capability()],
            cursor,
        };
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(json["schemaVersion"], 5);
        assert_eq!(json["installationGeneration"], 5);
        assert_eq!(json["generation"], 7);
        assert!(json.get("cursor").is_none());
    }

    #[test]
    fn snapshot_cursor_and_lease_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<CapabilitySnapshotCursor>();
        assert_send_sync::<CapabilitySnapshotLease>();
    }
    #[cfg(feature = "extensions")]
    #[test]
    fn injected_registry_acquires_one_exact_use_snapshot_lease() {
        std::thread::Builder::new()
            .name("capability-snapshot-lease".to_owned())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(injected_registry_snapshot_lease());
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[cfg(feature = "extensions")]
    async fn injected_registry_snapshot_lease() {
        let temporary = tempfile::tempdir().unwrap();
        let installation =
            InstallationId::new(a3s_use_core::InstallationKind::Workspace, "lease-tests").unwrap();
        let paths = a3s_use_extension::ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation.clone(),
        )
        .unwrap();
        crate::cognitive_package::open_control_lifecycle(
            &paths,
            std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
            None,
        )
        .await
        .unwrap();
        let extension_registry = a3s_use_extension::ExtensionRegistry::new(paths);
        let registry = super::super::CapabilityRegistry::new(extension_registry.clone());
        let snapshot = registry.snapshot().await.unwrap();
        assert!(snapshot.capabilities.is_empty());
        assert!(snapshot.cursor().packages.is_empty());

        let other_installation =
            InstallationId::new(a3s_use_core::InstallationKind::User, "lease-tests").unwrap();
        let other_registry =
            super::super::CapabilityRegistry::new(a3s_use_extension::ExtensionRegistry::new(
                a3s_use_extension::ExtensionPaths::new(
                    temporary.path().join("data"),
                    temporary.path().join("state"),
                    other_installation,
                )
                .unwrap(),
            ));
        assert_eq!(
            other_registry
                .acquire_snapshot_lease(snapshot.cursor())
                .await
                .unwrap_err()
                .code,
            "use.capability.snapshot_scope_mismatch"
        );

        let lease = registry
            .acquire_snapshot_lease(snapshot.cursor())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lease.package_count(), 0);
        assert_eq!(lease.cursor(), snapshot.cursor());

        // Empty Control remains authoritative: a second lease for the same
        // cursor still resolves while the Control-backed snapshot is current.
        let again = registry
            .acquire_snapshot_lease(snapshot.cursor())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again.package_count(), 0);
        assert_eq!(again.cursor(), snapshot.cursor());
    }

    #[cfg(feature = "extensions")]
    #[test]
    fn snapshot_lease_fails_closed_without_control_store() {
        std::thread::Builder::new()
            .name("capability-snapshot-lease-no-control".to_owned())
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async {
                        let temporary = tempfile::tempdir().unwrap();
                        let installation = InstallationId::new(
                            a3s_use_core::InstallationKind::Workspace,
                            "lease-no-control",
                        )
                        .unwrap();
                        let paths = a3s_use_extension::ExtensionPaths::new(
                            temporary.path().join("data"),
                            temporary.path().join("state"),
                            installation,
                        )
                        .unwrap();
                        std::fs::create_dir_all(paths.state_root()).unwrap();
                        let extension_registry =
                            a3s_use_extension::ExtensionRegistry::new(paths);
                        let registry =
                            super::super::CapabilityRegistry::new(extension_registry);
                        let error = registry.snapshot().await.unwrap_err();
                        assert_eq!(error.code, "use.capability.control_required");
                    });
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
