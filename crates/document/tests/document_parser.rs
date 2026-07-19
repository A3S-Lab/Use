use std::fs::File;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use a3s_use_core::{Artifact, Readiness, UseError, UseResult};
use a3s_use_document::{
    DocumentClient, DocumentImageOcrState, DocumentInspectRequest, DocumentOcrEngine,
    DocumentOcrPolicy, DocumentOcrRecommendation, DocumentParseRequest, DocumentSourceKind,
    DocumentTextOrigin,
};
use a3s_use_ocr::{OcrBlock, OcrBoundingBox, OcrDiagnostic, OcrPoint, OcrProviderKind, OcrResult};
use async_trait::async_trait;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[derive(Default)]
struct FakeState {
    extract_calls: AtomicUsize,
    first_use_calls: AtomicUsize,
}

#[derive(Clone)]
struct FakeOcr {
    state: Arc<FakeState>,
    text: String,
    models_ready: bool,
}

impl FakeOcr {
    fn ready(text: impl Into<String>) -> (Arc<Self>, Arc<FakeState>) {
        let state = Arc::new(FakeState::default());
        (
            Arc::new(Self {
                state: Arc::clone(&state),
                text: text.into(),
                models_ready: true,
            }),
            state,
        )
    }

    fn missing_until_first_use(text: impl Into<String>) -> (Arc<Self>, Arc<FakeState>) {
        let state = Arc::new(FakeState::default());
        (
            Arc::new(Self {
                state: Arc::clone(&state),
                text: text.into(),
                models_ready: false,
            }),
            state,
        )
    }

    async fn result(&self, path: &Path) -> UseResult<OcrResult> {
        let bytes = tokio::fs::read(path).await.map_err(|error| {
            UseError::new(
                "test.ocr.source_unreadable",
                format!("Failed to read fake OCR source: {error}"),
            )
        })?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let text = self.text.clone();
        Ok(OcrResult {
            provider: OcrProviderKind::PpOcrV6,
            engine: "fake-onnx-runtime".to_string(),
            model: "PP-OCRv6_small".to_string(),
            source: Artifact {
                path: path.to_path_buf(),
                media_type: "image/png".to_string(),
                size: bytes.len() as u64,
                sha256: digest,
            },
            blocks: if text.is_empty() {
                Vec::new()
            } else {
                vec![OcrBlock {
                    page: 1,
                    text: text.clone(),
                    confidence: 0.95,
                    detection_confidence: 0.9,
                    polygon: [
                        OcrPoint { x: 1, y: 2 },
                        OcrPoint { x: 101, y: 2 },
                        OcrPoint { x: 101, y: 22 },
                        OcrPoint { x: 1, y: 22 },
                    ],
                    bounding_box: OcrBoundingBox {
                        x: 1,
                        y: 2,
                        width: 100,
                        height: 20,
                    },
                }]
            },
            text,
            warnings: Vec::new(),
        })
    }
}

#[async_trait]
impl DocumentOcrEngine for FakeOcr {
    fn diagnostic(&self) -> OcrDiagnostic {
        OcrDiagnostic {
            readiness: if self.models_ready {
                Readiness::Ready
            } else {
                Readiness::Missing
            },
            provider: Some(OcrProviderKind::PpOcrV6),
            engine: Some("fake-onnx-runtime".to_string()),
            model: Some("PP-OCRv6_small".to_string()),
            model_dir: Some(PathBuf::from("/fake/models")),
            sends_source_off_device: false,
            message: if self.models_ready {
                "Fake models are ready.".to_string()
            } else {
                "Fake models are missing.".to_string()
            },
            suggestions: Vec::new(),
        }
    }

    async fn extract(&self, path: &Path) -> UseResult<OcrResult> {
        self.state.extract_calls.fetch_add(1, Ordering::SeqCst);
        if !self.models_ready {
            return Err(UseError::new(
                "use.ocr.models_missing",
                "Pinned PP-OCRv6 models are missing.",
            ));
        }
        self.result(path).await
    }

    async fn extract_with_first_use(&self, path: &Path) -> UseResult<OcrResult> {
        self.state.first_use_calls.fetch_add(1, Ordering::SeqCst);
        self.result(path).await
    }
}

#[tokio::test]
async fn inspect_combines_native_docx_structure_with_raster_candidates_without_ocr() {
    let fixture = Fixture::docx(Some("Native Office"), 1);
    let (ocr, state) = FakeOcr::ready("Raster evidence");
    let client = DocumentClient::with_ocr_engine(ocr);

    let result = client
        .inspect(DocumentInspectRequest {
            path: fixture.path.clone(),
        })
        .await
        .unwrap();

    assert_eq!(state.extract_calls.load(Ordering::SeqCst), 0);
    assert_eq!(state.first_use_calls.load(Ordering::SeqCst), 0);
    assert_eq!(result.source.kind, DocumentSourceKind::Word);
    assert_eq!(result.source.sha256.len(), 64);
    assert_eq!(result.source.content_sha256.as_deref().unwrap().len(), 64);
    assert!(result.native_text.contains("Native Office"));
    assert_eq!(result.units.len(), 1);
    assert_eq!(result.units[0].path, "/body");
    assert_eq!(result.images.len(), 1);
    assert_eq!(
        result.images[0].source_part.as_deref(),
        Some("/word/media/image1.png")
    );
    assert_eq!(
        result.images[0].recommendation,
        DocumentOcrRecommendation::Suggested
    );
    assert_eq!(result.images[0].sha256.as_deref().unwrap().len(), 64);
}

#[tokio::test]
async fn image_only_docx_auto_ocr_preserves_precise_provenance() {
    let fixture = Fixture::docx(None, 1);
    let (ocr, state) = FakeOcr::ready("A3S OCR 2026");
    let client = DocumentClient::with_ocr_engine(ocr);

    let result = client
        .parse(DocumentParseRequest::new(&fixture.path))
        .await
        .unwrap();

    assert_eq!(state.extract_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.first_use_calls.load(Ordering::SeqCst), 0);
    assert_eq!(result.text, "A3S OCR 2026");
    assert_eq!(result.ocr.selected_images, 1);
    assert_eq!(result.ocr.processed_images, 1);
    assert_eq!(result.ocr.reused_artifacts, 0);
    assert!(!result.ocr.sends_source_off_device);
    assert_eq!(result.images[0].ocr_state, DocumentImageOcrState::Processed);
    let block = result
        .blocks
        .iter()
        .find(|block| block.origin == DocumentTextOrigin::PpOcrV6)
        .unwrap();
    assert_eq!(block.source_sha256, result.source.sha256);
    assert_eq!(block.artifact_sha256, result.images[0].sha256);
    assert_eq!(block.source_part, result.images[0].source_part);
    assert_eq!(block.model.as_deref(), Some("PP-OCRv6_small"));
    assert_eq!(block.confidence, Some(0.95));
    assert!(block.polygon.is_some());
    assert!(block.bounding_box.is_some());
}

#[tokio::test]
async fn direct_parse_uses_first_use_preparation_but_read_only_parse_does_not() {
    let fixture = Fixture::image();
    let (missing, state) = FakeOcr::missing_until_first_use("Prepared locally");
    let client = DocumentClient::with_ocr_engine(missing);

    let error = client
        .parse(DocumentParseRequest::new(&fixture.path))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.ocr.models_missing");
    assert_eq!(state.extract_calls.load(Ordering::SeqCst), 1);
    assert_eq!(state.first_use_calls.load(Ordering::SeqCst), 0);

    let result = client
        .parse_with_first_use(DocumentParseRequest::new(&fixture.path))
        .await
        .unwrap();
    assert_eq!(result.text, "Prepared locally");
    assert_eq!(state.first_use_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn duplicate_ocr_text_remains_evidence_but_is_not_merged_twice() {
    let fixture = Fixture::docx(Some("Quarterly Revenue"), 1);
    let (ocr, _) = FakeOcr::ready("quarterly revenue");
    let client = DocumentClient::with_ocr_engine(ocr);
    let mut request = DocumentParseRequest::new(&fixture.path);
    request.ocr = DocumentOcrPolicy::Always;

    let result = client.parse(request).await.unwrap();

    assert_eq!(result.text, "Quarterly Revenue");
    let ocr = result
        .blocks
        .iter()
        .find(|block| block.origin == DocumentTextOrigin::PpOcrV6)
        .unwrap();
    assert!(!ocr.included_in_text);
}

#[tokio::test]
async fn explicit_semantic_path_limits_ocr_to_that_image() {
    let fixture = Fixture::docx(None, 2);
    let (ocr, state) = FakeOcr::ready("Selected");
    let client = DocumentClient::with_ocr_engine(ocr);
    let inspection = client
        .inspect(DocumentInspectRequest {
            path: fixture.path.clone(),
        })
        .await
        .unwrap();
    assert_eq!(inspection.images.len(), 2);
    let selected_path = inspection.images[1].semantic_path.clone();
    let mut request = DocumentParseRequest::new(&fixture.path);
    request.image_paths = vec![selected_path.clone()];

    let result = client.parse(request).await.unwrap();

    assert_eq!(state.extract_calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.ocr.selected_images, 1);
    assert_eq!(
        result
            .images
            .iter()
            .find(|image| image.semantic_path == selected_path)
            .unwrap()
            .ocr_state,
        DocumentImageOcrState::Processed
    );
    assert_eq!(
        result
            .images
            .iter()
            .filter(|image| image.ocr_state == DocumentImageOcrState::NotSelected)
            .count(),
        1
    );
}

#[tokio::test]
async fn repeated_embedded_artifact_runs_inference_once() {
    let fixture = Fixture::docx(None, 2);
    let (ocr, state) = FakeOcr::ready("Shared image");
    let client = DocumentClient::with_ocr_engine(ocr);
    let mut request = DocumentParseRequest::new(&fixture.path);
    request.ocr = DocumentOcrPolicy::Always;

    let result = client.parse(request).await.unwrap();

    assert_eq!(state.extract_calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.ocr.selected_images, 2);
    assert_eq!(result.ocr.processed_images, 1);
    assert_eq!(result.ocr.reused_artifacts, 1);
}

#[tokio::test]
async fn standalone_image_is_one_required_image_unit() {
    let fixture = Fixture::image();
    let (ocr, _) = FakeOcr::ready("Standalone");
    let client = DocumentClient::with_ocr_engine(ocr);

    let inspection = client
        .inspect(DocumentInspectRequest {
            path: fixture.path.clone(),
        })
        .await
        .unwrap();

    assert_eq!(inspection.source.kind, DocumentSourceKind::Image);
    assert_eq!(inspection.units.len(), 1);
    assert_eq!(inspection.units[0].path, "/image");
    assert_eq!(
        inspection.images[0].recommendation,
        DocumentOcrRecommendation::Required
    );
}

#[tokio::test]
async fn native_xlsx_and_pptx_units_are_supported_without_external_office() {
    let (ocr, state) = FakeOcr::ready("");
    let client = DocumentClient::with_ocr_engine(ocr);
    let spreadsheet = Fixture::xlsx();
    let presentation = Fixture::pptx();

    let xlsx = client
        .inspect(DocumentInspectRequest {
            path: spreadsheet.path.clone(),
        })
        .await
        .unwrap();
    let pptx = client
        .inspect(DocumentInspectRequest {
            path: presentation.path.clone(),
        })
        .await
        .unwrap();

    assert_eq!(xlsx.source.kind, DocumentSourceKind::Spreadsheet);
    assert_eq!(xlsx.units[0].path, "/Sheet1");
    assert!(xlsx.native_text.contains("/Sheet1/A1=Revenue"));
    assert_eq!(pptx.source.kind, DocumentSourceKind::Presentation);
    assert_eq!(pptx.units[0].path, "/slide[1]");
    assert!(pptx.native_text.contains("Native Presentation"));
    assert_eq!(state.extract_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn pdf_and_malformed_raster_fail_with_typed_errors() {
    let directory = TempDir::new().unwrap();
    let pdf = directory.path().join("source.pdf");
    std::fs::write(&pdf, b"%PDF-1.7\n").unwrap();
    let malformed = directory.path().join("source.png");
    std::fs::write(&malformed, b"not a raster").unwrap();
    let (ocr, _) = FakeOcr::ready("");
    let client = DocumentClient::with_ocr_engine(ocr);

    let pdf_error = client
        .inspect(DocumentInspectRequest { path: pdf })
        .await
        .unwrap_err();
    let raster_error = client
        .inspect(DocumentInspectRequest { path: malformed })
        .await
        .unwrap_err();

    assert_eq!(pdf_error.code, "use.document.source_type_unsupported");
    assert_eq!(raster_error.code, "use.document.source_type_unsupported");
}

#[tokio::test]
async fn invalid_ocr_bounds_and_unknown_paths_are_rejected_before_inference() {
    let fixture = Fixture::image();
    let (ocr, state) = FakeOcr::ready("");
    let client = DocumentClient::with_ocr_engine(ocr);
    let mut invalid_limit = DocumentParseRequest::new(&fixture.path);
    invalid_limit.max_images = 17;
    let mut unknown = DocumentParseRequest::new(&fixture.path);
    unknown.image_paths = vec!["/missing".to_string()];

    let limit_error = client.parse(invalid_limit).await.unwrap_err();
    let path_error = client.parse(unknown).await.unwrap_err();

    assert_eq!(limit_error.code, "use.document.ocr_limit_invalid");
    assert_eq!(path_error.code, "use.document.image_not_found");
    assert_eq!(state.extract_calls.load(Ordering::SeqCst), 0);
}

struct Fixture {
    _directory: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn image() -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("source.png");
        std::fs::write(&path, test_png()).unwrap();
        Self {
            _directory: directory,
            path,
        }
    }

    fn docx(native_text: Option<&str>, image_count: usize) -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("source.docx");
        write_docx(&path, native_text, image_count);
        Self {
            _directory: directory,
            path,
        }
    }

    fn xlsx() -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("source.xlsx");
        write_zip(
            &path,
            vec![
                ("[Content_Types].xml", xlsx_content_types().into_bytes()),
                (
                    "_rels/.rels",
                    root_relationship("xl/workbook.xml").into_bytes(),
                ),
                ("xl/workbook.xml", workbook_xml().into_bytes()),
                (
                    "xl/_rels/workbook.xml.rels",
                    workbook_relationships().into_bytes(),
                ),
                ("xl/worksheets/sheet1.xml", worksheet_xml().into_bytes()),
            ],
        );
        Self {
            _directory: directory,
            path,
        }
    }

    fn pptx() -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("source.pptx");
        write_zip(
            &path,
            vec![
                ("[Content_Types].xml", pptx_content_types().into_bytes()),
                (
                    "_rels/.rels",
                    root_relationship("ppt/presentation.xml").into_bytes(),
                ),
                ("ppt/presentation.xml", presentation_xml().into_bytes()),
                (
                    "ppt/_rels/presentation.xml.rels",
                    presentation_relationships().into_bytes(),
                ),
                ("ppt/slides/slide1.xml", slide_xml().into_bytes()),
            ],
        );
        Self {
            _directory: directory,
            path,
        }
    }
}

fn write_docx(path: &Path, native_text: Option<&str>, image_count: usize) {
    let mut body = String::new();
    if let Some(text) = native_text {
        body.push_str(&format!(
            "<w:p><w:r><w:t>{}</w:t></w:r></w:p>",
            xml_escape(text)
        ));
    }
    for index in 1..=image_count {
        body.push_str(&format!(
            "<w:p><w:r><w:drawing><wp:inline><wp:extent cx=\"1905000\" cy=\"762000\"/><wp:docPr id=\"{index}\" name=\"Picture {index}\" descr=\"Raster {index}\"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed=\"rIdImage{index}\"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"
        ));
    }
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><w:body>{body}<w:sectPr/></w:body></w:document>"
    );
    let relationships = (1..=image_count)
        .map(|index| {
            format!(
                "<Relationship Id=\"rIdImage{index}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/image1.png\"/>"
            )
        })
        .collect::<String>();
    let document_relationships = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{relationships}</Relationships>"
    );
    let mut entries = vec![
        ("[Content_Types].xml", docx_content_types().into_bytes()),
        (
            "_rels/.rels",
            root_relationship("word/document.xml").into_bytes(),
        ),
        ("word/document.xml", document.into_bytes()),
    ];
    if image_count > 0 {
        entries.push((
            "word/_rels/document.xml.rels",
            document_relationships.into_bytes(),
        ));
        entries.push(("word/media/image1.png", test_png()));
    }
    write_zip(path, entries);
}

fn write_zip(path: &Path, entries: Vec<(&str, Vec<u8>)>) {
    let file = File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in entries {
        archive.start_file(name, options).unwrap();
        archive.write_all(&bytes).unwrap();
    }
    archive.finish().unwrap();
}

fn test_png() -> Vec<u8> {
    let mut image = RgbaImage::new(200, 80);
    for pixel in image.pixels_mut() {
        *pixel = Rgba([255, 255, 255, 255]);
    }
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut output, ImageFormat::Png)
        .unwrap();
    output.into_inner()
}

fn root_relationship(target: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"{target}\"/></Relationships>"
    )
}

fn docx_content_types() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>".to_string()
}

fn xlsx_content_types() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>".to_string()
}

fn workbook_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets><sheet name=\"Sheet1\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>".to_string()
}

fn workbook_relationships() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>".to_string()
}

fn worksheet_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>Revenue</t></is></c></row></sheetData></worksheet>".to_string()
}

fn pptx_content_types() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/><Override PartName=\"/ppt/slides/slide1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/></Types>".to_string()
}

fn presentation_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><p:presentation xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><p:sldIdLst><p:sldId id=\"256\" r:id=\"rId1\"/></p:sldIdLst></p:presentation>".to_string()
}

fn presentation_relationships() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide1.xml\"/></Relationships>".to_string()
}

fn slide_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><p:cSld><p:spTree><p:nvGrpSpPr/><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title\"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Native Presentation</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>".to_string()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
