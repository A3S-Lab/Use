use std::collections::BTreeMap;
use std::path::Path;
#[cfg(all(test, unix))]
use std::path::PathBuf;
use std::process::Stdio;

use a3s_use_core::{Artifact, UseError, UseResult};
use base64::Engine;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::models::{OcrBlock, OcrBoundingBox, OcrProviderKind, OcrRequest, OcrResult};
use crate::provider::{Provider, ProviderConfig};
use crate::OcrDiagnostic;

const MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PROVIDER_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_VISION_PROMPT: &str = "Transcribe all visible text in reading order. Preserve line breaks and meaningful spacing. Return only the transcription; do not summarize, translate, or wrap it in Markdown.";

#[derive(Clone)]
pub struct OcrClient {
    providers: ProviderConfig,
    http: reqwest::Client,
}

impl OcrClient {
    pub fn from_env() -> UseResult<Self> {
        Self::from_provider_config(ProviderConfig::from_env()?)
    }

    fn from_provider_config(providers: ProviderConfig) -> UseResult<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("a3s-use-ocr/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| {
                UseError::new(
                    "use.ocr.client_failed",
                    format!("Failed to initialize the OCR HTTP client: {error}"),
                )
            })?;
        Ok(Self { providers, http })
    }

    #[cfg(all(test, unix))]
    pub(crate) fn with_tesseract(executable: PathBuf) -> UseResult<Self> {
        Self::from_provider_config(ProviderConfig::tesseract(executable))
    }

    pub fn diagnostic(&self) -> OcrDiagnostic {
        self.providers.diagnostic()
    }

    pub async fn extract(&self, request: OcrRequest) -> UseResult<OcrResult> {
        validate_request(&request)?;
        let source = read_source(&request.path).await?;
        let provider = self
            .providers
            .resolve(request.provider.unwrap_or(OcrProviderKind::Auto))?;
        let languages = if request.languages.is_empty() {
            vec!["eng".to_string()]
        } else {
            request.languages.clone()
        };

        let (text, blocks, warnings) = match &provider {
            Provider::Tesseract {
                executable,
                timeout,
            } => {
                let output = run_tesseract(
                    executable,
                    &source.artifact.path,
                    &languages,
                    request.page_segmentation_mode,
                    *timeout,
                )
                .await?;
                let (text, blocks) = parse_tesseract_tsv(&output)?;
                (text, blocks, Vec::new())
            }
            Provider::Vision {
                endpoint,
                api_key,
                model,
                timeout,
            } => {
                let text = self
                    .run_vision(
                        endpoint,
                        api_key.as_deref(),
                        model,
                        &source,
                        request.prompt.as_deref(),
                        *timeout,
                    )
                    .await?;
                let blocks = (!text.is_empty())
                    .then(|| OcrBlock {
                        page: 1,
                        text: text.clone(),
                        confidence: None,
                        bounding_box: None,
                    })
                    .into_iter()
                    .collect();
                (
                    text,
                    blocks,
                    vec![
                        "The vision provider does not return calibrated OCR confidence or bounding boxes."
                            .to_string(),
                    ],
                )
            }
        };

        Ok(OcrResult {
            provider: provider.kind(),
            source: source.artifact,
            languages,
            text,
            blocks,
            warnings,
        })
    }

    async fn run_vision(
        &self,
        endpoint: &url::Url,
        api_key: Option<&str>,
        model: &str,
        source: &SourceImage,
        prompt: Option<&str>,
        timeout: std::time::Duration,
    ) -> UseResult<String> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&source.bytes);
        let data_url = format!("data:{};base64,{encoded}", source.artifact.media_type);
        let prompt = prompt
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_VISION_PROMPT);
        let body = serde_json::json!({
            "model": model,
            "temperature": 0,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    {
                        "type": "image_url",
                        "image_url": {
                            "url": data_url,
                            "detail": "high"
                        }
                    }
                ]
            }]
        });
        let mut request = self
            .http
            .post(endpoint.clone())
            .timeout(timeout)
            .json(&body);
        if let Some(api_key) = api_key {
            request = request.bearer_auth(api_key);
        }
        let response = request.send().await.map_err(|error| {
            UseError::new(
                "use.ocr.vision_request_failed",
                format!("The vision OCR request failed: {error}"),
            )
            .with_detail("endpoint", redacted_endpoint(endpoint))
        })?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| {
            UseError::new(
                "use.ocr.vision_response_invalid",
                format!("Failed to read the vision OCR response: {error}"),
            )
        })?;
        if bytes.len() > MAX_PROVIDER_OUTPUT_BYTES {
            return Err(UseError::new(
                "use.ocr.output_too_large",
                "The vision OCR provider response exceeded 8 MiB.",
            ));
        }
        if !status.is_success() {
            let message = String::from_utf8_lossy(&bytes);
            return Err(UseError::new(
                "use.ocr.vision_request_failed",
                format!(
                    "The vision OCR provider returned HTTP {status}: {}",
                    bounded_text(&message, 1024)
                ),
            )
            .with_detail("status", u64::from(status.as_u16())));
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
            UseError::new(
                "use.ocr.vision_response_invalid",
                format!("The vision OCR provider returned invalid JSON: {error}"),
            )
        })?;
        let content = value.pointer("/choices/0/message/content").ok_or_else(|| {
            UseError::new(
                "use.ocr.vision_response_invalid",
                "The vision OCR response did not contain choices[0].message.content.",
            )
        })?;
        let text = vision_content_text(content)?;
        Ok(text.trim().to_string())
    }
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

fn validate_request(request: &OcrRequest) -> UseResult<()> {
    if request.languages.len() > 16 {
        return Err(UseError::new(
            "use.ocr.languages_invalid",
            "At most 16 OCR language identifiers may be requested.",
        ));
    }
    for language in &request.languages {
        if language.is_empty()
            || language.len() > 32
            || !language
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(UseError::new(
                "use.ocr.languages_invalid",
                format!("OCR language identifier '{language}' is invalid."),
            ));
        }
    }
    if request.page_segmentation_mode.is_some_and(|mode| mode > 13) {
        return Err(UseError::new(
            "use.ocr.page_segmentation_invalid",
            "Tesseract page segmentation mode must be from 0 through 13.",
        ));
    }
    if request
        .prompt
        .as_ref()
        .is_some_and(|prompt| prompt.len() > 8 * 1024)
    {
        return Err(UseError::new(
            "use.ocr.prompt_too_large",
            "The vision OCR prompt must not exceed 8192 bytes.",
        ));
    }
    Ok(())
}

async fn run_tesseract(
    executable: &Path,
    source: &Path,
    languages: &[String],
    page_segmentation_mode: Option<u8>,
    timeout: std::time::Duration,
) -> UseResult<Vec<u8>> {
    let mut command = Command::new(executable);
    command
        .arg(source)
        .arg("stdout")
        .arg("-l")
        .arg(languages.join("+"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(mode) = page_segmentation_mode {
        command.arg("--psm").arg(mode.to_string());
    }
    command.arg("tsv");

    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| {
            UseError::new(
                "use.ocr.provider_timeout",
                format!(
                    "Tesseract exceeded the {} ms OCR timeout.",
                    timeout.as_millis()
                ),
            )
        })?
        .map_err(|error| {
            UseError::new(
                "use.ocr.provider_failed",
                format!(
                    "Failed to launch Tesseract executable '{}': {error}",
                    executable.display()
                ),
            )
        })?;
    if output.stdout.len() > MAX_PROVIDER_OUTPUT_BYTES
        || output.stderr.len() > MAX_PROVIDER_OUTPUT_BYTES
    {
        return Err(UseError::new(
            "use.ocr.output_too_large",
            "Tesseract output exceeded 8 MiB.",
        ));
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(UseError::new(
            "use.ocr.provider_failed",
            format!(
                "Tesseract exited with {}: {}",
                output.status,
                bounded_text(&stderr, 2048)
            ),
        ));
    }
    Ok(output.stdout)
}

#[derive(Default)]
struct LineAccumulator {
    page: u32,
    words: Vec<String>,
    confidence_sum: f32,
    confidence_count: usize,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

fn parse_tesseract_tsv(output: &[u8]) -> UseResult<(String, Vec<OcrBlock>)> {
    let output = std::str::from_utf8(output).map_err(|error| {
        UseError::new(
            "use.ocr.provider_output_invalid",
            format!("Tesseract TSV output was not UTF-8: {error}"),
        )
    })?;
    let mut lines = BTreeMap::<(u32, u32, u32, u32), LineAccumulator>::new();
    for (index, row) in output.lines().enumerate() {
        if index == 0 && row.starts_with("level\t") {
            continue;
        }
        if row.trim().is_empty() {
            continue;
        }
        let columns = row.splitn(12, '\t').collect::<Vec<_>>();
        if columns.len() != 12 {
            return Err(UseError::new(
                "use.ocr.provider_output_invalid",
                format!(
                    "Tesseract TSV row {} did not contain 12 columns.",
                    index + 1
                ),
            ));
        }
        let level = parse_u32(columns[0], index)?;
        if level != 5 || columns[11].trim().is_empty() {
            continue;
        }
        let page = parse_u32(columns[1], index)?;
        let block = parse_u32(columns[2], index)?;
        let paragraph = parse_u32(columns[3], index)?;
        let line = parse_u32(columns[4], index)?;
        let left = parse_u32(columns[6], index)?;
        let top = parse_u32(columns[7], index)?;
        let width = parse_u32(columns[8], index)?;
        let height = parse_u32(columns[9], index)?;
        let confidence = columns[10]
            .parse::<f32>()
            .ok()
            .filter(|value| *value >= 0.0);
        let entry = lines
            .entry((page, block, paragraph, line))
            .or_insert_with(|| LineAccumulator {
                page,
                left,
                top,
                right: left.saturating_add(width),
                bottom: top.saturating_add(height),
                ..LineAccumulator::default()
            });
        entry.words.push(columns[11].trim().to_string());
        if let Some(confidence) = confidence {
            entry.confidence_sum += confidence;
            entry.confidence_count += 1;
        }
        entry.left = entry.left.min(left);
        entry.top = entry.top.min(top);
        entry.right = entry.right.max(left.saturating_add(width));
        entry.bottom = entry.bottom.max(top.saturating_add(height));
    }
    let blocks = lines
        .into_values()
        .filter_map(|line| {
            let text = line.words.join(" ");
            (!text.is_empty()).then(|| OcrBlock {
                page: line.page,
                text,
                confidence: (line.confidence_count > 0)
                    .then(|| line.confidence_sum / line.confidence_count as f32),
                bounding_box: Some(OcrBoundingBox {
                    x: line.left,
                    y: line.top,
                    width: line.right.saturating_sub(line.left),
                    height: line.bottom.saturating_sub(line.top),
                }),
            })
        })
        .collect::<Vec<_>>();
    let text = blocks
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok((text, blocks))
}

fn parse_u32(value: &str, row: usize) -> UseResult<u32> {
    value.parse::<u32>().map_err(|_| {
        UseError::new(
            "use.ocr.provider_output_invalid",
            format!(
                "Tesseract TSV row {} contained an invalid integer.",
                row + 1
            ),
        )
    })
}

fn vision_content_text(content: &serde_json::Value) -> UseResult<String> {
    if let Some(text) = content.as_str() {
        return Ok(text.to_string());
    }
    let Some(parts) = content.as_array() else {
        return Err(UseError::new(
            "use.ocr.vision_response_invalid",
            "Vision OCR message content was neither text nor a text-part array.",
        ));
    };
    let text = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(serde_json::Value::as_str)
                .or_else(|| part.as_str())
        })
        .collect::<Vec<_>>()
        .join("");
    if text.is_empty() {
        return Err(UseError::new(
            "use.ocr.vision_response_invalid",
            "Vision OCR message content did not contain text.",
        ));
    }
    Ok(text)
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

fn bounded_text(value: &str, max: usize) -> String {
    let mut text = value.chars().take(max).collect::<String>();
    if value.chars().count() > max {
        text.push('…');
    }
    text
}

fn redacted_endpoint(endpoint: &url::Url) -> String {
    let mut redacted = endpoint.clone();
    redacted.set_query(None);
    redacted.set_fragment(None);
    redacted.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn parses_tesseract_words_into_ordered_lines() {
        let tsv = b"level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n5\t1\t1\t1\t1\t1\t10\t20\t30\t10\t95.0\tHello\n5\t1\t1\t1\t1\t2\t45\t20\t35\t10\t85.0\tworld\n5\t1\t1\t1\t2\t1\t10\t40\t20\t10\t90.0\tNext\n";
        let (text, blocks) = parse_tesseract_tsv(tsv).unwrap();
        assert_eq!(text, "Hello world\nNext");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].confidence, Some(90.0));
        assert_eq!(
            blocks[0].bounding_box,
            Some(OcrBoundingBox {
                x: 10,
                y: 20,
                width: 70,
                height: 10,
            })
        );
    }

    #[test]
    fn detects_supported_image_signatures() {
        assert_eq!(
            detect_image_type(b"\x89PNG\r\n\x1a\nrest"),
            Some("image/png")
        );
        assert_eq!(detect_image_type(b"\xff\xd8\xffrest"), Some("image/jpeg"));
        assert_eq!(detect_image_type(b"not an image"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_provider_extracts_a_real_bounded_source_through_its_process_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("tesseract-fixture");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf 'level\\tpage_num\\tblock_num\\tpar_num\\tline_num\\tword_num\\tleft\\ttop\\twidth\\theight\\tconf\\ttext\\n5\\t1\\t1\\t1\\t1\\t1\\t2\\t3\\t20\\t8\\t98.0\\tA3S\\n5\\t1\\t1\\t1\\t1\\t2\\t24\\t3\\t30\\t8\\t96.0\\tUse\\n'\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let image = temp.path().join("scan.png");
        std::fs::write(&image, b"\x89PNG\r\n\x1a\nfixture").unwrap();

        let result = OcrClient::with_tesseract(executable)
            .unwrap()
            .extract(OcrRequest {
                path: image,
                languages: vec!["eng".to_string()],
                page_segmentation_mode: Some(6),
                provider: Some(OcrProviderKind::Tesseract),
                prompt: None,
            })
            .await
            .unwrap();

        assert_eq!(result.provider, OcrProviderKind::Tesseract);
        assert_eq!(result.text, "A3S Use");
        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.source.media_type, "image/png");
        assert_eq!(result.source.sha256.len(), 64);
    }
}
