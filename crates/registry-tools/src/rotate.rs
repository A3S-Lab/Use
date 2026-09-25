//! Every-intermediate-root rotation for an assembled Registry tree.
//!
//! Rotation replaces `metadata/root.json` with version N+1 that names the
//! next key set and is signed by that next root threshold. Online roles are
//! re-signed with the next keys. The previous root bytes are retained under
//! `metadata/root.history/` so operators keep every intermediate root.
//!
//! A3S clients pin an exact bootstrap root digest, so a rotated tree requires
//! an explicit Registry source replacement with the new pin. This command
//! refuses to rotate unless `--previous-keys-dir` still matches the published
//! root keyids (proof of current custody) and retains the prior root for
//! audit.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use serde_json::{json, Map, Value};

use crate::keys::{role_key_value, KeySet};
use crate::tools_error;

/// CLI entry: rotate the published root and resign online metadata.
pub(crate) fn rotate_root_command(options: &crate::Options) -> UseResult<()> {
    let registry = PathBuf::from(options.require("registry")?);
    let previous_keys = KeySet::load(Path::new(&options.require("previous-keys-dir")?))?;
    let next_keys = KeySet::load(Path::new(&options.require("next-keys-dir")?))?;
    let root_expires = options
        .optional("root-expires")
        .unwrap_or_else(|| default_expiry(365));
    let metadata_expires = options
        .optional("metadata-expires")
        .unwrap_or_else(|| default_expiry(30));
    let outcome = rotate_root(
        &registry,
        &previous_keys,
        &next_keys,
        &root_expires,
        &metadata_expires,
    )?;
    println!(
        "{}",
        serde_json::json!({
            "registry": registry.display().to_string(),
            "previousRootVersion": outcome.previous_root_version,
            "rootVersion": outcome.root_version,
            "previousRootSha256": format!("sha256:{}", outcome.previous_root_sha256),
            "rootSha256": format!("sha256:{}", outcome.root_sha256),
            "historyPath": outcome.history_path.display().to_string(),
            "metadataVersion": outcome.metadata_version,
        })
    );
    Ok(())
}

struct RotateOutcome {
    previous_root_version: u64,
    root_version: u64,
    previous_root_sha256: String,
    root_sha256: String,
    history_path: PathBuf,
    metadata_version: u64,
}

fn rotate_root(
    registry: &Path,
    previous_keys: &KeySet,
    next_keys: &KeySet,
    root_expires: &str,
    metadata_expires: &str,
) -> UseResult<RotateOutcome> {
    let metadata = registry.join("metadata");
    let root_path = metadata.join("root.json");
    let root_bytes = std::fs::read(&root_path).map_err(|error| {
        tools_error(
            "registry_tools.rotate_failed",
            &format!("Failed to read '{}': {error}", root_path.display()),
        )
    })?;
    let previous_root_sha256 = a3s_use_extension::sha256_hex(&root_bytes);
    let root_document: Value = serde_json::from_slice(&root_bytes).map_err(|error| {
        tools_error(
            "registry_tools.rotate_failed",
            &format!("root.json is not valid JSON: {error}"),
        )
    })?;
    let signed = root_document
        .get("signed")
        .ok_or_else(|| tools_error("registry_tools.rotate_failed", "root.json lacks signed."))?;
    let previous_version = signed
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            tools_error(
                "registry_tools.rotate_failed",
                "root.json signed.version must be an integer.",
            )
        })?;
    let current_root_keyids = role_keyids(signed, "root")?;
    let previous_keyids: BTreeSet<&str> = previous_keys
        .roots
        .iter()
        .map(|key| key.key_id.as_str())
        .collect();
    if current_root_keyids != previous_keyids {
        return Err(tools_error(
            "registry_tools.rotate_failed",
            "previous-keys-dir root shares do not match the published root keyids.",
        ));
    }
    if (previous_keys.roots.len() as u64) < previous_keys.root_policy.threshold {
        return Err(tools_error(
            "registry_tools.rotate_failed",
            "previous-keys-dir does not meet the published root threshold.",
        ));
    }
    if (next_keys.roots.len() as u64) < next_keys.root_policy.threshold {
        return Err(tools_error(
            "registry_tools.rotate_failed",
            "next-keys-dir does not meet its root threshold.",
        ));
    }

    let mut tuf_keys = Map::new();
    let mut roles = Map::new();
    for key in next_keys.roots.iter().chain([
        &next_keys.targets,
        &next_keys.snapshot,
        &next_keys.timestamp,
    ]) {
        tuf_keys.insert(key.key_id.clone(), role_key_value(key));
    }
    roles.insert(
        "root".to_owned(),
        json!({
            "keyids": next_keys.roots.iter().map(|key| key.key_id.clone()).collect::<Vec<_>>(),
            "threshold": next_keys.root_policy.threshold,
        }),
    );
    for key in [
        &next_keys.targets,
        &next_keys.snapshot,
        &next_keys.timestamp,
    ] {
        roles.insert(
            key.role.to_string(),
            json!({"keyids": [key.key_id.clone()], "threshold": 1}),
        );
    }
    let next_version = previous_version + 1;
    let root_signed = json!({
        "_type": "root",
        "spec_version": "1.0.0",
        "consistent_snapshot": false,
        "version": next_version,
        "expires": root_expires,
        "keys": Value::Object(tuf_keys),
        "roles": Value::Object(roles),
    });
    let root_signers: Vec<_> = next_keys
        .roots
        .iter()
        .map(|key| (&key.pair, key.key_id.as_str()))
        .collect();
    let new_root = a3s_use_extension::sign_tuf_document_with_keys(root_signers, root_signed);
    let root_sha256 = a3s_use_extension::sha256_hex(&new_root);

    let targets_path = metadata.join("targets.json");
    let snapshot_path = metadata.join("snapshot.json");
    let timestamp_path = metadata.join("timestamp.json");
    let targets_document = read_json(&targets_path)?;
    let previous_metadata_version = targets_document
        .pointer("/signed/version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            tools_error(
                "registry_tools.rotate_failed",
                "targets.json signed.version must be an integer.",
            )
        })?;
    let metadata_version = previous_metadata_version + 1;
    let mut targets_signed = targets_document
        .get("signed")
        .cloned()
        .ok_or_else(|| tools_error("registry_tools.rotate_failed", "targets.json lacks signed."))?;
    targets_signed["version"] = json!(metadata_version);
    targets_signed["expires"] = json!(metadata_expires);
    let targets = a3s_use_extension::sign_tuf_document(
        &next_keys.targets.pair,
        &next_keys.targets.key_id,
        targets_signed,
    );
    let snapshot_signed = json!({
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
    });
    let snapshot = a3s_use_extension::sign_tuf_document(
        &next_keys.snapshot.pair,
        &next_keys.snapshot.key_id,
        snapshot_signed,
    );
    let timestamp_signed = json!({
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
    });
    let timestamp = a3s_use_extension::sign_tuf_document(
        &next_keys.timestamp.pair,
        &next_keys.timestamp.key_id,
        timestamp_signed,
    );

    let history_dir = metadata.join("root.history");
    std::fs::create_dir_all(&history_dir).map_err(|error| {
        tools_error(
            "registry_tools.rotate_failed",
            &format!(
                "Failed to create root history directory '{}': {error}",
                history_dir.display()
            ),
        )
    })?;
    let history_path = history_dir.join(format!("root.{previous_version}.json"));
    if history_path.exists() {
        return Err(tools_error(
            "registry_tools.rotate_failed",
            &format!(
                "Refusing to overwrite retained intermediate root '{}'.",
                history_path.display()
            ),
        ));
    }
    std::fs::write(&history_path, &root_bytes).map_err(|error| {
        tools_error(
            "registry_tools.rotate_failed",
            &format!(
                "Failed to retain intermediate root '{}': {error}",
                history_path.display()
            ),
        )
    })?;
    write_bytes(&root_path, &new_root)?;
    write_bytes(&targets_path, &targets)?;
    write_bytes(&snapshot_path, &snapshot)?;
    write_bytes(&timestamp_path, &timestamp)?;

    Ok(RotateOutcome {
        previous_root_version: previous_version,
        root_version: next_version,
        previous_root_sha256,
        root_sha256,
        history_path,
        metadata_version,
    })
}

fn role_keyids<'a>(signed: &'a Value, role: &str) -> UseResult<BTreeSet<&'a str>> {
    let keyids = signed
        .pointer(&format!("/roles/{role}/keyids"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            tools_error(
                "registry_tools.rotate_failed",
                &format!("root.json lacks roles.{role}.keyids."),
            )
        })?;
    let mut set = BTreeSet::new();
    for value in keyids {
        let Some(key_id) = value.as_str() else {
            return Err(tools_error(
                "registry_tools.rotate_failed",
                &format!("root.json roles.{role}.keyids must be strings."),
            ));
        };
        set.insert(key_id);
    }
    Ok(set)
}

fn read_json(path: &Path) -> UseResult<Value> {
    let bytes = std::fs::read(path).map_err(|error| {
        tools_error(
            "registry_tools.rotate_failed",
            &format!("Failed to read '{}': {error}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        tools_error(
            "registry_tools.rotate_failed",
            &format!("'{}' is not valid JSON: {error}", path.display()),
        )
    })
}

fn write_bytes(path: &Path, bytes: &[u8]) -> UseResult<()> {
    std::fs::write(path, bytes).map_err(|error| {
        tools_error(
            "registry_tools.rotate_failed",
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
    // Howard Hinnant civil_from_days algorithm.
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
