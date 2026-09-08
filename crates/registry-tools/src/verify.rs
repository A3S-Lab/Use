//! Re-open an assembled registry exactly the way a released client does.

use std::collections::BTreeSet;
use std::path::Path;

use a3s_use_core::{
    PluginCatalogRecord, PluginPlanningBundle, PluginReleaseChannel, UseError, UseResult,
};
use tough::{ExpirationEnforcement, Limits, RepositoryLoader};
use url::Url;

use crate::tools_error;

/// Summary of one verification pass.
pub(crate) struct VerifyReport {
    pub(crate) root_sha256: String,
    pub(crate) packages: Vec<String>,
    pub(crate) targets_checked: u64,
}

/// CLI entry: verify one registry tree.
pub(crate) async fn verify_command(options: &crate::Options) -> UseResult<()> {
    let registry = Path::new(&options.require("registry")?)
        .canonicalize()
        .map_err(|error| {
            tools_error(
                "registry_tools.verify_failed",
                &format!("Failed to resolve the registry directory: {error}"),
            )
        })?;
    let expected = options.optional("expected-root-sha256");
    let report = verify_registry(&registry, expected.as_deref()).await?;
    println!(
        "{}",
        serde_json::json!({
            "rootSha256": format!("sha256:{}", report.root_sha256),
            "packages": report.packages,
            "targetsChecked": report.targets_checked,
            "registry": registry.display().to_string(),
        })
    );
    Ok(())
}

/// Verify the metadata chain, every catalog record, and every target digest.
pub(crate) async fn verify_registry(
    registry_root: &Path,
    expected_root_sha256: Option<&str>,
) -> UseResult<VerifyReport> {
    let metadata_directory = registry_root.join("metadata");
    let targets_directory = registry_root.join("targets");
    let root_bytes = std::fs::read(metadata_directory.join("root.json")).map_err(|error| {
        tools_error(
            "registry_tools.verify_failed",
            &format!(
                "Failed to read '{}': {error}",
                metadata_directory.join("root.json").display()
            ),
        )
    })?;
    let root_sha256 = a3s_use_extension::sha256_hex(&root_bytes);
    if let Some(expected) = expected_root_sha256 {
        let normalized = expected.trim_start_matches("sha256:").to_ascii_lowercase();
        if normalized != root_sha256 {
            return Err(tools_error(
                "registry_tools.verify_failed",
                &format!("The registry root digest is {root_sha256}, but {normalized} was pinned."),
            ));
        }
    }
    a3s_use_extension::inspect_bootstrap_root(&root_bytes).map_err(|error| {
        tools_error(
            "registry_tools.verify_failed",
            &format!(
                "The root role is not valid signed TUF JSON: {}",
                error.message
            ),
        )
    })?;
    let metadata_url = directory_url(&metadata_directory)?;
    let targets_url = directory_url(&targets_directory)?;
    let repository = RepositoryLoader::new(&root_bytes, metadata_url, targets_url)
        .transport(tough::FilesystemTransport)
        .limits(Limits {
            max_root_size: 1024 * 1024,
            max_targets_size: 10 * 1024 * 1024,
            max_timestamp_size: 1024 * 1024,
            max_snapshot_size: 1024 * 1024,
            max_root_updates: 64,
        })
        .expiration_enforcement(ExpirationEnforcement::Safe)
        .load()
        .await
        .map_err(|error| {
            tools_error(
                "registry_tools.verify_failed",
                &format!("The tough client rejected this registry: {error}"),
            )
        })?;

    let mut packages = Vec::new();
    let mut expected_planning: BTreeSet<String> = BTreeSet::new();
    let mut seen_planning: BTreeSet<String> = BTreeSet::new();
    let mut targets_checked = 0_u64;
    for (target_name, target) in repository.all_targets() {
        let name = target_name.raw();
        let signed_digest = serde_json::to_value(&target.hashes)
            .ok()
            .and_then(|value| value.get("sha256").cloned())
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| {
                tools_error(
                    "registry_tools.verify_failed",
                    &format!("Target '{name}' has no readable sha256 digest."),
                )
            })?;
        let bytes = read_target(&targets_directory, name)?;
        let actual = a3s_use_extension::sha256_hex(&bytes);
        if actual != signed_digest || target.length != bytes.len() as u64 {
            return Err(tools_error(
                "registry_tools.verify_failed",
                &format!("Target '{name}' on disk does not match its signed digest or length."),
            ));
        }
        targets_checked += 1;
        if let Some(metadata) = target.custom.get("a3s") {
            let record = PluginCatalogRecord::from_json(metadata.to_string().as_bytes())?;
            record.validate()?;
            validate_target_metadata(name, &record)?;
            let expected_planning_target = format!("{}planning-v1.json", archive_prefix(name));
            if record.planning.is_some() {
                expected_planning.insert(expected_planning_target);
            }
            packages.push(format!(
                "{}@{} {}",
                record.package_id, record.version, record.target
            ));
        } else if target.custom.contains_key("a3sPlanning") {
            PluginPlanningBundle::from_json(&bytes)?;
            seen_planning.insert(name.to_string());
        }
    }
    if packages.is_empty() {
        return Err(tools_error(
            "registry_tools.verify_failed",
            "The registry contains no catalog package targets.",
        ));
    }
    if seen_planning != expected_planning {
        return Err(tools_error(
            "registry_tools.verify_failed",
            &format!(
                "Planning targets {seen_planning:?} do not match the catalog expectations {expected_planning:?}."
            ),
        ));
    }
    Ok(VerifyReport {
        root_sha256,
        packages,
        targets_checked,
    })
}

/// The directory prefix one archive target name must carry.
fn archive_prefix(target_name: &str) -> &str {
    match target_name.rfind('/') {
        Some(index) => &target_name[..=index],
        None => "",
    }
}

fn read_target(targets_directory: &Path, target_name: &str) -> UseResult<Vec<u8>> {
    if target_name.contains("..") || target_name.starts_with('/') || target_name.contains('\\') {
        return Err(tools_error(
            "registry_tools.verify_failed",
            &format!("Target name '{target_name}' is not a safe relative path."),
        ));
    }
    std::fs::read(targets_directory.join(target_name)).map_err(|error| {
        tools_error(
            "registry_tools.verify_failed",
            &format!("Failed to read target '{target_name}': {error}"),
        )
    })
}

fn directory_url(path: &Path) -> Result<Url, UseError> {
    Url::from_directory_path(path).map_err(|()| {
        tools_error(
            "registry_tools.verify_failed",
            &format!(
                "The path '{}' cannot be represented as a URL.",
                path.display()
            ),
        )
    })
}

fn validate_target_metadata(target_name: &str, record: &PluginCatalogRecord) -> UseResult<()> {
    let expected_prefix = format!(
        "extensions/{}/{}/{}/{}/",
        record.package_id,
        record.version,
        channel_segment(record.channel),
        record.target
    );
    if !target_name.starts_with(&expected_prefix) {
        return Err(tools_error(
            "registry_tools.verify_failed",
            &format!(
                "Target '{target_name}' does not live under its catalog identity prefix '{expected_prefix}'."
            ),
        ));
    }
    if record.archive.target_name != target_name {
        return Err(tools_error(
            "registry_tools.verify_failed",
            &format!(
                "Catalog record for '{}' names archive '{}' but is attached to '{target_name}'.",
                record.package_id, record.archive.target_name
            ),
        ));
    }
    Ok(())
}

fn channel_segment(channel: PluginReleaseChannel) -> &'static str {
    match channel {
        PluginReleaseChannel::Stable => "stable",
        PluginReleaseChannel::Beta => "beta",
        PluginReleaseChannel::Nightly => "nightly",
    }
}
