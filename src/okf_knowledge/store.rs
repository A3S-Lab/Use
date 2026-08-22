use std::path::{Path, PathBuf};

use a3s_use_core::{
    OkfCapabilityProjection, OkfKnowledgeObservedState, OkfSelectedGeneration,
    PlanQualifiedSurfaceRef, PlanScope, PluginPackageId, PluginSurfaceKind, UseError, UseResult,
};
use a3s_use_extension::ExtensionPaths;
use sha2::{Digest, Sha256};

use super::OkfKnowledgeBinding;
use a3s_use_extension::{StateMaintenanceGuard, StateMaintenanceLock};

mod io;

use io::{
    acquire_lock, binding_path, ensure_owned_directory, read_bindings, read_optional_binding,
    remove_binding, validate_existing_directory_chain, write_binding,
};

pub const MAX_OKF_KNOWLEDGE_GENERATIONS: usize = 32;
const MAX_OKF_SCOPE_PUBLISHERS: usize = 512;
const MAX_OKF_SCOPE_PACKAGES: usize = 4_096;
const MAX_OKF_SCOPE_SURFACES: usize = 4_096;

/// Active OKF selection reconstructed exclusively from retained exact records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OkfKnowledgeBindingSnapshot {
    pub latest: Option<OkfKnowledgeBinding>,
    pub selected: Option<OkfKnowledgeBinding>,
    pub projection: Option<OkfCapabilityProjection>,
}

/// Durable store for receipt/observation pairs across Knowledge generations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfKnowledgeBindingStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl OkfKnowledgeBindingStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("bindings").join("knowledge"),
            state_root,
        }
    }

    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::new(paths.state_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub async fn put(&self, binding: &OkfKnowledgeBinding) -> UseResult<bool> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        binding.validate()?;
        let _lock = acquire_lock(&self.state_root, &self.root).await?;
        let directory = self.surface_directory(&binding.receipt.scope, &binding.receipt.surface)?;
        ensure_owned_directory(&self.state_root, Some(&directory)).await?;
        let mut records =
            read_bindings(&directory, &binding.receipt.scope, &binding.receipt.surface).await?;

        let generation = binding.receipt.generation;
        if let Some(position) = records
            .iter()
            .position(|record| record.receipt.generation == generation)
        {
            if records[position] == *binding {
                return Ok(false);
            }
            if records.last().map(|record| record.receipt.generation) != Some(generation)
                && binding.observation.state != OkfKnowledgeObservedState::Removed
            {
                return Err(stale_error(
                    "An older OKF Knowledge generation cannot change after a newer candidate exists unless receipt-owned cleanup marks it removed.",
                ));
            }
            validate_replacement(&records[position], binding)?;
            records[position] = binding.clone();
        } else {
            if records
                .last()
                .is_some_and(|record| record.receipt.generation >= generation)
            {
                return Err(stale_error(
                    "A stale OKF Knowledge generation cannot enter the binding store.",
                ));
            }
            if records.len() >= MAX_OKF_KNOWLEDGE_GENERATIONS {
                let latest_generation = records
                    .last()
                    .map(|record| record.receipt.generation)
                    .ok_or_else(selection_error)?;
                let mut removed = Vec::new();
                records.retain(|record| {
                    let prune = record.observation.state == OkfKnowledgeObservedState::Removed
                        && record.receipt.generation != latest_generation;
                    if prune {
                        removed.push(record.receipt.generation);
                    }
                    !prune
                });
                snapshot_from_records(&records)?;
                for generation in removed {
                    remove_binding(&binding_path(&directory, generation)).await?;
                }
            }
            if records.len() >= MAX_OKF_KNOWLEDGE_GENERATIONS {
                return Err(store_error(
                    "use.okf.knowledge_binding_limit_exceeded",
                    format!(
                        "The OKF Knowledge binding reached its retained-generation limit of {MAX_OKF_KNOWLEDGE_GENERATIONS}; receipt-owned cleanup is required before staging another generation."
                    ),
                ));
            }
            records.push(binding.clone());
        }
        snapshot_from_records(&records)?;

        let path = binding_path(&directory, generation);
        write_binding(&path, binding).await?;
        Ok(true)
    }

    pub async fn get(
        &self,
        scope: &PlanScope,
        surface: &PlanQualifiedSurfaceRef,
        generation: u64,
    ) -> UseResult<Option<OkfKnowledgeBinding>> {
        if generation == 0 {
            return Err(invalid_path_identity());
        }
        let directory = self.surface_directory(scope, surface)?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(None);
        }
        let path = binding_path(&directory, generation);
        let Some(binding) = read_optional_binding(&path).await? else {
            return Ok(None);
        };
        validate_ownership(&binding, scope, surface, generation)?;
        Ok(Some(binding))
    }

    pub async fn snapshot(
        &self,
        scope: &PlanScope,
        surface: &PlanQualifiedSurfaceRef,
    ) -> UseResult<OkfKnowledgeBindingSnapshot> {
        let directory = self.surface_directory(scope, surface)?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(OkfKnowledgeBindingSnapshot::default());
        }
        let records = read_bindings(&directory, scope, surface).await?;
        snapshot_from_records(&records)
    }

    pub(crate) async fn list_scope(
        &self,
        scope: &PlanScope,
    ) -> UseResult<Vec<OkfKnowledgeBinding>> {
        if !valid_machine_id(&scope.id) {
            return Err(invalid_path_identity());
        }
        let _lock = acquire_lock(&self.state_root, &self.root).await?;
        self.list_scope_unlocked(scope).await
    }

    /// Recreate only missing exact binding records from a fully validated
    /// recovery inventory. The caller must retain the global exclusive state
    /// maintenance guard and a durable active-restore marker while this method
    /// runs. Existing records are never replaced or removed.
    pub(crate) async fn restore_exact_inventory(
        &self,
        scope: &PlanScope,
        expected: &[OkfKnowledgeBinding],
        _maintenance: &StateMaintenanceGuard,
    ) -> UseResult<usize> {
        validate_recovery_inventory(scope, expected)?;
        let _lock = acquire_lock(&self.state_root, &self.root).await?;
        let current = self.list_scope_unlocked(scope).await?;
        let missing = missing_exact_bindings(&current, expected)?;
        let restored = missing.len();
        for binding in missing {
            let directory =
                self.surface_directory(&binding.receipt.scope, &binding.receipt.surface)?;
            ensure_owned_directory(&self.state_root, Some(&directory)).await?;
            let path = binding_path(&directory, binding.receipt.generation);
            if read_optional_binding(&path).await?.is_some() {
                return Err(recovery_conflict());
            }
            write_binding(&path, binding).await?;
        }
        let recovered = self.list_scope_unlocked(scope).await?;
        if recovered != expected {
            return Err(recovery_conflict());
        }
        Ok(restored)
    }

    async fn list_scope_unlocked(&self, scope: &PlanScope) -> UseResult<Vec<OkfKnowledgeBinding>> {
        let scope_digest = format!("{:x}", Sha256::digest(scope.id.as_bytes()));
        let scope_root = self.root.join(scope.kind.as_str()).join(scope_digest);
        if !validate_existing_directory_chain(&self.state_root, Some(&scope_root)).await? {
            return Ok(Vec::new());
        }

        let mut bindings = Vec::new();
        let mut publisher_count = 0_usize;
        let mut package_count = 0_usize;
        let mut surface_count = 0_usize;
        let mut publishers = read_directory(&scope_root, "scope").await?;
        while let Some(publisher) = next_directory(&mut publishers, &scope_root).await? {
            publisher_count = publisher_count.saturating_add(1);
            enforce_scope_bound(
                publisher_count,
                MAX_OKF_SCOPE_PUBLISHERS,
                "publisher directories",
            )?;
            let publisher_name = portable_name(&publisher)?;
            let publisher_path = publisher.path();
            let mut packages = read_directory(&publisher_path, "publisher").await?;
            while let Some(package) = next_directory(&mut packages, &publisher_path).await? {
                package_count = package_count.saturating_add(1);
                enforce_scope_bound(package_count, MAX_OKF_SCOPE_PACKAGES, "package directories")?;
                let package_name = portable_name(&package)?;
                let package_id = format!("{publisher_name}/{package_name}");
                PluginPackageId::parse(package_id.clone()).map_err(|_| invalid_path_identity())?;
                let package_path = package.path();
                let mut surfaces = read_directory(&package_path, "package").await?;
                while let Some(surface) = next_directory(&mut surfaces, &package_path).await? {
                    surface_count = surface_count.saturating_add(1);
                    enforce_scope_bound(
                        surface_count,
                        MAX_OKF_SCOPE_SURFACES,
                        "surface directories",
                    )?;
                    let surface_name = portable_name(&surface)?;
                    let surface_id = surface_name
                        .strip_prefix("okf-")
                        .filter(|value| valid_segment(value))
                        .ok_or_else(invalid_path_identity)?;
                    let qualified = PlanQualifiedSurfaceRef {
                        package_id: package_id.clone(),
                        surface: a3s_use_core::PluginSurfaceRef {
                            kind: PluginSurfaceKind::Okf,
                            id: surface_id.to_owned(),
                        },
                    };
                    bindings.extend(read_bindings(&surface.path(), scope, &qualified).await?);
                }
            }
        }
        bindings.sort_by(|left, right| {
            left.receipt
                .surface
                .cmp(&right.receipt.surface)
                .then_with(|| left.receipt.generation.cmp(&right.receipt.generation))
        });
        Ok(bindings)
    }

    fn surface_directory(
        &self,
        scope: &PlanScope,
        surface: &PlanQualifiedSurfaceRef,
    ) -> UseResult<PathBuf> {
        validate_path_identity(scope, surface)?;
        let package_id = PluginPackageId::parse(surface.package_id.clone())?;
        let (publisher, package) = package_id
            .as_str()
            .split_once('/')
            .ok_or_else(invalid_path_identity)?;
        let scope_digest = format!("{:x}", Sha256::digest(scope.id.as_bytes()));
        Ok(self
            .root
            .join(scope.kind.as_str())
            .join(scope_digest)
            .join(publisher)
            .join(package)
            .join(format!("okf-{}", surface.surface.id)))
    }
}

async fn read_directory(path: &Path, label: &str) -> UseResult<tokio::fs::ReadDir> {
    tokio::fs::read_dir(path).await.map_err(|error| {
        path_error(
            &format!("read OKF Knowledge {label} directory"),
            path,
            error,
        )
    })
}

async fn next_directory(
    entries: &mut tokio::fs::ReadDir,
    parent: &Path,
) -> UseResult<Option<tokio::fs::DirEntry>> {
    let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read OKF Knowledge binding layout", parent, error))?
    else {
        return Ok(None);
    };
    let metadata = tokio::fs::symlink_metadata(entry.path())
        .await
        .map_err(|error| {
            path_error(
                "inspect OKF Knowledge binding directory",
                &entry.path(),
                error,
            )
        })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(invalid_path_identity());
    }
    Ok(Some(entry))
}

fn portable_name(entry: &tokio::fs::DirEntry) -> UseResult<String> {
    entry
        .file_name()
        .into_string()
        .map_err(|_| invalid_path_identity())
}

fn enforce_scope_bound(count: usize, maximum: usize, label: &str) -> UseResult<()> {
    if count > maximum {
        return Err(store_error(
            "use.okf.knowledge_binding_limit_exceeded",
            format!("The OKF Knowledge scope exceeds its {label} bound of {maximum}."),
        ));
    }
    Ok(())
}

fn validate_replacement(
    current: &OkfKnowledgeBinding,
    next: &OkfKnowledgeBinding,
) -> UseResult<()> {
    if current.receipt != next.receipt {
        return Err(conflict_error(
            "One OKF Knowledge generation cannot replace its immutable projection receipt.",
        ));
    }
    if next.observation.observed_at_ms <= current.observation.observed_at_ms {
        return Err(stale_error(
            "An OKF Knowledge observation must advance its observation timestamp.",
        ));
    }

    use OkfKnowledgeObservedState::{Failed, Promoted, Removed, Staged};
    let current_observation = &current.observation;
    let next_observation = &next.observation;
    let allowed = match (current_observation.state, next_observation.state) {
        (state, next_state) if state == next_state => {
            current_observation.index_digest == next_observation.index_digest
                && current_observation.selected == next_observation.selected
        }
        (Staged, Promoted) => current_observation.index_digest == next_observation.index_digest,
        (Staged, Failed) => {
            current_observation.index_digest == next_observation.index_digest
                && current_observation.selected == next_observation.selected
        }
        (Staged | Failed | Promoted, Removed) => true,
        _ => false,
    };
    if !allowed {
        return Err(conflict_error(
            "The OKF Knowledge observation transition conflicts with retained generation evidence.",
        ));
    }
    Ok(())
}

pub(super) fn snapshot_from_records(
    records: &[OkfKnowledgeBinding],
) -> UseResult<OkfKnowledgeBindingSnapshot> {
    let Some(latest) = records.last() else {
        return Ok(OkfKnowledgeBindingSnapshot::default());
    };
    if latest.observation.state == OkfKnowledgeObservedState::Removed {
        return Ok(OkfKnowledgeBindingSnapshot {
            latest: Some(latest.clone()),
            selected: None,
            projection: None,
        });
    }
    let Some(selected_evidence) = latest.observation.selected.as_ref() else {
        return Ok(OkfKnowledgeBindingSnapshot {
            latest: Some(latest.clone()),
            selected: None,
            projection: None,
        });
    };
    let selected = records
        .iter()
        .find(|record| record.receipt.generation == selected_evidence.generation)
        .ok_or_else(selection_error)?;
    validate_selected_binding(selected_evidence, selected)?;
    let projection =
        OkfCapabilityProjection::from_promoted(&selected.receipt, &selected.observation)?;
    Ok(OkfKnowledgeBindingSnapshot {
        latest: Some(latest.clone()),
        selected: Some(selected.clone()),
        projection: Some(projection),
    })
}

fn validate_recovery_inventory(
    scope: &PlanScope,
    records: &[OkfKnowledgeBinding],
) -> UseResult<()> {
    let mut previous: Option<(PlanQualifiedSurfaceRef, u64)> = None;
    let mut start = 0_usize;
    while start < records.len() {
        let surface = records[start].receipt.surface.clone();
        let mut end = start;
        while end < records.len() && records[end].receipt.surface == surface {
            let binding = &records[end];
            binding.validate()?;
            validate_ownership(binding, scope, &surface, binding.receipt.generation)?;
            let key = (surface.clone(), binding.receipt.generation);
            if previous.as_ref().is_some_and(|previous| previous >= &key) {
                return Err(recovery_conflict());
            }
            previous = Some(key);
            end += 1;
        }
        if end - start > MAX_OKF_KNOWLEDGE_GENERATIONS {
            return Err(store_error(
                "use.okf.knowledge_binding_limit_exceeded",
                "The recovery inventory exceeds the retained-generation bound for one Knowledge surface.",
            ));
        }
        snapshot_from_records(&records[start..end])?;
        start = end;
    }
    Ok(())
}

fn missing_exact_bindings<'a>(
    current: &[OkfKnowledgeBinding],
    expected: &'a [OkfKnowledgeBinding],
) -> UseResult<Vec<&'a OkfKnowledgeBinding>> {
    let expected_by_key = expected
        .iter()
        .map(|binding| {
            (
                (binding.receipt.surface.clone(), binding.receipt.generation),
                binding,
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    if expected_by_key.len() != expected.len() {
        return Err(recovery_conflict());
    }
    let mut retained = std::collections::BTreeSet::new();
    for binding in current {
        let key = (binding.receipt.surface.clone(), binding.receipt.generation);
        if expected_by_key.get(&key).copied() != Some(binding) || !retained.insert(key) {
            return Err(recovery_conflict());
        }
    }
    Ok(expected_by_key
        .into_iter()
        .filter_map(|(key, binding)| (!retained.contains(&key)).then_some(binding))
        .collect())
}

fn validate_selected_binding(
    selected: &OkfSelectedGeneration,
    binding: &OkfKnowledgeBinding,
) -> UseResult<()> {
    let receipt = &binding.receipt;
    let observation = &binding.observation;
    if observation.state != OkfKnowledgeObservedState::Promoted
        || selected.generation != receipt.generation
        || selected.package_digest != receipt.package_digest
        || selected.bundle_digest != receipt.bundle.content_digest
        || selected.projection_receipt_digest != receipt.descriptor_digest()?
        || selected.index_schema != receipt.index_schema
        || selected.index_build_id != receipt.index_build_id
        || observation.index_digest.as_deref() != Some(selected.index_digest.as_str())
    {
        return Err(selection_error());
    }
    Ok(())
}

fn validate_path_identity(scope: &PlanScope, surface: &PlanQualifiedSurfaceRef) -> UseResult<()> {
    if !valid_machine_id(&scope.id)
        || PluginPackageId::parse(surface.package_id.clone()).is_err()
        || surface.surface.kind != PluginSurfaceKind::Okf
        || !valid_segment(&surface.surface.id)
    {
        return Err(invalid_path_identity());
    }
    Ok(())
}

fn validate_ownership(
    binding: &OkfKnowledgeBinding,
    scope: &PlanScope,
    surface: &PlanQualifiedSurfaceRef,
    generation: u64,
) -> UseResult<()> {
    if binding.receipt.scope != *scope
        || binding.receipt.surface != *surface
        || binding.receipt.generation != generation
    {
        return Err(store_error(
            "use.okf.knowledge_binding_ownership_mismatch",
            "An OKF Knowledge binding does not match its scope, surface, and generation path.",
        ));
    }
    Ok(())
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

fn selection_error() -> UseError {
    store_error(
        "use.okf.knowledge_binding_selection_invalid",
        "The latest OKF Knowledge observation does not select an exact retained promoted generation.",
    )
}

fn stale_error(message: impl Into<String>) -> UseError {
    store_error("use.okf.knowledge_binding_stale", message)
}

fn conflict_error(message: impl Into<String>) -> UseError {
    store_error("use.okf.knowledge_binding_conflict", message)
}

fn recovery_conflict() -> UseError {
    store_error(
        "use.okf.knowledge_binding_recovery_conflict",
        "The current Knowledge binding inventory is not an exact subset of the reviewed recovery inventory.",
    )
}

fn record_error(message: impl Into<String>) -> UseError {
    store_error("use.okf.knowledge_binding_record_invalid", message)
}

fn invalid_path_identity() -> UseError {
    store_error(
        "use.okf.knowledge_binding_path_invalid",
        "An OKF Knowledge binding scope, surface, generation, or owned path is invalid.",
    )
}

fn path_error(action: &str, path: &Path, error: std::io::Error) -> UseError {
    store_error(
        "use.okf.knowledge_binding_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

fn store_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
