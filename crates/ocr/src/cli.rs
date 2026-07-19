use std::path::PathBuf;

use a3s_use_core::{UseError, UseResult};
use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{OcrClient, OcrMcpServer, OcrRequest};

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
    name = "a3s-use-ocr",
    version,
    about = "Typed built-in OCR for A3S Use",
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
    /// Inspect local PP-OCRv6 readiness without reading an image.
    Doctor,
    /// Extract text and layout evidence from one local image.
    Extract { path: PathBuf },
    /// Run an extension protocol surface.
    Serve {
        /// Serve standard MCP over stdin/stdout.
        #[arg(long)]
        mcp: bool,
    },
}

pub async fn run(args: Vec<String>) -> UseResult<CommandOutput> {
    let mut argv = vec!["a3s-use-ocr".to_string()];
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
        OcrMcpServer::from_env()?.serve_stdio().await?;
        return Ok(CommandOutput::silent());
    }

    let client = OcrClient::from_env()?;
    match cli.command {
        Command::Doctor => CommandOutput::data(client.diagnostic()),
        Command::Extract { path } => {
            CommandOutput::data(client.extract_with_first_use(OcrRequest { path }).await?)
        }
        Command::Serve { .. } => Err(UseError::new(
            "use.ocr.command_invalid",
            "OCR MCP command dispatch reached an invalid state.",
        )),
    }
}

fn output_error(error: serde_json::Error) -> UseError {
    UseError::new(
        "use.ocr.output_invalid",
        format!("Failed to encode OCR command output: {error}"),
    )
}

fn usage_error(message: impl Into<String>) -> UseError {
    UseError::new("use.ocr.usage_invalid", message).with_suggestion("Run 'a3s use ocr --help'.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn doctor_is_versioned_even_when_no_provider_is_ready() {
        let output = run(vec!["doctor".to_string(), "--json".to_string()])
            .await
            .unwrap();
        assert_eq!(output.json["schemaVersion"], 1);
        assert_eq!(output.json["ok"], true);
        assert!(output.json["data"]["readiness"].is_string());
    }

    #[tokio::test]
    async fn serve_requires_an_explicit_protocol() {
        let error = run(vec!["serve".to_string()]).await.unwrap_err();
        assert_eq!(error.code, "use.ocr.usage_invalid");
    }
}
