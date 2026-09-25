//! Ed25519 custody material for the four TUF roles.
//!
//! Root may be a single share (`root.key`, threshold 1) or a threshold set
//! (`root-0.key` … `root-(N-1).key` plus `root.policy.json`). Online roles
//! remain one key each.

use std::fs;
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools_error;

const ROLES: [&str; 4] = ["root", "targets", "snapshot", "timestamp"];
const ROOT_POLICY_FILE: &str = "root.policy.json";

/// One role key with its derived TUF key id and custody seed.
pub(crate) struct RoleKey {
    pub(crate) role: &'static str,
    pub(crate) pair: Ed25519KeyPair,
    pub(crate) key_id: String,
    seed: [u8; 32],
    /// File stem without `.key` (`root`, `root-0`, `targets`, …).
    file_stem: String,
}

impl RoleKey {
    fn from_seed(role: &'static str, seed: [u8; 32], file_stem: String) -> UseResult<Self> {
        let pair = Ed25519KeyPair::from_seed_unchecked(seed.as_slice()).map_err(|error| {
            tools_error(
                "registry_tools.keygen_failed",
                &format!("The {role} seed is unusable: {error}"),
            )
        })?;
        let key_id = a3s_use_extension::ed25519_key_id(pair.public_key().as_ref());
        Ok(Self {
            role,
            pair,
            key_id,
            seed,
            file_stem,
        })
    }
}

/// Published root signing policy for one keys directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RootPolicy {
    pub(crate) threshold: u64,
    pub(crate) share_count: u64,
}

/// The complete signing key set for one registry.
pub(crate) struct KeySet {
    pub(crate) root_policy: RootPolicy,
    pub(crate) roots: Vec<RoleKey>,
    pub(crate) targets: RoleKey,
    pub(crate) snapshot: RoleKey,
    pub(crate) timestamp: RoleKey,
}

#[derive(Serialize, Deserialize)]
struct StoredKey {
    role: String,
    key_type: String,
    scheme: String,
    key_id: String,
    public_key: String,
    seed: String,
}

impl KeySet {
    /// Generate independent role keys, optionally as a threshold root set.
    pub(crate) fn generate(root_share_count: u64, root_threshold: u64) -> UseResult<Self> {
        validate_root_policy(root_share_count, root_threshold)?;
        let mut roots = Vec::with_capacity(root_share_count as usize);
        for index in 0..root_share_count {
            let file_stem = if root_share_count == 1 {
                "root".to_owned()
            } else {
                format!("root-{index}")
            };
            roots.push(generate_role("root", file_stem)?);
        }
        Ok(Self {
            root_policy: RootPolicy {
                threshold: root_threshold,
                share_count: root_share_count,
            },
            roots,
            targets: generate_role("targets", "targets".to_owned())?,
            snapshot: generate_role("snapshot", "snapshot".to_owned())?,
            timestamp: generate_role("timestamp", "timestamp".to_owned())?,
        })
    }

    /// Persist seed files (and root policy when thresholded) with 0600 permissions.
    ///
    /// Writing into an existing key file is refused so an accidental rerun
    /// cannot silently rotate a registry's identity.
    pub(crate) fn save(&self, directory: &Path) -> UseResult<Vec<PathBuf>> {
        if !directory.exists() {
            fs::create_dir_all(directory).map_err(|error| {
                tools_error(
                    "registry_tools.keys_write_failed",
                    &format!(
                        "Failed to create keys directory '{}': {error}",
                        directory.display()
                    ),
                )
            })?;
        }
        let mut written = Vec::new();
        for key in self
            .roots
            .iter()
            .chain([&self.targets, &self.snapshot, &self.timestamp])
        {
            let path = directory.join(format!("{}.key", key.file_stem));
            if path.exists() {
                return Err(tools_error(
                    "registry_tools.keys_write_failed",
                    &format!(
                        "Refusing to overwrite existing key file '{}'; move it aside to rotate.",
                        path.display()
                    ),
                ));
            }
            let stored = StoredKey {
                role: key.role.to_string(),
                key_type: "ed25519".to_string(),
                scheme: "ed25519".to_string(),
                key_id: key.key_id.clone(),
                public_key: a3s_use_extension::hex_lower(key.pair.public_key().as_ref()),
                seed: a3s_use_extension::hex_lower(&key.seed),
            };
            let bytes = serde_json::to_vec_pretty(&stored).map_err(|error| {
                tools_error(
                    "registry_tools.keys_write_failed",
                    &format!("Failed to serialize key material: {error}"),
                )
            })?;
            write_private(&path, &bytes)?;
            written.push(path);
        }
        if self.root_policy.share_count > 1 || self.root_policy.threshold > 1 {
            let policy_path = directory.join(ROOT_POLICY_FILE);
            if policy_path.exists() {
                return Err(tools_error(
                    "registry_tools.keys_write_failed",
                    &format!(
                        "Refusing to overwrite existing root policy '{}'; move it aside to rotate.",
                        policy_path.display()
                    ),
                ));
            }
            let bytes = serde_json::to_vec_pretty(&self.root_policy).map_err(|error| {
                tools_error(
                    "registry_tools.keys_write_failed",
                    &format!("Failed to serialize root policy: {error}"),
                )
            })?;
            write_private(&policy_path, &bytes)?;
            written.push(policy_path);
        }
        Ok(written)
    }

    /// Load a previously generated key set.
    pub(crate) fn load(directory: &Path) -> UseResult<Self> {
        let roots = load_root_shares(directory)?;
        let root_policy = load_root_policy(directory, roots.len() as u64)?;
        if (roots.len() as u64) < root_policy.threshold {
            return Err(tools_error(
                "registry_tools.keys_read_failed",
                &format!(
                    "Root threshold is {} but only {} share(s) are present under '{}'.",
                    root_policy.threshold,
                    roots.len(),
                    directory.display()
                ),
            ));
        }
        Ok(Self {
            root_policy,
            roots,
            targets: load_role(directory, "targets", "targets")?,
            snapshot: load_role(directory, "snapshot", "snapshot")?,
            timestamp: load_role(directory, "timestamp", "timestamp")?,
        })
    }
}

fn validate_root_policy(share_count: u64, threshold: u64) -> UseResult<()> {
    if share_count == 0 {
        return Err(tools_error(
            "registry_tools.arguments_invalid",
            "--root-share-count must be at least 1.",
        ));
    }
    if threshold == 0 || threshold > share_count {
        return Err(tools_error(
            "registry_tools.arguments_invalid",
            "--root-threshold must be between 1 and --root-share-count inclusive.",
        ));
    }
    Ok(())
}

fn load_root_policy(directory: &Path, available_shares: u64) -> UseResult<RootPolicy> {
    let path = directory.join(ROOT_POLICY_FILE);
    if !path.exists() {
        return Ok(RootPolicy {
            threshold: 1,
            share_count: available_shares.max(1),
        });
    }
    let bytes = fs::read(&path).map_err(|error| {
        tools_error(
            "registry_tools.keys_read_failed",
            &format!("Failed to read root policy '{}': {error}", path.display()),
        )
    })?;
    let policy: RootPolicy = serde_json::from_slice(&bytes).map_err(|error| {
        tools_error(
            "registry_tools.keys_read_failed",
            &format!(
                "Root policy '{}' is not valid JSON: {error}",
                path.display()
            ),
        )
    })?;
    validate_root_policy(policy.share_count, policy.threshold)?;
    if available_shares > policy.share_count {
        return Err(tools_error(
            "registry_tools.keys_read_failed",
            &format!(
                "Root policy declares {} share(s) but {} root key file(s) are present.",
                policy.share_count, available_shares
            ),
        ));
    }
    Ok(policy)
}

fn load_root_shares(directory: &Path) -> UseResult<Vec<RoleKey>> {
    let single = directory.join("root.key");
    if single.exists() {
        return Ok(vec![load_role(directory, "root", "root")?]);
    }
    let mut roots = Vec::new();
    let mut index = 0_u64;
    loop {
        let stem = format!("root-{index}");
        let path = directory.join(format!("{stem}.key"));
        if !path.exists() {
            break;
        }
        roots.push(load_role(directory, "root", &stem)?);
        index += 1;
        if index > 64 {
            return Err(tools_error(
                "registry_tools.keys_read_failed",
                "Refusing to load more than 64 root shares from one keys directory.",
            ));
        }
    }
    if roots.is_empty() {
        return Err(tools_error(
            "registry_tools.keys_read_failed",
            &format!(
                "Failed to read key file '{}': no root.key or root-0.key present.",
                directory.join("root.key").display()
            ),
        ));
    }
    Ok(roots)
}

fn generate_role(role: &'static str, file_stem: String) -> UseResult<RoleKey> {
    let mut seed = [0_u8; 32];
    SystemRandom::new().fill(&mut seed).map_err(|error| {
        tools_error(
            "registry_tools.keygen_failed",
            &format!("Failed to generate the {role} key seed: {error}"),
        )
    })?;
    RoleKey::from_seed(role, seed, file_stem)
}

fn load_role(directory: &Path, role: &str, file_stem: &str) -> UseResult<RoleKey> {
    let path = directory.join(format!("{file_stem}.key"));
    let bytes = fs::read(&path).map_err(|error| {
        tools_error(
            "registry_tools.keys_read_failed",
            &format!("Failed to read key file '{}': {error}", path.display()),
        )
    })?;
    let stored: StoredKey = serde_json::from_slice(&bytes).map_err(|error| {
        tools_error(
            "registry_tools.keys_read_failed",
            &format!("Key file '{}' is not valid JSON: {error}", path.display()),
        )
    })?;
    if stored.role != role || stored.key_type != "ed25519" || stored.scheme != "ed25519" {
        return Err(tools_error(
            "registry_tools.keys_read_failed",
            &format!(
                "Key file '{}' does not describe the {role} role.",
                path.display()
            ),
        ));
    }
    let seed = decode_hex(&stored.seed).ok_or_else(|| {
        tools_error(
            "registry_tools.keys_read_failed",
            &format!("Key file '{}' has an invalid seed.", path.display()),
        )
    })?;
    let seed: [u8; 32] = seed.try_into().map_err(|_| {
        tools_error(
            "registry_tools.keys_read_failed",
            &format!("Key file '{}' seed must be 32 bytes.", path.display()),
        )
    })?;
    let key = RoleKey::from_seed(
        ROLES
            .iter()
            .find(|candidate| **candidate == role)
            .copied()
            .expect("the role name was validated against the fixed role list before construction"),
        seed,
        file_stem.to_owned(),
    )
    .map_err(|error| {
        tools_error(
            "registry_tools.keys_read_failed",
            &format!(
                "Key file '{}' seed is unusable: {}",
                path.display(),
                error.message
            ),
        )
    })?;
    if key.key_id != stored.key_id {
        return Err(tools_error(
            "registry_tools.keys_read_failed",
            &format!(
                "Key file '{}' declares key id {} but its seed derives {}.",
                path.display(),
                stored.key_id,
                key.key_id
            ),
        ));
    }
    Ok(key)
}

fn write_private(path: &Path, bytes: &[u8]) -> UseResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = fs::write(path, bytes)
            .and_then(|()| fs::set_permissions(path, fs::Permissions::from_mode(0o600)))
        {
            return Err(private_write_error(path, &error.to_string()));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, bytes).map_err(|error| private_write_error(path, &error.to_string()))
    }
}

#[cfg(unix)]
fn private_write_error(path: &Path, error: &str) -> UseError {
    tools_error(
        "registry_tools.keys_write_failed",
        &format!(
            "Failed to write '{}' with 0600 permissions: {error}",
            path.display()
        ),
    )
}

#[cfg(not(unix))]
fn private_write_error(path: &Path, error: &str) -> UseError {
    tools_error(
        "registry_tools.keys_write_failed",
        &format!("Failed to write '{}': {error}", path.display()),
    )
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if input.len() % 2 != 0 {
        return None;
    }
    (0..input.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&input[index..index + 2], 16).ok())
        .collect()
}

fn parse_positive_u64(name: &str, value: &str) -> UseResult<u64> {
    value.parse::<u64>().map_err(|_| {
        tools_error(
            "registry_tools.arguments_invalid",
            &format!("--{name} must be a positive integer."),
        )
    })
}

/// CLI entry: generate and persist one independent key per TUF role.
pub(crate) fn keygen_command(options: &crate::Options) -> UseResult<()> {
    let directory = PathBuf::from(options.require("keys-dir")?);
    let root_share_count = options
        .optional("root-share-count")
        .map(|value| parse_positive_u64("root-share-count", &value))
        .transpose()?
        .unwrap_or(1);
    let root_threshold = options
        .optional("root-threshold")
        .map(|value| parse_positive_u64("root-threshold", &value))
        .transpose()?
        .unwrap_or(1);
    let key_set = KeySet::generate(root_share_count, root_threshold)?;
    let written = key_set.save(&directory)?;
    let root_key_ids: Vec<&str> = key_set
        .roots
        .iter()
        .map(|key| key.key_id.as_str())
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "keysDir": directory.display().to_string(),
            "written": written.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
            "rootPolicy": {
                "threshold": key_set.root_policy.threshold,
                "shareCount": key_set.root_policy.share_count,
            },
            "roleKeyIds": {
                "root": root_key_ids,
                "targets": key_set.targets.key_id,
                "snapshot": key_set.snapshot.key_id,
                "timestamp": key_set.timestamp.key_id,
            },
        })
    );
    Ok(())
}

/// The public key object for one role, in TUF wire form.
pub(crate) fn role_key_value(key: &RoleKey) -> serde_json::Value {
    json!({
        "keytype": "ed25519",
        "scheme": "ed25519",
        "keyval": {"public": a3s_use_extension::hex_lower(key.pair.public_key().as_ref())}
    })
}
