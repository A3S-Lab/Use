use serde::{Deserialize, Serialize};

use crate::UseResult;

use super::validation::{valid_registry_url, valid_segment, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PluginCatalogRecord,
};

const CATALOG_PROVENANCE_ERROR: &str = "use.plugin.catalog_provenance_invalid";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedCatalogProvenance {
    pub registry_name: String,
    pub registry_url: String,
    pub root_sha256: String,
    pub root_version: u64,
    pub timestamp_version: u64,
    pub snapshot_version: u64,
    pub targets_version: u64,
    pub catalog_record_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedPluginCatalogRecord {
    pub record: PluginCatalogRecord,
    pub provenance: VerifiedCatalogProvenance,
}

impl VerifiedCatalogProvenance {
    pub fn validate(&self) -> UseResult<()> {
        if !valid_segment(&self.registry_name)
            || !valid_registry_url(&self.registry_url)
            || !valid_sha256(&self.root_sha256)
            || self.root_version == 0
            || self.timestamp_version == 0
            || self.snapshot_version == 0
            || self.targets_version == 0
            || !valid_sha256(&self.catalog_record_digest)
        {
            return Err(provenance_error(
                "The verified catalog registry or TUF role evidence is invalid.",
            ));
        }
        Ok(())
    }
}

impl VerifiedPluginCatalogRecord {
    pub fn new(
        record: PluginCatalogRecord,
        provenance: VerifiedCatalogProvenance,
    ) -> UseResult<Self> {
        let verified = Self { record, provenance };
        verified.validate()?;
        Ok(verified)
    }

    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "verified plugin catalog record",
            CATALOG_PROVENANCE_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        self.record
            .validate()
            .map_err(|_| provenance_error("The verified catalog record is invalid."))?;
        self.provenance.validate()?;
        if self.record.descriptor_digest()? != self.provenance.catalog_record_digest {
            return Err(provenance_error(
                "The verified catalog provenance does not bind the canonical record.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(
            self,
            "verified plugin catalog record",
            CATALOG_PROVENANCE_ERROR,
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

fn provenance_error(message: impl Into<String>) -> crate::UseError {
    contract_error(CATALOG_PROVENANCE_ERROR, message)
}
