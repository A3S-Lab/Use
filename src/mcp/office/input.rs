use a3s_use_core::{UseError, UseResult};
use a3s_use_office::{
    NativeOfficeImage, NativeOfficeInsertPosition, NativeOfficeMutation, NativeOfficePartType,
    SpreadsheetCellValue,
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeFileInput {
    /// Local `.docx`, `.xlsx`, or `.pptx` path.
    pub(super) file: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeCreateInput {
    /// Stable session ID containing only ASCII letters, digits, `-`, and `_`.
    pub(super) session: String,
    /// New local `.docx`, `.xlsx`, or `.pptx` path. Existing entries are not replaced.
    pub(super) file: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeOpenInput {
    /// Stable session ID containing only ASCII letters, digits, `-`, and `_`.
    pub(super) session: String,
    /// Existing local `.docx`, `.xlsx`, or `.pptx` path.
    pub(super) file: String,
    /// Reject mutations and saves while still allowing semantic reads.
    #[serde(default)]
    pub(super) read_only: bool,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeGetInput {
    /// Open native Office session ID.
    pub(super) session: String,
    /// Stable one-based semantic path. Defaults to `/`.
    pub(super) path: Option<String>,
    /// Child depth to include, from 0 through 64. Defaults to 1.
    pub(super) depth: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeQueryInput {
    /// Open native Office session ID.
    pub(super) session: String,
    /// Native Office semantic selector.
    pub(super) selector: String,
    /// Maximum matches returned, from 1 through 1000. Defaults to 200.
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(super) enum OfficeView {
    Text,
    Outline,
    Stats,
    Html,
    Svg,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeViewInput {
    /// Open native Office session ID.
    pub(super) session: String,
    /// Semantic view to produce.
    pub(super) view: OfficeView,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeRawXmlInput {
    /// Open native Office session ID.
    pub(super) session: String,
    /// Existing OOXML XML part URI, such as `/word/document.xml`.
    pub(super) part: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeBatchInput {
    /// Open mutable native Office session ID.
    pub(super) session: String,
    /// Ordered mutations applied atomically in memory. Call `office_save` to persist them.
    pub(super) mutations: Vec<OfficeMutation>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeSaveInput {
    /// Open mutable native Office session ID.
    pub(super) session: String,
    /// Optional save-as destination. Omit to save the session's current file.
    pub(super) output: Option<String>,
    /// Allow an explicit save-as destination to replace an existing file.
    #[serde(default)]
    pub(super) overwrite: bool,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeCloseInput {
    /// Open native Office session ID.
    pub(super) session: String,
    /// Explicitly discard unsaved in-memory mutations.
    #[serde(default)]
    pub(super) discard: bool,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeMergeTemplateInput {
    /// Open session whose document is used as the immutable template.
    pub(super) session: String,
    /// Distinct `.docx`, `.xlsx`, or `.pptx` destination.
    pub(super) output: String,
    /// Object containing `{{key}}` replacement values.
    pub(super) data: serde_json::Value,
    /// Allow the distinct output path to replace an existing file.
    #[serde(default)]
    pub(super) overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum OfficeCellValue {
    Text { value: String },
    Number { value: String },
    Boolean { value: bool },
    Formula { expression: String },
}

impl From<OfficeCellValue> for SpreadsheetCellValue {
    fn from(value: OfficeCellValue) -> Self {
        match value {
            OfficeCellValue::Text { value } => Self::Text { value },
            OfficeCellValue::Number { value } => Self::Number { value },
            OfficeCellValue::Boolean { value } => Self::Boolean { value },
            OfficeCellValue::Formula { expression } => Self::Formula { expression },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum OfficeInsertPosition {
    Index { index: usize },
    Before { path: String },
    After { path: String },
}

impl From<OfficeInsertPosition> for NativeOfficeInsertPosition {
    fn from(value: OfficeInsertPosition) -> Self {
        match value {
            OfficeInsertPosition::Index { index } => Self::Index { index },
            OfficeInsertPosition::Before { path } => Self::Before { path },
            OfficeInsertPosition::After { path } => Self::After { path },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(super) enum OfficePartType {
    Chart,
    Header,
    Footer,
}

impl From<OfficePartType> for NativeOfficePartType {
    fn from(value: OfficePartType) -> Self {
        match value {
            OfficePartType::Chart => Self::Chart,
            OfficePartType::Header => Self::Header,
            OfficePartType::Footer => Self::Footer,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeImage {
    /// Base64-encoded PNG, JPEG, or GIF bytes. The format is detected and validated.
    pub(super) data_base64: String,
    pub(super) name: Option<String>,
    pub(super) alt_text: Option<String>,
    pub(super) width_px: Option<u32>,
    pub(super) height_px: Option<u32>,
}

impl OfficeImage {
    fn into_native(self) -> UseResult<NativeOfficeImage> {
        let max_encoded = MAX_IMAGE_BYTES.saturating_add(2) / 3 * 4 + 4;
        if self.data_base64.len() > max_encoded {
            return Err(UseError::new(
                "use.office.image_input_too_large",
                format!(
                    "Native Office MCP image data exceeds the {MAX_IMAGE_BYTES}-byte decoded limit."
                ),
            ));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.data_base64)
            .map_err(|error| {
                UseError::new(
                    "use.office.image_input_invalid",
                    format!("Native Office MCP image data is not valid base64: {error}"),
                )
            })?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(UseError::new(
                "use.office.image_input_too_large",
                format!(
                    "Native Office MCP image data exceeds the {MAX_IMAGE_BYTES}-byte decoded limit."
                ),
            ));
        }
        let mut image = NativeOfficeImage::from_bytes(bytes)?;
        if let Some(name) = self.name {
            image = image.with_name(name);
        }
        if let Some(alt_text) = self.alt_text {
            image = image.with_alt_text(alt_text);
        }
        if let Some(width_px) = self.width_px {
            image = image.with_width_px(width_px);
        }
        if let Some(height_px) = self.height_px {
            image = image.with_height_px(height_px);
        }
        Ok(image)
    }
}

/// MCP boundary model for the native editor's currently implemented mutations.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum OfficeMutation {
    SetText {
        path: String,
        text: String,
    },
    SetTableColumnWidth {
        path: String,
        #[serde(rename = "widthEmu")]
        width_emu: u64,
    },
    SetCellValue {
        path: String,
        value: OfficeCellValue,
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
        columns: Option<usize>,
    },
    AddTableColumn {
        parent: String,
        index: Option<usize>,
        #[serde(default)]
        text: String,
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
    AddImage {
        parent: String,
        image: OfficeImage,
    },
    AddPart {
        parent: String,
        #[serde(rename = "type")]
        part_type: OfficePartType,
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
        position: Option<usize>,
    },
    Move {
        path: String,
        #[serde(rename = "to")]
        target_parent: Option<String>,
        position: Option<OfficeInsertPosition>,
    },
    Copy {
        path: String,
        #[serde(rename = "to")]
        target_parent: Option<String>,
        position: Option<OfficeInsertPosition>,
        name: Option<String>,
    },
    Swap {
        path: String,
        with: String,
    },
    ReplaceXmlPart {
        part: String,
        xml: String,
    },
    Remove {
        path: String,
    },
}

impl OfficeMutation {
    pub(super) fn into_native(self) -> UseResult<NativeOfficeMutation> {
        Ok(match self {
            Self::SetText { path, text } => NativeOfficeMutation::SetText { path, text },
            Self::SetTableColumnWidth { path, width_emu } => {
                NativeOfficeMutation::SetTableColumnWidth { path, width_emu }
            }
            Self::SetCellValue { path, value } => NativeOfficeMutation::SetCellValue {
                path,
                value: value.into(),
            },
            Self::AddParagraph { parent, text } => {
                NativeOfficeMutation::AddParagraph { parent, text }
            }
            Self::AddTable {
                parent,
                rows,
                columns,
            } => NativeOfficeMutation::AddTable {
                parent,
                rows,
                columns,
            },
            Self::AddTableRow { parent, columns } => {
                NativeOfficeMutation::AddTableRow { parent, columns }
            }
            Self::AddTableColumn {
                parent,
                index,
                text,
            } => NativeOfficeMutation::AddTableColumn {
                parent,
                index,
                text,
            },
            Self::AddTableCell { parent, text } => {
                NativeOfficeMutation::AddTableCell { parent, text }
            }
            Self::AddSlide { parent, title } => NativeOfficeMutation::AddSlide { parent, title },
            Self::AddShape { parent, text } => NativeOfficeMutation::AddShape { parent, text },
            Self::AddImage { parent, image } => NativeOfficeMutation::AddImage {
                parent,
                image: image.into_native()?,
            },
            Self::AddPart { parent, part_type } => NativeOfficeMutation::AddPart {
                parent,
                part_type: part_type.into(),
            },
            Self::AddWorksheet { name } => NativeOfficeMutation::AddWorksheet { name },
            Self::InsertRows {
                sheet,
                start,
                count,
            } => NativeOfficeMutation::InsertRows {
                sheet,
                start,
                count,
            },
            Self::DeleteRows {
                sheet,
                start,
                count,
            } => NativeOfficeMutation::DeleteRows {
                sheet,
                start,
                count,
            },
            Self::InsertColumns {
                sheet,
                start,
                count,
            } => NativeOfficeMutation::InsertColumns {
                sheet,
                start,
                count,
            },
            Self::DeleteColumns {
                sheet,
                start,
                count,
            } => NativeOfficeMutation::DeleteColumns {
                sheet,
                start,
                count,
            },
            Self::RenameWorksheet { path, name } => {
                NativeOfficeMutation::RenameWorksheet { path, name }
            }
            Self::MoveWorksheet { path, position } => {
                NativeOfficeMutation::MoveWorksheet { path, position }
            }
            Self::CopyWorksheet {
                path,
                name,
                position,
            } => NativeOfficeMutation::CopyWorksheet {
                path,
                name,
                position,
            },
            Self::Move {
                path,
                target_parent,
                position,
            } => NativeOfficeMutation::Move {
                path,
                target_parent,
                position: position.map(Into::into),
            },
            Self::Copy {
                path,
                target_parent,
                position,
                name,
            } => NativeOfficeMutation::Copy {
                path,
                target_parent,
                position: position.map(Into::into),
                name,
            },
            Self::Swap { path, with } => NativeOfficeMutation::Swap { path, with },
            Self::ReplaceXmlPart { part, xml } => {
                NativeOfficeMutation::ReplaceXmlPart { part, xml }
            }
            Self::Remove { path } => NativeOfficeMutation::Remove { path },
        })
    }
}
