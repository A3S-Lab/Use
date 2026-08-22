use std::io;

use a3s_use_core::{PlanScope, UseResult};
use tokio::fs;

use super::{
    operation_invalid, operation_io, operation_limit, valid_digest_segment,
    validate_existing_directory_chain, RestoreOperation, RestoreOperationStatus,
    RestoreOperationStore, MAX_RESTORE_OPERATIONS_PER_SCOPE,
};

#[derive(Debug, Default)]
pub(in crate::okf_knowledge::recovery) struct RestoreOperationInventory {
    pub(in crate::okf_knowledge::recovery) directory_count: usize,
    pub(in crate::okf_knowledge::recovery) operations: Vec<RestoreOperation>,
}

impl RestoreOperationStore {
    pub(in crate::okf_knowledge::recovery) async fn inventory(
        &self,
        scope: &PlanScope,
    ) -> UseResult<RestoreOperationInventory> {
        let scope_directory = self.scope_directory(scope);
        let metadata = match fs::symlink_metadata(&scope_directory).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(RestoreOperationInventory::default())
            }
            Err(error) => {
                return Err(operation_io(
                    "inspect Knowledge restore scope",
                    &scope_directory,
                    error,
                ));
            }
        };
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(operation_invalid(
                "The Knowledge restore scope path is not an owned directory.",
            ));
        }
        validate_existing_directory_chain(&self.state_root, &scope_directory).await?;
        let mut entries = fs::read_dir(&scope_directory).await.map_err(|error| {
            operation_io("read Knowledge restore scope", &scope_directory, error)
        })?;
        let mut operation_count = 0_usize;
        let mut operations = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(|error| {
            operation_io("read Knowledge restore operation", &scope_directory, error)
        })? {
            operation_count = operation_count.saturating_add(1);
            if operation_count > MAX_RESTORE_OPERATIONS_PER_SCOPE {
                return Err(operation_limit());
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| operation_invalid("A Knowledge restore operation name is invalid."))?;
            let metadata = fs::symlink_metadata(entry.path()).await.map_err(|error| {
                operation_io("inspect Knowledge restore operation", &entry.path(), error)
            })?;
            if !valid_digest_segment(&name)
                || a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                || !metadata.is_dir()
            {
                return Err(operation_invalid(
                    "The Knowledge restore operation layout contains an unowned entry.",
                ));
            }
            let digest = format!("sha256:{name}");
            if let Some(operation) = self.load(scope, &digest).await? {
                operations.push(operation);
            }
        }
        operations.sort_by(|left, right| {
            right
                .started_at_ms
                .cmp(&left.started_at_ms)
                .then_with(|| left.plan_digest.cmp(&right.plan_digest))
        });
        Ok(RestoreOperationInventory {
            directory_count: operation_count,
            operations,
        })
    }

    pub(in crate::okf_knowledge::recovery) async fn nonterminal(
        &self,
        scope: &PlanScope,
    ) -> UseResult<Option<RestoreOperation>> {
        let inventory = self.inventory(scope).await?;
        let mut active = None;
        for operation in inventory.operations {
            if operation.status != RestoreOperationStatus::Completed
                && active.replace(operation).is_some()
            {
                return Err(operation_invalid(
                    "More than one nonterminal Knowledge restore exists for one scope.",
                ));
            }
        }
        Ok(active)
    }
}
