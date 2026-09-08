//! Reviewed admission records: the registry-side metadata one package
//! contributes on top of its own manifest.

use std::path::{Path, PathBuf};

use a3s_acl::{Block, Value};
use a3s_use_core::{PluginReleaseChannel, UseResult};
use serde_json::json;

use crate::tools_error;

/// One admission block from the registry's admission ACL.
#[derive(Debug, Clone)]
pub(crate) struct Admission {
    pub(crate) package_id: String,
    pub(crate) package_directory: PathBuf,
    pub(crate) channel: PluginReleaseChannel,
    pub(crate) target: String,
    pub(crate) archive_name: Option<String>,
    pub(crate) display_name: String,
    pub(crate) description: String,
    pub(crate) license: String,
    pub(crate) repository: Option<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) categories: Vec<String>,
    pub(crate) permission_ceiling: Option<PathBuf>,
}

/// Load every admission block from one ACL file.
///
/// `package_directory` and `permission_ceiling` resolve relative to the
/// admission file's directory so the registry repository stays portable.
pub(crate) fn load_admissions(path: &Path) -> UseResult<Vec<Admission>> {
    let input = std::fs::read_to_string(path).map_err(|error| {
        tools_error(
            "registry_tools.admission_read_failed",
            &format!(
                "Failed to read admissions file '{}': {error}",
                path.display()
            ),
        )
    })?;
    let document = a3s_acl::parse_acl(&input).map_err(|error| {
        tools_error(
            "registry_tools.admission_invalid",
            &format!("Failed to parse the admission ACL: {error}"),
        )
    })?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut admissions = Vec::new();
    for block in &document.blocks {
        if block.name != "admission" {
            return Err(tools_error(
                "registry_tools.admission_invalid",
                &format!(
                    "The admission file contains an unexpected '{}' block.",
                    block.name
                ),
            ));
        }
        admissions.push(parse_admission(block, base)?);
    }
    if admissions.is_empty() {
        return Err(tools_error(
            "registry_tools.admission_invalid",
            "The admission file contains no admission blocks.",
        ));
    }
    Ok(admissions)
}

fn parse_admission(block: &Block, base: &Path) -> UseResult<Admission> {
    let label = block.labels.first().cloned().unwrap_or_default();
    let attributes = &block.attributes;
    let require_string = |name: &str| -> UseResult<String> {
        match attributes.get(name) {
            Some(Value::String(value)) => Ok(value.clone()),
            Some(_) => Err(tools_error(
                "registry_tools.admission_invalid",
                &format!("Admission '{label}' attribute {name} must be a string."),
            )),
            None => Err(tools_error(
                "registry_tools.admission_invalid",
                &format!("Admission '{label}' is missing required attribute {name}."),
            )),
        }
    };
    let optional_string = |name: &str| -> UseResult<Option<String>> {
        match attributes.get(name) {
            Some(Value::String(value)) => Ok(Some(value.clone())),
            Some(_) => Err(tools_error(
                "registry_tools.admission_invalid",
                &format!("Admission '{label}' attribute {name} must be a string."),
            )),
            None => Ok(None),
        }
    };
    let string_list = |name: &str| -> UseResult<Vec<String>> {
        match attributes.get(name) {
            Some(Value::List(items)) => {
                let mut values = Vec::new();
                for item in items {
                    match item {
                        Value::String(value) => values.push(value.clone()),
                        _ => {
                            return Err(tools_error(
                                "registry_tools.admission_invalid",
                                &format!(
                                    "Admission '{label}' attribute {name} must be a string list."
                                ),
                            ));
                        }
                    }
                }
                Ok(values)
            }
            Some(_) => Err(tools_error(
                "registry_tools.admission_invalid",
                &format!("Admission '{label}' attribute {name} must be a string list."),
            )),
            None => Err(tools_error(
                "registry_tools.admission_invalid",
                &format!("Admission '{label}' is missing required attribute {name}."),
            )),
        }
    };

    let channel = match require_string("channel")?.as_str() {
        "stable" => PluginReleaseChannel::Stable,
        "beta" => PluginReleaseChannel::Beta,
        "nightly" => PluginReleaseChannel::Nightly,
        other => {
            return Err(tools_error(
                "registry_tools.admission_invalid",
                &format!(
                    "Admission '{label}' has unsupported channel '{other}'; use stable, beta, or nightly."
                ),
            ));
        }
    };
    let package_directory = PathBuf::from(require_string("package_directory")?);
    if package_directory.is_absolute() {
        return Err(tools_error(
            "registry_tools.admission_invalid",
            &format!(
                "Admission '{label}' package_directory must be relative to the admission file."
            ),
        ));
    }
    let permission_ceiling = optional_string("permission_ceiling")?
        .map(|value| {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                return Err(tools_error(
                    "registry_tools.admission_invalid",
                    &format!(
                        "Admission '{label}' permission_ceiling must be relative to the admission file."
                    ),
                ));
            }
            Ok(base.join(path))
        })
        .transpose()?;
    Ok(Admission {
        package_id: label.clone(),
        package_directory: base.join(package_directory),
        channel,
        target: require_string("target")?,
        archive_name: optional_string("archive_name")?,
        display_name: require_string("display_name")?,
        description: require_string("description")?,
        license: require_string("license")?,
        repository: optional_string("repository")?,
        keywords: string_list("keywords")?,
        categories: string_list("categories")?,
        permission_ceiling,
    })
}

/// The `custom.a3s` catalog metadata value for one target.
pub(crate) fn catalog_custom(record: &serde_json::Value) -> serde_json::Value {
    json!({"a3s": record})
}

/// The `custom` value marking a separately signed planning target.
pub(crate) fn planning_custom() -> serde_json::Value {
    json!({"a3sPlanning": {"schema": "a3s.use.plugin-planning-target.v1"}})
}
