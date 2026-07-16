use a3s_use_core::UseResult;

use super::{
    editor_error, node_not_found, parse_segments, prefix, preserve_space_attribute, qualified,
    validate_mutation_path,
};
use crate::xml_edit::{
    apply_patches, index_xml, insert_child, replace_text_descendants, IndexedXmlElement, XmlPatch,
};
use crate::{DocumentKind, LosslessXmlPart, NativeOfficePackage};

mod arrange;

pub(super) use arrange::{copy_node, move_node, swap_nodes};

const DOCUMENT_PART: &str = "word/document.xml";
const WORD_NAMESPACE: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const DEFAULT_COLUMN_WIDTH: u32 = 2_400;
const MAX_TABLE_ROWS: usize = 10_000;
const MAX_TABLE_COLUMNS: usize = 63;
const MAX_TABLE_CELLS: usize = 100_000;

pub(super) fn add_paragraph(
    package: &mut NativeOfficePackage,
    parent: &str,
    text: &str,
) -> UseResult<String> {
    require_word(package, "add-paragraph")?;
    if !matches!(parent, "/" | "/body") {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Native Word paragraphs can currently be added only to /body.",
        ));
    }
    let part = package.xml_part(DOCUMENT_PART)?;
    let index = index_xml(&part)?;
    let body = index
        .child("body", 1)
        .ok_or_else(|| node_not_found("/body"))?;
    let position = direct_child_count(body, "p") + 1;
    let prefix = prefix(&body.qualified_name);
    let paragraph_tag = qualified(prefix, "p");
    let run_tag = qualified(prefix, "r");
    let text_tag = qualified(prefix, "t");
    let escaped = crate::xml_edit::escape_text(text);
    let space = preserve_space_attribute(text);
    let fragment = format!(
        "<{paragraph_tag}><{run_tag}><{text_tag}{space}>{escaped}</{text_tag}></{run_tag}></{paragraph_tag}>"
    );
    let edited = if let Some(section) = body
        .children
        .iter()
        .find(|child| child.local_name == "sectPr")
    {
        apply_patches(
            &part,
            vec![XmlPatch::new(
                section.full_range.start..section.full_range.start,
                fragment,
            )],
        )?
    } else {
        insert_child(&part, body, fragment)?
    };
    package.set_part(DOCUMENT_PART, edited)?;
    Ok(format!("/body/p[{position}]"))
}

pub(super) fn set_text(package: &mut NativeOfficePackage, path: &str, text: &str) -> UseResult<()> {
    if !path.starts_with("/body") {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Native Word text mutation currently supports document-body paths.",
        ));
    }
    let part = package.xml_part(DOCUMENT_PART)?;
    let index = index_xml(&part)?;
    let target = locate_word_path(&index, path)?;
    if !matches!(target.local_name.as_str(), "p" | "r" | "tc") {
        return Err(editor_error(
            "use.office.mutation_type_unsupported",
            format!(
                "Word element '{}' does not support set-text.",
                target.local_name
            ),
        ));
    }

    if target.local_name == "tc" {
        let mut text_elements = Vec::new();
        target.descendants_named("t", &mut text_elements);
        if text_elements.is_empty() {
            let paragraph = target.child("p", 1).ok_or_else(|| {
                editor_error(
                    "use.office.word_table_cell_invalid",
                    format!("Word table cell '{path}' has no paragraph for text."),
                )
            })?;
            let edited = insert_child(&part, paragraph, word_text_fragment(paragraph, text, true))?;
            return package.set_part(DOCUMENT_PART, edited);
        }
    }

    let insertion = if target.local_name == "r" {
        Some(word_text_fragment(target, text, false))
    } else if target.local_name == "p" {
        Some(word_text_fragment(target, text, true))
    } else {
        None
    };
    let edited = replace_text_descendants(&part, target, "t", text, insertion)?;
    package.set_part(DOCUMENT_PART, edited)
}

pub(super) fn add_table(
    package: &mut NativeOfficePackage,
    parent: &str,
    rows: usize,
    columns: usize,
) -> UseResult<String> {
    require_word(package, "add-table")?;
    validate_dimensions(rows, columns)?;

    let part = package.xml_part(DOCUMENT_PART)?;
    let index = index_xml(&part)?;
    let (parent_path, container) = if matches!(parent, "/" | "/body") {
        (
            "/body",
            index
                .child("body", 1)
                .ok_or_else(|| node_not_found("/body"))?,
        )
    } else {
        validate_mutation_path(parent)?;
        let container = locate_word_path(&index, parent)?;
        if container.local_name != "tc" {
            return Err(editor_error(
                "use.office.mutation_path_unsupported",
                "Native Word tables can be added only to /body or a table cell.",
            ));
        }
        (parent, container)
    };
    let position = direct_child_count(container, "tbl") + 1;
    let table = table_xml(rows, columns);
    let edited = insert_block(&part, container, &table)?;
    package.set_part(DOCUMENT_PART, edited)?;
    Ok(format!("{parent_path}/tbl[{position}]"))
}

pub(super) fn add_table_row(
    package: &mut NativeOfficePackage,
    parent: &str,
    columns: Option<usize>,
) -> UseResult<String> {
    require_word(package, "add-table-row")?;
    validate_mutation_path(parent)?;

    let part = package.xml_part(DOCUMENT_PART)?;
    let index = index_xml(&part)?;
    let table = locate_word_path(&index, parent)?;
    if table.local_name != "tbl" {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Native Word table rows require a table parent such as /body/tbl[1].",
        ));
    }
    let existing_rows = direct_child_count(table, "tr");
    if existing_rows >= MAX_TABLE_ROWS {
        return Err(table_limit_error(format!(
            "Word tables are limited to {MAX_TABLE_ROWS} rows per native mutation surface."
        )));
    }
    let grid_columns = table_grid_columns(table);
    let columns = columns.unwrap_or_else(|| grid_columns.max(max_direct_cell_count(table)).max(1));
    validate_dimensions(1, columns)?;

    let edited = ensure_grid_width(&part, parent, columns)?;
    let edited_part = LosslessXmlPart::parse(DOCUMENT_PART.to_string(), edited)?;
    let edited_index = index_xml(&edited_part)?;
    let edited_table = locate_word_path(&edited_index, parent)?;
    let row = row_xml(columns);
    let edited = insert_child(&edited_part, edited_table, row)?;
    package.set_part(DOCUMENT_PART, edited)?;
    Ok(format!("{parent}/tr[{}]", existing_rows + 1))
}

pub(super) fn add_table_cell(
    package: &mut NativeOfficePackage,
    parent: &str,
    text: &str,
) -> UseResult<String> {
    require_word(package, "add-table-cell")?;
    validate_mutation_path(parent)?;

    let part = package.xml_part(DOCUMENT_PART)?;
    let index = index_xml(&part)?;
    let row = locate_word_path(&index, parent)?;
    if row.local_name != "tr" {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Native Word table cells require a row parent such as /body/tbl[1]/tr[1].",
        ));
    }
    let existing_cells = direct_child_count(row, "tc");
    let columns = existing_cells
        .checked_add(1)
        .ok_or_else(|| table_limit_error("Native Word table cell count overflowed.".to_string()))?;
    validate_dimensions(1, columns)?;
    let table_path = parent_path(parent).ok_or_else(|| node_not_found(parent))?;

    let edited = ensure_grid_width(&part, table_path, columns)?;
    let edited_part = LosslessXmlPart::parse(DOCUMENT_PART.to_string(), edited)?;
    let edited_index = index_xml(&edited_part)?;
    let edited_row = locate_word_path(&edited_index, parent)?;
    let edited = insert_child(&edited_part, edited_row, cell_xml(text))?;
    package.set_part(DOCUMENT_PART, edited)?;
    Ok(format!("{parent}/tc[{columns}]"))
}

pub(super) fn remove(package: &mut NativeOfficePackage, path: &str) -> UseResult<()> {
    require_word(package, "remove")?;
    validate_mutation_path(path)?;
    if !path.starts_with("/body/") {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Native Word remove currently supports document-body child paths.",
        ));
    }

    let part = package.xml_part(DOCUMENT_PART)?;
    let index = index_xml(&part)?;
    let target = locate_word_path(&index, path)?;
    let mut patches = Vec::new();
    match target.local_name.as_str() {
        "p" => validate_paragraph_removal(&index, path)?,
        "r" | "tbl" => {}
        "tr" => {
            let table_path = parent_path(path).ok_or_else(|| node_not_found(path))?;
            let table = locate_word_path(&index, table_path)?;
            let rows = direct_children(table, "tr");
            if rows.len() <= 1 {
                return Err(editor_error(
                    "use.office.word_last_table_row",
                    "A Word table must retain at least one row; remove the table instead.",
                ));
            }
            let max_cells = rows
                .into_iter()
                .filter(|row| row.full_range != target.full_range)
                .map(|row| direct_child_count(row, "tc"))
                .max()
                .unwrap_or(1);
            append_trailing_grid_prunes(table, max_cells, &mut patches);
        }
        "tc" => {
            let row_path = parent_path(path).ok_or_else(|| node_not_found(path))?;
            let row = locate_word_path(&index, row_path)?;
            if direct_child_count(row, "tc") <= 1 {
                return Err(editor_error(
                    "use.office.word_last_table_cell",
                    "A Word table row must retain at least one cell.",
                ));
            }
            let table_path = parent_path(row_path).ok_or_else(|| node_not_found(path))?;
            let table = locate_word_path(&index, table_path)?;
            let max_cells = direct_children(table, "tr")
                .into_iter()
                .map(|candidate| {
                    let count = direct_child_count(candidate, "tc");
                    if candidate.full_range == row.full_range {
                        count.saturating_sub(1)
                    } else {
                        count
                    }
                })
                .max()
                .unwrap_or(1);
            append_trailing_grid_prunes(table, max_cells, &mut patches);
        }
        name => {
            return Err(editor_error(
                "use.office.mutation_type_unsupported",
                format!("Word element '{name}' does not support native remove."),
            ))
        }
    }
    patches.push(XmlPatch::new(target.full_range.clone(), Vec::new()));
    let edited = apply_patches(&part, patches)?;
    package.set_part(DOCUMENT_PART, edited)
}

pub(super) fn locate_word_path<'a>(
    root: &'a IndexedXmlElement,
    path: &str,
) -> UseResult<&'a IndexedXmlElement> {
    if root.local_name != "document" {
        return Err(editor_error(
            "use.office.word_xml_invalid",
            "Word main part does not have a document root.",
        ));
    }
    let segments = parse_segments(path)?;
    let mut current = root;
    for segment in segments {
        let local_name = match segment.name.as_str() {
            "body" => "body",
            "p" | "paragraph" => "p",
            "r" | "run" => "r",
            "tbl" | "table" => "tbl",
            "tr" => "tr",
            "tc" | "cell" => "tc",
            "hyperlink" => "hyperlink",
            name => {
                return Err(editor_error(
                    "use.office.mutation_path_unsupported",
                    format!("Word path element '{name}' is not supported for native mutation."),
                ));
            }
        };
        current = current
            .child(local_name, segment.position.unwrap_or(1))
            .ok_or_else(|| node_not_found(path))?;
    }
    Ok(current)
}

fn word_text_fragment(target: &IndexedXmlElement, text: &str, wrap_run: bool) -> String {
    let prefix = prefix(&target.qualified_name);
    let text_tag = qualified(prefix, "t");
    let space = preserve_space_attribute(text);
    let text = crate::xml_edit::escape_text(text);
    if wrap_run {
        let run_tag = qualified(prefix, "r");
        format!("<{run_tag}><{text_tag}{space}>{text}</{text_tag}></{run_tag}>")
    } else {
        format!("<{text_tag}{space}>{text}</{text_tag}>")
    }
}

fn require_word(package: &NativeOfficePackage, operation: &str) -> UseResult<()> {
    if package.kind() == DocumentKind::Word {
        Ok(())
    } else {
        Err(editor_error(
            "use.office.mutation_type_unsupported",
            format!("Native {operation} is available only for Word documents."),
        ))
    }
}

fn validate_dimensions(rows: usize, columns: usize) -> UseResult<()> {
    if rows == 0 || columns == 0 {
        return Err(editor_error(
            "use.office.word_table_dimensions_invalid",
            "Native Word table dimensions must be positive integers.",
        ));
    }
    if rows > MAX_TABLE_ROWS || columns > MAX_TABLE_COLUMNS {
        return Err(table_limit_error(format!(
            "Native Word tables support at most {MAX_TABLE_ROWS} rows and {MAX_TABLE_COLUMNS} columns."
        )));
    }
    let cells = rows
        .checked_mul(columns)
        .ok_or_else(|| table_limit_error("Native Word table dimensions overflowed.".to_string()))?;
    if cells > MAX_TABLE_CELLS {
        return Err(table_limit_error(format!(
            "Native Word table creation is limited to {MAX_TABLE_CELLS} cells."
        )));
    }
    Ok(())
}

fn table_limit_error(message: String) -> a3s_use_core::UseError {
    editor_error("use.office.word_table_limit", message)
}

fn insert_block(
    part: &LosslessXmlPart,
    container: &IndexedXmlElement,
    block: &str,
) -> UseResult<Vec<u8>> {
    if container.empty {
        let content = if container.local_name == "tc" {
            format!("{block}<w:p xmlns:w=\"{WORD_NAMESPACE}\"/>")
        } else {
            block.to_string()
        };
        return insert_child(part, container, content);
    }
    let insertion = if container.local_name == "body" {
        container
            .children
            .iter()
            .find(|child| child.local_name == "sectPr")
            .map_or(container.content_range.end, |child| child.full_range.start)
    } else {
        container
            .children
            .iter()
            .rev()
            .find(|child| child.local_name == "p")
            .map_or(container.content_range.end, |child| child.full_range.start)
    };
    apply_patches(part, vec![XmlPatch::new(insertion..insertion, block)])
}

fn ensure_grid_width(
    part: &LosslessXmlPart,
    table_path: &str,
    columns: usize,
) -> UseResult<Vec<u8>> {
    let index = index_xml(part)?;
    let table = locate_word_path(&index, table_path)?;
    if table.local_name != "tbl" {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Native Word table grid mutation requires a table path.",
        ));
    }
    let existing = table_grid_columns(table);
    if existing >= columns {
        return Ok(part.raw().to_vec());
    }
    let missing = columns - existing;
    let columns_xml = (0..missing)
        .map(|_| {
            format!("<w:gridCol xmlns:w=\"{WORD_NAMESPACE}\" w:w=\"{DEFAULT_COLUMN_WIDTH}\"/>")
        })
        .collect::<String>();
    if let Some(grid) = table.child("tblGrid", 1) {
        return insert_child(part, grid, columns_xml);
    }
    let grid = format!("<w:tblGrid xmlns:w=\"{WORD_NAMESPACE}\">{columns_xml}</w:tblGrid>");
    let insertion = table
        .children
        .iter()
        .find(|child| child.local_name == "tr")
        .map_or(table.content_range.end, |row| row.full_range.start);
    apply_patches(part, vec![XmlPatch::new(insertion..insertion, grid)])
}

fn validate_paragraph_removal(index: &IndexedXmlElement, path: &str) -> UseResult<()> {
    let Some(parent_path) = parent_path(path) else {
        return Ok(());
    };
    let parent = locate_word_path(index, parent_path)?;
    if parent.local_name == "tc" && direct_child_count(parent, "p") <= 1 {
        return Err(editor_error(
            "use.office.word_last_cell_paragraph",
            "A Word table cell must retain a trailing paragraph.",
        ));
    }
    Ok(())
}

fn append_trailing_grid_prunes(
    table: &IndexedXmlElement,
    retained_columns: usize,
    patches: &mut Vec<XmlPatch>,
) {
    if has_grid_spans(table) {
        return;
    }
    let Some(grid) = table.child("tblGrid", 1) else {
        return;
    };
    for column in direct_children(grid, "gridCol")
        .into_iter()
        .skip(retained_columns)
    {
        patches.push(XmlPatch::new(column.full_range.clone(), Vec::new()));
    }
}

fn has_grid_spans(table: &IndexedXmlElement) -> bool {
    let mut spans = Vec::new();
    table.descendants_named("gridSpan", &mut spans);
    spans.into_iter().any(|span| {
        span.attributes
            .get("val")
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|value| value > 1)
    })
}

fn table_grid_columns(table: &IndexedXmlElement) -> usize {
    table
        .child("tblGrid", 1)
        .map_or(0, |grid| direct_child_count(grid, "gridCol"))
}

fn max_direct_cell_count(table: &IndexedXmlElement) -> usize {
    direct_children(table, "tr")
        .into_iter()
        .map(|row| direct_child_count(row, "tc"))
        .max()
        .unwrap_or(0)
}

fn direct_children<'a>(
    element: &'a IndexedXmlElement,
    local_name: &str,
) -> Vec<&'a IndexedXmlElement> {
    element
        .children
        .iter()
        .filter(|child| child.local_name == local_name)
        .collect()
}

fn direct_child_count(element: &IndexedXmlElement, local_name: &str) -> usize {
    element
        .children
        .iter()
        .filter(|child| child.local_name == local_name)
        .count()
}

fn parent_path(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

fn table_xml(rows: usize, columns: usize) -> String {
    let grid = (0..columns)
        .map(|_| format!("<w:gridCol w:w=\"{DEFAULT_COLUMN_WIDTH}\"/>"))
        .collect::<String>();
    let rows = (0..rows).map(|_| row_xml(columns)).collect::<String>();
    format!(
        "<w:tbl xmlns:w=\"{WORD_NAMESPACE}\"><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/><w:tblLayout w:type=\"autofit\"/><w:tblBorders><w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/><w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/></w:tblBorders></w:tblPr><w:tblGrid>{grid}</w:tblGrid>{rows}</w:tbl>"
    )
}

fn row_xml(columns: usize) -> String {
    let cells = (0..columns).map(|_| cell_xml("")).collect::<String>();
    format!("<w:tr xmlns:w=\"{WORD_NAMESPACE}\">{cells}</w:tr>")
}

fn cell_xml(text: &str) -> String {
    let text = crate::xml_edit::escape_text(text);
    let space = preserve_space_attribute(text.as_ref());
    format!(
        "<w:tc xmlns:w=\"{WORD_NAMESPACE}\"><w:tcPr><w:tcW w:w=\"{DEFAULT_COLUMN_WIDTH}\" w:type=\"dxa\"/></w:tcPr><w:p><w:r><w:t{space}>{text}</w:t></w:r></w:p></w:tc>"
    )
}
