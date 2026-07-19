//! Built-in projection glue for the first-party OCR domain.

use std::path::{Path, PathBuf};

use a3s_use_core::{DomainDiagnostic, Readiness};
use a3s_use_ocr::{OcrClient, OcrProviderKind};

pub(crate) fn diagnostic() -> DomainDiagnostic {
    match OcrClient::from_env() {
        Ok(client) => {
            let diagnostic = client.diagnostic();
            DomainDiagnostic {
                domain: "ocr".to_string(),
                readiness: diagnostic.readiness,
                provider: diagnostic.provider.map(provider_name).map(str::to_string),
                version: None,
                path: diagnostic.model_dir,
                message: diagnostic.message,
                suggestions: diagnostic.suggestions,
            }
        }
        Err(error) => DomainDiagnostic {
            domain: "ocr".to_string(),
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
    if let Some(root) = std::env::var_os("A3S_USE_OCR_SKILLS_DIR").map(PathBuf::from) {
        roots.push(root);
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            roots.push(parent.join("ocr-skills"));
        }
    }
    roots.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("crates")
            .join("ocr")
            .join("skills"),
    );

    for root in roots {
        let skill = root.join("a3s-use-ocr/SKILL.md");
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

fn provider_name(provider: OcrProviderKind) -> &'static str {
    match provider {
        OcrProviderKind::PpOcrV6 => "pp-ocr-v6",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn source_skill_is_available_to_development_builds() {
        let (root, skill) = primary_skill_surface().await.unwrap();
        assert!(root.is_absolute());
        assert!(skill.starts_with(root));
        assert!(skill.ends_with("a3s-use-ocr/SKILL.md"));
    }

    #[test]
    fn diagnostic_is_typed_even_without_a_provider() {
        let diagnostic = diagnostic();
        assert_eq!(diagnostic.domain, "ocr");
        assert!(matches!(
            diagnostic.readiness,
            Readiness::Ready | Readiness::Missing | Readiness::Broken
        ));
    }
}
