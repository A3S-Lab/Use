use serde::{Deserialize, Serialize};

use crate::error::ToolAcquireError;

pub const RECEIPT_SCHEMA: &str = "a3s.use.tool-acquire.receipt.v1";
pub const FAILURE_SCHEMA: &str = "a3s.use.tool-acquire.failure.v1";

/// Machine record that a version directory is the current install.
///
/// This is not a package receipt and must not be read as one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolReceipt {
    pub schema: String,
    pub name: String,
    pub spec: String,
    pub version: String,
    pub executable: String,
    pub sha256: String,
    pub installed_at_ms: u64,
}

/// Retry record for an install that did not become current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolFailureRecord {
    pub schema: String,
    pub name: String,
    pub spec: String,
    pub version: String,
    pub code: String,
    pub message: String,
    pub failed_at_ms: u64,
}

impl ToolReceipt {
    pub(crate) fn new(
        name: &str,
        spec: &str,
        version: &str,
        executable: &str,
        sha256: &str,
    ) -> Result<Self, ToolAcquireError> {
        Ok(Self {
            schema: RECEIPT_SCHEMA.to_string(),
            name: name.to_string(),
            spec: spec.to_string(),
            version: version.to_string(),
            executable: executable.to_string(),
            sha256: sha256.to_string(),
            installed_at_ms: unix_millis()?,
        })
    }
}

impl ToolFailureRecord {
    pub(crate) fn new(
        name: &str,
        spec: &str,
        version: &str,
        error: &ToolAcquireError,
    ) -> Result<Self, ToolAcquireError> {
        Ok(Self {
            schema: FAILURE_SCHEMA.to_string(),
            name: name.to_string(),
            spec: spec.to_string(),
            version: version.to_string(),
            code: error.code().to_string(),
            message: error.to_string(),
            failed_at_ms: unix_millis()?,
        })
    }
}

fn unix_millis() -> Result<u64, ToolAcquireError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .map_err(|error| {
            ToolAcquireError::io(
                "read system clock",
                std::path::PathBuf::from("."),
                std::io::Error::other(error),
            )
        })
}
