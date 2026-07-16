use a3s_use_core::UseResult;
use a3s_use_office::{NativeCreatedImage, NativeOfficeEditor, NativeOfficeImage};

use super::arguments::{AllowedOptions, ParsedArguments};
use super::{read_bounded_input, save_editor, usage_error, NativeInputKind, MAX_IMAGE_INPUT_BYTES};
use crate::cli::CommandOutput;

pub(super) async fn run(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::ADD)?;
    if parsed.positionals.len() != 2 {
        return Err(usage_error(
            "office native add requires <file> and <parent>",
        ));
    }
    let node_type = parsed
        .node_type
        .as_deref()
        .ok_or_else(|| usage_error("office native add requires --type <node-type>"))?;
    let source = &parsed.positionals[0];
    let parent = &parsed.positionals[1];
    validate_options(node_type, &parsed)?;
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    let text = parsed.text.as_deref().unwrap_or_default();
    let (operation, path, created_image): (&str, String, Option<NativeCreatedImage>) =
        match node_type {
            "paragraph" | "p" => ("add-paragraph", editor.add_paragraph(parent, text)?, None),
            "table" | "tbl" => (
                "add-table",
                editor.add_table(
                    parent,
                    parsed.rows.unwrap_or(1),
                    parsed.columns.unwrap_or(1),
                )?,
                None,
            ),
            "row" | "tr" => (
                "add-table-row",
                editor.add_table_row(parent, parsed.columns)?,
                None,
            ),
            "column" | "col" => (
                "add-table-column",
                editor.add_table_column(parent, parsed.index, text)?,
                None,
            ),
            "cell" | "tc" => ("add-table-cell", editor.add_table_cell(parent, text)?, None),
            "sheet" | "worksheet" => {
                if parent != "/" {
                    return Err(usage_error("native worksheets can be added only to /"));
                }
                let name = parsed
                    .name
                    .as_deref()
                    .ok_or_else(|| usage_error("native worksheet add requires --name <name>"))?;
                ("add-worksheet", editor.add_worksheet(name)?, None)
            }
            "slide" => ("add-slide", editor.add_slide(parent, text)?, None),
            "shape" => ("add-shape", editor.add_shape(parent, text)?, None),
            "picture" | "image" | "img" => {
                let input = parsed.input.as_deref().ok_or_else(|| {
                    usage_error("native picture add requires --input <png|jpeg|gif>")
                })?;
                let bytes =
                    read_bounded_input(input, MAX_IMAGE_INPUT_BYTES, NativeInputKind::Image)
                        .await?;
                let mut image = NativeOfficeImage::from_bytes(bytes)?;
                if let Some(name) = &parsed.name {
                    image = image.with_name(name);
                }
                if let Some(alt) = &parsed.alt {
                    image = image.with_alt_text(alt);
                }
                if let Some(width) = parsed.width {
                    image = image.with_width_px(width);
                }
                if let Some(height) = parsed.height {
                    image = image.with_height_px(height);
                }
                let created = editor.add_image(parent, image)?;
                let path = created.path.clone();
                ("add-image", path, Some(created))
            }
            _ => {
                return Err(usage_error(format!(
                    "native Office add type '{node_type}' is not supported yet"
                )))
            }
        };
    let node = editor.snapshot()?.get(&path, 2)?;
    save_editor(&mut editor, parsed.output.as_deref()).await?;
    let output_path = editor.package().path().to_path_buf();
    let in_place = output_path == source_path;

    Ok(CommandOutput::success(
        format!("Added {path} and saved '{}'.", output_path.display()),
        serde_json::json!({
            "operation": operation,
            "changed": true,
            "parent": parent,
            "path": path,
            "node": node,
            "createdImage": created_image,
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "inPlace": in_place,
            "revision": editor.package().source_revision()
        }),
    ))
}

fn validate_options(node_type: &str, parsed: &ParsedArguments) -> UseResult<()> {
    let is_picture = matches!(node_type, "picture" | "image" | "img");
    let accepts_rows = matches!(node_type, "table" | "tbl");
    let accepts_columns = matches!(node_type, "table" | "tbl" | "row" | "tr");
    let accepts_index = matches!(node_type, "column" | "col");
    let accepts_name = is_picture || matches!(node_type, "sheet" | "worksheet");
    let accepts_text = matches!(
        node_type,
        "paragraph" | "p" | "column" | "col" | "cell" | "tc" | "slide" | "shape"
    );
    if parsed.rows.is_some() && !accepts_rows {
        return Err(usage_error(format!(
            "native Office add type '{node_type}' does not accept --rows"
        )));
    }
    if parsed.columns.is_some() && !accepts_columns {
        return Err(usage_error(format!(
            "native Office add type '{node_type}' does not accept --columns"
        )));
    }
    if parsed.index.is_some() && !accepts_index {
        return Err(usage_error(format!(
            "native Office add type '{node_type}' does not accept --index"
        )));
    }
    if parsed.name.is_some() && !accepts_name {
        return Err(usage_error(format!(
            "native Office add type '{node_type}' does not accept --name"
        )));
    }
    if parsed.text.is_some() && !accepts_text {
        return Err(usage_error(format!(
            "native Office add type '{node_type}' does not accept --text"
        )));
    }
    if parsed.input.is_some() && !is_picture {
        return Err(usage_error(format!(
            "native Office add type '{node_type}' does not accept --input"
        )));
    }
    if parsed.alt.is_some() && !is_picture {
        return Err(usage_error(format!(
            "native Office add type '{node_type}' does not accept --alt"
        )));
    }
    if parsed.width.is_some() && !is_picture {
        return Err(usage_error(format!(
            "native Office add type '{node_type}' does not accept --width"
        )));
    }
    if parsed.height.is_some() && !is_picture {
        return Err(usage_error(format!(
            "native Office add type '{node_type}' does not accept --height"
        )));
    }
    Ok(())
}
