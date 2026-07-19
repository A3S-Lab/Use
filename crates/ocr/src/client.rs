use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use a3s_use_core::{Artifact, Readiness, UseError, UseResult};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::assets::{ocr_status, resolve_model_assets, OcrInstallSource};
use crate::config::MODEL_FAMILY;
use crate::engine::{EngineBlock, PpOcrV6Engine};
use crate::models::{
    OcrBlock, OcrBoundingBox, OcrDiagnostic, OcrPoint, OcrProviderKind, OcrRequest, OcrResult,
};
use crate::preprocess::decode_image;

const MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;
const ENGINE_NAME: &str = "onnx-runtime";

#[derive(Clone)]
pub struct OcrClient {
    loaded: Arc<Mutex<Option<LoadedEngine>>>,
}

struct LoadedEngine {
    model_dir: PathBuf,
    engine: PpOcrV6Engine,
}

impl OcrClient {
    pub fn from_env() -> UseResult<Self> {
        Ok(Self {
            loaded: Arc::new(Mutex::new(None)),
        })
    }

    pub fn diagnostic(&self) -> OcrDiagnostic {
        let status = ocr_status();
        let (readiness, suggestions) = if status.available {
            (Readiness::Ready, Vec::new())
        } else if status.source == OcrInstallSource::Missing {
            (
                Readiness::Missing,
                vec![
                    "Run 'a3s install use/ocr' to install the pinned local model bundle."
                        .to_string(),
                ],
            )
        } else {
            (
                Readiness::Broken,
                vec![
                    "Run 'a3s install use/ocr --force' to restore the pinned local model bundle."
                        .to_string(),
                ],
            )
        };
        OcrDiagnostic {
            readiness,
            provider: Some(OcrProviderKind::PpOcrV6),
            engine: Some(ENGINE_NAME.to_string()),
            model: Some(status.model),
            model_dir: status.model_dir,
            sends_source_off_device: false,
            message: if status.available {
                "Local PP-OCRv6 detection and recognition models are ready.".to_string()
            } else {
                status.detail
            },
            suggestions,
        }
    }

    pub async fn extract(&self, request: OcrRequest) -> UseResult<OcrResult> {
        let source = read_source(&request.path).await?;
        let loaded = Arc::clone(&self.loaded);
        tokio::task::spawn_blocking(move || {
            let image = decode_image(&source.bytes)?;
            let assets = resolve_model_assets()?;
            let mut loaded = loaded.lock().map_err(|_| {
                UseError::new(
                    "use.ocr.runtime_failed",
                    "The local PP-OCRv6 engine lock is poisoned.",
                )
            })?;
            let should_load = loaded
                .as_ref()
                .map(|loaded| loaded.model_dir != assets.root)
                .unwrap_or(true);
            if should_load {
                *loaded = Some(LoadedEngine {
                    model_dir: assets.root.clone(),
                    engine: PpOcrV6Engine::load(&assets)?,
                });
            }
            let engine = loaded.as_mut().ok_or_else(|| {
                UseError::new(
                    "use.ocr.runtime_failed",
                    "The local PP-OCRv6 engine failed to initialize.",
                )
            })?;
            let blocks = engine.engine.extract(&image)?;
            build_result(source.artifact, blocks)
        })
        .await
        .map_err(|error| {
            UseError::new(
                "use.ocr.runtime_failed",
                format!("The local PP-OCRv6 inference task failed: {error}"),
            )
        })?
    }
}

fn build_result(source: Artifact, blocks: Vec<EngineBlock>) -> UseResult<OcrResult> {
    let blocks = blocks
        .into_iter()
        .map(|block| {
            let [first, second, third, fourth] = block.polygon;
            let polygon = [
                ocr_point(first)?,
                ocr_point(second)?,
                ocr_point(third)?,
                ocr_point(fourth)?,
            ];
            let min_x = polygon.iter().map(|point| point.x).min().unwrap_or(0);
            let max_x = polygon.iter().map(|point| point.x).max().unwrap_or(0);
            let min_y = polygon.iter().map(|point| point.y).min().unwrap_or(0);
            let max_y = polygon.iter().map(|point| point.y).max().unwrap_or(0);
            Ok(OcrBlock {
                page: 1,
                text: block.text,
                confidence: block.confidence,
                detection_confidence: block.detection_confidence,
                polygon,
                bounding_box: OcrBoundingBox {
                    x: min_x,
                    y: min_y,
                    width: max_x.saturating_sub(min_x),
                    height: max_y.saturating_sub(min_y),
                },
            })
        })
        .collect::<UseResult<Vec<_>>>()?;
    let text = blocks
        .iter()
        .filter(|block| !block.text.trim().is_empty())
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(OcrResult {
        provider: OcrProviderKind::PpOcrV6,
        engine: ENGINE_NAME.to_string(),
        model: MODEL_FAMILY.to_string(),
        source,
        text,
        blocks,
        warnings: Vec::new(),
    })
}

fn ocr_point(point: imageproc::point::Point<f32>) -> UseResult<OcrPoint> {
    Ok(OcrPoint {
        x: finite_coordinate(point.x)?,
        y: finite_coordinate(point.y)?,
    })
}

fn finite_coordinate(value: f32) -> UseResult<u32> {
    if !value.is_finite() || value < 0.0 || value > u32::MAX as f32 {
        return Err(UseError::new(
            "use.ocr.provider_output_invalid",
            "PP-OCRv6 returned an invalid polygon coordinate.",
        ));
    }
    Ok(value.round() as u32)
}

struct SourceImage {
    artifact: Artifact,
    bytes: Vec<u8>,
}

async fn read_source(path: &Path) -> UseResult<SourceImage> {
    let canonical = tokio::fs::canonicalize(path).await.map_err(|error| {
        UseError::new(
            "use.ocr.source_unreadable",
            format!("Failed to resolve OCR source '{}': {error}", path.display()),
        )
    })?;
    let metadata = tokio::fs::metadata(&canonical).await.map_err(|error| {
        UseError::new(
            "use.ocr.source_unreadable",
            format!(
                "Failed to inspect OCR source '{}': {error}",
                canonical.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(UseError::new(
            "use.ocr.source_invalid",
            format!(
                "OCR source '{}' is not a regular file.",
                canonical.display()
            ),
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_INPUT_BYTES {
        return Err(UseError::new(
            "use.ocr.source_too_large",
            format!(
                "OCR source '{}' must contain between 1 byte and 32 MiB.",
                canonical.display()
            ),
        )
        .with_detail("size", metadata.len()));
    }
    let file = tokio::fs::File::open(&canonical).await.map_err(|error| {
        UseError::new(
            "use.ocr.source_unreadable",
            format!(
                "Failed to open OCR source '{}': {error}",
                canonical.display()
            ),
        )
    })?;
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_INPUT_BYTES) as usize);
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| {
            UseError::new(
                "use.ocr.source_unreadable",
                format!(
                    "Failed to read OCR source '{}': {error}",
                    canonical.display()
                ),
            )
        })?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(UseError::new(
            "use.ocr.source_too_large",
            format!(
                "OCR source '{}' must not exceed 32 MiB.",
                canonical.display()
            ),
        )
        .with_detail("sizeAtLeast", MAX_INPUT_BYTES + 1));
    }
    let media_type = detect_image_type(&bytes).ok_or_else(|| {
        UseError::new(
            "use.ocr.source_type_unsupported",
            "OCR accepts PNG, JPEG, WebP, GIF, BMP, and TIFF image bytes.",
        )
    })?;
    let digest = Sha256::digest(&bytes);
    Ok(SourceImage {
        artifact: Artifact {
            path: canonical,
            media_type: media_type.to_string(),
            size: bytes.len() as u64,
            sha256: format!("{digest:x}"),
        },
        bytes,
    })
}

fn detect_image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some("image/tiff")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_image_signatures() {
        assert_eq!(
            detect_image_type(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(detect_image_type(b"\xff\xd8\xffrest"), Some("image/jpeg"));
        assert_eq!(detect_image_type(b"not an image"), None);
    }

    #[test]
    fn diagnostic_never_discloses_an_off_device_provider() {
        let diagnostic = OcrClient::from_env().unwrap().diagnostic();
        assert_eq!(diagnostic.provider, Some(OcrProviderKind::PpOcrV6));
        assert!(!diagnostic.sends_source_off_device);
        assert_eq!(diagnostic.engine.as_deref(), Some(ENGINE_NAME));
    }
}
