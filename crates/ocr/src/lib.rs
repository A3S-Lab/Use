//! Typed optical character recognition for A3S Use.
//!
//! OCR is a first-party built-in Use domain and remains process-isolated from
//! A3S Code through its standard MCP server. Detection and recognition run
//! locally with the pinned PP-OCRv6_small ONNX models. There is no alternate
//! OCR provider or off-device fallback.

mod assets;
pub mod cli;
mod client;
mod config;
mod engine;
mod install;
pub mod mcp;
mod models;
mod postprocess;
mod preprocess;

pub use assets::{ocr_status, OcrInstallSource, OcrRuntimeStatus};
pub use client::OcrClient;
pub use install::{install_ppocr_v6, repair_ppocr_v6, uninstall_managed_ppocr_v6};
pub use mcp::OcrMcpServer;
pub use models::{
    OcrBlock, OcrBoundingBox, OcrDiagnostic, OcrPoint, OcrProviderKind, OcrRequest, OcrResult,
};

pub use a3s_use_core::{Artifact, Readiness, UseError, UseResult};
