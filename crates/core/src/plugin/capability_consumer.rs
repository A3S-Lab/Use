//! Typed consumer-profile negotiation for the capability boundary.
//!
//! The profile is a host-to-host contract. It describes which optional A3S
//! metadata a consumer is prepared to receive while keeping the universal
//! agent-facing surface on standard MCP. A negotiation never silently drops a
//! requested extension: every requested extension must be explicitly
//! supported by the embedding host.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::validation::strictly_sorted_unique;
use super::{canonical_digest, canonical_json, contract_error, parse_contract};

/// Current wire schema for a consumer profile request.
pub const CAPABILITY_CONSUMER_PROFILE_SCHEMA_V1: &str = "a3s.use.capability-consumer-profile.v1";
/// Current wire schema for a successful consumer negotiation.
pub const CAPABILITY_CONSUMER_NEGOTIATION_SCHEMA_V1: &str =
    "a3s.use.capability-consumer-negotiation.v1";

const PROFILE_ERROR: &str = "use.plugin.capability_consumer_profile_invalid";
const NEGOTIATION_ERROR: &str = "use.plugin.capability_consumer_negotiation_invalid";
/// Maximum number of optional extensions in one profile or negotiation.
pub const MAX_CAPABILITY_CONSUMER_EXTENSIONS: usize = 8;

/// The kind of consumer asking to bind to a capability publication.
///
/// `GenericMcp` is deliberately limited to the universal MCP boundary. An
/// `A3s` consumer may opt into the explicitly named A3S metadata extensions,
/// but those extensions never change the package generation or invocation
/// authority carried by the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityConsumerKind {
    GenericMcp,
    A3s,
}

/// Optional metadata extensions that an A3S consumer may negotiate.
///
/// These values are negotiation labels, not authorization grants. The host
/// must still bind each extension to the same immutable package publication
/// and policy as the standard MCP catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityConsumerExtension {
    Flow,
    Knowledge,
    Ui,
}

/// A bounded request for one consumer profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityConsumerProfile {
    pub schema: String,
    pub kind: CapabilityConsumerKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requested_extensions: Vec<CapabilityConsumerExtension>,
}

impl CapabilityConsumerProfile {
    /// Construct the no-extension profile used by ordinary MCP clients.
    pub fn generic_mcp() -> Self {
        Self {
            schema: CAPABILITY_CONSUMER_PROFILE_SCHEMA_V1.to_owned(),
            kind: CapabilityConsumerKind::GenericMcp,
            requested_extensions: Vec::new(),
        }
    }

    /// Construct an A3S profile with an explicit extension request set.
    pub fn a3s(
        extensions: impl IntoIterator<Item = CapabilityConsumerExtension>,
    ) -> UseResult<Self> {
        let profile = Self {
            schema: CAPABILITY_CONSUMER_PROFILE_SCHEMA_V1.to_owned(),
            kind: CapabilityConsumerKind::A3s,
            requested_extensions: sorted_extensions(extensions)?,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// Decode and validate a bounded profile document.
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "capability consumer profile",
            PROFILE_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != CAPABILITY_CONSUMER_PROFILE_SCHEMA_V1
            || self.requested_extensions.len() > MAX_CAPABILITY_CONSUMER_EXTENSIONS
            || !strictly_sorted_unique(&self.requested_extensions)
            || (self.kind == CapabilityConsumerKind::GenericMcp
                && !self.requested_extensions.is_empty())
        {
            return Err(profile_error(
                "The capability consumer profile schema, ordering, or extension policy is invalid.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "capability consumer profile", PROFILE_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn kind(&self) -> CapabilityConsumerKind {
        self.kind
    }

    pub fn requested_extensions(&self) -> &[CapabilityConsumerExtension] {
        &self.requested_extensions
    }

    pub fn is_generic_mcp(&self) -> bool {
        self.kind == CapabilityConsumerKind::GenericMcp
    }
}

/// The result of an explicit profile negotiation.
///
/// The accepted set is required to equal the requested set for an A3S
/// profile. This prevents a host from silently degrading a consumer into a
/// less capable contract while still reporting success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityConsumerNegotiation {
    pub schema: String,
    pub profile: CapabilityConsumerProfile,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_extensions: Vec<CapabilityConsumerExtension>,
}

impl CapabilityConsumerNegotiation {
    /// Construct the default generic MCP negotiation.
    pub fn generic_mcp() -> Self {
        Self {
            schema: CAPABILITY_CONSUMER_NEGOTIATION_SCHEMA_V1.to_owned(),
            profile: CapabilityConsumerProfile::generic_mcp(),
            accepted_extensions: Vec::new(),
        }
    }

    /// Negotiate a profile against the extensions explicitly exposed by the
    /// host. Unsupported requested extensions are rejected instead of being
    /// silently omitted.
    pub fn negotiate(
        profile: CapabilityConsumerProfile,
        supported_extensions: impl IntoIterator<Item = CapabilityConsumerExtension>,
    ) -> UseResult<Self> {
        profile.validate()?;
        let supported = sorted_extensions(supported_extensions)?;
        if profile
            .requested_extensions
            .iter()
            .any(|extension| !supported.contains(extension))
        {
            return Err(negotiation_error(
                "The host does not support every requested consumer extension.",
            ));
        }
        let negotiation = Self {
            schema: CAPABILITY_CONSUMER_NEGOTIATION_SCHEMA_V1.to_owned(),
            accepted_extensions: profile.requested_extensions.clone(),
            profile,
        };
        negotiation.validate()?;
        Ok(negotiation)
    }

    /// Decode and validate a bounded negotiation document.
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "capability consumer negotiation",
            NEGOTIATION_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        self.profile.validate()?;
        if self.schema != CAPABILITY_CONSUMER_NEGOTIATION_SCHEMA_V1
            || self.accepted_extensions.len() > MAX_CAPABILITY_CONSUMER_EXTENSIONS
            || !strictly_sorted_unique(&self.accepted_extensions)
            || self.accepted_extensions != self.profile.requested_extensions
        {
            return Err(negotiation_error(
                "The capability consumer negotiation schema or accepted set is invalid.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "capability consumer negotiation", NEGOTIATION_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn profile(&self) -> &CapabilityConsumerProfile {
        &self.profile
    }

    pub fn accepted_extensions(&self) -> &[CapabilityConsumerExtension] {
        &self.accepted_extensions
    }

    pub fn accepts(&self, extension: CapabilityConsumerExtension) -> bool {
        self.accepted_extensions.contains(&extension)
    }
}

fn sorted_extensions(
    extensions: impl IntoIterator<Item = CapabilityConsumerExtension>,
) -> UseResult<Vec<CapabilityConsumerExtension>> {
    let mut values = extensions.into_iter().collect::<Vec<_>>();
    if values.len() > MAX_CAPABILITY_CONSUMER_EXTENSIONS {
        return Err(profile_error(
            "The capability consumer extension set exceeds its bound.",
        ));
    }
    values.sort();
    let mut unique = BTreeSet::new();
    if values.iter().any(|value| !unique.insert(*value)) {
        return Err(profile_error(
            "The capability consumer extension set contains duplicates.",
        ));
    }
    Ok(values)
}

fn profile_error(message: impl Into<String>) -> UseError {
    contract_error(PROFILE_ERROR, message)
}

fn negotiation_error(message: impl Into<String>) -> UseError {
    contract_error(NEGOTIATION_ERROR, message)
}

#[cfg(test)]
#[path = "capability_consumer_tests.rs"]
mod tests;
