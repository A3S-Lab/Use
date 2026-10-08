use std::collections::BTreeMap;

use crate::error::ToolAcquireError;
use crate::spec::ToolSpec;

/// One file that must sit beside the executable in the version directory.
///
/// `relative_path` uses `/` and stays inside that directory. The store refuses
/// `..`, absolute paths, and a path that replaces the executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanionFile {
    pub relative_path: String,
    pub bytes: Vec<u8>,
    pub executable: bool,
}

/// One verified executable the store can publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPayload {
    pub version: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
    pub companions: Vec<CompanionFile>,
}

impl ToolPayload {
    /// An executable with no sibling files. Single-file tools use this.
    pub fn new(version: impl Into<String>, sha256: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            version: version.into(),
            sha256: sha256.into(),
            bytes,
            companions: Vec::new(),
        }
    }
}

/// A resolved spec, the executable name inside the payload, and an optional pin.
#[derive(Debug, Clone)]
pub struct ToolQuery {
    pub spec: ToolSpec,
    pub version: Option<String>,
    pub executable: String,
}

/// Supplies payload bytes. The store verifies `sha256` before it publishes.
pub trait ToolSource: Send + Sync {
    fn fetch(&self, query: &ToolQuery) -> Result<ToolPayload, ToolAcquireError>;
    fn latest(&self, spec: &ToolSpec) -> Result<String, ToolAcquireError>;
}

/// In-memory source for fixtures and hosts that already hold the bytes.
#[derive(Debug, Default, Clone)]
pub struct StaticSource {
    payloads: BTreeMap<String, ToolPayload>,
    latest: BTreeMap<String, String>,
    latest_failures: BTreeMap<String, ()>,
}

impl StaticSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, spec: impl Into<String>, payload: ToolPayload) -> &mut Self {
        self.payloads.insert(spec.into(), payload);
        self
    }

    pub fn insert_latest(
        &mut self,
        spec: impl Into<String>,
        version: impl Into<String>,
    ) -> &mut Self {
        let spec = spec.into();
        self.latest_failures.remove(&spec);
        self.latest.insert(spec, version.into());
        self
    }

    pub fn fail_latest(&mut self, spec: impl Into<String>) -> &mut Self {
        let spec = spec.into();
        self.latest.remove(&spec);
        self.latest_failures.insert(spec, ());
        self
    }
}

impl ToolSource for StaticSource {
    fn fetch(&self, query: &ToolQuery) -> Result<ToolPayload, ToolAcquireError> {
        self.payloads
            .get(query.spec.raw())
            .cloned()
            .ok_or(ToolAcquireError::AssetMissing)
    }

    fn latest(&self, spec: &ToolSpec) -> Result<String, ToolAcquireError> {
        if self.latest_failures.contains_key(spec.raw()) {
            return Err(ToolAcquireError::LatestUnavailable);
        }
        self.latest
            .get(spec.raw())
            .cloned()
            .ok_or(ToolAcquireError::LatestUnavailable)
    }
}
