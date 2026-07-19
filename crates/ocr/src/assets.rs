use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};

use crate::config::{load_detection, load_recognition, MODEL_FAMILY};

pub(crate) const RECEIPT_FILE: &str = ".a3s-ppocr-v6.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OcrInstallSource {
    Environment,
    Packaged,
    Managed,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcrRuntimeStatus {
    pub available: bool,
    pub source: OcrInstallSource,
    pub model: String,
    pub model_dir: Option<PathBuf>,
    pub managed_root: Option<PathBuf>,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelAssets {
    pub(crate) root: PathBuf,
    pub(crate) detection_model: PathBuf,
    pub(crate) detection_config: PathBuf,
    pub(crate) recognition_model: PathBuf,
    pub(crate) recognition_config: PathBuf,
    pub(crate) source: OcrInstallSource,
}

pub fn ocr_status() -> OcrRuntimeStatus {
    let managed_root = managed_root().ok();
    match resolve_model_assets() {
        Ok(assets) => OcrRuntimeStatus {
            available: true,
            source: assets.source,
            model: MODEL_FAMILY.to_string(),
            model_dir: Some(assets.root),
            managed_root,
            detail: "ready".to_string(),
        },
        Err(error) => OcrRuntimeStatus {
            available: false,
            source: error
                .details
                .get("source")
                .and_then(serde_json::Value::as_str)
                .map(source_from_name)
                .unwrap_or(OcrInstallSource::Missing),
            model: MODEL_FAMILY.to_string(),
            model_dir: error
                .details
                .get("modelDir")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from),
            managed_root,
            detail: error.message,
        },
    }
}

pub(crate) fn resolve_model_assets() -> UseResult<ModelAssets> {
    if let Some(path) = std::env::var_os("A3S_OCR_MODEL_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        let path = absolute(path)?;
        return validate_assets(&path, OcrInstallSource::Environment);
    }

    let managed = managed_model_dir()?;
    if path_exists(&managed)? {
        return validate_assets(&managed, OcrInstallSource::Managed);
    }

    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            let packaged = parent.join("ocr-models").join(MODEL_FAMILY);
            if path_exists(&packaged)? {
                return validate_assets(&packaged, OcrInstallSource::Packaged);
            }
        }
    }

    Err(UseError::new(
        "use.ocr.model_missing",
        format!("The local {MODEL_FAMILY} model bundle is not installed."),
    )
    .with_suggestion(
        "Call the bounded ocr_install MCP tool, or run 'a3s install use/ocr' explicitly.",
    )
    .with_detail("source", "missing")
    .with_detail("modelDir", managed.display().to_string()))
}

pub(crate) fn validate_assets(root: &Path, source: OcrInstallSource) -> UseResult<ModelAssets> {
    let root = std::fs::canonicalize(root).map_err(|error| {
        model_error(
            source,
            root,
            format!(
                "Failed to resolve the {MODEL_FAMILY} model directory '{}': {error}",
                root.display()
            ),
        )
    })?;
    let detection_model = checked_file(&root, "det/inference.onnx", 256 * 1024 * 1024, source)?;
    let detection_config = checked_file(&root, "det/inference.yml", 2 * 1024 * 1024, source)?;
    let recognition_model = checked_file(&root, "rec/inference.onnx", 256 * 1024 * 1024, source)?;
    let recognition_config = checked_file(&root, "rec/inference.yml", 2 * 1024 * 1024, source)?;

    load_detection(&detection_config)?;
    load_recognition(&recognition_config)?;

    Ok(ModelAssets {
        root,
        detection_model,
        detection_config,
        recognition_model,
        recognition_config,
        source,
    })
}

pub(crate) fn managed_root() -> UseResult<PathBuf> {
    if let Some(value) = std::env::var_os("A3S_USE_OCR_HOME") {
        return absolute(PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os("A3S_DATA_HOME") {
        return Ok(absolute(PathBuf::from(value))?.join("use/ocr"));
    }
    if let Some(value) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(absolute(PathBuf::from(value))?.join("a3s/use/ocr"));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        return Ok(absolute(home)?.join(".local/share/a3s/use/ocr"));
    }
    #[cfg(windows)]
    if let Some(value) = std::env::var_os("LOCALAPPDATA") {
        return Ok(absolute(PathBuf::from(value))?.join("a3s/use/ocr"));
    }
    Err(UseError::new(
        "use.ocr.data_home_missing",
        "Cannot determine the A3S Use OCR data directory.",
    ))
}

pub(crate) fn managed_model_dir() -> UseResult<PathBuf> {
    Ok(managed_root()?.join(MODEL_FAMILY))
}

fn checked_file(
    root: &Path,
    relative: &str,
    max_bytes: u64,
    source: OcrInstallSource,
) -> UseResult<PathBuf> {
    let path = root.join(relative);
    let canonical = std::fs::canonicalize(&path).map_err(|error| {
        model_error(
            source,
            root,
            format!(
                "Required {MODEL_FAMILY} asset '{}' is unreadable: {error}",
                path.display()
            ),
        )
    })?;
    if !canonical.starts_with(root) {
        return Err(model_error(
            source,
            root,
            format!(
                "Required {MODEL_FAMILY} asset '{}' escapes its model directory.",
                path.display()
            ),
        ));
    }
    let metadata = std::fs::metadata(&canonical).map_err(|error| {
        model_error(
            source,
            root,
            format!(
                "Failed to inspect {MODEL_FAMILY} asset '{}': {error}",
                canonical.display()
            ),
        )
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > max_bytes {
        return Err(model_error(
            source,
            root,
            format!(
                "{MODEL_FAMILY} asset '{}' must be a non-empty regular file no larger than {max_bytes} bytes.",
                canonical.display()
            ),
        ));
    }
    Ok(canonical)
}

fn path_exists(path: &Path) -> UseResult<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(UseError::new(
            "use.ocr.model_unreadable",
            format!(
                "Failed to inspect OCR model path '{}': {error}",
                path.display()
            ),
        )),
    }
}

fn model_error(source: OcrInstallSource, root: &Path, message: impl Into<String>) -> UseError {
    UseError::new("use.ocr.model_invalid", message)
        .with_suggestion(
            "Call the bounded ocr_install MCP tool, or run 'a3s install use/ocr --force' explicitly.",
        )
        .with_detail("source", source_name(source))
        .with_detail("modelDir", root.display().to_string())
}

fn source_name(source: OcrInstallSource) -> &'static str {
    match source {
        OcrInstallSource::Environment => "environment",
        OcrInstallSource::Packaged => "packaged",
        OcrInstallSource::Managed => "managed",
        OcrInstallSource::Missing => "missing",
    }
}

fn source_from_name(value: &str) -> OcrInstallSource {
    match value {
        "environment" => OcrInstallSource::Environment,
        "packaged" => OcrInstallSource::Packaged,
        "managed" => OcrInstallSource::Managed,
        _ => OcrInstallSource::Missing,
    }
}

fn absolute(path: PathBuf) -> UseResult<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|error| {
                UseError::new(
                    "use.ocr.path_resolution_failed",
                    format!("Failed to resolve OCR data path: {error}"),
                )
            })
    }
}
