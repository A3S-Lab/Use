use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use a3s_use_core::{Readiness, UseError, UseResult};
use url::Url;

use crate::{OcrDiagnostic, OcrProviderKind};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_VISION_BASE_URL: &str = "https://api.openai.com/v1/";

#[derive(Debug, Clone)]
pub(crate) enum Provider {
    Tesseract {
        executable: PathBuf,
        timeout: Duration,
    },
    Vision {
        endpoint: Url,
        api_key: Option<String>,
        model: String,
        timeout: Duration,
    },
}

impl Provider {
    pub(crate) fn kind(&self) -> OcrProviderKind {
        match self {
            Self::Tesseract { .. } => OcrProviderKind::Tesseract,
            Self::Vision { .. } => OcrProviderKind::Vision,
        }
    }

    pub(crate) fn diagnostic(&self) -> OcrDiagnostic {
        match self {
            Self::Tesseract { executable, .. } => OcrDiagnostic {
                readiness: Readiness::Ready,
                provider: Some(OcrProviderKind::Tesseract),
                executable: Some(executable.clone()),
                endpoint: None,
                model: None,
                sends_source_off_device: false,
                message: "The local Tesseract OCR provider is ready.".to_string(),
                suggestions: Vec::new(),
            },
            Self::Vision {
                endpoint, model, ..
            } => OcrDiagnostic {
                readiness: Readiness::Ready,
                provider: Some(OcrProviderKind::Vision),
                executable: None,
                endpoint: Some(redacted_endpoint(endpoint)),
                model: Some(model.clone()),
                sends_source_off_device: !is_loopback(endpoint),
                message: "The explicitly configured vision OCR provider is ready.".to_string(),
                suggestions: Vec::new(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProviderConfig {
    requested: OcrProviderKind,
    tesseract: Option<PathBuf>,
    vision: Option<VisionConfig>,
    timeout: Duration,
}

#[derive(Debug, Clone)]
struct VisionConfig {
    endpoint: Url,
    api_key: Option<String>,
    model: String,
}

impl ProviderConfig {
    pub(crate) fn from_env() -> UseResult<Self> {
        let requested = match env::var("A3S_OCR_PROVIDER")
            .unwrap_or_else(|_| "auto".to_string())
            .trim()
        {
            "" | "auto" => OcrProviderKind::Auto,
            "tesseract" => OcrProviderKind::Tesseract,
            "vision" => OcrProviderKind::Vision,
            value => {
                return Err(UseError::new(
                    "use.ocr.provider_invalid",
                    format!("Unknown OCR provider '{value}'; expected auto, tesseract, or vision."),
                ))
            }
        };

        let timeout = timeout_from_env()?;
        let tesseract = env::var_os("A3S_OCR_TESSERACT_EXECUTABLE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| find_on_path("tesseract"));
        let vision = vision_config_from_env()?;
        Ok(Self {
            requested,
            tesseract,
            vision,
            timeout,
        })
    }

    #[cfg(all(test, unix))]
    pub(crate) fn tesseract(executable: PathBuf) -> Self {
        Self {
            requested: OcrProviderKind::Tesseract,
            tesseract: Some(executable),
            vision: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub(crate) fn diagnostic(&self) -> OcrDiagnostic {
        match self.resolve(self.requested) {
            Ok(provider) => provider.diagnostic(),
            Err(error) => OcrDiagnostic {
                readiness: Readiness::Missing,
                provider: match self.requested {
                    OcrProviderKind::Auto => None,
                    provider => Some(provider),
                },
                executable: self.tesseract.clone(),
                endpoint: self
                    .vision
                    .as_ref()
                    .map(|vision| redacted_endpoint(&vision.endpoint)),
                model: self.vision.as_ref().map(|vision| vision.model.clone()),
                sends_source_off_device: self
                    .vision
                    .as_ref()
                    .is_some_and(|vision| !is_loopback(&vision.endpoint)),
                message: error.message,
                suggestions: error.suggestion.into_iter().collect(),
            },
        }
    }

    pub(crate) fn resolve(&self, requested: OcrProviderKind) -> UseResult<Provider> {
        let requested = if requested == OcrProviderKind::Auto {
            self.requested
        } else {
            requested
        };
        match requested {
            OcrProviderKind::Auto => {
                if let Some(executable) = &self.tesseract {
                    return tesseract_provider(executable, self.timeout);
                }
                if let Some(vision) = &self.vision {
                    return Ok(vision_provider(vision, self.timeout));
                }
                Err(missing_provider())
            }
            OcrProviderKind::Tesseract => self
                .tesseract
                .as_ref()
                .ok_or_else(missing_tesseract)
                .and_then(|path| tesseract_provider(path, self.timeout)),
            OcrProviderKind::Vision => self
                .vision
                .as_ref()
                .map(|vision| vision_provider(vision, self.timeout))
                .ok_or_else(missing_vision),
        }
    }
}

fn tesseract_provider(path: &Path, timeout: Duration) -> UseResult<Provider> {
    let path = std::fs::canonicalize(path).map_err(|error| {
        UseError::new(
            "use.ocr.provider_missing",
            format!(
                "Configured Tesseract executable '{}' is not readable: {error}",
                path.display()
            ),
        )
        .with_suggestion(
            "Install Tesseract explicitly or configure the vision provider; A3S Use will not install an OCR provider automatically.",
        )
    })?;
    let metadata = std::fs::metadata(&path).map_err(|error| {
        UseError::new(
            "use.ocr.provider_missing",
            format!(
                "Configured Tesseract executable '{}' is not readable: {error}",
                path.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(UseError::new(
            "use.ocr.provider_invalid",
            format!(
                "Configured Tesseract path '{}' is not a regular file.",
                path.display()
            ),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(UseError::new(
                "use.ocr.provider_invalid",
                format!(
                    "Configured Tesseract path '{}' is not executable.",
                    path.display()
                ),
            ));
        }
    }
    Ok(Provider::Tesseract {
        executable: path,
        timeout,
    })
}

fn vision_provider(config: &VisionConfig, timeout: Duration) -> Provider {
    Provider::Vision {
        endpoint: config.endpoint.clone(),
        api_key: config.api_key.clone(),
        model: config.model.clone(),
        timeout,
    }
}

fn vision_config_from_env() -> UseResult<Option<VisionConfig>> {
    let model = env::var("A3S_OCR_VISION_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let base_url = env::var("A3S_OCR_VISION_BASE_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let api_key = env::var("A3S_OCR_VISION_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if model.is_none() && base_url.is_none() && api_key.is_none() {
        return Ok(None);
    }
    let model = model.ok_or_else(|| {
        UseError::new(
            "use.ocr.vision_config_invalid",
            "A3S_OCR_VISION_MODEL is required when the vision OCR provider is configured.",
        )
    })?;
    let mut base = base_url.unwrap_or_else(|| DEFAULT_VISION_BASE_URL.to_string());
    if !base.ends_with('/') {
        base.push('/');
    }
    let base = Url::parse(&base).map_err(|error| {
        UseError::new(
            "use.ocr.vision_config_invalid",
            format!("A3S_OCR_VISION_BASE_URL is invalid: {error}"),
        )
    })?;
    validate_endpoint(&base, api_key.as_deref())?;
    let endpoint = base.join("chat/completions").map_err(|error| {
        UseError::new(
            "use.ocr.vision_config_invalid",
            format!("Failed to resolve the vision OCR endpoint: {error}"),
        )
    })?;
    Ok(Some(VisionConfig {
        endpoint,
        api_key,
        model,
    }))
}

fn validate_endpoint(endpoint: &Url, api_key: Option<&str>) -> UseResult<()> {
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        return Err(UseError::new(
            "use.ocr.vision_config_invalid",
            "The vision OCR endpoint must not contain embedded credentials.",
        ));
    }
    if endpoint.scheme() != "https" && !(endpoint.scheme() == "http" && is_loopback(endpoint)) {
        return Err(UseError::new(
            "use.ocr.vision_config_invalid",
            "The vision OCR endpoint must use HTTPS; loopback HTTP is allowed for local providers.",
        ));
    }
    if !is_loopback(endpoint) && api_key.is_none() {
        return Err(UseError::new(
            "use.ocr.vision_config_invalid",
            "A3S_OCR_VISION_API_KEY is required for a non-loopback vision endpoint.",
        ));
    }
    Ok(())
}

fn timeout_from_env() -> UseResult<Duration> {
    let Some(value) = env::var("A3S_OCR_TIMEOUT_MS").ok() else {
        return Ok(DEFAULT_TIMEOUT);
    };
    let millis = value.parse::<u64>().map_err(|_| {
        UseError::new(
            "use.ocr.timeout_invalid",
            "A3S_OCR_TIMEOUT_MS must be an integer from 1 through 300000.",
        )
    })?;
    if !(1..=300_000).contains(&millis) {
        return Err(UseError::new(
            "use.ocr.timeout_invalid",
            "A3S_OCR_TIMEOUT_MS must be an integer from 1 through 300000.",
        ));
    }
    Ok(Duration::from_millis(millis))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(executable_name(name)))
        .find(|candidate| candidate.is_file())
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn missing_provider() -> UseError {
    UseError::new(
        "use.ocr.provider_missing",
        "No OCR provider is configured or discoverable.",
    )
    .with_suggestion(
        "Install Tesseract explicitly, set A3S_OCR_TESSERACT_EXECUTABLE, or configure A3S_OCR_VISION_MODEL, A3S_OCR_VISION_BASE_URL, and A3S_OCR_VISION_API_KEY.",
    )
}

fn missing_tesseract() -> UseError {
    UseError::new(
        "use.ocr.provider_missing",
        "The Tesseract OCR provider is not installed or configured.",
    )
    .with_suggestion(
        "Install Tesseract explicitly or set A3S_OCR_TESSERACT_EXECUTABLE; A3S Use will not install it automatically.",
    )
}

fn missing_vision() -> UseError {
    UseError::new(
        "use.ocr.provider_missing",
        "The vision OCR provider is not configured.",
    )
    .with_suggestion(
        "Set A3S_OCR_VISION_MODEL and an approved HTTPS endpoint/API key before sending source images to a vision provider.",
    )
}

fn is_loopback(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
}

fn redacted_endpoint(url: &Url) -> String {
    let mut redacted = url.clone();
    redacted.set_query(None);
    redacted.set_fragment(None);
    redacted.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_insecure_remote_vision_endpoint() {
        let endpoint = Url::parse("http://ocr.example.com/v1/").unwrap();
        let error = validate_endpoint(&endpoint, Some("secret")).unwrap_err();
        assert_eq!(error.code, "use.ocr.vision_config_invalid");
    }

    #[test]
    fn permits_loopback_http_without_an_api_key() {
        let endpoint = Url::parse("http://127.0.0.1:8080/v1/").unwrap();
        validate_endpoint(&endpoint, None).unwrap();
    }
}
