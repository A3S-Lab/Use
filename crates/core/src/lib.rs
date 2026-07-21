use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod release;

pub use release::{
    HttpHealthContract, McpReleaseDescriptor, McpServiceContract, McpServiceTransport,
    ReleaseArtifact, ReleaseCompatibility, ReleaseDependency, ReleaseKind, ReleaseProvenance,
    ReleaseResolution, SkillBindingContract, SkillBindingTarget, SkillContentContract,
    SkillReleaseDescriptor, MAX_RELEASE_DESCRIPTOR_BYTES, MCP_RELEASE_SCHEMA, SKILL_RELEASE_SCHEMA,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UseSessionId(String);

impl UseSessionId {
    pub fn parse(value: impl Into<String>) -> Result<Self, UseError> {
        let value = value.into();
        if value.is_empty()
            || !value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(UseError::new(
                "use.session.invalid_id",
                "Session IDs may contain only ASCII letters, digits, '-' and '_'.",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RiskClass {
    Read,
    Navigate,
    Mutate,
    Submit,
    Download,
    Execute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub path: PathBuf,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Readiness {
    Ready,
    Missing,
    Broken,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainDiagnostic {
    pub domain: String,
    pub readiness: Readiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UseError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    #[serde(default)]
    pub details: BTreeMap<String, serde_json::Value>,
}

impl UseError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            suggestion: None,
            details: BTreeMap::new(),
        }
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    pub fn with_detail(
        mut self,
        name: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.details.insert(name.into(), value.into());
        self
    }
}

impl fmt::Display for UseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UseError {}

pub type UseResult<T> = Result<T, UseError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstUseInstallBlock {
    Offline,
    Disabled,
}

impl FirstUseInstallBlock {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Offline => "offline mode",
            Self::Disabled => "A3S_NO_AUTO_INSTALL",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstUseInstallPolicy {
    offline: bool,
    disabled: bool,
}

impl FirstUseInstallPolicy {
    pub const fn new(offline: bool, disabled: bool) -> Self {
        Self { offline, disabled }
    }

    pub fn from_env() -> UseResult<Self> {
        Self::from_values(
            std::env::var_os("A3S_OFFLINE"),
            std::env::var_os("A3S_NO_AUTO_INSTALL"),
        )
    }

    pub const fn blocked_by(self) -> Option<FirstUseInstallBlock> {
        if self.offline {
            Some(FirstUseInstallBlock::Offline)
        } else if self.disabled {
            Some(FirstUseInstallBlock::Disabled)
        } else {
            None
        }
    }

    pub const fn allows_install(self) -> bool {
        self.blocked_by().is_none()
    }

    fn from_values(offline: Option<OsString>, disabled: Option<OsString>) -> UseResult<Self> {
        Ok(Self {
            offline: parse_environment_boolean("A3S_OFFLINE", offline)?,
            disabled: parse_environment_boolean("A3S_NO_AUTO_INSTALL", disabled)?,
        })
    }
}

fn parse_environment_boolean(name: &'static str, value: Option<OsString>) -> UseResult<bool> {
    let Some(value) = value else {
        return Ok(false);
    };
    if value.is_empty() {
        return Ok(true);
    }
    let value = value.into_string().map_err(|_| {
        UseError::new(
            "use.first_use.policy_invalid",
            format!("{name} must contain a valid UTF-8 boolean value."),
        )
        .with_detail("variable", name)
    })?;
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(UseError::new(
            "use.first_use.policy_invalid",
            format!("{name} must be a boolean value."),
        )
        .with_detail("variable", name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_bounded_and_serializable() {
        let id = UseSessionId::parse("browser_01").unwrap();
        assert_eq!(id.as_str(), "browser_01");
        assert!(UseSessionId::parse("../escape").is_err());
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"browser_01\"");
    }

    #[test]
    fn stable_error_has_machine_code_and_suggestion() {
        let error = UseError::new("use.runtime.missing", "Runtime is missing.")
            .with_suggestion("Run a3s install use/browser.");
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["code"], "use.runtime.missing");
        assert!(value["suggestion"]
            .as_str()
            .unwrap()
            .contains("a3s install"));
    }

    #[test]
    fn first_use_policy_uses_a3s_boolean_conventions() {
        for value in [None, Some("0"), Some("false"), Some("no"), Some("off")] {
            let policy = FirstUseInstallPolicy::from_values(value.map(Into::into), None).unwrap();
            assert!(policy.allows_install());
        }
        for value in [Some(""), Some("1"), Some("true"), Some("yes"), Some("on")] {
            let policy = FirstUseInstallPolicy::from_values(value.map(Into::into), None).unwrap();
            assert_eq!(policy.blocked_by(), Some(FirstUseInstallBlock::Offline));
        }
    }

    #[test]
    fn offline_policy_takes_precedence_over_no_auto_install() {
        let policy =
            FirstUseInstallPolicy::from_values(Some("1".into()), Some("1".into())).unwrap();
        assert_eq!(policy.blocked_by(), Some(FirstUseInstallBlock::Offline));
    }

    #[test]
    fn invalid_first_use_policy_is_typed() {
        let error = FirstUseInstallPolicy::from_values(Some("sometimes".into()), None).unwrap_err();
        assert_eq!(error.code, "use.first_use.policy_invalid");
        assert_eq!(error.details["variable"], "A3S_OFFLINE");
    }
}
