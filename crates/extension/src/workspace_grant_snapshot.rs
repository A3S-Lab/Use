use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use a3s_use_core::{
    PluginWorkspaceGrant, PluginWorkspaceGrantSnapshot, UseError, UseResult,
    WorkspaceGrantEvidence, MAX_PLUGIN_PLAN_ITEMS, PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};
use tokio::fs;

use super::workspace_grant::{store_error, StoredWorkspaceGrant, WorkspaceGrantStore};
use super::workspace_grant_io::{
    acquire_lock, read_optional_record, validate_existing_directory_chain,
};

const MAX_STORED_RECORDS_PER_SCOPE: usize = 4_096;
const MAX_PACKAGE_DIRECTORIES_PER_SCOPE: usize = 4_096;
const MAX_PUBLISHER_DIRECTORIES_PER_SCOPE: usize = 512;

impl WorkspaceGrantStore {
    /// Builds a stable, canonical view of granted package generations in one scope.
    ///
    /// A package with two granted generations indicates an unfinished blue/green
    /// transition. Snapshotting fails closed until lifecycle recovery retires the
    /// prior generation.
    pub async fn snapshot_scope(
        &self,
        scope_id: &str,
        state_revision: u64,
    ) -> UseResult<PluginWorkspaceGrantSnapshot> {
        PluginWorkspaceGrantSnapshot {
            schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
            scope_id: scope_id.to_string(),
            state_revision,
            grants: Vec::new(),
        }
        .validate()?;
        let _lock = acquire_lock(self.state_root(), self.root()).await?;
        self.snapshot_scope_locked(scope_id, state_revision).await
    }

    pub(super) async fn snapshot_scope_locked(
        &self,
        scope_id: &str,
        state_revision: u64,
    ) -> UseResult<PluginWorkspaceGrantSnapshot> {
        let empty = PluginWorkspaceGrantSnapshot {
            schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
            scope_id: scope_id.to_string(),
            state_revision,
            grants: Vec::new(),
        };
        empty.validate()?;

        let scope_path = self.scope_path(scope_id)?;
        if !validate_existing_directory_chain(self.state_root(), Some(&scope_path)).await? {
            return Ok(empty);
        }

        let grants = collect_scope_grants(scope_id, &scope_path, state_revision).await?;
        let snapshot = PluginWorkspaceGrantSnapshot {
            grants: grants.into_values().collect(),
            ..empty
        };
        snapshot.validate()?;
        Ok(snapshot)
    }
}

async fn collect_scope_grants(
    scope_id: &str,
    scope_path: &Path,
    state_revision: u64,
) -> UseResult<BTreeMap<String, WorkspaceGrantEvidence>> {
    let mut grants = BTreeMap::new();
    let mut publisher_count = 0;
    let mut package_count = 0;
    let mut record_count = 0;
    let mut publishers = read_directory(scope_path, "workspace grant scope").await?;

    while let Some(publisher) = next_entry(&mut publishers, scope_path).await? {
        publisher_count += 1;
        enforce_bound(
            publisher_count,
            MAX_PUBLISHER_DIRECTORIES_PER_SCOPE,
            "publisher directories",
        )?;
        let publisher_path = publisher.path();
        require_real_directory(&publisher_path, "publisher").await?;
        let publisher_file_name = publisher.file_name();
        let publisher_name = utf8_name(&publisher_path, &publisher_file_name)?;
        PluginWorkspaceGrant::validate_package_id(&format!("{publisher_name}/snapshot"))
            .map_err(|_| invalid_layout("A workspace grant publisher directory is invalid."))?;

        let mut packages = read_directory(&publisher_path, "workspace grant publisher").await?;
        while let Some(package) = next_entry(&mut packages, &publisher_path).await? {
            package_count += 1;
            enforce_bound(
                package_count,
                MAX_PACKAGE_DIRECTORIES_PER_SCOPE,
                "package directories",
            )?;
            let package_path = package.path();
            require_real_directory(&package_path, "package").await?;
            let package_file_name = package.file_name();
            let package_name = utf8_name(&package_path, &package_file_name)?;
            let package_id = format!("{publisher_name}/{package_name}");
            PluginWorkspaceGrant::validate_identity(scope_id, &package_id)
                .map_err(|_| invalid_layout("A workspace grant package directory is invalid."))?;

            let mut records = read_directory(&package_path, "workspace grant package").await?;
            while let Some(entry) = next_entry(&mut records, &package_path).await? {
                record_count += 1;
                enforce_bound(record_count, MAX_STORED_RECORDS_PER_SCOPE, "stored records")?;
                let entry_path = entry.path();
                require_real_file(&entry_path).await?;
                let entry_file_name = entry.file_name();
                let file_name = utf8_name(&entry_path, &entry_file_name)?;
                if temporary_record_name(file_name) {
                    continue;
                }
                let package_digest = parse_generation_file(file_name)?;
                let record = read_optional_record(&entry_path).await?.ok_or_else(|| {
                    snapshot_error(
                        "use.plugin.grant_store.snapshot_changed",
                        "A workspace grant record disappeared while the stable snapshot was read.",
                    )
                })?;
                if record.scope_id() != scope_id
                    || record.package_id() != package_id
                    || record.package_digest() != package_digest
                {
                    return Err(snapshot_error(
                        "use.plugin.grant_store.ownership_mismatch",
                        "A workspace grant snapshot record does not match its generation path.",
                    ));
                }
                if record.revision() > state_revision {
                    return Err(snapshot_error(
                        "use.plugin.grant_store.snapshot_stale",
                        "The requested state revision is older than durable workspace grant state.",
                    ));
                }
                let StoredWorkspaceGrant::Granted(receipt) = record else {
                    continue;
                };
                if grants.len() >= MAX_PLUGIN_PLAN_ITEMS {
                    return Err(snapshot_error(
                        "use.plugin.grant_store.snapshot_too_large",
                        "The active workspace grant snapshot exceeds the plugin plan item bound.",
                    ));
                }
                let evidence = WorkspaceGrantEvidence {
                    package_id: package_id.clone(),
                    package_digest,
                    receipt_revision: receipt.revision,
                    grant_digest: receipt.grant_digest,
                };
                if grants.insert(package_id.clone(), evidence).is_some() {
                    return Err(snapshot_error(
                        "use.plugin.grant_store.snapshot_unstable",
                        "Multiple granted generations exist for one package; lifecycle recovery is required.",
                    ));
                }
            }
        }
    }
    Ok(grants)
}

async fn read_directory(path: &Path, label: &str) -> UseResult<fs::ReadDir> {
    fs::read_dir(path)
        .await
        .map_err(|error| snapshot_io_error(&format!("read {label} directory"), path, error))
}

async fn next_entry(entries: &mut fs::ReadDir, path: &Path) -> UseResult<Option<fs::DirEntry>> {
    entries
        .next_entry()
        .await
        .map_err(|error| snapshot_io_error("read workspace grant directory entry", path, error))
}

async fn require_real_directory(path: &Path, label: &str) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| snapshot_io_error("inspect workspace grant directory", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid_layout(format!(
            "A workspace grant {label} entry is not a real directory."
        )));
    }
    Ok(())
}

async fn require_real_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| snapshot_io_error("inspect workspace grant record", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid_layout(
            "A workspace grant generation entry is not a regular file.",
        ));
    }
    Ok(())
}

fn utf8_name<'a>(path: &Path, name: &'a std::ffi::OsStr) -> UseResult<&'a str> {
    name.to_str().ok_or_else(|| {
        invalid_layout(format!(
            "Workspace grant path '{}' is not portable UTF-8.",
            path.display()
        ))
    })
}

fn parse_generation_file(file_name: &str) -> UseResult<String> {
    let Some(digest) = file_name.strip_suffix(".json") else {
        return Err(invalid_layout(
            "A workspace grant generation file has an invalid extension.",
        ));
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(invalid_layout(
            "A workspace grant generation filename is not a lowercase SHA-256 digest.",
        ));
    }
    Ok(format!("sha256:{digest}"))
}

fn temporary_record_name(file_name: &str) -> bool {
    file_name.starts_with(".grant-") && file_name.ends_with(".tmp")
}

fn enforce_bound(count: usize, maximum: usize, label: &str) -> UseResult<()> {
    if count > maximum {
        return Err(snapshot_error(
            "use.plugin.grant_store.snapshot_too_large",
            format!("The workspace grant scope exceeds the {label} bound."),
        ));
    }
    Ok(())
}

fn invalid_layout(message: impl Into<String>) -> UseError {
    snapshot_error("use.plugin.grant_store.path_invalid", message)
}

fn snapshot_io_error(action: &str, path: &Path, error: io::Error) -> UseError {
    snapshot_error(
        "use.plugin.grant_store.io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

fn snapshot_error(code: &'static str, message: impl Into<String>) -> UseError {
    store_error(code, message)
}
