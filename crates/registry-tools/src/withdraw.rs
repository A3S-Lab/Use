//! Emergency withdrawal: republish online metadata without selected targets.
//!
//! Root identity is unchanged (bootstrap pin stays valid). Operators remove
//! one or more target paths from `targets.json`, delete their bytes, and
//! resign snapshot/timestamp with the current online keys. Clients must
//! refresh before install; do not retry an ambiguous prior install.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use serde_json::{json, Value};

use crate::keys::KeySet;
use crate::tools_error;

/// CLI entry: withdraw target paths from a published registry tree.
pub(crate) fn withdraw_targets_command(options: &crate::Options) -> UseResult<()> {
    let registry = PathBuf::from(options.require("registry")?);
    let keys = KeySet::load(Path::new(&options.require("keys-dir")?))?;
    let metadata_expires = options
        .optional("metadata-expires")
        .unwrap_or_else(|| default_expiry(30));
    let mut targets = options.all("target");
    if targets.iter().any(|value| value.trim().is_empty()) {
        return Err(tools_error(
            "registry_tools.arguments_invalid",
            "--target must be a non-empty target path.",
        ));
    }
    if targets.is_empty() {
        return Err(tools_error(
            "registry_tools.arguments_invalid",
            "At least one --target <path> is required for withdraw-targets.",
        ));
    }
    targets.sort();
    targets.dedup();
    let outcome = withdraw_targets(&registry, &keys, &targets, &metadata_expires)?;
    println!(
        "{}",
        serde_json::json!({
            "registry": registry.display().to_string(),
            "withdrawn": outcome.withdrawn,
            "remainingTargets": outcome.remaining_targets,
            "metadataVersion": outcome.metadata_version,
            "rootSha256": format!("sha256:{}", outcome.root_sha256),
        })
    );
    Ok(())
}

struct WithdrawOutcome {
    withdrawn: Vec<String>,
    remaining_targets: u64,
    metadata_version: u64,
    root_sha256: String,
}

fn withdraw_targets(
    registry: &Path,
    keys: &KeySet,
    withdraw: &[String],
    metadata_expires: &str,
) -> UseResult<WithdrawOutcome> {
    let metadata = registry.join("metadata");
    let root_bytes = std::fs::read(metadata.join("root.json")).map_err(|error| {
        tools_error(
            "registry_tools.withdraw_failed",
            &format!(
                "Failed to read '{}': {error}",
                metadata.join("root.json").display()
            ),
        )
    })?;
    let root_sha256 = a3s_use_extension::sha256_hex(&root_bytes);
    let root_document: Value = serde_json::from_slice(&root_bytes).map_err(|error| {
        tools_error(
            "registry_tools.withdraw_failed",
            &format!("root.json is not valid JSON: {error}"),
        )
    })?;
    ensure_online_keys_match(&root_document, keys)?;

    let targets_path = metadata.join("targets.json");
    let targets_document = read_json(&targets_path)?;
    let previous_version = targets_document
        .pointer("/signed/version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            tools_error(
                "registry_tools.withdraw_failed",
                "targets.json signed.version must be an integer.",
            )
        })?;
    let mut targets_map = targets_document
        .pointer("/signed/targets")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| {
            tools_error(
                "registry_tools.withdraw_failed",
                "targets.json lacks signed.targets.",
            )
        })?;

    let withdraw_set: BTreeSet<&str> = withdraw.iter().map(String::as_str).collect();
    let mut withdrawn = Vec::new();
    for name in &withdraw_set {
        if targets_map.remove(*name).is_none() {
            return Err(tools_error(
                "registry_tools.withdraw_failed",
                &format!("Target '{name}' is not present in signed targets metadata."),
            ));
        }
        withdrawn.push((*name).to_owned());
        let path = registry.join("targets").join(name);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|error| {
                tools_error(
                    "registry_tools.withdraw_failed",
                    &format!(
                        "Failed to delete target bytes '{}': {error}",
                        path.display()
                    ),
                )
            })?;
        }
    }

    let metadata_version = previous_version + 1;
    let mut targets_signed = targets_document.get("signed").cloned().ok_or_else(|| {
        tools_error(
            "registry_tools.withdraw_failed",
            "targets.json lacks signed.",
        )
    })?;
    targets_signed["version"] = json!(metadata_version);
    targets_signed["expires"] = json!(metadata_expires);
    targets_signed["targets"] = Value::Object(targets_map.clone());
    let targets = a3s_use_extension::sign_tuf_document(
        &keys.targets.pair,
        &keys.targets.key_id,
        targets_signed,
    );
    let snapshot = a3s_use_extension::sign_tuf_document(
        &keys.snapshot.pair,
        &keys.snapshot.key_id,
        json!({
            "_type": "snapshot",
            "spec_version": "1.0.0",
            "version": metadata_version,
            "expires": metadata_expires,
            "meta": {
                "targets.json": {
                    "version": metadata_version,
                    "length": targets.len(),
                    "hashes": {"sha256": a3s_use_extension::sha256_hex(&targets)},
                }
            }
        }),
    );
    let timestamp = a3s_use_extension::sign_tuf_document(
        &keys.timestamp.pair,
        &keys.timestamp.key_id,
        json!({
            "_type": "timestamp",
            "spec_version": "1.0.0",
            "version": metadata_version,
            "expires": metadata_expires,
            "meta": {
                "snapshot.json": {
                    "version": metadata_version,
                    "length": snapshot.len(),
                    "hashes": {"sha256": a3s_use_extension::sha256_hex(&snapshot)},
                }
            }
        }),
    );
    write_bytes(&targets_path, &targets)?;
    write_bytes(&metadata.join("snapshot.json"), &snapshot)?;
    write_bytes(&metadata.join("timestamp.json"), &timestamp)?;

    Ok(WithdrawOutcome {
        withdrawn,
        remaining_targets: targets_map.len() as u64,
        metadata_version,
        root_sha256,
    })
}

fn ensure_online_keys_match(root_document: &Value, keys: &KeySet) -> UseResult<()> {
    for (role, key) in [
        ("targets", &keys.targets),
        ("snapshot", &keys.snapshot),
        ("timestamp", &keys.timestamp),
    ] {
        let keyids = root_document
            .pointer(&format!("/signed/roles/{role}/keyids"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                tools_error(
                    "registry_tools.withdraw_failed",
                    &format!("root.json lacks roles.{role}.keyids."),
                )
            })?;
        let ok = keyids
            .iter()
            .filter_map(Value::as_str)
            .any(|key_id| key_id == key.key_id);
        if !ok {
            return Err(tools_error(
                "registry_tools.withdraw_failed",
                &format!("keys-dir {role} key does not match the published root role keyids."),
            ));
        }
    }
    Ok(())
}

fn read_json(path: &Path) -> UseResult<Value> {
    let bytes = std::fs::read(path).map_err(|error| {
        tools_error(
            "registry_tools.withdraw_failed",
            &format!("Failed to read '{}': {error}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        tools_error(
            "registry_tools.withdraw_failed",
            &format!("'{}' is not valid JSON: {error}", path.display()),
        )
    })
}

fn write_bytes(path: &Path, bytes: &[u8]) -> UseResult<()> {
    std::fs::write(path, bytes).map_err(|error| {
        tools_error(
            "registry_tools.withdraw_failed",
            &format!("Failed to write '{}': {error}", path.display()),
        )
    })
}

fn default_expiry(days: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_secs();
    let expiry = now + days * 24 * 60 * 60;
    let secs = expiry as i64;
    let days_since_epoch = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400) as u64;
    let (year, month, day) = civil_from_days(days_since_epoch);
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;
    let second = time_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
