use a3s_use_core::{UseError, UseResult};
use a3s_use_office::{
    DocumentNode, NativeOfficeDocument, NativeOfficeEditor, NativeOfficeMutation,
    SpreadsheetCellValue,
};
use serde::Deserialize;
use tokio::io::AsyncReadExt;

use crate::cli::CommandOutput;

const MAX_BATCH_INPUT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_BATCH_MUTATIONS: usize = 10_000;

const HELP: &str = concat!(
    "a3s-use office native — dependency-free OOXML operations\n\n",
    "usage:\n",
    "  a3s-use office native get <file> [path] [--depth <n>] [--json]\n",
    "  a3s-use office native query <file> <selector> [--json]\n",
    "  a3s-use office native view <file> text|outline|stats [--json]\n",
    "  a3s-use office native validate <file> [--json]\n",
    "  a3s-use office native create <file.docx|file.xlsx|file.pptx> [--json]\n",
    "  a3s-use office native add <file> <parent> --type paragraph|table|row|cell|sheet|slide|shape [--rows <n>] [--columns <n>] [--name <name>] [--text <value>] [--output <file>] [--json]\n",
    "  a3s-use office native set <file> <path> (--text <value>|--number <value>|--boolean <true|false>|--formula <expression>) [--output <file>] [--json]\n",
    "  a3s-use office native remove <file> <path> [--output <file>] [--json]\n",
    "  a3s-use office native batch <file> --input <mutations.json> [--output <file>] [--json]"
);

pub async fn run(args: &[String]) -> UseResult<CommandOutput> {
    match args.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => Ok(help()),
        Some("get") => get(args).await,
        Some("query") => query(args).await,
        Some("view") => view(args).await,
        Some("validate") => validate(args).await,
        Some("create") => create(args).await,
        Some("add") => add(args).await,
        Some("set") => set(args).await,
        Some("remove") => remove(args).await,
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
            "commands": ["get", "query", "view", "validate", "create", "add", "set", "remove", "batch"],
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

async fn view(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::NONE)?;
    if parsed.positionals.len() != 2 {
        return Err(usage_error(
            "office native view requires <file> and text, outline, or stats",
        ));
    }
    let document = NativeOfficeDocument::open(&parsed.positionals[0]).await?;
    match parsed.positionals[1].as_str() {
        "text" | "t" => {
            let view = document.text_view();
            Ok(CommandOutput::success(
                view.text.clone(),
                serde_json::json!({ "view": "text", "result": view }),
            ))
        }
        "outline" | "o" => {
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
        mode => Err(usage_error(format!(
            "native Office view mode '{mode}' is not text, outline, or stats"
        ))),
    }
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

async fn add(args: &[String]) -> UseResult<CommandOutput> {
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
    validate_add_options(node_type, &parsed)?;
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    let text = parsed.text.as_deref().unwrap_or_default();
    let (operation, path) = match node_type {
        "paragraph" | "p" => ("add-paragraph", editor.add_paragraph(parent, text)?),
        "table" | "tbl" => (
            "add-table",
            editor.add_table(
                parent,
                parsed.rows.unwrap_or(1),
                parsed.columns.unwrap_or(1),
            )?,
        ),
        "row" | "tr" => (
            "add-table-row",
            editor.add_table_row(parent, parsed.columns)?,
        ),
        "cell" | "tc" => ("add-table-cell", editor.add_table_cell(parent, text)?),
        "sheet" | "worksheet" => {
            if parent != "/" {
                return Err(usage_error("native worksheets can be added only to /"));
            }
            let name = parsed
                .name
                .as_deref()
                .ok_or_else(|| usage_error("native worksheet add requires --name <name>"))?;
            ("add-worksheet", editor.add_worksheet(name)?)
        }
        "slide" => ("add-slide", editor.add_slide(parent, text)?),
        "shape" => ("add-shape", editor.add_shape(parent, text)?),
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
            "kind": editor.package().kind(),
            "outputPath": output_path,
            "inPlace": in_place,
            "revision": editor.package().source_revision()
        }),
    ))
}

fn validate_add_options(node_type: &str, parsed: &ParsedArguments) -> UseResult<()> {
    let accepts_rows = matches!(node_type, "table" | "tbl");
    let accepts_columns = matches!(node_type, "table" | "tbl" | "row" | "tr");
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
    Ok(())
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
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if value_count != 1 {
        return Err(usage_error(
            "office native set requires exactly one of --text, --number, --boolean, or --formula",
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
    let operation = if let Some(value) = typed_value {
        editor.set_cell_value(path, value)?;
        "set-cell-value"
    } else {
        editor.set_text(path, parsed.text.as_deref().unwrap_or_default())?;
        "set-text"
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

async fn remove(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::REMOVE)?;
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

async fn batch(args: &[String]) -> UseResult<CommandOutput> {
    let parsed = ParsedArguments::parse(args, AllowedOptions::BATCH)?;
    if parsed.positionals.len() != 1 {
        return Err(usage_error("office native batch requires <file>"));
    }
    let input_path = parsed
        .input
        .as_deref()
        .ok_or_else(|| usage_error("office native batch requires --input <mutations.json>"))?;
    let input = read_batch_input(input_path).await?;
    let source = &parsed.positionals[0];
    let mut editor = NativeOfficeEditor::open(source).await?;
    let source_path = editor.package().path().to_path_buf();
    let result = editor.apply_batch(&input.mutations)?;
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeBatchInput {
    schema_version: u32,
    mutations: Vec<NativeOfficeMutation>,
}

async fn read_batch_input(path: &str) -> UseResult<NativeBatchInput> {
    let file = tokio::fs::File::open(path).await.map_err(|error| {
        batch_input_error(
            "use.office.batch_input_open_failed",
            path,
            format!("Failed to open native Office batch input '{path}': {error}"),
        )
    })?;
    let metadata = file.metadata().await.map_err(|error| {
        batch_input_error(
            "use.office.batch_input_open_failed",
            path,
            format!("Failed to inspect native Office batch input '{path}': {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(batch_input_error(
            "use.office.batch_input_invalid",
            path,
            "Native Office batch input must be a regular file.",
        ));
    }
    if metadata.len() > MAX_BATCH_INPUT_BYTES {
        return Err(batch_input_too_large(path));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut reader = file.take(MAX_BATCH_INPUT_BYTES + 1);
    reader.read_to_end(&mut bytes).await.map_err(|error| {
        batch_input_error(
            "use.office.batch_input_read_failed",
            path,
            format!("Failed to read native Office batch input '{path}': {error}"),
        )
    })?;
    if bytes.len() as u64 > MAX_BATCH_INPUT_BYTES {
        return Err(batch_input_too_large(path));
    }
    let input: NativeBatchInput = serde_json::from_slice(&bytes).map_err(|error| {
        batch_input_error(
            "use.office.batch_input_invalid",
            path,
            format!("Native Office batch input '{path}' is invalid JSON: {error}"),
        )
    })?;
    if input.schema_version != 1 {
        return Err(batch_input_error(
            "use.office.batch_schema_unsupported",
            path,
            format!(
                "Native Office batch schema version {} is not supported; expected 1.",
                input.schema_version
            ),
        ));
    }
    if input.mutations.len() > MAX_BATCH_MUTATIONS {
        return Err(batch_input_error(
            "use.office.batch_mutation_limit",
            path,
            format!(
                "Native Office batch contains {} mutations; the limit is {MAX_BATCH_MUTATIONS}.",
                input.mutations.len()
            ),
        ));
    }
    Ok(input)
}

fn batch_input_too_large(path: &str) -> UseError {
    batch_input_error(
        "use.office.batch_input_too_large",
        path,
        format!("Native Office batch input exceeds the {MAX_BATCH_INPUT_BYTES}-byte limit."),
    )
}

fn batch_input_error(code: &str, path: &str, message: impl Into<String>) -> UseError {
    UseError::new(code, message).with_detail("input", path)
}

#[derive(Debug, Default)]
struct ParsedArguments {
    positionals: Vec<String>,
    depth: Option<usize>,
    text: Option<String>,
    output: Option<String>,
    input: Option<String>,
    node_type: Option<String>,
    name: Option<String>,
    rows: Option<usize>,
    columns: Option<usize>,
    number: Option<String>,
    boolean: Option<String>,
    formula: Option<String>,
}

impl ParsedArguments {
    fn parse(args: &[String], allowed: AllowedOptions) -> UseResult<Self> {
        let mut parsed = Self::default();
        let mut index = 1;
        while index < args.len() {
            match args[index].as_str() {
                "--json" => index += 1,
                "--" => {
                    parsed.positionals.extend_from_slice(&args[index + 1..]);
                    break;
                }
                "--depth" if allowed.depth => {
                    if parsed.depth.is_some() {
                        return Err(usage_error("--depth may be specified only once"));
                    }
                    let value = option_value(args, index, "--depth")?;
                    parsed.depth = Some(value.parse::<usize>().map_err(|_| {
                        usage_error(format!(
                            "--depth requires a non-negative integer, received '{value}'"
                        ))
                    })?);
                    index += 2;
                }
                "--text" if allowed.text => {
                    set_string_option(&mut parsed.text, args, index, "--text")?;
                    index += 2;
                }
                "--number" if allowed.number => {
                    set_string_option(&mut parsed.number, args, index, "--number")?;
                    index += 2;
                }
                "--boolean" | "--bool" if allowed.boolean => {
                    set_string_option(&mut parsed.boolean, args, index, "--boolean")?;
                    index += 2;
                }
                "--formula" if allowed.formula => {
                    set_string_option(&mut parsed.formula, args, index, "--formula")?;
                    index += 2;
                }
                "--output" if allowed.output => {
                    set_string_option(&mut parsed.output, args, index, "--output")?;
                    index += 2;
                }
                "--input" if allowed.input => {
                    set_string_option(&mut parsed.input, args, index, "--input")?;
                    index += 2;
                }
                "--type" if allowed.node_type => {
                    set_string_option(&mut parsed.node_type, args, index, "--type")?;
                    index += 2;
                }
                "--name" if allowed.name => {
                    set_string_option(&mut parsed.name, args, index, "--name")?;
                    index += 2;
                }
                "--rows" if allowed.rows => {
                    set_usize_option(&mut parsed.rows, args, index, "--rows")?;
                    index += 2;
                }
                "--columns" | "--cols" if allowed.columns => {
                    set_usize_option(&mut parsed.columns, args, index, "--columns")?;
                    index += 2;
                }
                option if option.starts_with('-') => {
                    return Err(usage_error(format!(
                        "unknown native Office option '{option}'"
                    )));
                }
                value => {
                    parsed.positionals.push(value.to_string());
                    index += 1;
                }
            }
        }
        Ok(parsed)
    }
}

#[derive(Debug, Clone, Copy)]
struct AllowedOptions {
    depth: bool,
    text: bool,
    output: bool,
    input: bool,
    node_type: bool,
    name: bool,
    rows: bool,
    columns: bool,
    number: bool,
    boolean: bool,
    formula: bool,
}

impl AllowedOptions {
    const NONE: Self = Self {
        depth: false,
        text: false,
        output: false,
        input: false,
        node_type: false,
        name: false,
        rows: false,
        columns: false,
        number: false,
        boolean: false,
        formula: false,
    };
    const GET: Self = Self {
        depth: true,
        ..Self::NONE
    };
    const SET: Self = Self {
        text: true,
        output: true,
        number: true,
        boolean: true,
        formula: true,
        ..Self::NONE
    };
    const BATCH: Self = Self {
        output: true,
        input: true,
        ..Self::NONE
    };
    const ADD: Self = Self {
        text: true,
        output: true,
        node_type: true,
        name: true,
        rows: true,
        columns: true,
        ..Self::NONE
    };
    const REMOVE: Self = Self {
        output: true,
        ..Self::NONE
    };
}

fn option_value<'a>(args: &'a [String], index: usize, option: &str) -> UseResult<&'a str> {
    args.get(index + 1)
        .map(String::as_str)
        .ok_or_else(|| usage_error(format!("{option} requires a value")))
}

fn set_string_option(
    target: &mut Option<String>,
    args: &[String],
    index: usize,
    option: &str,
) -> UseResult<()> {
    if target.is_some() {
        return Err(usage_error(format!("{option} may be specified only once")));
    }
    *target = Some(option_value(args, index, option)?.to_string());
    Ok(())
}

fn set_usize_option(
    target: &mut Option<usize>,
    args: &[String],
    index: usize,
    option: &str,
) -> UseResult<()> {
    if target.is_some() {
        return Err(usage_error(format!("{option} may be specified only once")));
    }
    let value = option_value(args, index, option)?;
    *target = Some(value.parse::<usize>().map_err(|_| {
        usage_error(format!(
            "{option} requires a non-negative integer, received '{value}'"
        ))
    })?);
    Ok(())
}

fn parse_boolean_option(value: &str) -> UseResult<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(usage_error(format!(
            "--boolean requires true or false, received '{value}'"
        ))),
    }
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
        let error = read_batch_input(oversized.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.office.batch_input_too_large");

        let unsupported = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(unsupported.path(), br#"{"schemaVersion":2,"mutations":[]}"#).unwrap();
        let error = read_batch_input(unsupported.path().to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.office.batch_schema_unsupported");
    }
}
