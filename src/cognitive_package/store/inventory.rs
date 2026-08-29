use std::io;
use std::path::Path;

use a3s_use_core::{InstallationId, PluginOperationAction, PluginPackageLock, UseResult};
use tokio::fs;

use super::{
    action_name, path_error, path_identity_error, pending_record_path, read_optional, store_error,
    PendingPackageGraphOperation,
};

const MAX_PENDING_GRAPH_OPERATIONS: usize = 1_024;
const MAX_PENDING_GRAPH_INVENTORY_ENTRIES: usize = MAX_PENDING_GRAPH_OPERATIONS * 2;

/// Enumerate the bounded package-graph operation domain while the caller
/// holds `.package-graph.lock`. This turns an admitted pending record into the
/// durable writer owner that survives process exit without introducing a
/// second ownership file.
pub(super) async fn read_pending_operations_locked(
    root: &Path,
) -> UseResult<Vec<PendingPackageGraphOperation>> {
    let mut operations = Vec::new();
    let mut inventory_entries = 0_usize;
    for action in [
        PluginOperationAction::Install,
        PluginOperationAction::Upgrade,
        PluginOperationAction::Uninstall,
    ] {
        let action_root = root.join(action_name(action));
        match fs::symlink_metadata(&action_root).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => return Err(path_identity_error()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(path_error(
                    "inspect pending graph action directory",
                    &action_root,
                    error,
                ))
            }
        }
        let mut publishers = fs::read_dir(&action_root).await.map_err(|error| {
            path_error("open pending graph action directory", &action_root, error)
        })?;
        while let Some(publisher) = publishers.next_entry().await.map_err(|error| {
            path_error("read pending graph action directory", &action_root, error)
        })? {
            inventory_entries = inventory_entries.saturating_add(1);
            if inventory_entries > MAX_PENDING_GRAPH_INVENTORY_ENTRIES {
                return Err(store_error(
                    "The pending cognitive-package operation inventory exceeds its bound.",
                ));
            }
            let publisher_path = publisher.path();
            let metadata = fs::symlink_metadata(&publisher_path)
                .await
                .map_err(|error| {
                    path_error(
                        "inspect pending graph publisher directory",
                        &publisher_path,
                        error,
                    )
                })?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
                return Err(path_identity_error());
            }
            let mut records = fs::read_dir(&publisher_path).await.map_err(|error| {
                path_error(
                    "open pending graph publisher directory",
                    &publisher_path,
                    error,
                )
            })?;
            while let Some(record) = records.next_entry().await.map_err(|error| {
                path_error(
                    "read pending graph publisher directory",
                    &publisher_path,
                    error,
                )
            })? {
                inventory_entries = inventory_entries.saturating_add(1);
                if operations.len() >= MAX_PENDING_GRAPH_OPERATIONS {
                    return Err(store_error(
                        "The pending cognitive-package operation inventory exceeds its bound.",
                    ));
                }
                let path = record.path();
                let value = read_optional::<PendingPackageGraphOperation>(&path)
                    .await?
                    .ok_or_else(|| store_error("A pending graph record disappeared."))?;
                value.validate()?;
                if value.action() != action
                    || pending_record_path(root, action, value.root_package_id())? != path
                {
                    return Err(store_error(
                        "A pending graph operation does not match its owned path.",
                    ));
                }
                operations.push(value);
            }
        }
    }
    Ok(operations)
}

#[derive(Debug, Clone)]
pub(crate) struct PendingPackageGraphArtifactReferences {
    pub(crate) installation: InstallationId,
    pub(crate) cancelled: bool,
    pub(crate) package_locks: Vec<PluginPackageLock>,
}

/// Project durable graph operations into the minimum reference-bearing shape
/// needed by global reachability. The private operation schema remains owned
/// by the package-graph store.
pub(crate) async fn inspect_pending_artifact_references_locked(
    root: &Path,
) -> UseResult<Vec<PendingPackageGraphArtifactReferences>> {
    let operations = read_pending_operations_locked(root).await?;
    let mut references = Vec::with_capacity(operations.len());
    for operation in operations {
        let mut package_locks = Vec::new();
        package_locks.extend(operation.envelope.package_lock.iter().cloned());
        package_locks.extend(operation.prior_package_lock.iter().cloned());
        package_locks.extend(operation.envelope.prior_package_lock.iter().cloned());
        references.push(PendingPackageGraphArtifactReferences {
            installation: operation.envelope.plan.scope.clone(),
            cancelled: operation.phase() == super::PackageGraphOperationPhase::Cancelled,
            package_locks,
        });
    }
    Ok(references)
}
