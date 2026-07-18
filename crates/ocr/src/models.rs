use std::path::PathBuf;

use a3s_use_core::{Artifact, Readiness};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OcrProviderKind {
    Auto,
    Tesseract,
    Vision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcrRequest {
    #[schemars(description = "Local PNG, JPEG, WebP, GIF, BMP, or TIFF image path")]
    pub path: PathBuf,
    #[serde(default)]
    #[schemars(
        description = "OCR language identifiers; Tesseract values are joined with '+', for example ['eng', 'chi_sim']"
    )]
    pub languages: Vec<String>,
    #[serde(default)]
    #[schemars(description = "Optional Tesseract page segmentation mode from 0 through 13")]
    pub page_segmentation_mode: Option<u8>,
    #[serde(default)]
    #[schemars(description = "Override the configured provider for this call")]
    pub provider: Option<OcrProviderKind>,
    #[serde(default)]
    #[schemars(description = "Optional extraction instruction used only by the vision provider")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcrBoundingBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcrBlock {
    pub page: u32,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<OcrBoundingBox>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub provider: OcrProviderKind,
    #[schemars(with = "OcrArtifactSchema")]
    pub source: Artifact,
    pub languages: Vec<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<OcrBlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcrDiagnostic {
    #[schemars(with = "OcrReadinessSchema")]
    pub readiness: Readiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<OcrProviderKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub sends_source_off_device: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
}

#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct OcrArtifactSchema {
    path: PathBuf,
    media_type: String,
    size: u64,
    sha256: String,
}

#[derive(schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum OcrReadinessSchema {
    Ready,
    Missing,
    Broken,
    Unknown,
}
