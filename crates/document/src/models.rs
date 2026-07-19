use std::path::PathBuf;
use std::sync::Arc;

use a3s_use_core::Readiness;
use a3s_use_ocr::{OcrBoundingBox, OcrDiagnostic, OcrPoint};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const DEFAULT_DOCUMENT_OCR_MAX_IMAGES: usize = 8;
pub const MAX_DOCUMENT_OCR_IMAGES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentSourceKind {
    Word,
    Spreadsheet,
    Presentation,
    Image,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentOcrPolicy {
    Never,
    #[default]
    Auto,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentUnitKind {
    Document,
    Sheet,
    Slide,
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentTextOrigin {
    NativeOffice,
    PpOcrV6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentOcrRecommendation {
    Required,
    Suggested,
    Optional,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentImageOcrState {
    NotSelected,
    Processed,
    Empty,
    Unsupported,
    LimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentInspectRequest {
    #[schemars(description = "Local DOCX, XLSX, PPTX, PNG, JPEG, WebP, GIF, BMP, or TIFF path")]
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentParseRequest {
    #[schemars(description = "Local DOCX, XLSX, PPTX, PNG, JPEG, WebP, GIF, BMP, or TIFF path")]
    pub path: PathBuf,
    #[serde(default)]
    pub ocr: DocumentOcrPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "Exact semantic image paths from document_inspect; when present, OCR is limited to these images"
    )]
    pub image_paths: Vec<String>,
    #[serde(default = "default_ocr_max_images")]
    #[schemars(range(min = 1, max = 16))]
    pub max_images: usize,
}

impl DocumentParseRequest {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ocr: DocumentOcrPolicy::Auto,
            image_paths: Vec::new(),
            max_images: DEFAULT_DOCUMENT_OCR_MAX_IMAGES,
        }
    }
}

fn default_ocr_max_images() -> usize {
    DEFAULT_DOCUMENT_OCR_MAX_IMAGES
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSource {
    pub path: PathBuf,
    pub kind: DocumentSourceKind,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentUnit {
    pub kind: DocumentUnitKind,
    pub index: usize,
    pub path: String,
    pub label: String,
    pub native_text_characters: usize,
    pub image_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentImage {
    pub semantic_path: String,
    pub unit_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_part: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width_px: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_px: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
    pub ocr_eligible: bool,
    pub recommendation: DocumentOcrRecommendation,
    pub recommendation_reason: String,
    pub ocr_state: DocumentImageOcrState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentTextBlock {
    pub index: usize,
    pub origin: DocumentTextOrigin,
    pub text: String,
    pub semantic_path: String,
    pub unit_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_part: Option<String>,
    pub source_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "PP-OCRv6 recognition confidence from 0 through 1")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(description = "PP-OCRv6 DB detection confidence from 0 through 1")]
    pub detection_confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polygon: Option<[OcrPoint; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<OcrBoundingBox>,
    pub included_in_text: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentOcrSummary {
    pub policy: DocumentOcrPolicy,
    pub selected_images: usize,
    pub processed_images: usize,
    pub empty_images: usize,
    pub reused_artifacts: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub sends_source_off_device: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentInspectResult {
    pub source: DocumentSource,
    pub native_text: String,
    pub native_text_truncated: bool,
    pub native_block_count: usize,
    pub units: Vec<DocumentUnit>,
    pub images: Vec<DocumentImage>,
    pub ocr: OcrDiagnostic,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentParseResult {
    pub source: DocumentSource,
    pub text: String,
    pub text_truncated: bool,
    pub blocks: Vec<DocumentTextBlock>,
    pub units: Vec<DocumentUnit>,
    pub images: Vec<DocumentImage>,
    pub ocr: DocumentOcrSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDiagnostic {
    #[schemars(with = "DocumentReadinessSchema")]
    pub readiness: Readiness,
    pub native_office_ready: bool,
    pub supported_extensions: Vec<String>,
    pub ocr: OcrDiagnostic,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
enum DocumentReadinessSchema {
    Ready,
    Missing,
    Broken,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedNativeBlock {
    pub text: String,
    pub semantic_path: String,
    pub unit_path: String,
    pub source_part: Option<String>,
    pub artifact_sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedImage {
    pub descriptor: DocumentImage,
    pub bytes: Option<Arc<[u8]>>,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedDocument {
    pub source: DocumentSource,
    pub native_blocks: Vec<PreparedNativeBlock>,
    pub native_text_truncated: bool,
    pub units: Vec<DocumentUnit>,
    pub images: Vec<PreparedImage>,
    pub warnings: Vec<String>,
}
