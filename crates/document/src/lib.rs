//! Agentic document parsing built from native Office structure and local PP-OCRv6.
//!
//! The parser reads DOCX, XLSX, and PPTX packages through `a3s-use-office` and
//! applies `a3s-use-ocr` only to selected embedded raster images. Standalone
//! raster images are also supported. Source bytes never leave the device.

pub mod cli;
mod client;
#[cfg(feature = "mcp")]
pub mod mcp;
mod models;
mod ocr_engine;
mod source;

pub use client::DocumentClient;
pub use models::{
    DocumentDiagnostic, DocumentImage, DocumentImageOcrState, DocumentInspectRequest,
    DocumentInspectResult, DocumentOcrPolicy, DocumentOcrRecommendation, DocumentOcrSummary,
    DocumentParseRequest, DocumentParseResult, DocumentSource, DocumentSourceKind,
    DocumentTextBlock, DocumentTextOrigin, DocumentUnit, DocumentUnitKind,
    DEFAULT_DOCUMENT_OCR_MAX_IMAGES, MAX_DOCUMENT_OCR_IMAGES,
};
pub use ocr_engine::DocumentOcrEngine;

pub use a3s_use_core::{Readiness, UseError, UseResult};
