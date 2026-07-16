use a3s_use_core::{UseError, UseResult};
use a3s_use_office::{
    NativeOfficeComment, NativeOfficeCommentPosition, NativeOfficeCommentUpdate,
    NativeOfficeHorizontalAlignment, NativeOfficeHyperlink, NativeOfficeImage,
    NativeOfficeInsertPosition, NativeOfficeIssueFilter, NativeOfficeMutation,
    NativeOfficePartType, NativeOfficeRgbColor, NativeOfficeTextFormat, NativeOfficeTextMatchMode,
    NativeOfficeTextReplacement, SpreadsheetCellValue,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(super) enum OfficeView {
    Text,
    Annotated,
    Outline,
    Stats,
    Issues,
    Html,
    Svg,
    Screenshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum OfficeIssueFilter {
    Format,
    Content,
    Structure,
    MissingAltText,
    BrokenPartRef,
    FormulaNotEvaluated,
    FormulaRefMissingSheet,
    FormulaEvalError,
    LowContrast,
}

impl From<OfficeIssueFilter> for NativeOfficeIssueFilter {
    fn from(value: OfficeIssueFilter) -> Self {
        match value {
            OfficeIssueFilter::Format => Self::Format,
            OfficeIssueFilter::Content => Self::Content,
            OfficeIssueFilter::Structure => Self::Structure,
            OfficeIssueFilter::MissingAltText => Self::MissingAltText,
            OfficeIssueFilter::BrokenPartRef => Self::BrokenPartRef,
            OfficeIssueFilter::FormulaNotEvaluated => Self::FormulaNotEvaluated,
            OfficeIssueFilter::FormulaRefMissingSheet => Self::FormulaRefMissingSheet,
            OfficeIssueFilter::FormulaEvalError => Self::FormulaEvalError,
            OfficeIssueFilter::LowContrast => Self::LowContrast,
        }
    }
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeViewInput {
    /// Open native Office session ID.
    pub(super) session: String,
    /// Semantic view to produce.
    pub(super) view: OfficeView,
    /// Optional broad category or stable subtype filter for an issues view.
    pub(super) issue_type: Option<OfficeIssueFilter>,
    /// Maximum annotated entries or issues returned, from 1 through 1000. Defaults to 200.
    pub(super) limit: Option<usize>,
    /// Required no-clobber local `.png` path for a screenshot view.
    pub(super) output: Option<String>,
    /// Screenshot deadline in milliseconds; defaults to 30000 and cannot exceed 120000.
    pub(super) timeout_ms: Option<u64>,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(super) enum OfficeHorizontalAlignment {
    Left,
    Center,
    Right,
    Justify,
}

impl From<OfficeHorizontalAlignment> for NativeOfficeHorizontalAlignment {
    fn from(value: OfficeHorizontalAlignment) -> Self {
        match value {
            OfficeHorizontalAlignment::Left => Self::Left,
            OfficeHorizontalAlignment::Center => Self::Center,
            OfficeHorizontalAlignment::Right => Self::Right,
            OfficeHorizontalAlignment::Justify => Self::Justify,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeRgbColor {
    /// Red channel from 0 through 255.
    red: u8,
    /// Green channel from 0 through 255.
    green: u8,
    /// Blue channel from 0 through 255.
    blue: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeTextFormat {
    /// Explicitly enable or disable bold text.
    bold: Option<bool>,
    /// Explicitly enable or disable italic text.
    italic: Option<bool>,
    /// Font family applied to the supported script slots.
    font_family: Option<String>,
    /// Exact font size in centipoints (1/100 point), from 100 through 40000.
    font_size_centipoints: Option<u32>,
    /// Exact 24-bit RGB text color.
    text_color: Option<OfficeRgbColor>,
    /// Paragraph or cell horizontal alignment.
    alignment: Option<OfficeHorizontalAlignment>,
}

impl From<OfficeTextFormat> for NativeOfficeTextFormat {
    fn from(value: OfficeTextFormat) -> Self {
        Self {
            bold: value.bold,
            italic: value.italic,
            font_family: value.font_family,
            font_size_centipoints: value.font_size_centipoints,
            text_color: value
                .text_color
                .map(|color| NativeOfficeRgbColor::new(color.red, color.green, color.blue)),
            alignment: value.alignment.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(super) enum OfficeTextMatchMode {
    /// Case-sensitive, non-overlapping substring matching.
    Literal,
    /// Linear-time Rust regular expression matching with capture expansion.
    Regex,
}

impl From<OfficeTextMatchMode> for NativeOfficeTextMatchMode {
    fn from(value: OfficeTextMatchMode) -> Self {
        match value {
            OfficeTextMatchMode::Literal => Self::Literal,
            OfficeTextMatchMode::Regex => Self::Regex,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeTextReplacement {
    /// Required literal substring or Rust regular expression.
    find: String,
    /// Literal replacement for literal mode, or capture-aware replacement for regex mode.
    replace: String,
    /// Explicit match mode. Literal mode is recommended for untrusted input.
    mode: OfficeTextMatchMode,
}

impl OfficeTextReplacement {
    fn into_native(self) -> UseResult<NativeOfficeTextReplacement> {
        NativeOfficeTextReplacement::new(self.find, self.replace, self.mode.into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum OfficeHyperlinkTarget {
    /// Inert absolute HTTP, HTTPS, or mailto URI.
    External { uri: String },
    /// Format-specific in-document location. Presentation slide jumps are not yet supported.
    Internal { location: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeHyperlink {
    pub(super) target: OfficeHyperlinkTarget,
    /// Optional Word or Spreadsheet display text. Presentation shapes use their existing text.
    pub(super) display: Option<String>,
    /// Optional hover tooltip.
    pub(super) tooltip: Option<String>,
}

impl OfficeHyperlink {
    fn into_native(self) -> UseResult<NativeOfficeHyperlink> {
        let mut hyperlink = match self.target {
            OfficeHyperlinkTarget::External { uri } => NativeOfficeHyperlink::external(uri)?,
            OfficeHyperlinkTarget::Internal { location } => {
                NativeOfficeHyperlink::internal(location)?
            }
        };
        if let Some(display) = self.display {
            hyperlink = hyperlink.with_display(display);
        }
        if let Some(tooltip) = self.tooltip {
            hyperlink = hyperlink.with_tooltip(tooltip);
        }
        Ok(hyperlink)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeCommentPosition {
    /// Horizontal slide coordinate in English Metric Units.
    x_emu: i32,
    /// Vertical slide coordinate in English Metric Units.
    y_emu: i32,
}

impl From<OfficeCommentPosition> for NativeOfficeCommentPosition {
    fn from(value: OfficeCommentPosition) -> Self {
        Self::new(value.x_emu, value.y_emu)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeComment {
    /// Required accountable author name.
    author: String,
    /// Required plain comment body.
    text: String,
    /// Optional Word or Presentation author initials. Spreadsheet rejects this field.
    initials: Option<String>,
    /// Optional legacy Presentation slide position. Word and Spreadsheet reject this field.
    position: Option<OfficeCommentPosition>,
}

impl OfficeComment {
    fn into_native(self) -> UseResult<NativeOfficeComment> {
        let mut comment = NativeOfficeComment::new(self.author, self.text)?;
        if let Some(initials) = self.initials {
            comment = comment.with_initials(initials);
        }
        if let Some(position) = self.position {
            comment = comment.with_position(position.into());
        }
        Ok(comment)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OfficeCommentUpdate {
    /// Replacement author name. Omit to preserve the current author.
    author: Option<String>,
    /// Replacement plain comment body. Omit to preserve the current body.
    text: Option<String>,
    /// Replacement Word or Presentation initials.
    initials: Option<String>,
    /// Replacement legacy Presentation slide position.
    position: Option<OfficeCommentPosition>,
}

impl From<OfficeCommentUpdate> for NativeOfficeCommentUpdate {
    fn from(value: OfficeCommentUpdate) -> Self {
        Self {
            author: value.author,
            text: value.text,
            initials: value.initials,
            position: value.position.map(Into::into),
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
    ReplaceText {
        /// Root, container, paragraph/run, worksheet/cell/range, slide/shape, or notes scope.
        path: String,
        replacement: OfficeTextReplacement,
    },
    SetText {
        path: String,
        text: String,
    },
    SetTextFormat {
        path: String,
        format: OfficeTextFormat,
    },
    SetHyperlink {
        path: String,
        hyperlink: OfficeHyperlink,
    },
    SetComment {
        path: String,
        update: OfficeCommentUpdate,
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
    AddComment {
        parent: String,
        comment: OfficeComment,
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
            Self::ReplaceText { path, replacement } => NativeOfficeMutation::ReplaceText {
                path,
                replacement: replacement.into_native()?,
            },
            Self::SetText { path, text } => NativeOfficeMutation::SetText { path, text },
            Self::SetTextFormat { path, format } => NativeOfficeMutation::SetTextFormat {
                path,
                format: format.into(),
            },
            Self::SetHyperlink { path, hyperlink } => NativeOfficeMutation::SetHyperlink {
                path,
                hyperlink: hyperlink.into_native()?,
            },
            Self::SetComment { path, update } => NativeOfficeMutation::SetComment {
                path,
                update: update.into(),
            },
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
            Self::AddComment { parent, comment } => NativeOfficeMutation::AddComment {
                parent,
                comment: comment.into_native()?,
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
