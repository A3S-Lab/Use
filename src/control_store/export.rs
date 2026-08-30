use a3s_use_core::{InstallationId, UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::schema::{ControlStoreMetadata, CONTROL_STORE_SCHEMA_VERSION};

const CONTROL_STORE_EXPORT_SCHEMA: &str = "a3s.use.control-store-export.v1";
const MAX_CONTROL_STORE_EXPORT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlStoreExport {
    schema: String,
    store_schema_version: u32,
    pub(super) installation: InstallationId,
    pub(super) current_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifiedControlStoreExport {
    pub(super) export: ControlStoreExport,
    pub(super) descriptor_digest: String,
}

pub(super) fn encode(metadata: &ControlStoreMetadata) -> UseResult<Vec<u8>> {
    validate_metadata(metadata)?;
    let bytes = canonical_json(&ControlStoreExport {
        schema: CONTROL_STORE_EXPORT_SCHEMA.to_string(),
        store_schema_version: metadata.schema_version,
        installation: metadata.installation.clone(),
        current_generation: metadata.current_generation,
    })?;
    if bytes.len() > MAX_CONTROL_STORE_EXPORT_BYTES {
        return Err(export_error(
            "The canonical Control Store export exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

pub(super) fn verify(
    bytes: &[u8],
    expected_installation: &InstallationId,
) -> UseResult<VerifiedControlStoreExport> {
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_STORE_EXPORT_BYTES {
        return Err(export_error(
            "The Control Store export is empty or exceeds its byte bound.",
        ));
    }
    let export: ControlStoreExport = serde_json::from_slice(bytes)
        .map_err(|_| export_error("The Control Store export is not valid schema-v1 JSON."))?;
    validate_export(&export)?;
    let canonical = canonical_json(&export)?;
    if canonical != bytes {
        return Err(export_error(
            "The Control Store export is not in canonical JSON form.",
        ));
    }
    if export.installation != *expected_installation {
        return Err(identity_error());
    }
    Ok(VerifiedControlStoreExport {
        descriptor_digest: sha256_digest(&canonical),
        export,
    })
}

fn validate_export(export: &ControlStoreExport) -> UseResult<()> {
    if export.schema != CONTROL_STORE_EXPORT_SCHEMA
        || export.store_schema_version != CONTROL_STORE_SCHEMA_VERSION
        || export.installation.validate().is_err()
    {
        return Err(export_error(
            "The Control Store export identity or schema is invalid or unsupported.",
        ));
    }
    Ok(())
}

fn validate_metadata(metadata: &ControlStoreMetadata) -> UseResult<()> {
    if metadata.schema_version != CONTROL_STORE_SCHEMA_VERSION
        || metadata.installation.validate().is_err()
    {
        return Err(export_error(
            "The Control Store metadata cannot be represented by the current export schema.",
        ));
    }
    Ok(())
}

fn canonical_json<T: Serialize>(value: &T) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value
        .serialize(&mut serializer)
        .map_err(|error| export_error(format!("Canonical export encoding failed: {error}")))?;
    Ok(bytes)
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn identity_error() -> UseError {
    UseError::new(
        "use.control_store.identity_mismatch",
        "The Control Store export belongs to a different installation.",
    )
}

fn export_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.export_invalid", message)
}
