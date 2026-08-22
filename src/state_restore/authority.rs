use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{PluginSurfaceKind, UseResult};
use a3s_use_extension::{ExtensionPaths, ExtensionReceipt, ExtensionRegistry};

use super::{authority_mismatch, digest_entries, restore_io};
use crate::state_backup::{
    StateBackupEntry, StateBackupFamily, StateBackupManifest, StateBackupPackageAuthority,
};

pub(super) async fn validate_live_authority(
    paths: &ExtensionPaths,
    backup: &StateBackupManifest,
    live: &[StateBackupEntry],
) -> UseResult<String> {
    let backup_authority = authority_entries(&backup.entries);
    let live_authority = authority_entries(live);
    if backup_authority != live_authority {
        return Err(authority_mismatch(
            "Live Registry or Grant authority differs from the coordinated backup.",
        ));
    }

    let snapshot = ExtensionRegistry::new(paths.clone())
        .published_snapshot()
        .await?;
    if !snapshot.pending_cutovers.is_empty()
        || snapshot.generation != backup.authority.registry_generation
        || snapshot.descriptor_digest()? != backup.authority.registry_digest
    {
        return Err(authority_mismatch(
            "The live Registry projection does not match the backup authority.",
        ));
    }
    let packages = read_receipt_authority(paths, live).await?;
    if packages != backup.authority.packages {
        return Err(authority_mismatch(
            "The live installed receipts do not match the backup authority.",
        ));
    }
    validate_snapshot_receipts(&snapshot.routes, paths, live).await?;
    digest_entries(&live_authority)
}

async fn read_receipt_authority(
    paths: &ExtensionPaths,
    live: &[StateBackupEntry],
) -> UseResult<Vec<StateBackupPackageAuthority>> {
    let mut packages = Vec::new();
    for entry in live
        .iter()
        .filter(|entry| receipt_path(&entry.path).is_some())
    {
        let expected_id = receipt_path(&entry.path).expect("filtered receipt path");
        let path = paths.state_root().join(&entry.path);
        let bytes = tokio::fs::read(&path).await.map_err(|error| {
            restore_io(format!("A live Registry receipt cannot be read: {error}"))
        })?;
        let receipt: ExtensionReceipt = serde_json::from_slice(&bytes).map_err(|_| {
            authority_mismatch("A live Registry receipt is not valid current JSON.")
        })?;
        if receipt.package_id != expected_id {
            return Err(authority_mismatch(
                "A live Registry receipt does not match its owned package path.",
            ));
        }
        packages.push(StateBackupPackageAuthority {
            package_id: receipt.package_id.clone(),
            receipt_digest: receipt.descriptor_digest()?,
        });
    }
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    if packages
        .windows(2)
        .any(|pair| pair[0].package_id >= pair[1].package_id)
    {
        return Err(authority_mismatch(
            "The live Registry contains duplicate installed receipt authority.",
        ));
    }
    Ok(packages)
}

async fn validate_snapshot_receipts(
    routes: &[a3s_use_extension::ExtensionRouteBinding],
    paths: &ExtensionPaths,
    live: &[StateBackupEntry],
) -> UseResult<()> {
    let mut receipts = BTreeMap::new();
    for entry in live
        .iter()
        .filter(|entry| receipt_path(&entry.path).is_some())
    {
        let bytes = tokio::fs::read(paths.state_root().join(&entry.path))
            .await
            .map_err(|error| {
                restore_io(format!("A live Registry receipt cannot be read: {error}"))
            })?;
        let receipt: ExtensionReceipt = serde_json::from_slice(&bytes).map_err(|_| {
            authority_mismatch("A live Registry receipt is not valid current JSON.")
        })?;
        receipts.insert(receipt.package_id.clone(), receipt);
    }
    if routes.len() != receipts.len() {
        return Err(authority_mismatch(
            "The live Registry routes and installed receipts have not converged.",
        ));
    }
    for route in routes {
        let receipt = receipts.get(&route.package_id).ok_or_else(|| {
            authority_mismatch("A live Registry route has no exact installed receipt.")
        })?;
        let surfaces = receipt
            .selected_surfaces
            .iter()
            .map(|surface| surface_kind_name(surface.kind))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if route.component_id != receipt.component_id
            || route.route != receipt.route
            || route.version != receipt.version
            || route.package_root != receipt.package_root
            || route.manifest_sha256 != receipt.manifest_sha256
            || route.package_sha256 != receipt.package_sha256
            || route.lifecycle_generation != receipt.lifecycle_generation
            || route.enabled != receipt.enabled
            || route.surfaces != surfaces
        {
            return Err(authority_mismatch(
                "A live Registry route differs from its exact installed receipt.",
            ));
        }
    }
    Ok(())
}

fn receipt_path(path: &str) -> Option<String> {
    let parts = path.split('/').collect::<Vec<_>>();
    let ["extensions", publisher, file] = parts.as_slice() else {
        return None;
    };
    let package = file.strip_suffix(".json")?;
    a3s_use_core::PluginPackageId::parse(format!("{publisher}/{package}"))
        .ok()
        .map(|id| id.to_string())
}

fn surface_kind_name(kind: PluginSurfaceKind) -> &'static str {
    match kind {
        PluginSurfaceKind::Flow => "flow",
        PluginSurfaceKind::Mcp => "mcp",
        PluginSurfaceKind::Okf => "okf",
        PluginSurfaceKind::Skill => "skill",
        PluginSurfaceKind::Tool => "tool",
        PluginSurfaceKind::Ui => "ui",
    }
}

pub(super) fn authority_entries(entries: &[StateBackupEntry]) -> Vec<StateBackupEntry> {
    entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.family,
                StateBackupFamily::Registry | StateBackupFamily::Grants
            )
        })
        .cloned()
        .collect()
}
