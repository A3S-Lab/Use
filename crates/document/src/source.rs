use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use a3s_use_core::{UseError, UseResult};
use a3s_use_office::{
    DocumentKind, DocumentNode, NativeOfficeDocument, NativeOfficePackage, OfficeNodeType,
    PackageLimits, RelationshipSource, RelationshipTarget,
};
use image::{ImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::models::{
    DocumentImage, DocumentImageOcrState, DocumentOcrRecommendation, DocumentSource,
    DocumentSourceKind, DocumentUnit, DocumentUnitKind, PreparedDocument, PreparedImage,
    PreparedNativeBlock,
};

pub(crate) const MAX_DOCUMENT_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_DOCUMENT_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_DOCUMENT_TEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_DOCUMENT_PARTS: usize = 4_096;
const MAX_DOCUMENT_UNCOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DOCUMENT_COMPRESSION_RATIO: u64 = 200;
const MAX_DOCUMENT_IMAGES: usize = 256;
const MAX_DOCUMENT_NATIVE_BLOCKS: usize = 50_000;
const MAX_IMAGE_SIDE: u32 = 16_384;
const MAX_IMAGE_ALLOC: u64 = 256 * 1024 * 1024;

pub(crate) async fn prepare(path: &Path) -> UseResult<PreparedDocument> {
    let path = resolve_regular_file(path).await?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("docx" | "xlsx" | "pptx") => prepare_office(path).await,
        Some("pdf") => Err(unsupported_source(&path, Some("application/pdf"))),
        _ => prepare_image(path).await,
    }
}

async fn prepare_office(path: PathBuf) -> UseResult<PreparedDocument> {
    let limits = PackageLimits {
        max_archive_bytes: MAX_DOCUMENT_SOURCE_BYTES,
        max_entries: MAX_DOCUMENT_PARTS,
        max_part_bytes: MAX_DOCUMENT_IMAGE_BYTES,
        max_uncompressed_bytes: MAX_DOCUMENT_UNCOMPRESSED_BYTES,
        max_compression_ratio: MAX_DOCUMENT_COMPRESSION_RATIO,
    };
    let package = NativeOfficePackage::open_with_limits(&path, limits).await?;
    tokio::task::spawn_blocking(move || prepare_office_package(package))
        .await
        .map_err(|error| {
            UseError::new(
                "use.document.parse_failed",
                format!("The native Office parsing task failed: {error}"),
            )
        })?
}

fn prepare_office_package(package: NativeOfficePackage) -> UseResult<PreparedDocument> {
    let source = DocumentSource {
        path: package.path().to_path_buf(),
        kind: source_kind(package.kind()),
        media_type: office_media_type(package.kind()).to_string(),
        size: package.source_revision().archive_bytes,
        sha256: package.source_revision().sha256.clone(),
        content_sha256: Some(package.content_sha256()),
    };
    let document = NativeOfficeDocument::from_package(package)?;
    let mut units = collect_units(&document);
    let unit_by_path = units
        .iter()
        .enumerate()
        .map(|(index, unit)| (unit.path.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut state = OfficeCollection {
        document: &document,
        unit_by_path: &unit_by_path,
        units: &mut units,
        native_blocks: Vec::new(),
        images: Vec::new(),
        warnings: Vec::new(),
        part_hashes: BTreeMap::new(),
        image_bytes: BTreeMap::new(),
        native_bytes: 0,
        native_text_truncated: false,
    };
    collect_office_node(document.root(), document.kind(), None, None, &mut state)?;
    apply_recommendations(&mut state.images, state.units);
    let native_blocks = std::mem::take(&mut state.native_blocks);
    let native_text_truncated = state.native_text_truncated;
    let images = std::mem::take(&mut state.images);
    let warnings = std::mem::take(&mut state.warnings);
    drop(state);
    Ok(PreparedDocument {
        source,
        native_blocks,
        native_text_truncated,
        units,
        images,
        warnings,
    })
}

async fn prepare_image(path: PathBuf) -> UseResult<PreparedDocument> {
    let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
        source_error(
            "use.document.source_unreadable",
            format!(
                "Failed to inspect document source '{}': {error}",
                path.display()
            ),
        )
    })?;
    if metadata.len() == 0 || metadata.len() > MAX_DOCUMENT_IMAGE_BYTES {
        return Err(source_error(
            "use.document.source_too_large",
            format!(
                "Image source '{}' must contain between 1 byte and 32 MiB.",
                path.display()
            ),
        )
        .with_detail("size", metadata.len()));
    }
    let file = tokio::fs::File::open(&path).await.map_err(|error| {
        source_error(
            "use.document.source_unreadable",
            format!(
                "Failed to open document source '{}': {error}",
                path.display()
            ),
        )
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_DOCUMENT_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| {
            source_error(
                "use.document.source_unreadable",
                format!(
                    "Failed to read document source '{}': {error}",
                    path.display()
                ),
            )
        })?;
    if bytes.len() as u64 > MAX_DOCUMENT_IMAGE_BYTES {
        return Err(source_error(
            "use.document.source_too_large",
            format!("Image source '{}' exceeds 32 MiB.", path.display()),
        ));
    }
    let image_bytes: Arc<[u8]> = Arc::from(bytes);
    let inspection_bytes = Arc::clone(&image_bytes);
    let info = tokio::task::spawn_blocking(move || inspect_image(&inspection_bytes))
        .await
        .map_err(|error| {
            source_error(
                "use.document.source_invalid",
                format!("The image inspection task failed: {error}"),
            )
        })?
        .map_err(|_| unsupported_source(&path, None))?;
    let sha256 = sha256(&image_bytes);
    let size = image_bytes.len() as u64;
    let unit = DocumentUnit {
        kind: DocumentUnitKind::Image,
        index: 1,
        path: "/image".to_string(),
        label: "Image".to_string(),
        native_text_characters: 0,
        image_count: 1,
    };
    let image = PreparedImage {
        descriptor: DocumentImage {
            semantic_path: "/image".to_string(),
            unit_path: "/image".to_string(),
            source_part: None,
            media_type: Some(info.media_type.to_string()),
            size: Some(size),
            sha256: Some(sha256.clone()),
            width_px: Some(info.width),
            height_px: Some(info.height),
            alt_text: None,
            ocr_eligible: true,
            recommendation: DocumentOcrRecommendation::Required,
            recommendation_reason:
                "A standalone raster image has no native text layer; OCR is required.".to_string(),
            ocr_state: DocumentImageOcrState::NotSelected,
        },
        bytes: Some(image_bytes),
    };
    Ok(PreparedDocument {
        source: DocumentSource {
            path,
            kind: DocumentSourceKind::Image,
            media_type: info.media_type.to_string(),
            size,
            sha256,
            content_sha256: None,
        },
        native_blocks: Vec::new(),
        native_text_truncated: false,
        units: vec![unit],
        images: vec![image],
        warnings: Vec::new(),
    })
}

struct OfficeCollection<'a> {
    document: &'a NativeOfficeDocument,
    unit_by_path: &'a BTreeMap<String, usize>,
    units: &'a mut Vec<DocumentUnit>,
    native_blocks: Vec<PreparedNativeBlock>,
    images: Vec<PreparedImage>,
    warnings: Vec<String>,
    part_hashes: BTreeMap<String, String>,
    image_bytes: BTreeMap<String, Arc<[u8]>>,
    native_bytes: usize,
    native_text_truncated: bool,
}

fn collect_office_node(
    node: &DocumentNode,
    kind: DocumentKind,
    owner_part: Option<&str>,
    unit_index: Option<usize>,
    state: &mut OfficeCollection<'_>,
) -> UseResult<()> {
    let unit_index = state.unit_by_path.get(&node.path).copied().or(unit_index);
    let node_owner = if node.node_type == OfficeNodeType::Picture {
        owner_part
    } else {
        node.format
            .get("part")
            .map(|part| part.trim_start_matches('/'))
            .or(owner_part)
    };

    if should_collect_native(node, kind) {
        collect_native_block(node, kind, node_owner, unit_index, state)?;
    }
    if node.node_type == OfficeNodeType::Picture {
        collect_picture(node, node_owner, unit_index, state)?;
    }
    for child in &node.children {
        collect_office_node(child, kind, node_owner, unit_index, state)?;
    }
    Ok(())
}

fn should_collect_native(node: &DocumentNode, kind: DocumentKind) -> bool {
    if node.text.trim().is_empty() {
        return false;
    }
    matches!(
        (kind, node.node_type),
        (DocumentKind::Word, OfficeNodeType::Paragraph)
            | (DocumentKind::Spreadsheet, OfficeNodeType::Cell)
            | (DocumentKind::Presentation, OfficeNodeType::Slide)
    )
}

fn collect_native_block(
    node: &DocumentNode,
    kind: DocumentKind,
    owner_part: Option<&str>,
    unit_index: Option<usize>,
    state: &mut OfficeCollection<'_>,
) -> UseResult<()> {
    let Some(unit_index) = unit_index else {
        return Ok(());
    };
    if state.native_text_truncated {
        return Ok(());
    }
    if state.native_blocks.len() >= MAX_DOCUMENT_NATIVE_BLOCKS {
        state.native_text_truncated = true;
        state
            .warnings
            .push("Native text block limit reached; remaining blocks were omitted.".to_string());
        return Ok(());
    }
    let text = if kind == DocumentKind::Spreadsheet {
        format!("{}={}", node.path, node.text)
    } else {
        node.text.clone()
    };
    let remaining = MAX_DOCUMENT_TEXT_BYTES.saturating_sub(state.native_bytes);
    let (text, truncated) = truncate_utf8(&text, remaining);
    if text.is_empty() {
        state.native_text_truncated = true;
        state
            .warnings
            .push("Native text byte limit reached; remaining text was omitted.".to_string());
        return Ok(());
    }
    let source_part = owner_part.map(|part| format!("/{}", part.trim_start_matches('/')));
    let artifact_sha256 = match owner_part {
        Some(part) => Some(part_sha256(part, state)?),
        None => None,
    };
    state.native_bytes = state.native_bytes.saturating_add(text.len());
    state.units[unit_index].native_text_characters = state.units[unit_index]
        .native_text_characters
        .saturating_add(node.text.chars().count());
    state.native_blocks.push(PreparedNativeBlock {
        text,
        semantic_path: node.path.clone(),
        unit_path: state.units[unit_index].path.clone(),
        source_part,
        artifact_sha256,
    });
    if truncated {
        state.native_text_truncated = true;
        state
            .warnings
            .push("Native text byte limit reached; the final block was truncated.".to_string());
    }
    Ok(())
}

fn collect_picture(
    node: &DocumentNode,
    owner_part: Option<&str>,
    unit_index: Option<usize>,
    state: &mut OfficeCollection<'_>,
) -> UseResult<()> {
    if state.images.len() >= MAX_DOCUMENT_IMAGES {
        if state.images.len() == MAX_DOCUMENT_IMAGES {
            state.warnings.push(format!(
                "Document image discovery stopped at the {MAX_DOCUMENT_IMAGES}-image limit."
            ));
        }
        return Ok(());
    }
    let Some(unit_index) = unit_index else {
        state.warnings.push(format!(
            "Picture '{}' is outside a supported document unit and was not selected for OCR.",
            node.path
        ));
        return Ok(());
    };
    state.units[unit_index].image_count = state.units[unit_index].image_count.saturating_add(1);
    let part = resolve_picture_part(state.document, node, owner_part);
    let mut descriptor = DocumentImage {
        semantic_path: node.path.clone(),
        unit_path: state.units[unit_index].path.clone(),
        source_part: part.as_ref().map(|part| format!("/{part}")),
        media_type: None,
        size: None,
        sha256: None,
        width_px: None,
        height_px: None,
        alt_text: node
            .format
            .get("alt")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        ocr_eligible: false,
        recommendation: DocumentOcrRecommendation::Unsupported,
        recommendation_reason:
            "The embedded picture could not be resolved to a supported raster image.".to_string(),
        ocr_state: DocumentImageOcrState::NotSelected,
    };
    let Some(part) = part else {
        state.images.push(PreparedImage {
            descriptor,
            bytes: None,
        });
        return Ok(());
    };
    let bytes = state.document.package().part(&part)?;
    descriptor.size = Some(bytes.len() as u64);
    descriptor.sha256 = Some(sha256(bytes));
    descriptor.media_type = state
        .document
        .opc()
        .content_types()
        .content_type(&part)
        .map(str::to_string);
    if bytes.is_empty() || bytes.len() as u64 > MAX_DOCUMENT_IMAGE_BYTES {
        descriptor.recommendation_reason =
            "The embedded raster exceeds the bounded OCR input size.".to_string();
        state.images.push(PreparedImage {
            descriptor,
            bytes: None,
        });
        return Ok(());
    }
    let Ok(info) = inspect_image(bytes) else {
        descriptor.recommendation_reason =
            "The embedded part is not a valid supported raster image.".to_string();
        state.images.push(PreparedImage {
            descriptor,
            bytes: None,
        });
        return Ok(());
    };
    if descriptor.media_type.as_deref() != Some(info.media_type) {
        if let Some(declared) = descriptor.media_type.as_deref() {
            state.warnings.push(format!(
                "Picture '{}' declares '{declared}' but contains '{}'; byte detection is authoritative.",
                node.path, info.media_type
            ));
        }
        descriptor.media_type = Some(info.media_type.to_string());
    }
    descriptor.width_px = Some(info.width);
    descriptor.height_px = Some(info.height);
    descriptor.ocr_eligible = true;
    let digest = descriptor.sha256.clone().ok_or_else(|| {
        source_error(
            "use.document.image_invalid",
            format!("Picture '{}' has no computed source digest.", node.path),
        )
    })?;
    let bytes = state
        .image_bytes
        .entry(digest)
        .or_insert_with(|| Arc::from(bytes))
        .clone();
    state.images.push(PreparedImage {
        descriptor,
        bytes: Some(bytes),
    });
    Ok(())
}

fn resolve_picture_part(
    document: &NativeOfficeDocument,
    node: &DocumentNode,
    owner_part: Option<&str>,
) -> Option<String> {
    if let Some(part) = node.format.get("part") {
        return Some(part.trim_start_matches('/').to_string());
    }
    let owner = node
        .format
        .get("ownerPart")
        .map(String::as_str)
        .or(owner_part)?
        .trim_start_matches('/');
    let relationship_id = node.format.get("relationshipId")?;
    let source = RelationshipSource::Part {
        part_name: owner.to_string(),
    };
    let relationship = document
        .opc()
        .relationships()
        .relationship(&source, relationship_id)?;
    if !relationship.relationship_type.ends_with("/image") {
        return None;
    }
    match &relationship.target {
        RelationshipTarget::Internal { part_name, .. } => Some(part_name.clone()),
        RelationshipTarget::External { .. } => None,
    }
}

fn part_sha256(part: &str, state: &mut OfficeCollection<'_>) -> UseResult<String> {
    let part = part.trim_start_matches('/');
    if let Some(digest) = state.part_hashes.get(part) {
        return Ok(digest.clone());
    }
    let digest = sha256(state.document.package().part(part)?);
    state.part_hashes.insert(part.to_string(), digest.clone());
    Ok(digest)
}

fn collect_units(document: &NativeOfficeDocument) -> Vec<DocumentUnit> {
    let mut nodes = Vec::new();
    collect_unit_nodes(document.root(), document.kind(), &mut nodes);
    nodes
        .into_iter()
        .enumerate()
        .map(|(offset, node)| DocumentUnit {
            kind: match document.kind() {
                DocumentKind::Word => DocumentUnitKind::Document,
                DocumentKind::Spreadsheet => DocumentUnitKind::Sheet,
                DocumentKind::Presentation => DocumentUnitKind::Slide,
            },
            index: offset + 1,
            path: node.path.clone(),
            label: unit_label(node, document.kind(), offset + 1),
            native_text_characters: 0,
            image_count: 0,
        })
        .collect()
}

fn collect_unit_nodes<'a>(
    node: &'a DocumentNode,
    kind: DocumentKind,
    output: &mut Vec<&'a DocumentNode>,
) {
    let is_unit = match kind {
        DocumentKind::Word => matches!(
            node.node_type,
            OfficeNodeType::Body | OfficeNodeType::Header | OfficeNodeType::Footer
        ),
        DocumentKind::Spreadsheet => node.node_type == OfficeNodeType::Worksheet,
        DocumentKind::Presentation => node.node_type == OfficeNodeType::Slide,
    };
    if is_unit {
        output.push(node);
    }
    for child in &node.children {
        collect_unit_nodes(child, kind, output);
    }
}

fn unit_label(node: &DocumentNode, kind: DocumentKind, index: usize) -> String {
    match kind {
        DocumentKind::Word => match node.node_type {
            OfficeNodeType::Body => "Document body".to_string(),
            OfficeNodeType::Header => format!("Header {index}"),
            OfficeNodeType::Footer => format!("Footer {index}"),
            _ => format!("Document unit {index}"),
        },
        DocumentKind::Spreadsheet => node.path.trim_start_matches('/').to_string(),
        DocumentKind::Presentation => format!("Slide {index}"),
    }
}

fn apply_recommendations(images: &mut [PreparedImage], units: &[DocumentUnit]) {
    let unit_text = units
        .iter()
        .map(|unit| (unit.path.as_str(), unit.native_text_characters))
        .collect::<BTreeMap<_, _>>();
    for image in images {
        if !image.descriptor.ocr_eligible {
            image.descriptor.recommendation = DocumentOcrRecommendation::Unsupported;
            continue;
        }
        let native_characters = unit_text
            .get(image.descriptor.unit_path.as_str())
            .copied()
            .unwrap_or(0);
        if native_characters == 0 {
            image.descriptor.recommendation = DocumentOcrRecommendation::Required;
            image.descriptor.recommendation_reason =
                "This document unit has no native text; OCR is required to recover raster text."
                    .to_string();
        } else if native_characters < 80 && meaningful_image(&image.descriptor) {
            image.descriptor.recommendation = DocumentOcrRecommendation::Suggested;
            image.descriptor.recommendation_reason =
                "This document unit has sparse native text and a meaningful raster image."
                    .to_string();
        } else {
            image.descriptor.recommendation = DocumentOcrRecommendation::Optional;
            image.descriptor.recommendation_reason =
                "Native text is available; OCR is optional for this raster image.".to_string();
        }
    }
}

fn meaningful_image(image: &DocumentImage) -> bool {
    matches!(
        (image.width_px, image.height_px),
        (Some(width), Some(height)) if width >= 96 && height >= 48
    ) || image.size.is_some_and(|size| size >= 4 * 1024)
}

struct ImageInfo {
    media_type: &'static str,
    width: u32,
    height: u32,
}

fn inspect_image(bytes: &[u8]) -> Result<ImageInfo, ()> {
    let format = image::guess_format(bytes).map_err(|_| ())?;
    let media_type = image_media_type(format).ok_or(())?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC);
    reader.limits(limits);
    let (width, height) = reader.into_dimensions().map_err(|_| ())?;
    if width == 0 || height == 0 {
        return Err(());
    }
    Ok(ImageInfo {
        media_type,
        width,
        height,
    })
}

fn image_media_type(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::WebP => Some("image/webp"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::Bmp => Some("image/bmp"),
        ImageFormat::Tiff => Some("image/tiff"),
        _ => None,
    }
}

fn source_kind(kind: DocumentKind) -> DocumentSourceKind {
    match kind {
        DocumentKind::Word => DocumentSourceKind::Word,
        DocumentKind::Spreadsheet => DocumentSourceKind::Spreadsheet,
        DocumentKind::Presentation => DocumentSourceKind::Presentation,
    }
}

fn office_media_type(kind: DocumentKind) -> &'static str {
    match kind {
        DocumentKind::Word => {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        }
        DocumentKind::Spreadsheet => {
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        }
        DocumentKind::Presentation => {
            "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        }
    }
}

async fn resolve_regular_file(path: &Path) -> UseResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                source_error(
                    "use.document.source_unreadable",
                    format!("Failed to resolve document source path: {error}"),
                )
            })?
            .join(path)
    };
    let metadata = tokio::fs::symlink_metadata(&absolute)
        .await
        .map_err(|error| {
            source_error(
                "use.document.source_unreadable",
                format!(
                    "Failed to inspect document source '{}': {error}",
                    absolute.display()
                ),
            )
        })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(source_error(
            "use.document.source_invalid",
            format!(
                "Document source '{}' must be a regular, non-symlink file.",
                absolute.display()
            ),
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_DOCUMENT_SOURCE_BYTES {
        return Err(source_error(
            "use.document.source_too_large",
            format!(
                "Document source '{}' must contain between 1 byte and 64 MiB.",
                absolute.display()
            ),
        )
        .with_detail("size", metadata.len()));
    }
    tokio::fs::canonicalize(&absolute).await.map_err(|error| {
        source_error(
            "use.document.source_unreadable",
            format!(
                "Failed to canonicalize document source '{}': {error}",
                absolute.display()
            ),
        )
    })
}

fn unsupported_source(path: &Path, detected: Option<&str>) -> UseError {
    let mut error = source_error(
        "use.document.source_type_unsupported",
        format!(
            "Document source '{}' must be DOCX, XLSX, PPTX, PNG, JPEG, WebP, GIF, BMP, or TIFF; PDF is not supported.",
            path.display()
        ),
    )
    .with_detail(
        "supportedExtensions",
        serde_json::json!(["docx", "xlsx", "pptx", "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"]),
    );
    if let Some(detected) = detected {
        error = error.with_detail("detectedMediaType", detected);
    }
    error
}

fn source_error(code: &str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn truncate_utf8(value: &str, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value.to_string(), false);
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}
