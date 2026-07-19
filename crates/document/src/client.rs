use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use a3s_use_core::{Readiness, UseError, UseResult};
use a3s_use_ocr::{OcrProviderKind, OcrResult};

use crate::models::{
    DocumentDiagnostic, DocumentImage, DocumentImageOcrState, DocumentInspectRequest,
    DocumentInspectResult, DocumentOcrPolicy, DocumentOcrRecommendation, DocumentOcrSummary,
    DocumentParseRequest, DocumentParseResult, DocumentTextBlock, DocumentTextOrigin,
    PreparedDocument,
};
use crate::ocr_engine::{DocumentOcrEngine, PpOcrV6DocumentOcr};
use crate::source::{prepare, truncate_utf8, MAX_DOCUMENT_TEXT_BYTES};
use crate::MAX_DOCUMENT_OCR_IMAGES;

const MAX_DOCUMENT_OUTPUT_BLOCKS: usize = 60_000;
const MAX_OCR_BLOCKS_PER_IMAGE: usize = 4_096;
const MAX_BLOCK_TEXT_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct DocumentClient {
    ocr: Arc<dyn DocumentOcrEngine>,
}

impl DocumentClient {
    pub fn from_env() -> UseResult<Self> {
        Ok(Self::with_ocr_engine(Arc::new(
            PpOcrV6DocumentOcr::from_env()?,
        )))
    }

    pub fn with_ocr_engine(ocr: Arc<dyn DocumentOcrEngine>) -> Self {
        Self { ocr }
    }

    pub fn diagnostic(&self) -> DocumentDiagnostic {
        let ocr = self.ocr.diagnostic();
        let ocr_ready = ocr.readiness == Readiness::Ready;
        DocumentDiagnostic {
            readiness: Readiness::Ready,
            native_office_ready: true,
            supported_extensions: supported_extensions(),
            message: if ocr_ready {
                "Native Office parsing and local PP-OCRv6 are ready.".to_string()
            } else {
                "Native Office parsing is ready; local PP-OCRv6 must be prepared before raster text extraction."
                    .to_string()
            },
            suggestions: if ocr_ready {
                Vec::new()
            } else {
                vec![
                    "In MCP, request document_install_ocr through host confirmation; direct CLI parsing prepares PP-OCRv6 automatically."
                        .to_string(),
                ]
            },
            ocr,
        }
    }

    pub async fn inspect(
        &self,
        request: DocumentInspectRequest,
    ) -> UseResult<DocumentInspectResult> {
        let prepared = prepare(&request.path).await?;
        let (native_text, joined_truncated) = bounded_native_text(&prepared);
        Ok(DocumentInspectResult {
            source: prepared.source,
            native_text,
            native_text_truncated: prepared.native_text_truncated || joined_truncated,
            native_block_count: prepared.native_blocks.len(),
            units: prepared.units,
            images: prepared
                .images
                .into_iter()
                .map(|image| image.descriptor)
                .collect(),
            ocr: self.ocr.diagnostic(),
            warnings: prepared.warnings,
        })
    }

    /// Parse without installing models. This is the read-only MCP boundary.
    pub async fn parse(&self, request: DocumentParseRequest) -> UseResult<DocumentParseResult> {
        self.parse_internal(request, false).await
    }

    /// Parse with bounded first-use model preparation. This is the direct CLI boundary.
    pub async fn parse_with_first_use(
        &self,
        request: DocumentParseRequest,
    ) -> UseResult<DocumentParseResult> {
        self.parse_internal(request, true).await
    }

    async fn parse_internal(
        &self,
        request: DocumentParseRequest,
        first_use: bool,
    ) -> UseResult<DocumentParseResult> {
        validate_request(&request)?;
        let mut prepared = prepare(&request.path).await?;
        let selected = select_images(&request, &mut prepared)?;
        let selected_count = selected.len();
        let mut artifacts = BTreeMap::new();
        let mut processed_images = 0_usize;
        let mut reused_artifacts = 0_usize;
        let temporary = if selected.is_empty() {
            None
        } else {
            Some(tempfile::tempdir().map_err(|error| {
                UseError::new(
                    "use.document.temporary_file_failed",
                    format!("Failed to create a private OCR staging directory: {error}"),
                )
            })?)
        };

        for image_index in &selected {
            let image = prepared.images.get(*image_index).ok_or_else(|| {
                UseError::new(
                    "use.document.selection_invalid",
                    "Selected document image index is outside the prepared image set.",
                )
            })?;
            let digest = image.descriptor.sha256.clone().ok_or_else(|| {
                UseError::new(
                    "use.document.image_invalid",
                    format!(
                        "Selected image '{}' has no source digest.",
                        image.descriptor.semantic_path
                    ),
                )
            })?;
            if artifacts.contains_key(&digest) {
                reused_artifacts = reused_artifacts.saturating_add(1);
                continue;
            }
            let bytes = image.bytes.as_ref().ok_or_else(|| {
                UseError::new(
                    "use.document.image_unsupported",
                    format!(
                        "Selected image '{}' has no eligible raster bytes.",
                        image.descriptor.semantic_path
                    ),
                )
            })?;
            let directory = temporary.as_ref().ok_or_else(|| {
                UseError::new(
                    "use.document.temporary_file_failed",
                    "The private OCR staging directory was not initialized.",
                )
            })?;
            let staged_path = directory.path().join(format!("{digest}.image"));
            tokio::fs::write(&staged_path, bytes)
                .await
                .map_err(|error| {
                    UseError::new(
                        "use.document.temporary_file_failed",
                        format!(
                            "Failed to stage image '{}' for local OCR: {error}",
                            image.descriptor.semantic_path
                        ),
                    )
                })?;
            let result = if first_use && artifacts.is_empty() {
                self.ocr.extract_with_first_use(&staged_path).await?
            } else {
                self.ocr.extract(&staged_path).await?
            };
            validate_ocr_result(&digest, &result, &image.descriptor)?;
            artifacts.insert(digest, result);
            processed_images = processed_images.saturating_add(1);
        }

        let mut empty_images = 0_usize;
        for image_index in &selected {
            let image = prepared.images.get_mut(*image_index).ok_or_else(|| {
                UseError::new(
                    "use.document.selection_invalid",
                    "Selected document image index is outside the mutable image set.",
                )
            })?;
            let digest = image.descriptor.sha256.as_ref().ok_or_else(|| {
                UseError::new(
                    "use.document.image_invalid",
                    "Selected document image has no source digest.",
                )
            })?;
            let result = artifacts.get(digest).ok_or_else(|| {
                UseError::new(
                    "use.document.ocr_result_missing",
                    "Local OCR returned no result for a selected document image.",
                )
            })?;
            if result.text.trim().is_empty()
                && result
                    .blocks
                    .iter()
                    .all(|block| block.text.trim().is_empty())
            {
                image.descriptor.ocr_state = DocumentImageOcrState::Empty;
                empty_images = empty_images.saturating_add(1);
            } else {
                image.descriptor.ocr_state = DocumentImageOcrState::Processed;
            }
        }

        let diagnostic = self.ocr.diagnostic();
        let (provider, engine, model) = ocr_identity(&artifacts, &diagnostic);
        let mut warnings = prepared.warnings.clone();
        let mut blocks = build_blocks(&prepared, &selected, &artifacts, &mut warnings)?;
        let (text, text_truncated) = merge_text(&mut blocks);
        Ok(DocumentParseResult {
            source: prepared.source,
            text,
            text_truncated: prepared.native_text_truncated || text_truncated,
            blocks,
            units: prepared.units,
            images: prepared
                .images
                .into_iter()
                .map(|image| image.descriptor)
                .collect(),
            ocr: DocumentOcrSummary {
                policy: request.ocr,
                selected_images: selected_count,
                processed_images,
                empty_images,
                reused_artifacts,
                provider,
                engine,
                model,
                sends_source_off_device: false,
            },
            warnings,
        })
    }
}

fn validate_request(request: &DocumentParseRequest) -> UseResult<()> {
    if request.max_images == 0 || request.max_images > MAX_DOCUMENT_OCR_IMAGES {
        return Err(UseError::new(
            "use.document.ocr_limit_invalid",
            format!("Document OCR maxImages must be between 1 and {MAX_DOCUMENT_OCR_IMAGES}."),
        )
        .with_detail("maxImages", request.max_images));
    }
    let mut paths = BTreeSet::new();
    for path in &request.image_paths {
        if path.trim().is_empty() || !paths.insert(path) {
            return Err(UseError::new(
                "use.document.image_selection_invalid",
                "Explicit imagePaths must be non-empty and unique.",
            )
            .with_detail("imagePath", path.clone()));
        }
    }
    if request.image_paths.len() > request.max_images {
        return Err(UseError::new(
            "use.document.ocr_limit_exceeded",
            "Explicit image selection exceeds maxImages.",
        )
        .with_detail("selectedImages", request.image_paths.len())
        .with_detail("maxImages", request.max_images));
    }
    Ok(())
}

fn select_images(
    request: &DocumentParseRequest,
    prepared: &mut PreparedDocument,
) -> UseResult<Vec<usize>> {
    if request.ocr == DocumentOcrPolicy::Never {
        if !request.image_paths.is_empty() {
            prepared.warnings.push(
                "Explicit imagePaths were ignored because the OCR policy is 'never'.".to_string(),
            );
        }
        return Ok(Vec::new());
    }

    let mut candidates = if request.image_paths.is_empty() {
        prepared
            .images
            .iter()
            .enumerate()
            .filter(|(_, image)| {
                image.descriptor.ocr_eligible
                    && match request.ocr {
                        DocumentOcrPolicy::Never => false,
                        DocumentOcrPolicy::Auto => matches!(
                            image.descriptor.recommendation,
                            DocumentOcrRecommendation::Required
                                | DocumentOcrRecommendation::Suggested
                        ),
                        DocumentOcrPolicy::Always => true,
                    }
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    } else {
        let by_path = prepared
            .images
            .iter()
            .enumerate()
            .map(|(index, image)| (image.descriptor.semantic_path.as_str(), index))
            .collect::<BTreeMap<_, _>>();
        let mut selected = Vec::with_capacity(request.image_paths.len());
        for path in &request.image_paths {
            let index = by_path.get(path.as_str()).copied().ok_or_else(|| {
                UseError::new(
                    "use.document.image_not_found",
                    format!("Document image path '{path}' does not exist."),
                )
                .with_detail("imagePath", path.clone())
            })?;
            if !prepared.images[index].descriptor.ocr_eligible {
                return Err(UseError::new(
                    "use.document.image_unsupported",
                    format!("Document image path '{path}' is not eligible for local OCR."),
                )
                .with_detail("imagePath", path.clone()));
            }
            selected.push(index);
        }
        selected
    };

    if candidates.len() > request.max_images {
        for index in &candidates[request.max_images..] {
            prepared.images[*index].descriptor.ocr_state = DocumentImageOcrState::LimitReached;
        }
        prepared.warnings.push(format!(
            "OCR selected the first {} images and omitted {} at the configured limit.",
            request.max_images,
            candidates.len() - request.max_images
        ));
        candidates.truncate(request.max_images);
    }
    Ok(candidates)
}

fn validate_ocr_result(
    expected_digest: &str,
    result: &OcrResult,
    image: &DocumentImage,
) -> UseResult<()> {
    if result.source.sha256 != expected_digest {
        return Err(UseError::new(
            "use.document.ocr_provenance_invalid",
            format!(
                "Local OCR returned a source digest that does not match image '{}'.",
                image.semantic_path
            ),
        )
        .with_detail("expectedSha256", expected_digest)
        .with_detail("actualSha256", result.source.sha256.clone()));
    }
    if result.provider != OcrProviderKind::PpOcrV6 {
        return Err(UseError::new(
            "use.document.ocr_provider_invalid",
            "Document parsing accepts only the local PP-OCRv6 provider.",
        ));
    }
    Ok(())
}

fn build_blocks(
    prepared: &PreparedDocument,
    selected: &[usize],
    artifacts: &BTreeMap<String, OcrResult>,
    warnings: &mut Vec<String>,
) -> UseResult<Vec<DocumentTextBlock>> {
    let mut output = Vec::new();
    let selected = selected.iter().copied().collect::<BTreeSet<_>>();
    for unit in &prepared.units {
        for native in prepared
            .native_blocks
            .iter()
            .filter(|block| block.unit_path == unit.path)
        {
            if output.len() >= MAX_DOCUMENT_OUTPUT_BLOCKS {
                warnings.push(
                    "Document output block limit reached; remaining evidence was omitted."
                        .to_string(),
                );
                return Ok(output);
            }
            output.push(DocumentTextBlock {
                index: output.len() + 1,
                origin: DocumentTextOrigin::NativeOffice,
                text: native.text.clone(),
                semantic_path: native.semantic_path.clone(),
                unit_path: native.unit_path.clone(),
                source_part: native.source_part.clone(),
                source_sha256: prepared.source.sha256.clone(),
                artifact_sha256: native.artifact_sha256.clone(),
                model: None,
                confidence: None,
                detection_confidence: None,
                polygon: None,
                bounding_box: None,
                included_in_text: !native.text.trim().is_empty(),
            });
        }
        for (image_index, image) in prepared
            .images
            .iter()
            .enumerate()
            .filter(|(_, image)| image.descriptor.unit_path == unit.path)
        {
            if !selected.contains(&image_index) {
                continue;
            }
            let digest = image.descriptor.sha256.as_ref().ok_or_else(|| {
                UseError::new(
                    "use.document.image_invalid",
                    "Selected document image has no source digest.",
                )
            })?;
            let result = artifacts.get(digest).ok_or_else(|| {
                UseError::new(
                    "use.document.ocr_result_missing",
                    "Selected document image has no local OCR result.",
                )
            })?;
            if result.blocks.is_empty() && !result.text.trim().is_empty() {
                push_ocr_block(
                    &mut output,
                    prepared,
                    image,
                    result,
                    &result.text,
                    None,
                    warnings,
                );
            } else {
                for block in result.blocks.iter().take(MAX_OCR_BLOCKS_PER_IMAGE) {
                    if output.len() >= MAX_DOCUMENT_OUTPUT_BLOCKS {
                        warnings.push(
                            "Document output block limit reached; remaining OCR evidence was omitted."
                                .to_string(),
                        );
                        return Ok(output);
                    }
                    push_ocr_block(
                        &mut output,
                        prepared,
                        image,
                        result,
                        &block.text,
                        Some(block),
                        warnings,
                    );
                }
                if result.blocks.len() > MAX_OCR_BLOCKS_PER_IMAGE {
                    warnings.push(format!(
                        "OCR evidence for '{}' was limited to {} blocks.",
                        image.descriptor.semantic_path, MAX_OCR_BLOCKS_PER_IMAGE
                    ));
                }
            }
        }
    }
    mark_duplicate_ocr_text(&mut output);
    Ok(output)
}

fn push_ocr_block(
    output: &mut Vec<DocumentTextBlock>,
    prepared: &PreparedDocument,
    image: &crate::models::PreparedImage,
    result: &OcrResult,
    text: &str,
    block: Option<&a3s_use_ocr::OcrBlock>,
    warnings: &mut Vec<String>,
) {
    let (text, truncated) = truncate_utf8(text, MAX_BLOCK_TEXT_BYTES);
    if truncated {
        warnings.push(format!(
            "One OCR block for '{}' exceeded {} bytes and was truncated.",
            image.descriptor.semantic_path, MAX_BLOCK_TEXT_BYTES
        ));
    }
    output.push(DocumentTextBlock {
        index: output.len() + 1,
        origin: DocumentTextOrigin::PpOcrV6,
        included_in_text: !text.trim().is_empty(),
        text,
        semantic_path: image.descriptor.semantic_path.clone(),
        unit_path: image.descriptor.unit_path.clone(),
        source_part: image.descriptor.source_part.clone(),
        source_sha256: prepared.source.sha256.clone(),
        artifact_sha256: image.descriptor.sha256.clone(),
        model: Some(result.model.clone()),
        confidence: block.map(|block| block.confidence),
        detection_confidence: block.map(|block| block.detection_confidence),
        polygon: block.map(|block| block.polygon),
        bounding_box: block.map(|block| block.bounding_box),
    });
}

fn mark_duplicate_ocr_text(blocks: &mut [DocumentTextBlock]) {
    let mut seen_by_unit: BTreeMap<String, String> = BTreeMap::new();
    for block in blocks
        .iter()
        .filter(|block| block.origin == DocumentTextOrigin::NativeOffice && block.included_in_text)
    {
        append_normalized(
            seen_by_unit.entry(block.unit_path.clone()).or_default(),
            &block.text,
        );
    }
    for block in blocks
        .iter_mut()
        .filter(|block| block.origin == DocumentTextOrigin::PpOcrV6 && block.included_in_text)
    {
        let normalized = normalize_text(&block.text);
        if normalized.is_empty() {
            block.included_in_text = false;
            continue;
        }
        let seen = seen_by_unit.entry(block.unit_path.clone()).or_default();
        if seen.contains(&normalized) {
            block.included_in_text = false;
        } else {
            if !seen.is_empty() {
                seen.push(' ');
            }
            seen.push_str(&normalized);
        }
    }
}

fn append_normalized(output: &mut String, value: &str) {
    let normalized = normalize_text(value);
    if normalized.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push(' ');
    }
    output.push_str(&normalized);
}

fn normalize_text(value: &str) -> String {
    let mut output = String::new();
    let mut whitespace = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if whitespace && !output.is_empty() {
                output.push(' ');
            }
            whitespace = false;
            output.push(character);
        } else {
            whitespace = true;
        }
    }
    output
}

fn merge_text(blocks: &mut [DocumentTextBlock]) -> (String, bool) {
    let mut output = String::new();
    let mut previous_unit: Option<&str> = None;
    let mut truncated = false;
    for block in blocks {
        if !block.included_in_text || block.text.trim().is_empty() {
            continue;
        }
        if truncated {
            block.included_in_text = false;
            continue;
        }
        let separator = if output.is_empty() {
            ""
        } else if previous_unit == Some(block.unit_path.as_str()) {
            "\n"
        } else {
            "\n\n"
        };
        let remaining = MAX_DOCUMENT_TEXT_BYTES.saturating_sub(output.len());
        if separator.len() >= remaining {
            block.included_in_text = false;
            truncated = true;
            continue;
        }
        output.push_str(separator);
        let remaining = MAX_DOCUMENT_TEXT_BYTES.saturating_sub(output.len());
        let (text, block_truncated) = truncate_utf8(block.text.trim(), remaining);
        if text.is_empty() {
            block.included_in_text = false;
            truncated = true;
            continue;
        }
        output.push_str(&text);
        previous_unit = Some(block.unit_path.as_str());
        truncated |= block_truncated;
    }
    (output, truncated)
}

fn bounded_native_text(prepared: &PreparedDocument) -> (String, bool) {
    let mut output = String::new();
    let mut truncated = false;
    for block in &prepared.native_blocks {
        if !output.is_empty() {
            if output.len() == MAX_DOCUMENT_TEXT_BYTES {
                truncated = true;
                break;
            }
            output.push('\n');
        }
        let remaining = MAX_DOCUMENT_TEXT_BYTES.saturating_sub(output.len());
        let (text, block_truncated) = truncate_utf8(&block.text, remaining);
        output.push_str(&text);
        if block_truncated {
            truncated = true;
            break;
        }
    }
    (output, truncated)
}

fn ocr_identity(
    artifacts: &BTreeMap<String, OcrResult>,
    diagnostic: &a3s_use_ocr::OcrDiagnostic,
) -> (Option<String>, Option<String>, Option<String>) {
    if let Some(result) = artifacts.values().next() {
        return (
            Some(provider_name(result.provider).to_string()),
            Some(result.engine.clone()),
            Some(result.model.clone()),
        );
    }
    (
        diagnostic.provider.map(provider_name).map(str::to_string),
        diagnostic.engine.clone(),
        diagnostic.model.clone(),
    )
}

fn provider_name(provider: OcrProviderKind) -> &'static str {
    match provider {
        OcrProviderKind::PpOcrV6 => "pp-ocr-v6",
    }
}

fn supported_extensions() -> Vec<String> {
    [
        "docx", "xlsx", "pptx", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_normalization_is_unicode_aware_and_stable() {
        assert_eq!(normalize_text(" Hello,\n世界! "), "hello 世界");
    }

    #[test]
    fn duplicate_ocr_text_is_kept_as_evidence_but_not_merged() {
        let mut blocks = vec![
            DocumentTextBlock {
                index: 1,
                origin: DocumentTextOrigin::NativeOffice,
                text: "Quarterly Revenue".to_string(),
                semantic_path: "/body/p[1]".to_string(),
                unit_path: "/body".to_string(),
                source_part: None,
                source_sha256: "a".repeat(64),
                artifact_sha256: None,
                model: None,
                confidence: None,
                detection_confidence: None,
                polygon: None,
                bounding_box: None,
                included_in_text: true,
            },
            DocumentTextBlock {
                index: 2,
                origin: DocumentTextOrigin::PpOcrV6,
                text: "quarterly revenue".to_string(),
                semantic_path: "/body/picture[1]".to_string(),
                unit_path: "/body".to_string(),
                source_part: None,
                source_sha256: "a".repeat(64),
                artifact_sha256: Some("b".repeat(64)),
                model: Some("PP-OCRv6_small".to_string()),
                confidence: Some(0.9),
                detection_confidence: Some(0.9),
                polygon: None,
                bounding_box: None,
                included_in_text: true,
            },
        ];
        mark_duplicate_ocr_text(&mut blocks);
        assert!(blocks[0].included_in_text);
        assert!(!blocks[1].included_in_text);
    }
}
