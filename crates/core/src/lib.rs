use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
}
