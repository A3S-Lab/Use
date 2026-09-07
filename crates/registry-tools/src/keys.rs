//! Ed25519 custody material for the four TUF roles.

use std::fs;
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools_error;

const ROLES: [&str; 4] = ["root", "targets", "snapshot", "timestamp"];

/// One role key with its derived TUF key id and custody seed.
pub(crate) struct RoleKey {
    pub(crate) role: &'static str,
    pub(crate) pair: Ed25519KeyPair,
    pub(crate) key_id: String,
    seed: [u8; 32],
}

impl RoleKey {
    fn from_seed(role: &'static str, seed: [u8; 32]) -> UseResult<Self> {
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
        })
    }

    fn file_stem(&self) -> String {
        format!("{}.key", self.role)
    }
}

/// The complete signing key set for one registry.
pub(crate) struct KeySet {
    pub(crate) root: RoleKey,
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
    /// Generate four fresh independent role keys.
    pub(crate) fn generate() -> UseResult<Self> {
        Ok(Self {
            root: generate_role("root")?,
            targets: generate_role("targets")?,
            snapshot: generate_role("snapshot")?,
            timestamp: generate_role("timestamp")?,
        })
    }

    /// Persist one seed file per role with 0600 permissions.
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
        for key in [&self.root, &self.targets, &self.snapshot, &self.timestamp] {
            let path = directory.join(key.file_stem());
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
        Ok(written)
    }

    /// Load a previously generated key set.
    pub(crate) fn load(directory: &Path) -> UseResult<Self> {
        Ok(Self {
            root: load_role(directory, "root")?,
            targets: load_role(directory, "targets")?,
            snapshot: load_role(directory, "snapshot")?,
            timestamp: load_role(directory, "timestamp")?,
        })
    }
}

fn generate_role(role: &'static str) -> UseResult<RoleKey> {
    let mut seed = [0_u8; 32];
    SystemRandom::new().fill(&mut seed).map_err(|error| {
        tools_error(
            "registry_tools.keygen_failed",
            &format!("Failed to generate the {role} key seed: {error}"),
        )
    })?;
    RoleKey::from_seed(role, seed)
}

fn load_role(directory: &Path, role: &str) -> UseResult<RoleKey> {
    let path = directory.join(format!("{role}.key"));
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

/// CLI entry: generate and persist one independent key per TUF role.
pub(crate) fn keygen_command(options: &crate::Options) -> UseResult<()> {
    let directory = PathBuf::from(options.require("keys-dir")?);
    let key_set = KeySet::generate()?;
    let written = key_set.save(&directory)?;
    println!(
        "{}",
        serde_json::json!({
            "keysDir": directory.display().to_string(),
            "written": written.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
            "roleKeyIds": {
                "root": key_set.root.key_id,
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
