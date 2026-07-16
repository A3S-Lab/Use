use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use a3s_use_office::{NativeOfficeDocument, NativeOfficeRenderedView};

use super::{single_line, usage_error, AllowedOptions, CommandOutput, ParsedArguments};

pub(super) async fn run(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::VIEW)?;
    if parsed.positionals.len() != 2 {
        return Err(usage_error(
            "office native view requires <file> and text, outline, stats, html, or svg",
        ));
    }
    let document = NativeOfficeDocument::open(&parsed.positionals[0]).await?;
    match parsed.positionals[1].as_str() {
        "text" | "t" => {
            reject_output(&parsed, "text")?;
            let view = document.text_view();
            Ok(CommandOutput::success(
                view.text.clone(),
                serde_json::json!({ "view": "text", "result": view }),
            ))
        }
        "outline" | "o" => {
            reject_output(&parsed, "outline")?;
            let outline = document.outline();
            let human = outline
                .iter()
                .map(|entry| {
                    format!(
                        "{}{} ({}) {}",
                        "  ".repeat(entry.level),
                        entry.path,
                        entry.node_type.label(),
                        single_line(&entry.text)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(CommandOutput::success(
                human,
                serde_json::json!({ "view": "outline", "result": outline }),
            ))
        }
        "stats" | "s" => {
            reject_output(&parsed, "stats")?;
            let statistics = document.statistics();
            let human = format!(
                "{} nodes, {} words, {} characters",
                statistics.node_count, statistics.word_count, statistics.character_count
            );
            Ok(CommandOutput::success(
                human,
                serde_json::json!({ "view": "stats", "result": statistics }),
            ))
        }
        "html" | "h" => rendered(document.html_view()?, parsed.output.as_deref()).await,
        "svg" => rendered(document.svg_view()?, parsed.output.as_deref()).await,
        mode => Err(usage_error(format!(
            "native Office view mode '{mode}' is not text, outline, stats, html, or svg"
        ))),
    }
}

fn reject_output(parsed: &ParsedArguments, view: &str) -> UseResult<()> {
    if parsed.output.is_none() {
        return Ok(());
    }
    Err(usage_error(format!(
        "--output is available for html and svg views, not {view}"
    )))
}

async fn rendered(
    view: NativeOfficeRenderedView,
    output: Option<&str>,
) -> UseResult<CommandOutput> {
    if let Some(path) = output.filter(|path| *path != "-") {
        write_new_output(Path::new(path), view.content.as_bytes()).await?;
        return Ok(CommandOutput::success(
            format!(
                "Wrote native Office {:?} semantic preview to '{}'.",
                view.format, path
            ),
            serde_json::json!({
                "view": view.format,
                "result": {
                    "kind": view.kind,
                    "format": view.format,
                    "mediaType": view.media_type,
                    "unitCount": view.unit_count,
                    "byteLength": view.byte_length,
                    "sha256": view.sha256,
                    "outputPath": path
                }
            }),
        ));
    }
    Ok(CommandOutput::success(
        view.content.clone(),
        serde_json::json!({ "view": view.format, "result": view }),
    ))
}

async fn write_new_output(path: &Path, bytes: &[u8]) -> UseResult<()> {
    let path = path.to_path_buf();
    let task_path = path.clone();
    let bytes = bytes.to_vec();
    tokio::task::spawn_blocking(move || write_new_output_blocking(&task_path, &bytes))
        .await
        .map_err(|error| {
            render_output_error(
                "use.office.render_output_failed",
                &path,
                format!("Native Office render output task failed: {error}"),
            )
        })?
}

fn write_new_output_blocking(path: &Path, bytes: &[u8]) -> UseResult<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        render_output_error(
            "use.office.render_output_failed",
            path,
            format!("Failed to stage native Office render output: {error}"),
        )
    })?;
    temporary.write_all(bytes).map_err(|error| {
        render_output_error(
            "use.office.render_output_failed",
            path,
            format!("Failed to write staged native Office render output: {error}"),
        )
    })?;
    temporary.as_file().sync_all().map_err(|error| {
        render_output_error(
            "use.office.render_output_failed",
            path,
            format!("Failed to sync staged native Office render output: {error}"),
        )
    })?;
    temporary.persist_noclobber(path).map_err(|error| {
        if error.error.kind() == ErrorKind::AlreadyExists {
            render_output_error(
                "use.office.render_output_exists",
                path,
                format!(
                    "Native Office render output '{}' already exists; refusing to overwrite it.",
                    path.display()
                ),
            )
            .with_suggestion("Choose a new HTML or SVG output path.")
        } else {
            render_output_error(
                "use.office.render_output_failed",
                path,
                format!(
                    "Failed to publish native Office render output: {}",
                    error.error
                ),
            )
        }
    })?;
    Ok(())
}

fn render_output_error(code: &'static str, path: &Path, message: impl Into<String>) -> UseError {
    UseError::new(code, message).with_detail("output", display_path(path))
}

fn display_path(path: &Path) -> String {
    let path = PathBuf::from(path);
    path.display().to_string()
}
