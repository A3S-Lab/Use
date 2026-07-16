use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};

use crate::discovery::office_error;
use crate::semantic::NativeOfficeDocument;
use crate::{DocumentKind, NativeOfficePackage};

mod part;
mod presentation;
mod raw;
mod spreadsheet;
mod word;

#[cfg(test)]
mod part_tests;

pub use part::{NativeCreatedPart, NativeOfficePartType};
pub use raw::NativeRawXmlPart;

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
    AddPart {
        parent: String,
        #[serde(rename = "type")]
        part_type: NativeOfficePartType,
    },
    AddWorksheet {
        name: String,
    },
    InsertRows {
        sheet: String,
        start: u32,
        count: u32,
    },
    DeleteRows {
        sheet: String,
        start: u32,
        count: u32,
    },
    InsertColumns {
        sheet: String,
        start: String,
        count: u32,
    },
    DeleteColumns {
        sheet: String,
        start: String,
        count: u32,
    },
    RenameWorksheet {
        path: String,
        name: String,
    },
    MoveWorksheet {
        path: String,
        position: usize,
    },
    CopyWorksheet {
        path: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<usize>,
    },
    ReplaceXmlPart {
        part: String,
        xml: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_parts: Vec<NativeCreatedPart>,
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

    /// Safely parses and returns an existing OOXML XML part.
    pub fn raw_xml_part(&self, part: &str) -> UseResult<NativeRawXmlPart> {
        raw::inspect(&self.package, part)
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

    /// Creates a known XML part with its content type and owner relationship.
    pub fn add_part(
        &mut self,
        parent: impl Into<String>,
        part_type: NativeOfficePartType,
    ) -> UseResult<NativeCreatedPart> {
        let result = self.apply_batch(&[NativeOfficeMutation::AddPart {
            parent: parent.into(),
            part_type,
        }])?;
        result.created_parts.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office part mutation returned no creation receipt.",
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

    pub fn insert_rows(
        &mut self,
        sheet: impl Into<String>,
        start: u32,
        count: u32,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::InsertRows {
            sheet: sheet.into(),
            start,
            count,
        })
    }

    pub fn delete_rows(
        &mut self,
        sheet: impl Into<String>,
        start: u32,
        count: u32,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::DeleteRows {
            sheet: sheet.into(),
            start,
            count,
        })
    }

    pub fn insert_columns(
        &mut self,
        sheet: impl Into<String>,
        start: impl Into<String>,
        count: u32,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::InsertColumns {
            sheet: sheet.into(),
            start: start.into(),
            count,
        })
    }

    pub fn delete_columns(
        &mut self,
        sheet: impl Into<String>,
        start: impl Into<String>,
        count: u32,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::DeleteColumns {
            sheet: sheet.into(),
            start: start.into(),
            count,
        })
    }

    pub fn rename_worksheet(
        &mut self,
        path: impl Into<String>,
        name: impl Into<String>,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::RenameWorksheet {
            path: path.into(),
            name: name.into(),
        })
    }

    pub fn move_worksheet(
        &mut self,
        path: impl Into<String>,
        position: usize,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::MoveWorksheet {
            path: path.into(),
            position,
        })
    }

    pub fn copy_worksheet(
        &mut self,
        path: impl Into<String>,
        name: impl Into<String>,
        position: Option<usize>,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::CopyWorksheet {
            path: path.into(),
            name: name.into(),
            position,
        })
    }

    /// Replaces an existing, non-OPC-metadata XML part transactionally.
    pub fn replace_xml_part(
        &mut self,
        part: impl Into<String>,
        xml: impl Into<String>,
    ) -> UseResult<String> {
        self.single_path(NativeOfficeMutation::ReplaceXmlPart {
            part: part.into(),
            xml: xml.into(),
        })
    }

    fn single_path(&mut self, mutation: NativeOfficeMutation) -> UseResult<String> {
        let result = self.apply_batch(&[mutation])?;
        result.paths.into_iter().next().ok_or_else(|| {
            editor_error(
                "use.office.batch_validation_failed",
                "Native Office mutation returned no path.",
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
                created_parts: Vec::new(),
            });
        }
        let original = self.package.clone();
        let mut paths = Vec::with_capacity(mutations.len());
        let mut created_parts = Vec::new();
        for mutation in mutations {
            let mut created_part = None;
            let result = match mutation {
                NativeOfficeMutation::SetText { path, text } => {
                    set_text(&mut self.package, path, text).map(|()| path.clone())
                }
                NativeOfficeMutation::SetCellValue { path, value } => {
                    spreadsheet::set_cell_value(&mut self.package, path, value)
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
                NativeOfficeMutation::AddPart { parent, part_type } => {
                    part::add(&mut self.package, parent, *part_type).map(|created| {
                        let path = created.path.clone();
                        created_part = Some(created);
                        path
                    })
                }
                NativeOfficeMutation::AddWorksheet { name } => {
                    spreadsheet::add_worksheet(&mut self.package, name)
                }
                NativeOfficeMutation::InsertRows {
                    sheet,
                    start,
                    count,
                } => spreadsheet::insert_rows(&mut self.package, sheet, *start, *count),
                NativeOfficeMutation::DeleteRows {
                    sheet,
                    start,
                    count,
                } => spreadsheet::delete_rows(&mut self.package, sheet, *start, *count),
                NativeOfficeMutation::InsertColumns {
                    sheet,
                    start,
                    count,
                } => spreadsheet::insert_columns(&mut self.package, sheet, start, *count),
                NativeOfficeMutation::DeleteColumns {
                    sheet,
                    start,
                    count,
                } => spreadsheet::delete_columns(&mut self.package, sheet, start, *count),
                NativeOfficeMutation::RenameWorksheet { path, name } => {
                    spreadsheet::rename_worksheet(&mut self.package, path, name)
                }
                NativeOfficeMutation::MoveWorksheet { path, position } => {
                    spreadsheet::move_worksheet(&mut self.package, path, *position)
                }
                NativeOfficeMutation::CopyWorksheet {
                    path,
                    name,
                    position,
                } => spreadsheet::copy_worksheet(&mut self.package, path, name, *position),
                NativeOfficeMutation::ReplaceXmlPart { part, xml } => {
                    raw::replace(&mut self.package, part, xml)
                }
                NativeOfficeMutation::Remove { path } => {
                    remove_node(&mut self.package, path).map(|()| path.clone())
                }
            };
            match result {
                Ok(path) => {
                    paths.push(path);
                    if let Some(created) = created_part {
                        created_parts.push(created);
                    }
                }
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
            created_parts,
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
        DocumentKind::Spreadsheet => spreadsheet::set_text(package, path, text),
        DocumentKind::Presentation => presentation::set_text(package, path, text),
    }
}

fn remove_node(package: &mut NativeOfficePackage, path: &str) -> UseResult<()> {
    validate_mutation_path(path)?;
    match package.kind() {
        DocumentKind::Word => word::remove(package, path),
        DocumentKind::Spreadsheet => spreadsheet::remove(package, path),
        DocumentKind::Presentation => presentation::remove(package, path),
    }
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
