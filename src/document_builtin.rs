//! Built-in projection glue for native Office plus PP-OCRv6 document parsing.

use std::path::{Path, PathBuf};

use a3s_use_core::{DomainDiagnostic, Readiness};
use a3s_use_document::DocumentClient;

pub(crate) fn diagnostic() -> DomainDiagnostic {
    match DocumentClient::from_env() {
        Ok(client) => {
            let diagnostic = client.diagnostic();
            DomainDiagnostic {
                domain: "document".to_string(),
                readiness: diagnostic.readiness,
                provider: Some("native-office+pp-ocr-v6".to_string()),
                version: None,
                path: diagnostic.ocr.model_dir,
                message: diagnostic.message,
                suggestions: diagnostic.suggestions,
            }
        }
        Err(error) => DomainDiagnostic {
            domain: "document".to_string(),
            readiness: Readiness::Broken,
            provider: None,
            version: None,
            path: None,
            message: error.message,
            suggestions: error.suggestion.into_iter().collect(),
        },
    }
}

pub(crate) async fn primary_skill_surface() -> Option<(PathBuf, PathBuf)> {
    let mut roots = Vec::new();
    if let Some(root) = std::env::var_os("A3S_USE_DOCUMENT_SKILLS_DIR").map(PathBuf::from) {
        roots.push(root);
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            roots.push(parent.join("document-skills"));
        }
    }
    roots.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("crates")
            .join("document")
            .join("skills"),
    );

    for root in roots {
        let skill = root.join("a3s-use-document/SKILL.md");
        let Ok(root) = tokio::fs::canonicalize(root).await else {
            continue;
        };
        let Ok(skill) = tokio::fs::canonicalize(skill).await else {
            continue;
        };
        if skill.starts_with(&root) {
            return Some((root, skill));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn source_skill_is_available_to_development_builds() {
        let (root, skill) = primary_skill_surface().await.unwrap();
        assert!(root.is_absolute());
        assert!(skill.starts_with(root));
        assert!(skill.ends_with("a3s-use-document/SKILL.md"));
    }

    #[test]
    fn diagnostic_keeps_native_parsing_ready_without_models() {
        let diagnostic = diagnostic();
        assert_eq!(diagnostic.domain, "document");
        assert_eq!(diagnostic.readiness, Readiness::Ready);
        assert_eq!(
            diagnostic.provider.as_deref(),
            Some("native-office+pp-ocr-v6")
        );
    }
}
