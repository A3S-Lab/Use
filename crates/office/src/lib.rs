use std::path::{Path, PathBuf};

use a3s_use_core::{DomainDiagnostic, Readiness, UseError, UseResult, UseSessionId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentKind {
    Word,
    Spreadsheet,
    Presentation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDocument {
    pub path: PathBuf,
    pub kind: DocumentKind,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadRequest {
    pub session: UseSessionId,
    pub selector: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutationRequest {
    pub session: UseSessionId,
    pub request_id: String,
    pub operation: String,
    pub input: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationResult {
    pub request_id: String,
    pub output: serde_json::Value,
}

#[async_trait]
pub trait OfficeProvider: Send + Sync {
    async fn open(&self, request: OpenDocument) -> UseResult<UseSessionId>;
    async fn read(&self, request: ReadRequest) -> UseResult<serde_json::Value>;
    async fn mutate(&self, request: MutationRequest) -> UseResult<OperationResult>;
    async fn close(&self, session: UseSessionId, save: bool) -> UseResult<()>;
}

/// Return this error when a mutation may have reached OfficeCLI but its reply
/// was lost. Callers must inspect the document before deciding to retry.
pub fn outcome_unknown(request_id: &str) -> UseError {
    let mut error = UseError::new(
        "use.office.outcome_unknown",
        "The Office mutation may have completed, but its outcome is unknown.",
    )
    .with_suggestion("Inspect or reopen the document before issuing another mutation.");
    error.details.insert(
        "requestId".to_string(),
        serde_json::Value::String(request_id.to_string()),
    );
    error
}

pub fn doctor() -> DomainDiagnostic {
    match discover_office_cli() {
        Some(path) => DomainDiagnostic {
            domain: "office".to_string(),
            readiness: Readiness::Ready,
            provider: Some("office-cli".to_string()),
            version: None,
            path: Some(path),
            message: "OfficeCLI is available.".to_string(),
            suggestions: Vec::new(),
        },
        None => DomainDiagnostic {
            domain: "office".to_string(),
            readiness: Readiness::Missing,
            provider: None,
            version: None,
            path: None,
            message: "The supported OfficeCLI provider is not installed.".to_string(),
            suggestions: vec!["Run 'a3s install use/office'.".to_string()],
        },
    }
}

pub fn discover_office_cli() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("A3S_OFFICECLI_EXECUTABLE").map(PathBuf::from) {
        if executable(&path) {
            return Some(path);
        }
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for name in ["officecli", "office-cli"] {
            let candidate = directory.join(name);
            if executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_mutation_has_stable_non_retryable_error() {
        let error = outcome_unknown("request-7");
        assert_eq!(error.code, "use.office.outcome_unknown");
        assert_eq!(error.details["requestId"], "request-7");
        assert!(error.suggestion.unwrap().contains("Inspect"));
    }

    #[test]
    fn provider_contract_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn OfficeProvider>();
    }
}
