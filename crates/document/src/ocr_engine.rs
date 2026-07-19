use std::path::Path;

use a3s_use_core::UseResult;
use a3s_use_ocr::{OcrClient, OcrDiagnostic, OcrRequest, OcrResult};
use async_trait::async_trait;

/// Injectable OCR boundary used by the document parser.
///
/// Production calls are backed only by the local PP-OCRv6 engine. The separate
/// methods make the first-use installation boundary explicit: MCP parsing calls
/// [`Self::extract`], while the direct CLI calls [`Self::extract_with_first_use`].
#[async_trait]
pub trait DocumentOcrEngine: Send + Sync {
    fn diagnostic(&self) -> OcrDiagnostic;

    async fn extract(&self, path: &Path) -> UseResult<OcrResult>;

    async fn extract_with_first_use(&self, path: &Path) -> UseResult<OcrResult>;
}

#[derive(Clone)]
pub(crate) struct PpOcrV6DocumentOcr {
    client: OcrClient,
}

impl PpOcrV6DocumentOcr {
    pub(crate) fn from_env() -> UseResult<Self> {
        Ok(Self {
            client: OcrClient::from_env()?,
        })
    }
}

#[async_trait]
impl DocumentOcrEngine for PpOcrV6DocumentOcr {
    fn diagnostic(&self) -> OcrDiagnostic {
        self.client.diagnostic()
    }

    async fn extract(&self, path: &Path) -> UseResult<OcrResult> {
        self.client
            .extract(OcrRequest {
                path: path.to_path_buf(),
            })
            .await
    }

    async fn extract_with_first_use(&self, path: &Path) -> UseResult<OcrResult> {
        self.client
            .extract_with_first_use(OcrRequest {
                path: path.to_path_buf(),
            })
            .await
    }
}
