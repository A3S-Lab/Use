use a3s_use_core::{UseError, UseResult};
use a3s_use_office::{
    DocumentNode, NativeOfficeDocument, NativeOfficeEditor, NativeOfficeHorizontalAlignment,
    NativeOfficeMutation, NativeOfficeRgbColor, NativeOfficeTextFormat, SpreadsheetCellValue,
};
use tokio::io::AsyncReadExt;

use crate::cli::CommandOutput;

mod add;
mod arguments;
mod arrange;
mod merge;
mod part;
mod raw;
mod replay;
mod view;
mod watch;

use arguments::{parse_boolean_option, AllowedOptions, ParsedArguments};

const MAX_BATCH_INPUT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_IMAGE_INPUT_BYTES: u64 = 64 * 1024 * 1024;

const HELP: &str = concat!(
    "a3s-use office native — dependency-free OOXML operations\n\n",
    "usage:\n",
    "  a3s-use office native get <file> [path] [--depth <n>] [--json]\n",
    "  a3s-use office native query <file> <selector> [--json]\n",
    "  a3s-use office native view <file> text|annotated|outline|stats|issues|html|svg|screenshot [--type <filter>] [--limit <n>] [--output <file>] [--timeout-ms <ms>] [--json]\n",
    "  a3s-use office native watch <file> [--port <0-65535>] [--poll-ms <50-10000>] [--timeout-ms <ms>] [--json]\n",
    "  a3s-use office native raw <file> <part> [--output <xml-file>] [--json]\n",
    "  a3s-use office native raw-set <file> <part> --input <xml-file> [--output <file>] [--json]\n",
    "  a3s-use office native dump <file> [path] [--output <batch.json>] [--json]\n",
    "  a3s-use office native merge <template> <output> --data <json|@file.json> [--force] [--json]\n",
    "  a3s-use office native validate <file> [--json]\n",
    "  a3s-use office native create <file.docx|file.xlsx|file.pptx> [--json]\n",
    "  a3s-use office native add <file> <parent> --type paragraph|table|row|cell|sheet|slide|shape|picture [--input <image>] [--name <name>] [--alt <text>] [--width <pixels>] [--height <pixels>] [--rows <n>] [--columns <n>] [--text <value>] [--output <file>] [--json]\n",
    "  a3s-use office native add-part <file> <parent> --type chart|header|footer [--output <file>] [--json]\n",
    "  a3s-use office native set <file> <path> [--text <value>|--number <value>|--boolean <true|false>|--formula <expression>|--width-emu <n>] [--bold <true|false>] [--italic <true|false>] [--font-family <name>] [--font-size <points>] [--text-color <RRGGBB>] [--align <left|center|right|justify>] [--output <file>] [--json]\n",
    "  a3s-use office native remove <file> <path> [--output <file>] [--json]\n",
    "  a3s-use office native move <file> <path> [--to <parent>] [--index <zero-based>|--before <path>|--after <path>] [--output <file>] [--json]\n",
    "  a3s-use office native copy <file> <path> [--to <parent>] [--name <worksheet-name>] [--index <zero-based>|--before <path>|--after <path>] [--output <file>] [--json]\n",
    "  a3s-use office native swap <file> <path> <with> [--output <file>] [--json]\n",
    "  a3s-use office native insert-rows|delete-rows <file> <sheet> <start> [--count <n>] [--output <file>] [--json]\n",
    "  a3s-use office native insert-columns|delete-columns <file> <sheet> <start> [--count <n>] [--output <file>] [--json]\n",
    "  a3s-use office native rename-sheet <file> <sheet> <new-name> [--output <file>] [--json]\n",
    "  a3s-use office native move-sheet <file> <sheet> <one-based-position> [--output <file>] [--json]\n",
    "  a3s-use office native copy-sheet <file> <sheet> <new-name> [--position <one-based-position>] [--output <file>] [--json]\n",
    "  a3s-use office native batch <file> --input <batch.json> [--output <file>] [--json]"
);

pub async fn run(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => Ok(help()),
        Some("get") => get(args).await,
        Some("query") => query(args).await,
        Some("view") => view::run(args).await,
        Some("watch") => watch::run(args).await,
        Some("raw") => raw::inspect(args).await,
        Some("raw-set") => raw::replace(args).await,
        Some("dump") => replay::dump(args).await,
        Some("merge") => merge::run(args).await,
        Some("validate") => validate(args).await,
        Some("create") => create(args).await,
        Some("add") => add::run(args).await,
        Some("add-part") => part::add(args).await,
        Some("set") => set(args).await,
        Some("remove") => remove(args).await,
        Some("move") => arrange::move_node(args).await,
        Some("copy") => arrange::copy_node(args).await,
        Some("swap") => arrange::swap_nodes(args).await,
        Some("insert-rows") => edit_structure(args, StructureOperation::InsertRows).await,
        Some("delete-rows") => edit_structure(args, StructureOperation::DeleteRows).await,
        Some("insert-columns") => edit_structure(args, StructureOperation::InsertColumns).await,
        Some("delete-columns") => edit_structure(args, StructureOperation::DeleteColumns).await,
        Some("rename-sheet") => rename_sheet(args).await,
        Some("move-sheet") => move_sheet(args).await,
        Some("copy-sheet") => copy_sheet(args).await,
        Some("batch") => batch(args).await,
        Some(command) => Err(usage_error(format!(
            "unknown native Office command '{command}'"
        ))),
    }
}

fn help() -> CommandOutput {
    CommandOutput::success(
        HELP,
        serde_json::json!({
            "commands": [
                "get", "query", "view", "watch", "raw", "raw-set", "dump", "merge", "validate", "create", "add", "add-part", "set", "remove", "move", "copy", "swap",
                "insert-rows", "delete-rows", "insert-columns", "delete-columns",
                "rename-sheet", "move-sheet", "copy-sheet", "batch"
            ],
            "formats": ["docx", "xlsx", "pptx"],
            "runtimeDependencies": [],
            "atomicBatch": true
        }),
    )
}

async fn get(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::GET)?;
    if !(1..=2).contains(&parsed.positionals.len()) {
        return Err(usage_error(
            "office native get requires <file> and an optional [path]",
        ));
    }
    let depth = parsed.depth.unwrap_or(1);
    if depth > 64 {
        return Err(usage_error("--depth cannot exceed 64"));
    }
    let document = NativeOfficeDocument::open(&parsed.positionals[0]).await?;
    let path = parsed.positionals.get(1).map_or("/", String::as_str);
    let node = document.get(path, depth)?;
    let human = format_node(&node, 0);
    Ok(CommandOutput::success(
        human,
        serde_json::json!({ "node": node }),
    ))
}

async fn query(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::NONE)?;
    if parsed.positionals.len() != 2 {
        return Err(usage_error(
            "office native query requires <file> and <selector>",
        ));
    }
    let document = NativeOfficeDocument::open(&parsed.positionals[0]).await?;
    let results = document.query(&parsed.positionals[1])?;
    let mut human = format!("Matches: {}", results.len());
    for node in &results {
        human.push_str(&format!("\n  {}: {}", node.path, single_line(&node.text)));
    }
    Ok(CommandOutput::success(
        human,
        serde_json::json!({
            "matches": results.len(),
            "results": results
        }),
    ))
}

async fn validate(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::NONE)?;
    if parsed.positionals.len() != 1 {
        return Err(usage_error("office native validate requires <file>"));
    }
    let document = NativeOfficeDocument::open(&parsed.positionals[0]).await?;
    Ok(CommandOutput::success(
        format!(
            "Valid native {:?} document: {}",
            document.kind(),
            parsed.positionals[0]
        ),
        serde_json::json!({
            "valid": true,
            "kind": document.kind(),
            "path": parsed.positionals[0]
        }),
    ))
}

async fn create(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::NONE)?;
    if parsed.positionals.len() != 1 {
        return Err(usage_error("office native create requires <file>"));
    }
    let editor = NativeOfficeEditor::create(&parsed.positionals[0]).await?;
    let output_path = editor.package().path().to_path_buf();
    Ok(CommandOutput::success(
        format!(
            "Created blank native Office document '{}'.",
            output_path.display()
        ),
        serde_json::json!({
            "operation": "create",
            "created": true,
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "revision": editor.package().source_revision()
        }),
    ))
}

async fn set(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::SET)?;
    if parsed.positionals.len() != 2 {
        return Err(usage_error("office native set requires <file> and <path>"));
    }
    let value_count = [
        parsed.text.is_some(),
        parsed.number.is_some(),
        parsed.boolean.is_some(),
        parsed.formula.is_some(),
        parsed.width_emu.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    let format = parse_text_format(&parsed)?;
    if value_count > 1 {
        return Err(usage_error(
            "office native set accepts at most one of --text, --number, --boolean, --formula, or --width-emu",
        ));
    }
    if value_count == 0 && format.is_none() {
        return Err(usage_error(
            "office native set requires content, width, or at least one typed formatting option",
        ));
    }
    if parsed.width_emu.is_some() && format.is_some() {
        return Err(usage_error(
            "--width-emu cannot be combined with text formatting options",
        ));
    }
    let typed_value = if let Some(value) = parsed.number.as_ref() {
        Some(SpreadsheetCellValue::Number {
            value: value.clone(),
        })
    } else if let Some(value) = parsed.boolean.as_deref() {
        Some(SpreadsheetCellValue::Boolean {
            value: parse_boolean_option(value)?,
        })
    } else {
        parsed
            .formula
            .as_ref()
            .map(|expression| SpreadsheetCellValue::Formula {
                expression: expression.clone(),
            })
    };
    let source = &parsed.positionals[0];
    let path = &parsed.positionals[1];
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    let operation = if let Some(width_emu) = parsed.width_emu {
        editor.set_table_column_width(path, width_emu)?;
        "set-table-column-width"
    } else {
        let mut mutations = Vec::with_capacity(2);
        if let Some(value) = typed_value {
            mutations.push(NativeOfficeMutation::SetCellValue {
                path: path.clone(),
                value,
            });
        } else if let Some(text) = &parsed.text {
            mutations.push(NativeOfficeMutation::SetText {
                path: path.clone(),
                text: text.clone(),
            });
        }
        if let Some(format) = format {
            mutations.push(NativeOfficeMutation::SetTextFormat {
                path: path.clone(),
                format,
            });
        }
        editor.apply_batch(&mutations)?;
        match (
            mutations
                .iter()
                .any(|mutation| matches!(mutation, NativeOfficeMutation::SetTextFormat { .. })),
            mutations.len(),
            mutations.first(),
        ) {
            (true, 1, _) => "set-text-format",
            (true, _, _) => "set-content-and-text-format",
            (false, _, Some(NativeOfficeMutation::SetCellValue { .. })) => "set-cell-value",
            _ => "set-text",
        }
    };
    let node = editor.snapshot()?.get(path, 0)?;
    save_editor(&mut editor, parsed.output.as_deref()).await?;
    let output_path = editor.package().path().to_path_buf();
    let in_place = output_path == source_path;

    Ok(CommandOutput::success(
        format!("Updated {path} and saved '{}'.", output_path.display()),
        serde_json::json!({
            "operation": operation,
            "changed": true,
            "path": path,
            "node": node,
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "inPlace": in_place,
            "revision": editor.package().source_revision()
        }),
    ))
}

fn parse_text_format(parsed: &ParsedArguments) -> UseResult<Option<NativeOfficeTextFormat>> {
    let format = NativeOfficeTextFormat {
        bold: parsed
            .bold
            .as_deref()
            .map(|value| parse_format_boolean("--bold", value))
            .transpose()?,
        italic: parsed
            .italic
            .as_deref()
            .map(|value| parse_format_boolean("--italic", value))
            .transpose()?,
        font_family: parsed.font_family.clone(),
        font_size_centipoints: parsed
            .font_size
            .as_deref()
            .map(parse_font_size)
            .transpose()?,
        text_color: parsed
            .text_color
            .as_deref()
            .map(parse_text_color)
            .transpose()?,
        alignment: parsed
            .alignment
            .as_deref()
            .map(parse_alignment)
            .transpose()?,
    };
    Ok((!format.is_empty()).then_some(format))
}

fn parse_format_boolean(option: &str, value: &str) -> UseResult<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(usage_error(format!(
            "{option} requires true or false, received '{value}'"
        ))),
    }
}

fn parse_font_size(value: &str) -> UseResult<u32> {
    let normalized = value
        .strip_suffix("pt")
        .or_else(|| value.strip_suffix("pT"))
        .or_else(|| value.strip_suffix("Pt"))
        .or_else(|| value.strip_suffix("PT"))
        .unwrap_or(value);
    let (whole, fraction) = normalized
        .split_once('.')
        .map_or((normalized, ""), |(whole, fraction)| (whole, fraction));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 2
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(usage_error(format!(
            "--font-size requires points with at most two decimals, received '{value}'"
        )));
    }
    let whole = whole.parse::<u32>().map_err(|_| {
        usage_error(format!(
            "--font-size requires points with at most two decimals, received '{value}'"
        ))
    })?;
    let fraction = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u32>().unwrap_or_default() * 10,
        _ => fraction.parse::<u32>().unwrap_or_default(),
    };
    let centipoints = whole
        .checked_mul(100)
        .and_then(|value| value.checked_add(fraction))
        .ok_or_else(|| usage_error("--font-size is too large"))?;
    if !(100..=40_000).contains(&centipoints) {
        return Err(usage_error("--font-size must be from 1 through 400 points"));
    }
    Ok(centipoints)
}

fn parse_text_color(value: &str) -> UseResult<NativeOfficeRgbColor> {
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(usage_error(format!(
            "--text-color requires exactly six hexadecimal RGB digits, received '{value}'"
        )));
    }
    let component = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&value[range], 16).map_err(|_| {
            usage_error(format!(
                "--text-color requires exactly six hexadecimal RGB digits, received '{value}'"
            ))
        })
    };
    Ok(NativeOfficeRgbColor::new(
        component(0..2)?,
        component(2..4)?,
        component(4..6)?,
    ))
}

fn parse_alignment(value: &str) -> UseResult<NativeOfficeHorizontalAlignment> {
    match value.to_ascii_lowercase().as_str() {
        "left" => Ok(NativeOfficeHorizontalAlignment::Left),
        "center" | "centre" => Ok(NativeOfficeHorizontalAlignment::Center),
        "right" => Ok(NativeOfficeHorizontalAlignment::Right),
        "justify" | "justified" => Ok(NativeOfficeHorizontalAlignment::Justify),
        _ => Err(usage_error(format!(
            "--align requires left, center, right, or justify, received '{value}'"
        ))),
    }
}

async fn remove(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::MUTATE)?;
    if parsed.positionals.len() != 2 {
        return Err(usage_error(
            "office native remove requires <file> and <path>",
        ));
    }
    let source = &parsed.positionals[0];
    let path = &parsed.positionals[1];
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    editor.remove(path)?;
    save_editor(&mut editor, parsed.output.as_deref()).await?;
    let output_path = editor.package().path().to_path_buf();
    let in_place = output_path == source_path;

    Ok(CommandOutput::success(
        format!("Removed {path} and saved '{}'.", output_path.display()),
        serde_json::json!({
            "operation": "remove",
            "changed": true,
            "path": path,
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "inPlace": in_place,
            "revision": editor.package().source_revision()
        }),
    ))
}

#[derive(Debug, Clone, Copy)]
enum StructureOperation {
    InsertRows,
    DeleteRows,
    InsertColumns,
    DeleteColumns,
}

impl StructureOperation {
    fn name(self) -> &'static str {
        match self {
            Self::InsertRows => "insert-rows",
            Self::DeleteRows => "delete-rows",
            Self::InsertColumns => "insert-columns",
            Self::DeleteColumns => "delete-columns",
        }
    }
}

async fn edit_structure(
    args: &[String],
    operation: StructureOperation,
) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::STRUCTURE)?;
    if parsed.positionals.len() != 3 {
        return Err(usage_error(format!(
            "office native {} requires <file>, <sheet>, and <start>",
            operation.name()
        )));
    }
    let source = &parsed.positionals[0];
    let sheet = &parsed.positionals[1];
    let start = &parsed.positionals[2];
    let count = parsed.count.unwrap_or(1);
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    let path = match operation {
        StructureOperation::InsertRows => {
            editor.insert_rows(sheet, parse_row_start(start)?, count)?
        }
        StructureOperation::DeleteRows => {
            editor.delete_rows(sheet, parse_row_start(start)?, count)?
        }
        StructureOperation::InsertColumns => editor.insert_columns(sheet, start, count)?,
        StructureOperation::DeleteColumns => editor.delete_columns(sheet, start, count)?,
    };
    save_editor(&mut editor, parsed.output.as_deref()).await?;
    let output_path = editor.package().path().to_path_buf();
    let in_place = output_path == source_path;
    Ok(CommandOutput::success(
        format!(
            "Applied {} at {path} and saved '{}'.",
            operation.name(),
            output_path.display()
        ),
        serde_json::json!({
            "operation": operation.name(),
            "changed": true,
            "sheet": sheet,
            "start": start,
            "count": count,
            "path": path,
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "inPlace": in_place,
            "revision": editor.package().source_revision()
        }),
    ))
}

async fn rename_sheet(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::MUTATE)?;
    if parsed.positionals.len() != 3 {
        return Err(usage_error(
            "office native rename-sheet requires <file>, <sheet>, and <new-name>",
        ));
    }
    let source = &parsed.positionals[0];
    let sheet = &parsed.positionals[1];
    let name = &parsed.positionals[2];
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    let path = editor.rename_worksheet(sheet, name)?;
    save_editor(&mut editor, parsed.output.as_deref()).await?;
    let output_path = editor.package().path().to_path_buf();
    let in_place = output_path == source_path;
    Ok(CommandOutput::success(
        format!(
            "Renamed {sheet} to {path} and saved '{}'.",
            output_path.display()
        ),
        serde_json::json!({
            "operation": "rename-sheet",
            "changed": sheet != &path,
            "from": sheet,
            "path": path,
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "inPlace": in_place,
            "revision": editor.package().source_revision()
        }),
    ))
}

async fn move_sheet(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::MUTATE)?;
    if parsed.positionals.len() != 3 {
        return Err(usage_error(
            "office native move-sheet requires <file>, <sheet>, and <one-based-position>",
        ));
    }
    let source = &parsed.positionals[0];
    let sheet = &parsed.positionals[1];
    let position = parsed.positionals[2].parse::<usize>().map_err(|_| {
        usage_error(format!(
            "move-sheet position must be a positive integer, received '{}'",
            parsed.positionals[2]
        ))
    })?;
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    let path = editor.move_worksheet(sheet, position)?;
    save_editor(&mut editor, parsed.output.as_deref()).await?;
    let output_path = editor.package().path().to_path_buf();
    let in_place = output_path == source_path;
    Ok(CommandOutput::success(
        format!(
            "Moved {sheet} to position {position} and saved '{}'.",
            output_path.display()
        ),
        serde_json::json!({
            "operation": "move-sheet",
            "changed": true,
            "path": path,
            "position": position,
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "inPlace": in_place,
            "revision": editor.package().source_revision()
        }),
    ))
}

async fn copy_sheet(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::COPY)?;
    if parsed.positionals.len() != 3 {
        return Err(usage_error(
            "office native copy-sheet requires <file>, <sheet>, and <new-name>",
        ));
    }
    if parsed.position == Some(0) {
        return Err(usage_error("copy-sheet --position must be at least 1"));
    }
    let source = &parsed.positionals[0];
    let sheet = &parsed.positionals[1];
    let name = &parsed.positionals[2];
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    let path = editor.copy_worksheet(sheet, name, parsed.position)?;
    save_editor(&mut editor, parsed.output.as_deref()).await?;
    let output_path = editor.package().path().to_path_buf();
    let in_place = output_path == source_path;
    Ok(CommandOutput::success(
        format!(
            "Copied {sheet} to {path} and saved '{}'.",
            output_path.display()
        ),
        serde_json::json!({
            "operation": "copy-sheet",
            "changed": true,
            "from": sheet,
            "path": path,
            "position": parsed.position,
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "inPlace": in_place,
            "revision": editor.package().source_revision()
        }),
    ))
}

fn parse_row_start(value: &str) -> UseResult<u32> {
    value.parse::<u32>().map_err(|_| {
        usage_error(format!(
            "Spreadsheet row start must be a positive integer, received '{value}'"
        ))
    })
}

async fn batch(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::BATCH)?;
    if parsed.positionals.len() != 1 {
        return Err(usage_error("office native batch requires <file>"));
    }
    let input_path = parsed
        .input
        .as_deref()
        .ok_or_else(|| usage_error("office native batch requires --input <batch.json>"))?;
    let input = replay::read_batch_input(input_path).await?;
    let mutation_count = input.mutation_count();
    let replayed = matches!(&input, replay::NativeBatchInput::Replay(_));
    let source = &parsed.positionals[0];
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    let result = match &input {
        replay::NativeBatchInput::Mutations(mutations) => editor.apply_batch(mutations)?,
        replay::NativeBatchInput::Replay(artifact) => editor.apply_replay(artifact)?,
    };
    save_editor(&mut editor, parsed.output.as_deref()).await?;
    let output_path = editor.package().path().to_path_buf();
    let in_place = output_path == source_path;

    Ok(CommandOutput::success(
        format!(
            "Applied {} native Office mutation(s) and saved '{}'.",
            result.applied,
            output_path.display()
        ),
        serde_json::json!({
            "operation": "batch",
            "changed": result.applied > 0,
            "atomic": true,
            "replay": replayed,
            "inputMutations": mutation_count,
            "result": result,
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "inPlace": in_place,
            "revision": editor.package().source_revision()
        }),
    ))
}

async fn save_editor(editor: &mut NativeOfficeEditor, output: Option<&str>) -> UseResult<()> {
    if let Some(output) = output {
        editor.save_as(output).await
    } else {
        editor.save().await
    }
}

#[derive(Debug, Clone, Copy)]
enum NativeInputKind {
    Batch,
    Image,
    RawXml,
    TemplateData,
}

impl NativeInputKind {
    fn code_prefix(self) -> &'static str {
        match self {
            Self::Batch => "use.office.batch_input",
            Self::Image => "use.office.image_input",
            Self::RawXml => "use.office.raw_input",
            Self::TemplateData => "use.office.template_data_input",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Batch => "Native Office batch input",
            Self::Image => "Native Office image input",
            Self::RawXml => "Native Office raw XML input",
            Self::TemplateData => "Native Office template data input",
        }
    }
}

async fn read_bounded_input(path: &str, limit: u64, kind: NativeInputKind) -> UseResult<Vec<u8>> {
    let path_metadata = tokio::fs::symlink_metadata(path).await.map_err(|error| {
        input_error(
            kind,
            "open_failed",
            path,
            format!("Failed to inspect {} '{path}': {error}", kind.label()),
        )
    })?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(input_error(
            kind,
            "invalid",
            path,
            format!("{} must be a regular, non-symlink file.", kind.label()),
        ));
    }
    if path_metadata.len() > limit {
        return Err(input_too_large(kind, path, limit));
    }

    let file = tokio::fs::File::open(path).await.map_err(|error| {
        input_error(
            kind,
            "open_failed",
            path,
            format!("Failed to open {} '{path}': {error}", kind.label()),
        )
    })?;
    let metadata = file.metadata().await.map_err(|error| {
        input_error(
            kind,
            "open_failed",
            path,
            format!("Failed to inspect {} '{path}': {error}", kind.label()),
        )
    })?;
    if !metadata.is_file() {
        return Err(input_error(
            kind,
            "invalid",
            path,
            format!("{} changed and is no longer a regular file.", kind.label()),
        ));
    }
    if metadata.len() > limit {
        return Err(input_too_large(kind, path, limit));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut reader = file.take(limit + 1);
    reader.read_to_end(&mut bytes).await.map_err(|error| {
        input_error(
            kind,
            "read_failed",
            path,
            format!("Failed to read {} '{path}': {error}", kind.label()),
        )
    })?;
    if bytes.len() as u64 > limit {
        return Err(input_too_large(kind, path, limit));
    }
    Ok(bytes)
}

fn input_too_large(kind: NativeInputKind, path: &str, limit: u64) -> UseError {
    input_error(
        kind,
        "too_large",
        path,
        format!("{} exceeds the {limit}-byte limit.", kind.label()),
    )
}

fn input_error(
    kind: NativeInputKind,
    suffix: &str,
    path: &str,
    message: impl Into<String>,
) -> UseError {
    UseError::new(format!("{}_{suffix}", kind.code_prefix()), message).with_detail("input", path)
}

fn batch_input_error(code: &str, path: &str, message: impl Into<String>) -> UseError {
    UseError::new(code, message).with_detail("input", path)
}

fn format_node(node: &DocumentNode, level: usize) -> String {
    let mut output = format!(
        "{}{} ({}) \"{}\" children={}",
        "  ".repeat(level),
        node.path,
        node.node_type.label(),
        single_line(&node.text),
        node.child_count
    );
    for (key, value) in &node.format {
        output.push_str(&format!(" {key}={}", single_line(value)));
    }
    for child in &node.children {
        output.push('\n');
        output.push_str(&format_node(child, level + 1));
    }
    output
}

fn single_line(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

fn usage_error(message: impl Into<String>) -> UseError {
    UseError::new("use.cli.invalid_usage", message).with_suggestion(HELP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_arguments_are_bounded_and_reject_unknown_options() {
        assert_eq!(
            ParsedArguments::parse(
                &["get".into(), "file.docx".into(), "--depth".into()],
                AllowedOptions::GET,
            )
            .unwrap_err()
            .code,
            "use.cli.invalid_usage"
        );
        assert_eq!(
            ParsedArguments::parse(&["view".into(), "--output".into()], AllowedOptions::NONE)
                .unwrap_err()
                .code,
            "use.cli.invalid_usage"
        );
        assert_eq!(
            ParsedArguments::parse(
                &[
                    "set".into(),
                    "file.docx".into(),
                    "/body/p[1]".into(),
                    "--text".into(),
                    "one".into(),
                    "--text".into(),
                    "two".into(),
                ],
                AllowedOptions::SET,
            )
            .unwrap_err()
            .code,
            "use.cli.invalid_usage"
        );

        let parsed = ParsedArguments::parse(
            &[
                "add".into(),
                "file.docx".into(),
                "/body".into(),
                "--type".into(),
                "table".into(),
                "--rows".into(),
                "2".into(),
                "--cols".into(),
                "3".into(),
            ],
            AllowedOptions::ADD,
        )
        .unwrap();
        assert_eq!(parsed.rows, Some(2));
        assert_eq!(parsed.columns, Some(3));

        let parsed = ParsedArguments::parse(
            &[
                "view".into(),
                "book.xlsx".into(),
                "issues".into(),
                "--type".into(),
                "formula_not_evaluated".into(),
                "--limit".into(),
                "10".into(),
            ],
            AllowedOptions::VIEW,
        )
        .unwrap();
        assert_eq!(parsed.node_type.as_deref(), Some("formula_not_evaluated"));
        assert_eq!(parsed.limit, Some(10));

        assert_eq!(
            ParsedArguments::parse(
                &[
                    "add".into(),
                    "file.docx".into(),
                    "/body".into(),
                    "--columns".into(),
                    "2".into(),
                    "--cols".into(),
                    "3".into(),
                ],
                AllowedOptions::ADD,
            )
            .unwrap_err()
            .code,
            "use.cli.invalid_usage"
        );
    }

    #[tokio::test]
    async fn native_batch_inputs_are_versioned_and_size_bounded() {
        let oversized = tempfile::NamedTempFile::new().unwrap();
        oversized
            .as_file()
            .set_len(MAX_BATCH_INPUT_BYTES + 1)
            .unwrap();
        let error = replay::read_batch_input(oversized.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.office.batch_input_too_large");

        let unsupported = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(unsupported.path(), br#"{"schemaVersion":2,"mutations":[]}"#).unwrap();
        let error = replay::read_batch_input(unsupported.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.office.batch_schema_unsupported");

        let unsupported_replay = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            unsupported_replay.path(),
            serde_json::to_vec(&serde_json::json!({
                "format": "a3s.office.native-replay",
                "schemaVersion": 2,
                "documentKind": "word",
                "scope": "/",
                "base": "blank",
                "baseSha256": "0".repeat(64),
                "resultSha256": "0".repeat(64),
                "mutations": []
            }))
            .unwrap(),
        )
        .unwrap();
        let error = replay::read_batch_input(unsupported_replay.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.office.replay_schema_unsupported");

        let oversized_xml = tempfile::NamedTempFile::new().unwrap();
        oversized_xml
            .as_file()
            .set_len(raw::MAX_RAW_XML_INPUT_BYTES + 1)
            .unwrap();
        let error = raw::read_xml_input(oversized_xml.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.office.raw_input_too_large");

        let invalid_utf8 = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(invalid_utf8.path(), [0xff, 0xfe, 0xfd]).unwrap();
        let error = raw::read_xml_input(invalid_utf8.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.office.raw_input_invalid");

        #[cfg(unix)]
        {
            let directory = tempfile::tempdir().unwrap();
            let link = directory.path().join("input.xml");
            std::os::unix::fs::symlink(invalid_utf8.path(), &link).unwrap();
            let error = raw::read_xml_input(link.to_str().unwrap())
                .await
                .unwrap_err();
            assert_eq!(error.code, "use.office.raw_input_invalid");
        }
    }
}
