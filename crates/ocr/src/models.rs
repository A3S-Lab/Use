use std::path::PathBuf;

use a3s_use_core::{Artifact, Readiness};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OcrProviderKind {
    PpOcrV6,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcrRequest {
    #[schemars(description = "Local PNG, JPEG, WebP, GIF, BMP, or TIFF image path")]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcrPoint {
    pub x: u32,
    pub y: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    #[schemars(description = "PP-OCRv6 text recognition confidence from 0 through 1")]
    pub confidence: f32,
    #[schemars(description = "PP-OCRv6 DB text detection confidence from 0 through 1")]
    pub detection_confidence: f32,
    #[schemars(description = "Four PP-OCRv6 polygon vertices in source-image coordinates")]
    pub polygon: [OcrPoint; 4],
    pub bounding_box: OcrBoundingBox,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OcrResult {
    pub provider: OcrProviderKind,
    pub engine: String,
    pub model: String,
    #[schemars(with = "OcrArtifactSchema")]
    pub source: Artifact,
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
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_dir: Option<PathBuf>,
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
