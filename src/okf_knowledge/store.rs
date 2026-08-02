use std::path::{Path, PathBuf};

use a3s_use_core::{
    OkfCapabilityProjection, OkfKnowledgeObservedState, OkfSelectedGeneration,
    PlanQualifiedSurfaceRef, PluginPackageId, PluginSurfaceKind, UseError, UseResult,
};
use a3s_use_extension::ExtensionPaths;
use sha2::{Digest, Sha256};

use super::OkfKnowledgeBinding;

mod io;

use io::{
    acquire_lock, binding_path, ensure_owned_directory, read_bindings, read_optional_binding,
    validate_existing_directory_chain, write_binding,
};

pub const MAX_OKF_KNOWLEDGE_GENERATIONS: usize = 32;

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

    pub async fn put(&self, binding: &OkfKnowledgeBinding) -> UseResult<bool> {
        binding.validate()?;
        let _lock = acquire_lock(&self.state_root, &self.root).await?;
        let directory =
            self.surface_directory(&binding.receipt.scope_id, &binding.receipt.surface)?;
        ensure_owned_directory(&self.state_root, Some(&directory)).await?;
        let mut records = read_bindings(
            &directory,
            &binding.receipt.scope_id,
            &binding.receipt.surface,
        )
        .await?;

        let generation = binding.receipt.generation;
        if let Some(position) = records
            .iter()
            .position(|record| record.receipt.generation == generation)
        {
            if records[position] == *binding {
                return Ok(false);
            }
            if records.last().map(|record| record.receipt.generation) != Some(generation) {
                return Err(stale_error(
                    "An older OKF Knowledge generation cannot change after a newer candidate exists.",
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
        scope_id: &str,
        surface: &PlanQualifiedSurfaceRef,
        generation: u64,
    ) -> UseResult<Option<OkfKnowledgeBinding>> {
        if generation == 0 {
            return Err(invalid_path_identity());
        }
        let directory = self.surface_directory(scope_id, surface)?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(None);
        }
        let path = binding_path(&directory, generation);
        let Some(binding) = read_optional_binding(&path).await? else {
            return Ok(None);
        };
        validate_ownership(&binding, scope_id, surface, generation)?;
        Ok(Some(binding))
    }

    pub async fn snapshot(
        &self,
        scope_id: &str,
        surface: &PlanQualifiedSurfaceRef,
    ) -> UseResult<OkfKnowledgeBindingSnapshot> {
        let directory = self.surface_directory(scope_id, surface)?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(OkfKnowledgeBindingSnapshot::default());
        }
        let records = read_bindings(&directory, scope_id, surface).await?;
        snapshot_from_records(&records)
    }

    fn surface_directory(
        &self,
        scope_id: &str,
        surface: &PlanQualifiedSurfaceRef,
    ) -> UseResult<PathBuf> {
        validate_path_identity(scope_id, surface)?;
        let package_id = PluginPackageId::parse(surface.package_id.clone())?;
        let (publisher, package) = package_id
            .as_str()
            .split_once('/')
            .ok_or_else(invalid_path_identity)?;
        let scope_digest = format!("{:x}", Sha256::digest(scope_id.as_bytes()));
        Ok(self
            .root
            .join(scope_digest)
            .join(publisher)
            .join(package)
            .join(format!("okf-{}", surface.surface.id)))
    }
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

fn snapshot_from_records(
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

fn validate_path_identity(scope_id: &str, surface: &PlanQualifiedSurfaceRef) -> UseResult<()> {
    if !valid_machine_id(scope_id)
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
    scope_id: &str,
    surface: &PlanQualifiedSurfaceRef,
    generation: u64,
) -> UseResult<()> {
    if binding.receipt.scope_id != scope_id
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
