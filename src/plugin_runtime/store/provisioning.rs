use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{PlanQualifiedSurfaceRef, PlanScope, UseResult};
use serde::Serialize;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::plugin_runtime::{
    RuntimeBindingReceipt, RuntimeServiceProvisioningPhase, RuntimeServiceProvisioningReceipt,
};

use super::{
    binding_inventory, binding_path, ensure_owned_directory, invalid_path_identity, path_error,
    read_optional_receipt, store_error, sync_parent, unique_suffix,
    validate_existing_directory_chain, validate_ownership, write_receipt, RuntimeBindingStore,
    MAX_BINDING_RECEIPT_BYTES, MAX_RUNTIME_BINDING_GENERATIONS,
};

impl RuntimeBindingStore {
    /// Persist one exact Service provisioning phase before a package
    /// lifecycle checkpoint can proceed.
    pub async fn put_provisioning(
        &self,
        receipt: &RuntimeServiceProvisioningReceipt,
    ) -> UseResult<bool> {
        receipt.validate()?;
        let _lock = self.acquire_lock().await?;
        let directory = self.surface_directory(&receipt.scope, &receipt.surface)?;
        ensure_owned_directory(&self.root, Some(&directory)).await?;
        let inventory = binding_inventory(&directory).await?;
        let path = provisioning_path(&directory, receipt.generation);
        if let Some(current) = read_optional_provisioning(&path).await? {
            validate_provisioning_ownership(
                &current,
                &receipt.scope,
                &receipt.surface,
                receipt.generation,
            )?;
            if current == *receipt {
                return Ok(false);
            }
            validate_provisioning_transition(&current, receipt)?;
        } else {
            if !inventory.contains(receipt.generation)
                && inventory.len() >= MAX_RUNTIME_BINDING_GENERATIONS
            {
                return Err(super::generation_limit_error());
            }
            if read_optional_receipt(&binding_path(&directory, receipt.generation))
                .await?
                .is_some()
            {
                return Err(provisioning_error(
                    "use.plugin.runtime.provisioning_conflict",
                    "A final Runtime binding already owns this Service generation.",
                ));
            }
        }
        write_provisioning(&path, receipt).await?;
        Ok(true)
    }

    pub async fn get_provisioning(
        &self,
        scope: &PlanScope,
        surface: &PlanQualifiedSurfaceRef,
        generation: u64,
    ) -> UseResult<Option<RuntimeServiceProvisioningReceipt>> {
        if generation == 0 {
            return Err(invalid_path_identity());
        }
        let directory = self.surface_directory(scope, surface)?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(None);
        }
        let path = provisioning_path(&directory, generation);
        let Some(receipt) = read_optional_provisioning(&path).await? else {
            return Ok(None);
        };
        validate_provisioning_ownership(&receipt, scope, surface, generation)?;
        Ok(Some(receipt))
    }

    pub async fn remove_provisioning(
        &self,
        expected: &RuntimeServiceProvisioningReceipt,
    ) -> UseResult<bool> {
        expected.validate()?;
        let _lock = self.acquire_lock().await?;
        let directory = self.surface_directory(&expected.scope, &expected.surface)?;
        if !validate_existing_directory_chain(&self.state_root, Some(&directory)).await? {
            return Ok(false);
        }
        let path = provisioning_path(&directory, expected.generation);
        let Some(current) = read_optional_provisioning(&path).await? else {
            return Ok(false);
        };
        if current != *expected {
            return Err(provisioning_error(
                "use.plugin.runtime.provisioning_ownership_changed",
                "The Runtime Service provisioning evidence changed before removal and was preserved.",
            ));
        }
        fs::remove_file(&path)
            .await
            .map_err(|error| path_error("remove Runtime provisioning receipt", &path, error))?;
        sync_parent(path.parent()).await?;
        Ok(true)
    }

    /// Commit a Gateway-ready provisioning record into the final binding
    /// receipt. The final receipt is synced before the pending record is
    /// removed, so a crash can leave both records but can never leave neither.
    pub async fn commit_provisioning(
        &self,
        expected: &RuntimeServiceProvisioningReceipt,
        binding: &RuntimeBindingReceipt,
    ) -> UseResult<bool> {
        expected.validate()?;
        binding.validate()?;
        let RuntimeBindingReceipt::Service(service) = binding else {
            return Err(provisioning_error(
                "use.plugin.runtime.provisioning_conflict",
                "A Runtime Service provisioning receipt cannot commit a Task binding.",
            ));
        };
        if expected.phase != RuntimeServiceProvisioningPhase::GatewayReady
            || expected.binding_receipt()? != *service
        {
            return Err(provisioning_error(
                "use.plugin.runtime.provisioning_conflict",
                "The final Runtime binding does not match the exact Gateway-ready provisioning evidence.",
            ));
        }

        let _lock = self.acquire_lock().await?;
        let directory = self.surface_directory(&expected.scope, &expected.surface)?;
        ensure_owned_directory(&self.root, Some(&directory)).await?;
        let pending_path = provisioning_path(&directory, expected.generation);
        let binding_path = binding_path(&directory, expected.generation);
        let current_pending = read_optional_provisioning(&pending_path).await?;
        let current_binding = read_optional_receipt(&binding_path).await?;

        if let Some(current) = &current_binding {
            validate_ownership(
                current,
                binding.scope(),
                binding.surface(),
                binding.generation(),
            )?;
            if current != binding {
                return Err(store_error(
                    "use.plugin.runtime.binding_conflict",
                    "A Runtime binding generation has conflicting immutable content.",
                ));
            }
        }

        match current_pending {
            Some(current) if current == *expected => {}
            Some(_) => {
                return Err(provisioning_error(
                    "use.plugin.runtime.provisioning_ownership_changed",
                    "The Runtime Service provisioning evidence changed before commit and was preserved.",
                ))
            }
            None if current_binding.as_ref().is_some_and(|current| current == binding) => {
                return Ok(false)
            }
            None => {
                return Err(provisioning_error(
                    "use.plugin.runtime.provisioning_missing",
                    "The exact Runtime Service provisioning evidence is missing.",
                ))
            }
        }

        if current_binding.is_none() {
            write_receipt(&binding_path, binding).await?;
        }
        fs::remove_file(&pending_path).await.map_err(|error| {
            path_error(
                "remove committed Runtime provisioning receipt",
                &pending_path,
                error,
            )
        })?;
        sync_parent(pending_path.parent()).await?;
        Ok(true)
    }
}

fn validate_provisioning_transition(
    current: &RuntimeServiceProvisioningReceipt,
    next: &RuntimeServiceProvisioningReceipt,
) -> UseResult<()> {
    let same_identity = current.schema == next.schema
        && current.surface == next.surface
        && current.package_digest == next.package_digest
        && current.scope == next.scope
        && current.grant_digest == next.grant_digest
        && current.descriptor_digest == next.descriptor_digest
        && current.provider_id == next.provider_id
        && current.provider_build_id == next.provider_build_id
        && current.capability_digest == next.capability_digest
        && current.enforcement == next.enforcement
        && current.unit_id == next.unit_id
        && current.generation == next.generation
        && current.spec_digest == next.spec_digest
        && current.semantics_profile_digest == next.semantics_profile_digest
        && current.contract == next.contract
        && current.lifecycle_idempotency_key == next.lifecycle_idempotency_key
        && current.apply_request_id == next.apply_request_id;
    let monotonic_phase = next.phase > current.phase
        || next.phase == RuntimeServiceProvisioningPhase::RuntimeApplied
            && current.phase == RuntimeServiceProvisioningPhase::RuntimeApplied
            && next
                .observation
                .as_ref()
                .zip(current.observation.as_ref())
                .is_some_and(|(next, current)| next.observed_at_ms >= current.observed_at_ms);
    if !same_identity || !monotonic_phase {
        return Err(provisioning_error(
            "use.plugin.runtime.provisioning_conflict",
            "A Runtime Service provisioning generation has conflicting or regressed evidence.",
        ));
    }
    Ok(())
}

fn validate_provisioning_ownership(
    receipt: &RuntimeServiceProvisioningReceipt,
    scope: &PlanScope,
    surface: &PlanQualifiedSurfaceRef,
    generation: u64,
) -> UseResult<()> {
    if receipt.scope != *scope || receipt.surface != *surface || receipt.generation != generation {
        return Err(provisioning_error(
            "use.plugin.runtime.provisioning_ownership_mismatch",
            "A Runtime Service provisioning receipt does not match its scope, surface, and generation path.",
        ));
    }
    Ok(())
}

pub(super) fn provisioning_path(directory: &Path, generation: u64) -> PathBuf {
    directory.join(format!("{generation:020}.provisioning.json"))
}

pub(super) async fn read_optional_provisioning(
    path: &Path,
) -> UseResult<Option<RuntimeServiceProvisioningReceipt>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(path_error(
                "inspect Runtime provisioning receipt",
                path,
                error,
            ))
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_BINDING_RECEIPT_BYTES
    {
        return Err(provisioning_error(
            "use.plugin.runtime.provisioning_receipt_invalid",
            format!(
                "Runtime provisioning receipt '{}' is not a bounded regular file.",
                path.display()
            ),
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read Runtime provisioning receipt", path, error))?;
    let receipt =
        serde_json::from_slice::<RuntimeServiceProvisioningReceipt>(&bytes).map_err(|error| {
            provisioning_error(
                "use.plugin.runtime.provisioning_receipt_invalid",
                format!(
                    "Runtime provisioning receipt '{}' is invalid JSON: {error}",
                    path.display()
                ),
            )
        })?;
    receipt.validate()?;
    Ok(Some(receipt))
}

async fn write_provisioning(
    path: &Path,
    receipt: &RuntimeServiceProvisioningReceipt,
) -> UseResult<()> {
    write_bounded_json(path, receipt).await
}

async fn write_bounded_json<T: Serialize>(path: &Path, value: &T) -> UseResult<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        provisioning_error(
            "use.plugin.runtime.provisioning_receipt_invalid",
            format!("Failed to encode Runtime provisioning receipt: {error}"),
        )
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_BINDING_RECEIPT_BYTES {
        return Err(provisioning_error(
            "use.plugin.runtime.provisioning_receipt_invalid",
            "The Runtime provisioning receipt exceeds its storage bound.",
        ));
    }
    let parent = path.parent().ok_or_else(invalid_path_identity)?;
    let temporary = parent.join(format!(".provisioning-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary Runtime provisioning", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "write temporary Runtime provisioning",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "sync temporary Runtime provisioning",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) = activate_temporary(temporary.clone(), path.to_path_buf()).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent(Some(parent)).await
}

async fn activate_temporary(temporary: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        temporary.persist(target).map_err(|error| error.error)
    })
    .await
    .map_err(|error| {
        provisioning_error(
            "use.plugin.runtime.binding_io",
            format!(
                "Failed to activate Runtime provisioning '{}': blocking task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| path_error("activate Runtime provisioning", &error_target, error))
}

fn provisioning_error(code: &'static str, message: impl Into<String>) -> a3s_use_core::UseError {
    a3s_use_core::UseError::new(code, message)
}
