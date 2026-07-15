use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use a3s_acl::{Block, Value};
use a3s_use_core::{RiskClass, UseError, UseResult};
use serde::{Deserialize, Serialize};

mod package;
mod paths;
mod registry;
mod registry_io;
mod route_lock;

pub use paths::ExtensionPaths;
pub use registry::{
    ActivationResult, ExtensionReceipt, ExtensionRegistry, ExtensionRegistrySnapshot,
    ExtensionRouteBinding, ExtensionRouteLease, ExtensionTrust, InstallOptions, InstallResult,
    InstalledExtension, UninstallResult,
};

const RESERVED_ROUTES: &[&str] = &[
    "browser",
    "box",
    "office",
    "capabilities",
    "component",
    "extension",
    "doctor",
    "mcp",
    "help",
    "version",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub package_id: String,
    pub version: String,
    pub route: String,
    pub actions: Vec<RiskClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli: Option<CliSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillSurface>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSurface {
    pub executable: PathBuf,
    pub json_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSurface {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransport {
    Stdio,
    StreamableHttp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSurface {
    pub path: PathBuf,
}

impl ExtensionManifest {
    pub fn parse_acl(input: &str) -> UseResult<Self> {
        let document = a3s_acl::parse_acl(input).map_err(|error| {
            UseError::new(
                "use.extension.manifest_invalid",
                format!("Failed to parse extension ACL: {error}"),
            )
        })?;
        if document.blocks.len() != 1 || document.blocks[0].name != "extension" {
            return Err(UseError::new(
                "use.extension.manifest_invalid",
                "The manifest must contain only one extension block.",
            ));
        }
        let extension_blocks = document
            .blocks
            .iter()
            .filter(|block| block.name == "extension")
            .collect::<Vec<_>>();
        let [block] = extension_blocks.as_slice() else {
            return Err(UseError::new(
                "use.extension.manifest_invalid",
                "The manifest must contain exactly one extension block.",
            ));
        };
        parse_extension_block(block)
    }

    pub fn validate_package_root(&self, package_root: &Path) -> UseResult<()> {
        for path in self
            .cli
            .iter()
            .map(|surface| &surface.executable)
            .chain(self.mcp.iter().map(|surface| &surface.executable))
            .chain(self.skill.iter().map(|surface| &surface.path))
        {
            validate_relative_path(path)?;
            let resolved = package_root.join(path);
            if !resolved.starts_with(package_root) {
                return Err(UseError::new(
                    "use.extension.path_escape",
                    format!("Surface path '{}' escapes the package.", path.display()),
                ));
            }
        }
        Ok(())
    }
}

fn parse_extension_block(block: &Block) -> UseResult<ExtensionManifest> {
    require_known_attributes(block, &["schema_version", "version", "route", "actions"])?;
    let package_id = block
        .labels
        .first()
        .cloned()
        .ok_or_else(|| manifest_error("The extension block requires a package ID label."))?;
    if block.labels.len() != 1 || !valid_package_id(&package_id) {
        return Err(manifest_error(
            "Package IDs must be '<publisher>/<name>' lowercase identifiers.",
        ));
    }
    let schema_number = number_attribute(block, "schema_version")?;
    if !schema_number.is_finite()
        || schema_number.fract() != 0.0
        || !(0.0..=u32::MAX as f64).contains(&schema_number)
    {
        return Err(manifest_error(
            "Extension schema_version must be a non-negative integer.",
        ));
    }
    let schema_version = schema_number as u32;
    if schema_version != 1 {
        return Err(manifest_error(
            "Only extension schema version 1 is supported.",
        ));
    }
    let version = string_attribute(block, "version")?;
    semver::Version::parse(&version)
        .map_err(|error| manifest_error(format!("Invalid extension version: {error}")))?;
    let route = string_attribute(block, "route")?;
    if !valid_segment(&route) || RESERVED_ROUTES.contains(&route.as_str()) {
        return Err(manifest_error(format!(
            "Extension route '{route}' is invalid or reserved."
        )));
    }
    let action_names = list_attribute(block, "actions")?;
    if action_names.iter().collect::<BTreeSet<_>>().len() != action_names.len() {
        return Err(manifest_error("Action classes must be unique."));
    }
    let actions = action_names
        .into_iter()
        .map(|action| parse_risk_class(&action))
        .collect::<UseResult<Vec<_>>>()?;
    let mut seen = BTreeSet::new();
    let mut cli = None;
    let mut mcp = None;
    let mut skill = None;
    for surface in &block.blocks {
        if !seen.insert(surface.name.as_str()) {
            return Err(manifest_error(format!(
                "Duplicate '{}' surface.",
                surface.name
            )));
        }
        match surface.name.as_str() {
            "cli" => cli = Some(parse_cli(surface)?),
            "mcp" => mcp = Some(parse_mcp(surface)?),
            "skill" => skill = Some(parse_skill(surface)?),
            name => {
                return Err(manifest_error(format!(
                    "Unknown extension surface '{name}'."
                )))
            }
        }
    }
    if cli.is_none() && mcp.is_none() && skill.is_none() {
        return Err(manifest_error(
            "An extension must declare CLI, MCP, and/or Skill.",
        ));
    }
    Ok(ExtensionManifest {
        schema_version,
        package_id,
        version,
        route,
        actions,
        cli,
        mcp,
        skill,
    })
}

fn parse_cli(block: &Block) -> UseResult<CliSurface> {
    require_surface_shape(block)?;
    require_known_attributes(block, &["executable", "json_output"])?;
    let executable = PathBuf::from(string_attribute(block, "executable")?);
    validate_relative_path(&executable)?;
    Ok(CliSurface {
        executable,
        json_output: optional_bool_attribute(block, "json_output")?.unwrap_or(false),
    })
}

fn parse_mcp(block: &Block) -> UseResult<McpSurface> {
    require_surface_shape(block)?;
    require_known_attributes(block, &["executable", "args", "transport"])?;
    let executable = PathBuf::from(string_attribute(block, "executable")?);
    validate_relative_path(&executable)?;
    let transport = match string_attribute(block, "transport")?.as_str() {
        "stdio" => McpTransport::Stdio,
        "streamable-http" => McpTransport::StreamableHttp,
        value => {
            return Err(manifest_error(format!(
                "Unsupported MCP transport '{value}'."
            )))
        }
    };
    Ok(McpSurface {
        executable,
        args: optional_list_attribute(block, "args")?,
        transport,
    })
}

fn parse_skill(block: &Block) -> UseResult<SkillSurface> {
    require_surface_shape(block)?;
    require_known_attributes(block, &["path"])?;
    let path = PathBuf::from(string_attribute(block, "path")?);
    validate_relative_path(&path)?;
    if path.file_name().and_then(|value| value.to_str()) != Some("SKILL.md") {
        return Err(manifest_error("Skill surfaces must point to SKILL.md."));
    }
    Ok(SkillSurface { path })
}

fn validate_relative_path(path: &Path) -> UseResult<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::CurDir
            )
        })
    {
        return Err(UseError::new(
            "use.extension.path_escape",
            format!("Surface path '{}' is not package-relative.", path.display()),
        ));
    }
    Ok(())
}

fn require_surface_shape(block: &Block) -> UseResult<()> {
    if !block.labels.is_empty() || !block.blocks.is_empty() {
        return Err(manifest_error(format!(
            "The '{}' surface cannot have labels or nested blocks.",
            block.name
        )));
    }
    Ok(())
}

fn require_known_attributes(block: &Block, allowed: &[&str]) -> UseResult<()> {
    if let Some(unknown) = block
        .attributes
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(manifest_error(format!(
            "Unknown '{}' attribute '{}'.",
            block.name, unknown
        )));
    }
    Ok(())
}

fn string_attribute(block: &Block, name: &str) -> UseResult<String> {
    block
        .attributes
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            manifest_error(format!(
                "'{}' requires string attribute '{name}'.",
                block.name
            ))
        })
}

fn number_attribute(block: &Block, name: &str) -> UseResult<f64> {
    block
        .attributes
        .get(name)
        .and_then(Value::as_number)
        .ok_or_else(|| {
            manifest_error(format!(
                "'{}' requires numeric attribute '{name}'.",
                block.name
            ))
        })
}

fn optional_bool_attribute(block: &Block, name: &str) -> UseResult<Option<bool>> {
    match block.attributes.get(name) {
        None => Ok(None),
        Some(value) => value.as_bool().map(Some).ok_or_else(|| {
            manifest_error(format!(
                "'{}' requires boolean attribute '{name}'.",
                block.name
            ))
        }),
    }
}

fn list_attribute(block: &Block, name: &str) -> UseResult<Vec<String>> {
    let Some(Value::List(values)) = block.attributes.get(name) else {
        return Err(manifest_error(format!(
            "'{}' requires list attribute '{name}'.",
            block.name
        )));
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| manifest_error(format!("'{name}' accepts only strings.")))
        })
        .collect()
}

fn optional_list_attribute(block: &Block, name: &str) -> UseResult<Vec<String>> {
    if block.attributes.contains_key(name) {
        list_attribute(block, name)
    } else {
        Ok(Vec::new())
    }
}

fn parse_risk_class(value: &str) -> UseResult<RiskClass> {
    match value {
        "read" => Ok(RiskClass::Read),
        "navigate" => Ok(RiskClass::Navigate),
        "mutate" => Ok(RiskClass::Mutate),
        "submit" => Ok(RiskClass::Submit),
        "download" => Ok(RiskClass::Download),
        "execute" => Ok(RiskClass::Execute),
        _ => Err(manifest_error(format!("Unknown action class '{value}'."))),
    }
}

fn valid_package_id(value: &str) -> bool {
    let segments = value.split('/').collect::<Vec<_>>();
    segments.len() == 2 && segments.into_iter().all(valid_segment)
}

fn valid_segment(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn manifest_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.manifest_invalid", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
extension "acme/slack" {
  schema_version = 1
  version        = "1.2.0"
  route          = "slack"
  actions        = ["read", "mutate"]

  cli {
    executable  = "bin/a3s-use-acme-slack"
    json_output = true
  }

  mcp {
    executable = "bin/a3s-use-acme-slack"
    args       = ["serve", "--mcp"]
    transport  = "stdio"
  }

  skill {
    path = "skills/slack/SKILL.md"
  }
}
"#;

    #[test]
    fn parses_acl_into_native_surfaces() {
        let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
        assert_eq!(manifest.package_id, "acme/slack");
        assert!(manifest.cli.is_some());
        assert!(manifest.mcp.is_some());
        assert!(manifest.skill.is_some());
    }

    #[test]
    fn rejects_custom_rpc_fields_and_path_escape() {
        let custom_rpc = MANIFEST.replace(
            "json_output = true",
            "json_output = true\n    jsonrpc = \"2.0\"",
        );
        assert!(ExtensionManifest::parse_acl(&custom_rpc).is_err());
        let escaping = MANIFEST.replace("bin/a3s-use-acme-slack", "../a3s-use-acme-slack");
        assert!(ExtensionManifest::parse_acl(&escaping).is_err());
    }

    #[test]
    fn rejects_reserved_routes() {
        for route in ["browser", "box"] {
            let manifest = MANIFEST.replace(
                "route          = \"slack\"",
                &format!("route = \"{route}\""),
            );
            assert!(ExtensionManifest::parse_acl(&manifest).is_err());
        }
    }
}
