//! Load the Registry-owned capability-description trust store from a signed
//! TUF target.
//!
//! Description verification keys are public policy under the same Registry
//! trust root as package catalogs. A production host must not inject fixture
//! keys on the product path; it loads this fixed target, then feeds the
//! verified store into signed descriptor projection.

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::fs;
use tough::{Repository, TargetName};

use crate::capability_description_verifier::{
    CapabilityDescriptionTrustStore, CAPABILITY_DESCRIPTION_TRUST_STORE_SCHEMA_V1,
};

use super::catalog::{load_verified_cached_repository, record_catalog_refresh};
use super::download::download_and_cache_target;
use super::target_cache::stage_cached_target;
use super::{
    hex_lower, load_repository, verified_registry_metadata, RemoteRegistryAccess, TrustedRegistry,
};

pub const CAPABILITY_DESCRIPTION_TRUST_STORE_TARGET: &str =
    "capability/description-trust-store-v1.json";
const TRUST_STORE_CUSTOM_KEY: &str = "a3sCapabilityDescriptionTrustStore";
const MAX_TRUST_STORE_TARGET_BYTES: u64 = 512 * 1024;

/// Provenance-bound trust store loaded from a signed Registry target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedCapabilityDescriptionTrustStore {
    pub registry_name: String,
    pub registry_url: String,
    pub root_sha256: String,
    pub snapshot_version: u64,
    pub targets_version: u64,
    pub target_name: String,
    pub target_sha256: String,
    pub target_byte_length: u64,
    pub store: CapabilityDescriptionTrustStore,
}

impl VerifiedCapabilityDescriptionTrustStore {
    pub fn store(&self) -> &CapabilityDescriptionTrustStore {
        &self.store
    }

    pub fn into_store(self) -> CapabilityDescriptionTrustStore {
        self.store
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.registry_name.trim().is_empty()
            || self.registry_url.trim().is_empty()
            || !valid_digest(&self.root_sha256)
            || !valid_digest(&self.target_sha256)
            || self.target_name != CAPABILITY_DESCRIPTION_TRUST_STORE_TARGET
            || self.target_byte_length == 0
            || self.target_byte_length > MAX_TRUST_STORE_TARGET_BYTES
        {
            return Err(trust_store_invalid(
                "The verified capability description trust store provenance is invalid.",
            ));
        }
        self.store.validate()?;
        Ok(())
    }
}

/// Refresh Registry metadata and load the signed description trust store.
pub async fn load_capability_description_trust_store(
    registry: &TrustedRegistry,
) -> UseResult<VerifiedCapabilityDescriptionTrustStore> {
    let repository = load_repository(registry).await?;
    let metadata = verified_registry_metadata(registry, &repository)?;
    record_catalog_refresh(registry, &repository, &metadata).await?;
    load_trust_store(registry, &repository, RemoteRegistryAccess::Refreshed).await
}

/// Load the signed description trust store from already-verified cached metadata.
pub async fn load_cached_capability_description_trust_store(
    registry: &TrustedRegistry,
) -> UseResult<VerifiedCapabilityDescriptionTrustStore> {
    let repository = load_verified_cached_repository(registry).await?;
    load_trust_store(registry, &repository, RemoteRegistryAccess::Cached).await
}

async fn load_trust_store(
    registry: &TrustedRegistry,
    repository: &Repository,
    access: RemoteRegistryAccess,
) -> UseResult<VerifiedCapabilityDescriptionTrustStore> {
    let target_name = TargetName::new(CAPABILITY_DESCRIPTION_TRUST_STORE_TARGET).map_err(|_| {
        trust_store_invalid("The capability description trust store target name is invalid.")
    })?;
    let target = find_target(repository, &target_name).ok_or_else(|| {
        trust_store_error(
            "use.extension.description_trust_store_missing",
            "The signed capability description trust store target is absent from TUF metadata.",
        )
    })?;
    let marker = target.custom.get(TRUST_STORE_CUSTOM_KEY);
    if marker
        != Some(&serde_json::json!({
            "schema": CAPABILITY_DESCRIPTION_TRUST_STORE_SCHEMA_V1
        }))
    {
        return Err(trust_store_invalid(
            "The capability description trust store target has invalid signed role metadata.",
        ));
    }
    let target_sha256 = signed_target_sha256(target)?;
    if target.length == 0 || target.length > MAX_TRUST_STORE_TARGET_BYTES {
        return Err(trust_store_invalid(
            "The capability description trust store target exceeds its byte bound.",
        ));
    }
    let bytes = load_target_bytes(
        registry,
        repository,
        &target_name,
        target.length,
        &target_sha256,
        access,
    )
    .await?;
    if bytes.len() as u64 != target.length {
        return Err(trust_store_invalid(
            "The downloaded capability description trust store length does not match TUF metadata.",
        ));
    }
    let store = CapabilityDescriptionTrustStore::from_json(&bytes).map_err(|error| {
        trust_store_invalid(format!(
            "Failed to decode the signed capability description trust store: {}",
            error.message
        ))
    })?;
    let verified = VerifiedCapabilityDescriptionTrustStore {
        registry_name: registry.name().to_owned(),
        registry_url: registry.base_url().as_str().to_owned(),
        root_sha256: format!("sha256:{}", normalize_digest(registry.root_sha256())),
        snapshot_version: repository.snapshot().signed.version.get(),
        targets_version: repository.targets().signed.version.get(),
        target_name: CAPABILITY_DESCRIPTION_TRUST_STORE_TARGET.to_owned(),
        target_sha256,
        target_byte_length: target.length,
        store,
    };
    verified.validate()?;
    Ok(verified)
}

async fn load_target_bytes(
    registry: &TrustedRegistry,
    repository: &Repository,
    target_name: &TargetName,
    length: u64,
    sha256: &str,
    access: RemoteRegistryAccess,
) -> UseResult<Vec<u8>> {
    let digest = normalize_digest(sha256);
    let file_name = target_name
        .resolved()
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            trust_store_invalid("The capability description trust store file name is invalid.")
        })?;
    let (_temporary, path): (TempDir, _) = match access {
        RemoteRegistryAccess::Refreshed => {
            download_and_cache_target(
                repository,
                target_name,
                registry.datastore(),
                registry.artifact_store(),
                &registry.targets_url()?,
                registry.network_policy(),
                repository.root().signed.consistent_snapshot,
                length,
                digest,
                registry.target_cache_policy(),
                false,
            )
            .await?
        }
        RemoteRegistryAccess::Cached => {
            stage_cached_target(registry, file_name, length, digest).await?
        }
    };
    fs::read(&path).await.map_err(|error| {
        trust_store_error(
            "use.extension.description_trust_store_io",
            format!("Failed to read the verified capability description trust store: {error}"),
        )
    })
}

fn signed_target_sha256(target: &tough::schema::Target) -> UseResult<String> {
    let bytes = target.hashes.sha256.as_ref();
    if bytes.len() != 32 {
        return Err(trust_store_invalid(
            "The capability description trust store target has no valid SHA-256 digest.",
        ));
    }
    Ok(format!("sha256:{}", hex_lower(bytes)))
}

fn find_target<'a>(
    repository: &'a Repository,
    target_name: &TargetName,
) -> Option<&'a tough::schema::Target> {
    repository
        .all_targets()
        .find(|(name, _)| *name == target_name)
        .map(|(_, target)| target)
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn normalize_digest(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
}

fn trust_store_invalid(message: impl Into<String>) -> UseError {
    trust_store_error("use.extension.description_trust_store_invalid", message)
}

fn trust_store_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

/// Signed custom metadata object written beside the trust-store target.
pub fn description_trust_store_custom() -> serde_json::Value {
    serde_json::json!({
        TRUST_STORE_CUSTOM_KEY: {
            "schema": CAPABILITY_DESCRIPTION_TRUST_STORE_SCHEMA_V1
        }
    })
}
