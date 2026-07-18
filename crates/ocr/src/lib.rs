//! Typed optical character recognition for A3S Use.
//!
//! OCR is a first-party built-in Use domain and remains process-isolated from
//! A3S Code through its standard MCP server. The crate supports a local
//! Tesseract executable and an explicitly configured OpenAI-compatible vision
//! endpoint without silently installing either provider.

pub mod cli;
mod client;
pub mod mcp;
mod models;
mod provider;

pub use client::OcrClient;
pub use mcp::OcrMcpServer;
pub use models::{OcrBlock, OcrBoundingBox, OcrDiagnostic, OcrProviderKind, OcrRequest, OcrResult};

pub use a3s_use_core::{Artifact, Readiness, UseError, UseResult};
