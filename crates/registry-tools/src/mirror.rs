//! Compare two assembled Registry trees for mirror replacement drills.
//!
//! Fail-closed: any root digest mismatch or target digest/set drift is an
//! error. Operators use this before promoting a mirror or after failover.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;

use crate::tools_error;

/// CLI entry: require two registries to publish the same root and targets.
pub(crate) fn compare_mirrors_command(options: &crate::Options) -> UseResult<()> {
    let left = PathBuf::from(options.require("left")?);
    let right = PathBuf::from(options.require("right")?);
    let report = compare_mirrors(&left, &right)?;
    println!(
        "{}",
        serde_json::json!({
            "left": left.display().to_string(),
            "right": right.display().to_string(),
            "leftRootSha256": format!("sha256:{}", report.left_root_sha256),
            "rightRootSha256": format!("sha256:{}", report.right_root_sha256),
            "targetsCompared": report.targets_compared,
            "identical": report.identical,
        })
    );
    if !report.identical {
        return Err(tools_error(
            "registry_tools.mirror_mismatch",
            &report.mismatch_message,
        ));
    }
    Ok(())
}

struct CompareReport {
    left_root_sha256: String,
    right_root_sha256: String,
    targets_compared: u64,
    identical: bool,
    mismatch_message: String,
}

fn compare_mirrors(left: &Path, right: &Path) -> UseResult<CompareReport> {
    let left_root = read_root_sha256(left)?;
    let right_root = read_root_sha256(right)?;
    let left_targets = target_digests(left)?;
    let right_targets = target_digests(right)?;
    let mut mismatch = String::new();
    if left_root != right_root {
        mismatch =
            format!("Root digest mismatch: left sha256:{left_root} right sha256:{right_root}.");
    } else if left_targets != right_targets {
        mismatch = "Target path/digest set mismatch between the two registries.".to_owned();
    }
    Ok(CompareReport {
        left_root_sha256: left_root,
        right_root_sha256: right_root,
        targets_compared: left_targets.len() as u64,
        identical: mismatch.is_empty(),
        mismatch_message: mismatch,
    })
}

fn read_root_sha256(registry: &Path) -> UseResult<String> {
    let path = registry.join("metadata").join("root.json");
    let bytes = std::fs::read(&path).map_err(|error| {
        tools_error(
            "registry_tools.mirror_failed",
            &format!("Failed to read '{}': {error}", path.display()),
        )
    })?;
    Ok(a3s_use_extension::sha256_hex(&bytes))
}

fn target_digests(registry: &Path) -> UseResult<BTreeMap<String, String>> {
    let targets_dir = registry.join("targets");
    if !targets_dir.is_dir() {
        return Err(tools_error(
            "registry_tools.mirror_failed",
            &format!("Missing targets directory '{}'.", targets_dir.display()),
        ));
    }
    let mut digests = BTreeMap::new();
    collect_files(&targets_dir, &targets_dir, &mut digests)?;
    Ok(digests)
}

fn collect_files(
    root: &Path,
    current: &Path,
    digests: &mut BTreeMap<String, String>,
) -> UseResult<()> {
    let entries = std::fs::read_dir(current).map_err(|error| {
        tools_error(
            "registry_tools.mirror_failed",
            &format!("Failed to read '{}': {error}", current.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            tools_error(
                "registry_tools.mirror_failed",
                &format!(
                    "Failed to read entry under '{}': {error}",
                    current.display()
                ),
            )
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, digests)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| {
                tools_error(
                    "registry_tools.mirror_failed",
                    &format!("Target '{}' escapes the targets root.", path.display()),
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(&path).map_err(|error| {
            tools_error(
                "registry_tools.mirror_failed",
                &format!("Failed to read '{}': {error}", path.display()),
            )
        })?;
        digests.insert(relative, a3s_use_extension::sha256_hex(&bytes));
    }
    Ok(())
}
