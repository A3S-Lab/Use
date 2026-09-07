//! Assemble and sign the complete static registry tree.

use std::path::Path;

use a3s_use_core::UseResult;
use serde_json::{json, Map, Value};

use crate::admission::{catalog_custom, load_admissions, planning_custom};
use crate::catalog_build::assemble_admission;
use crate::keys::{role_key_value, KeySet};
use crate::tools_error;

/// Result evidence for one assembled publication.
pub(crate) struct AssembleOutcome {
    pub(crate) root_sha256: String,
    pub(crate) metadata_version: u64,
    pub(crate) target_names: Vec<String>,
}

/// CLI entry: build `metadata/` and `targets/` under `--out-root`.
pub(crate) async fn assemble_command(options: &crate::Options) -> UseResult<()> {
    let keys_dir = options.require("keys-dir")?;
    let admissions_path = options.require("admissions")?;
    let out_root = Path::new(&options.require("out-root")?).to_path_buf();
    let metadata_version: u64 = options
        .optional("metadata-version")
        .map(|value| {
            value.parse().map_err(|_| {
                tools_error(
                    "registry_tools.arguments_invalid",
                    "--metadata-version must be a positive integer.",
                )
            })
        })
        .transpose()?
        .unwrap_or(1);
    if metadata_version == 0 {
        return Err(tools_error(
            "registry_tools.arguments_invalid",
            "--metadata-version must be a positive integer.",
        ));
    }
    let root_expires = options
        .optional("root-expires")
        .unwrap_or_else(|| default_expiry(365));
    let metadata_expires = options
        .optional("metadata-expires")
        .unwrap_or_else(|| default_expiry(30));

    let outcome = assemble_registry(
        &KeySet::load(Path::new(&keys_dir))?,
        Path::new(&admissions_path),
        &out_root,
        metadata_version,
        &root_expires,
        &metadata_expires,
    )
    .await?;
    println!(
        "{}",
        serde_json::json!({
            "rootSha256": format!("sha256:{}", outcome.root_sha256),
            "metadataVersion": outcome.metadata_version,
            "targetNames": outcome.target_names,
            "outRoot": out_root.display().to_string(),
        })
    );
    Ok(())
}

fn default_expiry(days: u64) -> String {
    let seconds = (days * 24 * 60 * 60) as i64;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);
    let expiry = now + seconds;
    // RFC 3339 without pulling a datetime dependency: epoch-day arithmetic.
    format_epoch_as_rfc3339(expiry)
}

fn format_epoch_as_rfc3339(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let seconds_of_day = epoch_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's civil-from-days algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Sign and write the complete tree for the admitted package set.
pub(crate) async fn assemble_registry(
    keys: &KeySet,
    admissions_path: &Path,
    out_root: &Path,
    metadata_version: u64,
    root_expires: &str,
    metadata_expires: &str,
) -> UseResult<AssembleOutcome> {
    let admissions = load_admissions(admissions_path)?;
    let mut assembled = Vec::new();
    for admission in &admissions {
        assembled.push(assemble_admission(admission).await?);
    }

    let mut tuf_keys = Map::new();
    let mut roles = Map::new();
    for key in [&keys.root, &keys.targets, &keys.snapshot, &keys.timestamp] {
        tuf_keys.insert(key.key_id.clone(), role_key_value(key));
        roles.insert(
            key.role.to_string(),
            json!({"keyids": [key.key_id.clone()], "threshold": 1}),
        );
    }
    let root_signed = json!({
        "_type": "root",
        "spec_version": "1.0.0",
        "consistent_snapshot": false,
        "version": 1,
        "expires": root_expires,
        "keys": Value::Object(tuf_keys),
        "roles": Value::Object(roles),
    });
    let root =
        a3s_use_extension::sign_tuf_document(&keys.root.pair, &keys.root.key_id, root_signed);
    let root_sha256 = a3s_use_extension::sha256_hex(&root);

    let mut targets_map = Map::new();
    let mut target_writes = Vec::new();
    let mut target_names = Vec::new();
    for entry in &assembled {
        let record: Value = serde_json::from_slice(&entry.record_json).map_err(|error| {
            tools_error(
                "registry_tools.assemble_failed",
                &format!(
                    "The catalog record for '{}' cannot be re-encoded: {error}",
                    entry.record.package_id
                ),
            )
        })?;
        targets_map.insert(
            entry.archive_target_name.clone(),
            json!({
                "length": entry.archive.len(),
                "hashes": {"sha256": a3s_use_extension::sha256_hex(&entry.archive)},
                "custom": catalog_custom(&record),
            }),
        );
        target_writes.push((entry.archive_target_name.clone(), entry.archive.clone()));
        target_names.push(entry.archive_target_name.clone());
        if let Some(planning) = &entry.planning {
            targets_map.insert(
                planning.target_name.clone(),
                json!({
                    "length": planning.bytes.len(),
                    "hashes": {"sha256": a3s_use_extension::sha256_hex(&planning.bytes)},
                    "custom": planning_custom(),
                }),
            );
            target_writes.push((planning.target_name.clone(), planning.bytes.clone()));
            target_names.push(planning.target_name.clone());
        }
    }
    let targets_signed = json!({
        "_type": "targets",
        "spec_version": "1.0.0",
        "version": metadata_version,
        "expires": metadata_expires,
        "targets": Value::Object(targets_map),
    });
    let targets = a3s_use_extension::sign_tuf_document(
        &keys.targets.pair,
        &keys.targets.key_id,
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
        &keys.snapshot.pair,
        &keys.snapshot.key_id,
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
        &keys.timestamp.pair,
        &keys.timestamp.key_id,
        timestamp_signed,
    );

    let metadata_directory = out_root.join("metadata");
    let targets_directory = out_root.join("targets");
    std::fs::create_dir_all(&metadata_directory).map_err(|error| {
        tools_error(
            "registry_tools.assemble_failed",
            &format!(
                "Failed to create '{}': {error}",
                metadata_directory.display()
            ),
        )
    })?;
    write_bytes(&metadata_directory.join("root.json"), &root)?;
    write_bytes(&metadata_directory.join("timestamp.json"), &timestamp)?;
    write_bytes(&metadata_directory.join("snapshot.json"), &snapshot)?;
    write_bytes(&metadata_directory.join("targets.json"), &targets)?;
    for (target_name, bytes) in target_writes {
        let path = targets_directory.join(&target_name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                tools_error(
                    "registry_tools.assemble_failed",
                    &format!("Failed to create '{}': {error}", parent.display()),
                )
            })?;
        }
        write_bytes(&path, &bytes)?;
    }
    Ok(AssembleOutcome {
        root_sha256,
        metadata_version,
        target_names,
    })
}

fn write_bytes(path: &Path, bytes: &[u8]) -> UseResult<()> {
    std::fs::write(path, bytes).map_err(|error| {
        tools_error(
            "registry_tools.assemble_failed",
            &format!("Failed to write '{}': {error}", path.display()),
        )
    })
}
