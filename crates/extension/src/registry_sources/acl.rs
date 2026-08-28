use std::collections::{BTreeMap, HashMap};

use a3s_acl::{Block, Document, Value};
use a3s_use_core::{UseError, UseResult};
use sha2::{Digest, Sha256};

use super::{
    RegistrySource, RegistrySourceSnapshot, UsePaths, VerifiedTargetCachePolicy,
    MAX_CONFIGURED_REGISTRY_SOURCES, REGISTRY_SOURCE_CONFIG_SCHEMA_VERSION,
};

const ROOT_BLOCK: &str = "registries";
const SOURCE_BLOCK: &str = "registry";

#[derive(Debug, Clone, Default)]
pub(super) struct RegistrySourcesDocument {
    pub(super) default_registry: Option<String>,
    pub(super) sources: BTreeMap<String, RegistrySource>,
}

impl RegistrySourcesDocument {
    pub(super) fn encode(&self) -> String {
        let mut root_attributes = HashMap::new();
        root_attributes.insert(
            "schema_version".to_owned(),
            Value::Number(REGISTRY_SOURCE_CONFIG_SCHEMA_VERSION as f64),
        );
        if let Some(default_registry) = &self.default_registry {
            root_attributes.insert(
                "default_registry".to_owned(),
                Value::String(default_registry.clone()),
            );
        }
        let blocks = self.sources.values().map(source_block).collect::<Vec<_>>();
        let document = Document {
            blocks: vec![Block {
                name: ROOT_BLOCK.to_owned(),
                labels: Vec::new(),
                blocks,
                attributes: root_attributes,
            }],
        };
        let mut encoded = a3s_acl::generate_acl(&document);
        encoded.push('\n');
        encoded
    }

    pub(super) fn revision(&self) -> String {
        format!("{:x}", Sha256::digest(self.encode().as_bytes()))
    }

    pub(super) fn snapshot(&self) -> RegistrySourceSnapshot {
        RegistrySourceSnapshot {
            schema_version: REGISTRY_SOURCE_CONFIG_SCHEMA_VERSION,
            revision: self.revision(),
            default_registry: self.default_registry.clone(),
            sources: self.sources.values().cloned().collect(),
        }
    }
}

pub(super) fn decode(input: &str, paths: &UsePaths) -> UseResult<RegistrySourcesDocument> {
    let parsed = a3s_acl::parse_acl(input)
        .map_err(|error| config_error(format!("Failed to parse Registry source ACL: {error}")))?;
    let [root] = parsed.blocks.as_slice() else {
        return Err(config_error(
            "Registry source configuration must contain exactly one registries block.",
        ));
    };
    if root.name != ROOT_BLOCK || !root.labels.is_empty() {
        return Err(config_error(
            "Registry source configuration must contain exactly one unlabeled registries block.",
        ));
    }
    require_known_attributes(root, &["schema_version", "default_registry"])?;
    let schema_version = number_attribute(root, "schema_version")?;
    if schema_version != REGISTRY_SOURCE_CONFIG_SCHEMA_VERSION as f64 {
        return Err(config_error(format!(
            "Registry source schema version must be {REGISTRY_SOURCE_CONFIG_SCHEMA_VERSION}."
        )));
    }
    let default_registry = optional_string_attribute(root, "default_registry")?;
    if root.blocks.len() > MAX_CONFIGURED_REGISTRY_SOURCES {
        return Err(config_error(format!(
            "Registry source configuration cannot exceed {MAX_CONFIGURED_REGISTRY_SOURCES} entries."
        )));
    }
    let mut sources = BTreeMap::new();
    for block in &root.blocks {
        let source = parse_source(block, paths)?;
        if sources.insert(source.name.clone(), source).is_some() {
            return Err(config_error(
                "Registry source configuration contains a duplicate source name.",
            ));
        }
    }
    let enabled_count = sources.values().filter(|source| source.enabled).count();
    match (&default_registry, enabled_count) {
        (None, 0) => {}
        (Some(default), _) if sources.get(default).is_some_and(|source| source.enabled) => {}
        (None, _) => {
            return Err(config_error(
                "An enabled Registry source set requires one enabled default source.",
            ))
        }
        (Some(_), _) => {
            return Err(config_error(
                "The default Registry source is missing or disabled.",
            ))
        }
    }
    let document = RegistrySourcesDocument {
        default_registry,
        sources,
    };
    if document.encode() != input {
        return Err(config_error(
            "Registry source configuration is not in canonical A3S ACL form.",
        ));
    }
    Ok(document)
}

fn source_block(source: &RegistrySource) -> Block {
    let mut attributes = HashMap::new();
    attributes.insert("url".to_owned(), Value::String(source.registry_url.clone()));
    attributes.insert(
        "root_sha256".to_owned(),
        Value::String(source.root_sha256.clone()),
    );
    attributes.insert("enabled".to_owned(), Value::Bool(source.enabled));
    attributes.insert(
        "imported_trusted_root".to_owned(),
        Value::Bool(source.imported_trusted_root),
    );
    attributes.insert(
        "cache_max_bytes".to_owned(),
        Value::String(source.cache_policy.max_bytes().to_string()),
    );
    attributes.insert(
        "cache_max_entries".to_owned(),
        Value::String(source.cache_policy.max_entries().to_string()),
    );
    attributes.insert(
        "cache_min_free_bytes".to_owned(),
        Value::String(source.cache_policy.min_free_bytes().to_string()),
    );
    Block {
        name: SOURCE_BLOCK.to_owned(),
        labels: vec![source.name.clone()],
        blocks: Vec::new(),
        attributes,
    }
}

fn parse_source(block: &Block, paths: &UsePaths) -> UseResult<RegistrySource> {
    if block.name != SOURCE_BLOCK || block.labels.len() != 1 || !block.blocks.is_empty() {
        return Err(config_error(
            "Each Registry source must be one labeled registry block without nested blocks.",
        ));
    }
    require_known_attributes(
        block,
        &[
            "url",
            "root_sha256",
            "enabled",
            "imported_trusted_root",
            "cache_max_bytes",
            "cache_max_entries",
            "cache_min_free_bytes",
        ],
    )?;
    let policy = VerifiedTargetCachePolicy::new(
        decimal_attribute(block, "cache_max_bytes")?,
        decimal_attribute(block, "cache_max_entries")?,
        decimal_attribute(block, "cache_min_free_bytes")?,
    )?;
    RegistrySource::from_persisted(
        paths,
        block.labels[0].clone(),
        string_attribute(block, "url")?,
        string_attribute(block, "root_sha256")?,
        bool_attribute(block, "enabled")?,
        bool_attribute(block, "imported_trusted_root")?,
        policy,
    )
}

fn require_known_attributes(block: &Block, allowed: &[&str]) -> UseResult<()> {
    if let Some(attribute) = block
        .attributes
        .keys()
        .find(|attribute| !allowed.contains(&attribute.as_str()))
    {
        Err(config_error(format!(
            "Registry source configuration contains unknown attribute '{attribute}'."
        )))
    } else {
        Ok(())
    }
}

fn value_attribute<'a>(block: &'a Block, name: &str) -> UseResult<&'a Value> {
    block.attributes.get(name).ok_or_else(|| {
        config_error(format!(
            "Registry source configuration requires attribute '{name}'."
        ))
    })
}

fn string_attribute(block: &Block, name: &str) -> UseResult<String> {
    value_attribute(block, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            config_error(format!(
                "Registry source attribute '{name}' must be a string."
            ))
        })
}

fn optional_string_attribute(block: &Block, name: &str) -> UseResult<Option<String>> {
    block
        .attributes
        .get(name)
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                config_error(format!(
                    "Registry source attribute '{name}' must be a string."
                ))
            })
        })
        .transpose()
}

fn number_attribute(block: &Block, name: &str) -> UseResult<f64> {
    value_attribute(block, name)?.as_number().ok_or_else(|| {
        config_error(format!(
            "Registry source attribute '{name}' must be a number."
        ))
    })
}

fn bool_attribute(block: &Block, name: &str) -> UseResult<bool> {
    value_attribute(block, name)?.as_bool().ok_or_else(|| {
        config_error(format!(
            "Registry source attribute '{name}' must be a boolean."
        ))
    })
}

fn decimal_attribute(block: &Block, name: &str) -> UseResult<u64> {
    let value = string_attribute(block, name)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(config_error(format!(
            "Registry source attribute '{name}' must be an unsigned decimal string."
        )));
    }
    value.parse::<u64>().map_err(|error| {
        config_error(format!(
            "Registry source attribute '{name}' is outside the supported range: {error}"
        ))
    })
}

fn config_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.registry_sources_invalid", message)
}
