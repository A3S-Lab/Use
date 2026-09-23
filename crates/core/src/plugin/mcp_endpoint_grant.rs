//! Signed contract for a host-configured Streamable HTTP MCP endpoint.
//!
//! The registry pins identity, transport, and allowed hosts. The endpoint URL
//! and authorization stay in the host grant store and are never part of this
//! contract.

use serde::{Deserialize, Serialize};

use super::validation::{strictly_sorted_unique, valid_dns_name, valid_sha256};
use super::{canonical_digest, contract_error, parse_contract, UseResult};

pub const MCP_ENDPOINT_GRANT_SCHEMA: &str = "a3s.use.mcp-endpoint-grant.v1";
pub const MCP_ENDPOINT_GRANT_TRANSPORT: &str = "streamable-http";
const GRANT_ERROR: &str = "use.plugin.mcp_endpoint_grant_invalid";
const MAX_ALLOWED_HOSTS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpEndpointGrantContract {
    pub schema: String,
    pub transport: String,
    pub allowed_hosts: Vec<String>,
}

impl McpEndpointGrantContract {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(input, "MCP endpoint grant", GRANT_ERROR, Self::validate)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != MCP_ENDPOINT_GRANT_SCHEMA
            || self.transport != MCP_ENDPOINT_GRANT_TRANSPORT
            || self.allowed_hosts.is_empty()
            || self.allowed_hosts.len() > MAX_ALLOWED_HOSTS
            || !strictly_sorted_unique(&self.allowed_hosts)
            || self
                .allowed_hosts
                .iter()
                .any(|host| !valid_dns_name(host) || host.contains('*'))
        {
            return Err(grant_error(
                "An MCP endpoint grant must pin streamable-http and a sorted exact-host allowlist.",
            ));
        }
        Ok(())
    }

    pub fn digest(bytes: &[u8]) -> UseResult<String> {
        let contract = Self::from_json(bytes)?;
        contract.validate()?;
        let digest = canonical_digest(bytes);
        if !valid_sha256(&digest) {
            return Err(grant_error(
                "An MCP endpoint grant digest is not a SHA-256 identity.",
            ));
        }
        Ok(digest)
    }

    pub fn allows_host(&self, host: &str) -> bool {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        self.allowed_hosts.iter().any(|allowed| allowed == &host)
    }
}

fn grant_error(message: impl Into<String>) -> crate::UseError {
    contract_error(GRANT_ERROR, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> McpEndpointGrantContract {
        McpEndpointGrantContract {
            schema: MCP_ENDPOINT_GRANT_SCHEMA.to_owned(),
            transport: MCP_ENDPOINT_GRANT_TRANSPORT.to_owned(),
            allowed_hosts: vec!["modelscope.ai".to_owned(), "www.modelscope.ai".to_owned()],
        }
    }

    #[test]
    fn a_grant_contract_pins_exact_hosts_and_rejects_suffixes() {
        let contract = contract();
        contract.validate().unwrap();
        assert!(contract.allows_host("www.modelscope.ai"));
        assert!(contract.allows_host("WWW.MODELSCOPE.AI."));
        assert!(!contract.allows_host("evil.modelscope.ai"));
        assert!(!contract.allows_host("modelscope.cn"));
    }

    #[test]
    fn a_grant_contract_rejects_secrets_and_wildcards() {
        let mut extra = serde_json::to_value(contract()).unwrap();
        extra["authorization"] = serde_json::json!("secret");
        assert!(McpEndpointGrantContract::from_json(extra.to_string().as_bytes()).is_err());

        let mut wildcard = contract();
        wildcard.allowed_hosts = vec!["*.modelscope.ai".to_owned()];
        assert!(wildcard.validate().is_err());
    }
}
