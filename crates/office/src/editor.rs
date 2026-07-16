use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};

use crate::discovery::office_error;
use crate::semantic::{NativeOfficeDocument, OfficeNodeType};
use crate::xml_edit::{escape_text, index_xml, insert_child, IndexedXmlElement, XmlPatch};
use crate::{DocumentKind, NativeOfficePackage};

mod presentation;
mod spreadsheet;
mod word;

/// Typed Spreadsheet cell content written without a shared-string dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SpreadsheetCellValue {
    Text { value: String },
    Number { value: String },
    Boolean { value: bool },
    Formula { expression: String },
}

/// Typed in-process mutation supported by an atomic native batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NativeOfficeMutation {
    SetText {
        path: String,
        text: String,
    },
    SetCellValue {
        path: String,
        value: SpreadsheetCellValue,
    },
    AddParagraph {
        parent: String,
        text: String,
    },
    AddTable {
        parent: String,
        rows: usize,
        columns: usize,
    },
    AddTableRow {
        parent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        columns: Option<usize>,
    },
    AddTableCell {
        parent: String,
        text: String,
    },
    AddSlide {
        parent: String,
        title: String,
    },
    AddShape {
        parent: String,
        text: String,
    },
    AddWorksheet {
        name: String,
    },
    Remove {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBatchResult {
    pub applied: usize,
    pub paths: Vec<String>,
}

/// Loss-preserving OOXML editor with transactional in-memory batches.
#[derive(Debug, Clone)]
pub struct NativeOfficeEditor {
    package: NativeOfficePackage,
}

impl NativeOfficeEditor {
    pub async fn create(path: impl AsRef<Path>) -> UseResult<Self> {
        Self::from_package(NativeOfficePackage::create(path).await?)
    }

    pub async fn open(path: impl AsRef<Path>) -> UseResult<Self> {
        let package = NativeOfficePackage::open(path).await?;
        NativeOfficeDocument::from_package(package.clone())?;
        Ok(Self { package })
    }

    pub fn from_package(package: NativeOfficePackage) -> UseResult<Self> {
        NativeOfficeDocument::from_package(package.clone())?;
        Ok(Self { package })
    }

    pub fn package(&self) -> &NativeOfficePackage {
        &self.package
    }

    pub fn snapshot(&self) -> UseResult<NativeOfficeDocument> {
        NativeOfficeDocument::from_package(self.package.clone())
    }

    pub fn is_dirty(&self) -> bool {
        self.package.is_dirty()
    }

    pub fn set_text(&mut self, path: impl Into<String>, text: impl Into<String>) -> UseResult<()> {
        self.apply_batch(&[NativeOfficeMutation::SetText {
            path: path.into(),
            text: text.into(),
        }])?;
        Ok(())
    }

    pub fn set_cell_value(
        &mut self,
        path: impl Into<String>,
        value: SpreadsheetCellValue,
    ) -> UseResult<()> {
        self.apply_batch(&[NativeOfficeMutation::SetCellValue {
            path: path.into(),
            value,
        }])?;
        Ok(())
    }

    pub fn add_paragraph(
        &mut self,
        parent: impl Into<String>,
        text: impl Into<String>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddParagraph {
            parent: parent.into(),
            text: text.into(),
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office paragraph mutation returned no path.",
            )
        })
    }

    pub fn add_slide(
        &mut self,
        parent: impl Into<String>,
        title: impl Into<String>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddSlide {
            parent: parent.into(),
            title: title.into(),
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office slide mutation returned no path.",
            )
        })
    }

    pub fn add_table(
        &mut self,
        parent: impl Into<String>,
        rows: usize,
        columns: usize,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddTable {
            parent: parent.into(),
            rows,
            columns,
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office table mutation returned no path.",
            )
        })
    }

    pub fn add_table_row(
        &mut self,
        parent: impl Into<String>,
        columns: Option<usize>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddTableRow {
            parent: parent.into(),
            columns,
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office table-row mutation returned no path.",
            )
        })
    }

    pub fn add_table_cell(
        &mut self,
        parent: impl Into<String>,
        text: impl Into<String>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddTableCell {
            parent: parent.into(),
            text: text.into(),
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office table-cell mutation returned no path.",
            )
        })
    }

    pub fn add_shape(
        &mut self,
        parent: impl Into<String>,
        text: impl Into<String>,
    ) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddShape {
            parent: parent.into(),
            text: text.into(),
        }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office shape mutation returned no path.",
            )
        })
    }

    pub fn remove(&mut self, path: impl Into<String>) -> UseResult<String> {
        let result = self.apply_batch(&[NativeOfficeMutation::Remove { path: path.into() }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office remove mutation returned no path.",
            )
        })
    }

    pub fn add_worksheet(&mut self, name: impl Into<String>) -> UseResult<String> {
        let result =
            self.apply_batch(&[NativeOfficeMutation::AddWorksheet { name: name.into() }])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office worksheet mutation returned no path.",
            )
        })
    }

    pub fn apply_batch(
        &mut self,
        mutations: &[NativeOfficeMutation],
    ) -> UseResult<NativeBatchResult> {
        if mutations.is_empty() {
            return Ok(NativeBatchResult {
                applied: 0,
                paths: Vec::new(),
            });
        }
        let original = self.package.clone();
        let mut paths = Vec::with_capacity(mutations.len());
        for mutation in mutations {
            let result = match mutation {
                NativeOfficeMutation::SetText { path, text } => {
                    set_text(&mut self.package, path, text).map(|()| path.clone())
                }
                NativeOfficeMutation::SetCellValue { path, value } => {
                    set_spreadsheet_cell_value(&mut self.package, path, value)
                        .map(|()| path.clone())
                }
                NativeOfficeMutation::AddParagraph { parent, text } => {
                    word::add_paragraph(&mut self.package, parent, text)
                }
                NativeOfficeMutation::AddTable {
                    parent,
                    rows,
                    columns,
                } => word::add_table(&mut self.package, parent, *rows, *columns),
                NativeOfficeMutation::AddTableRow { parent, columns } => {
                    word::add_table_row(&mut self.package, parent, *columns)
                }
                NativeOfficeMutation::AddTableCell { parent, text } => {
                    word::add_table_cell(&mut self.package, parent, text)
                }
                NativeOfficeMutation::AddSlide { parent, title } => {
                    presentation::add_slide(&mut self.package, parent, title)
                }
                NativeOfficeMutation::AddShape { parent, text } => {
                    presentation::add_shape(&mut self.package, parent, text)
                }
                NativeOfficeMutation::AddWorksheet { name } => {
                    spreadsheet::add_worksheet(&mut self.package, name)
                }
                NativeOfficeMutation::Remove { path } => {
                    remove_node(&mut self.package, path).map(|()| path.clone())
                }
            };
            match result {
                Ok(path) => paths.push(path),
                Err(error) => {
                    self.package = original;
                    return Err(error);
                }
            }
        }
        if let Err(error) = NativeOfficeDocument::from_package(self.package.clone()) {
            self.package = original;
            return Err(editor_error(
                "use.office.batch_validation_failed",
                format!("Native Office batch failed post-mutation validation: {error}"),
            ));
        }
        Ok(NativeBatchResult {
            applied: paths.len(),
            paths,
        })
    }

    pub async fn save(&mut self) -> UseResult<()> {
        self.package.save().await
    }

    pub async fn save_as(&mut self, path: impl AsRef<Path>) -> UseResult<()> {
        self.package.save_as(path).await
    }
}

fn set_text(package: &mut NativeOfficePackage, path: &str, text: &str) -> UseResult<()> {
    validate_mutation_path(path)?;
    match package.kind() {
        DocumentKind::Word => word::set_text(package, path, text),
        DocumentKind::Spreadsheet => set_spreadsheet_text(package, path, text),
        DocumentKind::Presentation => presentation::set_text(package, path, text),
    }
}

fn remove_node(package: &mut NativeOfficePackage, path: &str) -> UseResult<()> {
    validate_mutation_path(path)?;
    match package.kind() {
        DocumentKind::Word => word::remove(package, path),
        DocumentKind::Spreadsheet => {
            if path.trim_start_matches('/').contains('/') {
                remove_spreadsheet_cell(package, path)
            } else {
                spreadsheet::remove_worksheet(package, path)
            }
        }
        DocumentKind::Presentation => presentation::remove(package, path),
    }
}

fn set_spreadsheet_text(
    package: &mut NativeOfficePackage,
    path: &str,
    text: &str,
) -> UseResult<()> {
    set_spreadsheet_cell_value(
        package,
        path,
        &SpreadsheetCellValue::Text {
            value: text.to_string(),
        },
    )
}

fn set_spreadsheet_cell_value(
    package: &mut NativeOfficePackage,
    path: &str,
    value: &SpreadsheetCellValue,
) -> UseResult<()> {
    if package.kind() != DocumentKind::Spreadsheet {
        return Err(editor_error(
            "use.office.mutation_type_unsupported",
            "Typed cell values are available only for Spreadsheet documents.",
        ));
    }
    validate_mutation_path(path)?;
    let value = normalize_spreadsheet_cell_value(value)?;
    let (sheet_path, reference) = path.rsplit_once('/').ok_or_else(|| node_not_found(path))?;
    let (column, row, reference) = spreadsheet_cell_coordinates(reference)?;
    if sheet_path.is_empty() {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Spreadsheet cell mutation requires a single-cell path such as /Sheet1/A1.",
        ));
    }
    let snapshot = NativeOfficeDocument::from_package(package.clone())?;
    let sheet = snapshot
        .root()
        .children
        .iter()
        .find(|node| {
            node.node_type == OfficeNodeType::Worksheet
                && node.path.eq_ignore_ascii_case(sheet_path)
        })
        .ok_or_else(|| node_not_found(path))?;
    let part_name = sheet.format.get("part").ok_or_else(|| {
        editor_error(
            "use.office.spreadsheet_sheet_invalid",
            "Spreadsheet semantic sheet has no source part.",
        )
    })?;
    let part = package.xml_part(part_name)?;
    let index = index_xml(&part)?;
    let sheet_data = index
        .descendant("sheetData")
        .ok_or_else(|| node_not_found(path))?;
    let cell = indexed_spreadsheet_cell(sheet_data, column, row).map(|(_, cell)| cell);
    let edited = if let Some(cell) = cell {
        let replacement = spreadsheet_cell_fragment(cell, &reference, &value);
        crate::xml_edit::apply_patches(
            &part,
            vec![XmlPatch::new(cell.full_range.clone(), replacement)],
        )?
    } else {
        insert_spreadsheet_cell(&part, sheet_data, column, row, &reference, &value)?
    };
    let edited = update_spreadsheet_dimension(part_name, edited)?;
    package.set_part(part_name, edited)?;
    if matches!(value, SpreadsheetCellValue::Formula { .. }) {
        mark_workbook_for_recalculation(package)?;
    }
    Ok(())
}

fn remove_spreadsheet_cell(package: &mut NativeOfficePackage, path: &str) -> UseResult<()> {
    let (sheet_path, reference) = path.rsplit_once('/').ok_or_else(|| node_not_found(path))?;
    let (column, row, _) = spreadsheet_cell_coordinates(reference)?;
    if sheet_path.is_empty() {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Spreadsheet remove requires a single-cell path such as /Sheet1/A1.",
        ));
    }
    let snapshot = NativeOfficeDocument::from_package(package.clone())?;
    let sheet = snapshot
        .root()
        .children
        .iter()
        .find(|node| {
            node.node_type == OfficeNodeType::Worksheet
                && node.path.eq_ignore_ascii_case(sheet_path)
        })
        .ok_or_else(|| node_not_found(path))?;
    let part_name = sheet.format.get("part").ok_or_else(|| {
        editor_error(
            "use.office.spreadsheet_sheet_invalid",
            "Spreadsheet semantic sheet has no source part.",
        )
    })?;
    let part = package.xml_part(part_name)?;
    let index = index_xml(&part)?;
    let sheet_data = index
        .descendant("sheetData")
        .ok_or_else(|| node_not_found(path))?;
    let (_, cell) =
        indexed_spreadsheet_cell(sheet_data, column, row).ok_or_else(|| node_not_found(path))?;
    let edited = crate::xml_edit::apply_patches(
        &part,
        vec![XmlPatch::new(cell.full_range.clone(), Vec::new())],
    )?;
    let edited = update_spreadsheet_dimension(part_name, edited)?;
    package.set_part(part_name, edited)
}

fn insert_spreadsheet_cell(
    part: &crate::LosslessXmlPart,
    sheet_data: &IndexedXmlElement,
    column: u32,
    row_number: u32,
    reference: &str,
    value: &SpreadsheetCellValue,
) -> UseResult<Vec<u8>> {
    let cell = new_spreadsheet_cell_fragment(prefix(&sheet_data.qualified_name), reference, value);
    let rows = indexed_spreadsheet_rows(sheet_data);
    if let Some((_, row)) = rows.iter().find(|(number, _)| *number == row_number) {
        let mut inferred_column = 0_u32;
        let insertion = row
            .children
            .iter()
            .filter(|child| child.local_name == "c")
            .find(|child| {
                let current = child
                    .attributes
                    .get("r")
                    .and_then(|reference| spreadsheet_cell_coordinates(reference).ok())
                    .map(|(column, _, _)| column)
                    .unwrap_or_else(|| {
                        inferred_column = inferred_column.saturating_add(1);
                        inferred_column
                    });
                inferred_column = current;
                current > column
            });
        return if let Some(next) = insertion {
            crate::xml_edit::apply_patches(
                part,
                vec![XmlPatch::new(
                    next.full_range.start..next.full_range.start,
                    cell,
                )],
            )
        } else {
            insert_child(part, row, cell)
        };
    }

    let prefix = prefix(&sheet_data.qualified_name);
    let row_tag = qualified(prefix, "row");
    let row = format!("<{row_tag} r=\"{row_number}\">{cell}</{row_tag}>");
    if let Some((_, next)) = rows.iter().find(|(number, _)| *number > row_number) {
        crate::xml_edit::apply_patches(
            part,
            vec![XmlPatch::new(
                next.full_range.start..next.full_range.start,
                row,
            )],
        )
    } else {
        insert_child(part, sheet_data, row)
    }
}

fn indexed_spreadsheet_rows(sheet_data: &IndexedXmlElement) -> Vec<(u32, &IndexedXmlElement)> {
    let mut inferred = 0_u32;
    sheet_data
        .children
        .iter()
        .filter(|child| child.local_name == "row")
        .map(|row| {
            let number = row
                .attributes
                .get("r")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or_else(|| inferred.saturating_add(1));
            inferred = number;
            (number, row)
        })
        .collect()
}

fn indexed_spreadsheet_cell(
    sheet_data: &IndexedXmlElement,
    target_column: u32,
    target_row: u32,
) -> Option<(&IndexedXmlElement, &IndexedXmlElement)> {
    for (row_number, row) in indexed_spreadsheet_rows(sheet_data) {
        let mut inferred_column = 0_u32;
        for cell in row.children.iter().filter(|child| child.local_name == "c") {
            let (column, cell_row) = cell
                .attributes
                .get("r")
                .and_then(|reference| spreadsheet_cell_coordinates(reference).ok())
                .map(|(column, row, _)| (column, row))
                .unwrap_or_else(|| {
                    inferred_column = inferred_column.saturating_add(1);
                    (inferred_column, row_number)
                });
            inferred_column = column;
            if column == target_column && cell_row == target_row {
                return Some((row, cell));
            }
        }
    }
    None
}

fn new_spreadsheet_cell_fragment(
    prefix: Option<&str>,
    reference: &str,
    value: &SpreadsheetCellValue,
) -> String {
    let cell_tag = qualified(prefix, "c");
    let (cell_type, content) = spreadsheet_cell_content(prefix, value);
    let cell_type = cell_type.map_or_else(String::new, |value_type| {
        format!(" t=\"{}\"", escape_attribute(value_type))
    });
    format!("<{cell_tag} r=\"{reference}\"{cell_type}>{content}</{cell_tag}>")
}

fn update_spreadsheet_dimension(part_name: &str, bytes: Vec<u8>) -> UseResult<Vec<u8>> {
    let part = crate::LosslessXmlPart::parse(part_name.to_string(), bytes)?;
    let index = index_xml(&part)?;
    let sheet_data = index
        .descendant("sheetData")
        .ok_or_else(|| node_not_found(part_name))?;
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (row_number, row) in indexed_spreadsheet_rows(sheet_data) {
        let mut inferred_column = 0_u32;
        for cell in row.children.iter().filter(|child| child.local_name == "c") {
            let (column, row_number, _) = cell
                .attributes
                .get("r")
                .and_then(|reference| spreadsheet_cell_coordinates(reference).ok())
                .unwrap_or_else(|| {
                    inferred_column = inferred_column.saturating_add(1);
                    (inferred_column, row_number, String::new())
                });
            inferred_column = column;
            bounds = Some(match bounds {
                Some((min_column, min_row, max_column, max_row)) => (
                    min_column.min(column),
                    min_row.min(row_number),
                    max_column.max(column),
                    max_row.max(row_number),
                ),
                None => (column, row_number, column, row_number),
            });
        }
    }
    let dimension = bounds.map_or_else(
        || "A1".to_string(),
        |(min_column, min_row, max_column, max_row)| {
            let start = format!("{}{min_row}", spreadsheet_column_name(min_column));
            let end = format!("{}{max_row}", spreadsheet_column_name(max_column));
            if start == end {
                start
            } else {
                format!("{start}:{end}")
            }
        },
    );
    if let Some(existing) = index.child("dimension", 1) {
        let tag = &existing.qualified_name;
        return crate::xml_edit::apply_patches(
            &part,
            vec![XmlPatch::new(
                existing.full_range.clone(),
                format!("<{tag} ref=\"{dimension}\"/>"),
            )],
        );
    }
    let tag = qualified(prefix(&index.qualified_name), "dimension");
    let insertion = index
        .children
        .iter()
        .find(|child| child.local_name != "sheetPr")
        .map_or(index.content_range.end, |child| child.full_range.start);
    crate::xml_edit::apply_patches(
        &part,
        vec![XmlPatch::new(
            insertion..insertion,
            format!("<{tag} ref=\"{dimension}\"/>"),
        )],
    )
}

fn spreadsheet_cell_fragment(
    cell: &IndexedXmlElement,
    reference: &str,
    value: &SpreadsheetCellValue,
) -> String {
    let prefix = prefix(&cell.qualified_name);
    let cell_tag = qualified(prefix, "c");
    let mut attributes = cell.attributes.clone();
    attributes.insert("r".into(), reference.to_ascii_uppercase());
    let (cell_type, content) = spreadsheet_cell_content(prefix, value);
    if let Some(cell_type) = cell_type {
        attributes.insert("t".into(), cell_type.to_string());
    } else {
        attributes.remove("t");
    }
    let attributes = attributes
        .into_iter()
        .map(|(name, value)| format!(" {name}=\"{}\"", escape_attribute(&value)))
        .collect::<String>();
    format!("<{cell_tag}{attributes}>{content}</{cell_tag}>")
}

fn spreadsheet_cell_content(
    prefix: Option<&str>,
    value: &SpreadsheetCellValue,
) -> (Option<&'static str>, String) {
    match value {
        SpreadsheetCellValue::Text { value } => {
            let inline_tag = qualified(prefix, "is");
            let text_tag = qualified(prefix, "t");
            let space = preserve_space_attribute(value);
            let value = escape_text(value);
            (
                Some("inlineStr"),
                format!("<{inline_tag}><{text_tag}{space}>{value}</{text_tag}></{inline_tag}>"),
            )
        }
        SpreadsheetCellValue::Number { value } => {
            let value_tag = qualified(prefix, "v");
            (None, format!("<{value_tag}>{value}</{value_tag}>"))
        }
        SpreadsheetCellValue::Boolean { value } => {
            let value_tag = qualified(prefix, "v");
            let value = if *value { "1" } else { "0" };
            (Some("b"), format!("<{value_tag}>{value}</{value_tag}>"))
        }
        SpreadsheetCellValue::Formula { expression } => {
            let formula_tag = qualified(prefix, "f");
            let expression = escape_text(expression);
            (None, format!("<{formula_tag}>{expression}</{formula_tag}>"))
        }
    }
}

fn normalize_spreadsheet_cell_value(
    value: &SpreadsheetCellValue,
) -> UseResult<SpreadsheetCellValue> {
    match value {
        SpreadsheetCellValue::Text { value } => Ok(SpreadsheetCellValue::Text {
            value: value.clone(),
        }),
        SpreadsheetCellValue::Number { value } => {
            if value.is_empty()
                || value.len() > 128
                || value.trim() != value
                || !value.parse::<f64>().ok().is_some_and(f64::is_finite)
            {
                return Err(editor_error(
                    "use.office.spreadsheet_number_invalid",
                    "Spreadsheet numeric values must be bounded finite numbers without surrounding whitespace.",
                )
                .with_detail("length", value.len()));
            }
            Ok(SpreadsheetCellValue::Number {
                value: value.clone(),
            })
        }
        SpreadsheetCellValue::Boolean { value } => {
            Ok(SpreadsheetCellValue::Boolean { value: *value })
        }
        SpreadsheetCellValue::Formula { expression } => {
            let expression = expression.strip_prefix('=').unwrap_or(expression);
            if expression.is_empty()
                || expression.chars().count() > 8_192
                || expression.chars().any(char::is_control)
            {
                return Err(editor_error(
                    "use.office.spreadsheet_formula_invalid",
                    "Spreadsheet formulas must contain 1-8192 non-control characters.",
                ));
            }
            Ok(SpreadsheetCellValue::Formula {
                expression: expression.to_string(),
            })
        }
    }
}

fn mark_workbook_for_recalculation(package: &mut NativeOfficePackage) -> UseResult<()> {
    let workbook = package.xml_part("xl/workbook.xml")?;
    let index = index_xml(&workbook)?;
    let edited = if let Some(calc) = index.child("calcPr", 1) {
        let mut attributes = calc.qualified_attributes.clone();
        attributes
            .entry("calcId".into())
            .or_insert_with(|| "0".into());
        attributes.insert("calcMode".into(), "auto".into());
        attributes.insert("fullCalcOnLoad".into(), "1".into());
        attributes.insert("forceFullCalc".into(), "1".into());
        let attributes = attributes
            .into_iter()
            .map(|(name, value)| format!(" {name}=\"{}\"", escape_attribute(&value)))
            .collect::<String>();
        let terminator = if calc.empty { "/>" } else { ">" };
        crate::xml_edit::apply_patches(
            &workbook,
            vec![XmlPatch::new(
                calc.start_tag_range.clone(),
                format!("<{}{attributes}{terminator}", calc.qualified_name),
            )],
        )?
    } else {
        let tag = qualified(prefix(&index.qualified_name), "calcPr");
        let fragment = format!(
            "<{tag} calcId=\"0\" calcMode=\"auto\" fullCalcOnLoad=\"1\" forceFullCalc=\"1\"/>"
        );
        let insertion = index
            .children
            .iter()
            .find(|child| {
                matches!(
                    child.local_name.as_str(),
                    "oleSize"
                        | "customWorkbookViews"
                        | "pivotCaches"
                        | "smartTagPr"
                        | "smartTagTypes"
                        | "webPublishing"
                        | "fileRecoveryPr"
                        | "webPublishObjects"
                        | "extLst"
                )
            })
            .map_or(index.content_range.end, |child| child.full_range.start);
        crate::xml_edit::apply_patches(
            &workbook,
            vec![XmlPatch::new(insertion..insertion, fragment)],
        )?
    };
    package.set_part("xl/workbook.xml", edited)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PathSegment {
    name: String,
    position: Option<usize>,
}

fn parse_segments(path: &str) -> UseResult<Vec<PathSegment>> {
    validate_mutation_path(path)?;
    path.trim_start_matches('/')
        .split('/')
        .map(|segment| {
            if let Some((name, position)) = segment.split_once('[') {
                let position = position.strip_suffix(']').ok_or_else(|| {
                    editor_error(
                        "use.office.path_invalid",
                        format!("Office path segment '{segment}' is missing ']'."),
                    )
                })?;
                let position = position.parse::<usize>().map_err(|_| {
                    editor_error(
                        "use.office.path_invalid",
                        format!("Office mutation path segment '{segment}' is not numeric."),
                    )
                })?;
                if name.is_empty() || position == 0 {
                    return Err(editor_error(
                        "use.office.path_invalid",
                        "Office mutation paths use non-empty, one-based segments.",
                    ));
                }
                Ok(PathSegment {
                    name: name.to_ascii_lowercase(),
                    position: Some(position),
                })
            } else if segment.is_empty() {
                Err(editor_error(
                    "use.office.path_invalid",
                    "Office mutation paths cannot contain empty segments.",
                ))
            } else {
                Ok(PathSegment {
                    name: segment.to_ascii_lowercase(),
                    position: None,
                })
            }
        })
        .collect()
}

fn validate_mutation_path(path: &str) -> UseResult<()> {
    if path.is_empty()
        || path == "/"
        || !path.starts_with('/')
        || path.len() > 4_096
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(editor_error(
            "use.office.path_invalid",
            "Office mutation path must be absolute, bounded, and traversal-free.",
        ));
    }
    Ok(())
}

fn spreadsheet_cell_coordinates(reference: &str) -> UseResult<(u32, u32, String)> {
    let reference = reference.to_ascii_uppercase();
    let column_length = reference
        .chars()
        .take_while(|character| character.is_ascii_alphabetic())
        .count();
    if column_length == 0
        || column_length == reference.len()
        || !reference[column_length..]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return Err(editor_error(
            "use.office.spreadsheet_cell_reference_invalid",
            format!("Spreadsheet cell reference '{reference}' is invalid."),
        ));
    }
    let column = reference[..column_length]
        .bytes()
        .try_fold(0_u32, |column, byte| {
            column
                .checked_mul(26)
                .and_then(|value| value.checked_add(u32::from(byte - b'A') + 1))
        })
        .filter(|column| (1..=16_384).contains(column))
        .ok_or_else(|| {
            editor_error(
                "use.office.spreadsheet_cell_reference_invalid",
                format!("Spreadsheet cell reference '{reference}' is outside columns A:XFD."),
            )
        })?;
    let row = reference[column_length..]
        .parse::<u32>()
        .ok()
        .filter(|row| (1..=1_048_576).contains(row))
        .ok_or_else(|| {
            editor_error(
                "use.office.spreadsheet_cell_reference_invalid",
                format!("Spreadsheet cell reference '{reference}' is outside row limits."),
            )
        })?;
    Ok((
        column,
        row,
        format!("{}{row}", spreadsheet_column_name(column)),
    ))
}

fn spreadsheet_column_name(mut column: u32) -> String {
    let mut bytes = Vec::new();
    while column > 0 {
        column -= 1;
        bytes.push(b'A' + (column % 26) as u8);
        column /= 26;
    }
    bytes.reverse();
    String::from_utf8(bytes).unwrap_or_default()
}

fn prefix(qualified_name: &str) -> Option<&str> {
    qualified_name.rsplit_once(':').map(|(prefix, _)| prefix)
}

fn qualified(prefix: Option<&str>, local_name: &str) -> String {
    prefix.map_or_else(
        || local_name.to_string(),
        |prefix| format!("{prefix}:{local_name}"),
    )
}

fn preserve_space_attribute(text: &str) -> &'static str {
    if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
        " xml:space=\"preserve\""
    } else {
        ""
    }
}

fn escape_attribute(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

fn node_not_found(path: &str) -> UseError {
    editor_error(
        "use.office.node_not_found",
        format!("Office semantic path '{path}' does not exist."),
    )
    .with_detail("path", path)
}

fn editor_error(code: &str, message: impl Into<String>) -> UseError {
    office_error(code, message)
}
