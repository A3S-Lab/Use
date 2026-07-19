use std::path::PathBuf;

use a3s_use_core::{UseError, UseResult};
use clap::error::ErrorKind;
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;

#[cfg(feature = "mcp")]
use crate::mcp::DocumentMcpServer;
use crate::{
    DocumentClient, DocumentInspectRequest, DocumentOcrPolicy, DocumentParseRequest,
    DEFAULT_DOCUMENT_OCR_MAX_IMAGES,
};

#[derive(Debug)]
pub struct CommandOutput {
    pub human: String,
    pub json: serde_json::Value,
    pub exit_code: u8,
    pub should_print: bool,
}

impl CommandOutput {
    fn data<T>(value: T) -> UseResult<Self>
    where
        T: Serialize,
    {
        let data = serde_json::to_value(value).map_err(output_error)?;
        let human = serde_json::to_string_pretty(&data).map_err(output_error)?;
        Ok(Self {
            human,
            json: serde_json::json!({
                "schemaVersion": 1,
                "ok": true,
                "data": data,
            }),
            exit_code: 0,
            should_print: true,
        })
    }

    fn text(value: String) -> Self {
        Self {
            human: value.clone(),
            json: serde_json::json!({
                "schemaVersion": 1,
                "ok": true,
                "data": { "text": value },
            }),
            exit_code: 0,
            should_print: true,
        }
    }

    #[cfg(feature = "mcp")]
    fn silent() -> Self {
        Self {
            human: String::new(),
            json: serde_json::Value::Null,
            exit_code: 0,
            should_print: false,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "a3s-use-document",
    version,
    about = "Native Office and local PP-OCRv6 document parsing for A3S Use",
    arg_required_else_help = true
)]
struct Cli {
    /// Emit one versioned JSON document.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect native text, semantic units, embedded images, and OCR readiness.
    Doctor,
    /// Inspect a local document without installing models or running OCR.
    Inspect { path: PathBuf },
    /// Parse native text and selected local raster evidence.
    Parse {
        path: PathBuf,
        /// OCR selection policy.
        #[arg(long, value_enum, default_value = "auto")]
        ocr: OcrPolicyArgument,
        /// Exact semantic image path from `document inspect`; repeat as needed.
        #[arg(long = "image-path")]
        image_paths: Vec<String>,
        /// Maximum raster image occurrences to process.
        #[arg(long, default_value_t = DEFAULT_DOCUMENT_OCR_MAX_IMAGES)]
        max_images: usize,
    },
    /// Run an extension protocol surface.
    Serve {
        /// Serve standard MCP over stdin/stdout.
        #[arg(long)]
        mcp: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OcrPolicyArgument {
    Never,
    Auto,
    Always,
}

impl From<OcrPolicyArgument> for DocumentOcrPolicy {
    fn from(value: OcrPolicyArgument) -> Self {
        match value {
            OcrPolicyArgument::Never => Self::Never,
            OcrPolicyArgument::Auto => Self::Auto,
            OcrPolicyArgument::Always => Self::Always,
        }
    }
}

pub async fn run(args: Vec<String>) -> UseResult<CommandOutput> {
    let mut argv = vec!["a3s-use-document".to_string()];
    argv.extend(args);
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return Ok(CommandOutput::text(error.to_string()));
        }
        Err(error) => return Err(usage_error(error.to_string())),
    };

    if let Command::Serve { mcp } = &cli.command {
        if !mcp {
            return Err(usage_error("serve requires --mcp"));
        }
        if cli.json {
            return Err(usage_error("--json cannot be combined with serve --mcp"));
        }
        #[cfg(feature = "mcp")]
        {
            DocumentMcpServer::from_env()?.serve_stdio().await?;
            return Ok(CommandOutput::silent());
        }
        #[cfg(not(feature = "mcp"))]
        {
            return Err(UseError::new(
                "use.document.mcp_disabled",
                "Document MCP support is disabled in this custom build.",
            ));
        }
    }

    let client = DocumentClient::from_env()?;
    match cli.command {
        Command::Doctor => CommandOutput::data(client.diagnostic()),
        Command::Inspect { path } => {
            CommandOutput::data(client.inspect(DocumentInspectRequest { path }).await?)
        }
        Command::Parse {
            path,
            ocr,
            image_paths,
            max_images,
        } => CommandOutput::data(
            client
                .parse_with_first_use(DocumentParseRequest {
                    path,
                    ocr: ocr.into(),
                    image_paths,
                    max_images,
                })
                .await?,
        ),
        Command::Serve { .. } => Err(UseError::new(
            "use.document.command_invalid",
            "Document MCP command dispatch reached an invalid state.",
        )),
    }
}

fn output_error(error: serde_json::Error) -> UseError {
    UseError::new(
        "use.document.output_invalid",
        format!("Failed to encode document command output: {error}"),
    )
}

fn usage_error(message: impl Into<String>) -> UseError {
    UseError::new("use.document.usage_invalid", message)
        .with_suggestion("Run 'a3s use document --help'.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn doctor_is_versioned_and_non_installing() {
        let output = run(vec!["doctor".to_string(), "--json".to_string()])
            .await
            .unwrap();
        assert_eq!(output.json["schemaVersion"], 1);
        assert_eq!(output.json["ok"], true);
        assert_eq!(output.json["data"]["nativeOfficeReady"], true);
    }

    #[tokio::test]
    async fn serve_requires_an_explicit_protocol() {
        let error = run(vec!["serve".to_string()]).await.unwrap_err();
        assert_eq!(error.code, "use.document.usage_invalid");
    }
}
