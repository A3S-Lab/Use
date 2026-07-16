use a3s_use_core::UseResult;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use url::Url;

use super::part::{NativeCreatedPart, NativeOfficePartType};

/// Horizontal text alignment supported consistently by native Office formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeOfficeHorizontalAlignment {
    Left,
    Center,
    Right,
    Justify,
}

/// An exact 24-bit RGB text color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeOfficeRgbColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl NativeOfficeRgbColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub(crate) fn hex(self) -> String {
        format!("{:02X}{:02X}{:02X}", self.red, self.green, self.blue)
    }
}

/// Typed text formatting shared by Word runs, Spreadsheet cells, and
/// Presentation runs.
///
/// Font sizes use integer centipoints (1/100 point), which is DrawingML's
/// native unit and avoids floating-point serialization. Word supports only
/// half-point increments and rejects values it cannot represent exactly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeOfficeTextFormat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size_centipoints: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_color: Option<NativeOfficeRgbColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<NativeOfficeHorizontalAlignment>,
}

impl NativeOfficeTextFormat {
    pub fn is_empty(&self) -> bool {
        self.bold.is_none()
            && self.italic.is_none()
            && self.font_family.is_none()
            && self.font_size_centipoints.is_none()
            && self.text_color.is_none()
            && self.alignment.is_none()
    }

    pub(crate) fn validate(&self) -> UseResult<()> {
        if self.is_empty() {
            return Err(super::editor_error(
                "use.office.text_format_empty",
                "Native Office text formatting requires at least one typed property.",
            ));
        }
        if let Some(family) = &self.font_family {
            if family.is_empty()
                || family.len() > 255
                || family.trim() != family
                || family.chars().any(char::is_control)
            {
                return Err(super::editor_error(
                    "use.office.font_family_invalid",
                    "Native Office font families must contain 1-255 non-control UTF-8 bytes without surrounding whitespace.",
                ));
            }
        }
        if let Some(size) = self.font_size_centipoints {
            if !(100..=40_000).contains(&size) {
                return Err(super::editor_error(
                    "use.office.font_size_invalid",
                    "Native Office font sizes must be from 100 through 40000 centipoints (1-400 points).",
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn has_character_properties(&self) -> bool {
        self.bold.is_some()
            || self.italic.is_some()
            || self.font_family.is_some()
            || self.font_size_centipoints.is_some()
            || self.text_color.is_some()
    }
}

/// A hyperlink destination represented without executing or resolving it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NativeOfficeHyperlinkTarget {
    /// An inert absolute HTTP, HTTPS, or mail address stored as an external
    /// OOXML relationship.
    External { uri: String },
    /// A format-specific in-document location such as a Word bookmark or
    /// Spreadsheet cell location.
    Internal { location: String },
}

/// A complete typed hyperlink value shared by native Office formats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeOfficeHyperlink {
    pub target: NativeOfficeHyperlinkTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
}

impl NativeOfficeHyperlink {
    /// Creates a validated inert external hyperlink.
    pub fn external(uri: impl Into<String>) -> UseResult<Self> {
        let hyperlink = Self {
            target: NativeOfficeHyperlinkTarget::External { uri: uri.into() },
            display: None,
            tooltip: None,
        };
        hyperlink.validate()?;
        Ok(hyperlink)
    }

    /// Creates a validated format-specific internal hyperlink target.
    pub fn internal(location: impl Into<String>) -> UseResult<Self> {
        let hyperlink = Self {
            target: NativeOfficeHyperlinkTarget::Internal {
                location: location.into(),
            },
            display: None,
            tooltip: None,
        };
        hyperlink.validate()?;
        Ok(hyperlink)
    }

    pub fn with_display(mut self, display: impl Into<String>) -> Self {
        self.display = Some(display.into());
        self
    }

    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub(crate) fn validate(&self) -> UseResult<()> {
        match &self.target {
            NativeOfficeHyperlinkTarget::External { uri } => validate_external_uri(uri)?,
            NativeOfficeHyperlinkTarget::Internal { location } => {
                validate_hyperlink_text(
                    location,
                    2_048,
                    "use.office.hyperlink_location_invalid",
                    "Native Office internal hyperlink locations",
                )?;
            }
        }
        if let Some(display) = &self.display {
            validate_hyperlink_text(
                display,
                32_768,
                "use.office.hyperlink_display_invalid",
                "Native Office hyperlink display text",
            )?;
        }
        if let Some(tooltip) = &self.tooltip {
            validate_hyperlink_text(
                tooltip,
                4_096,
                "use.office.hyperlink_tooltip_invalid",
                "Native Office hyperlink tooltips",
            )?;
        }
        Ok(())
    }

    pub(crate) fn default_display(&self) -> &str {
        match &self.target {
            NativeOfficeHyperlinkTarget::External { uri } => uri,
            NativeOfficeHyperlinkTarget::Internal { location } => location,
        }
    }
}

fn validate_external_uri(uri: &str) -> UseResult<()> {
    validate_hyperlink_text(
        uri,
        2_048,
        "use.office.hyperlink_uri_invalid",
        "Native Office external hyperlink URIs",
    )?;
    let parsed = Url::parse(uri).map_err(|error| {
        super::editor_error(
            "use.office.hyperlink_uri_invalid",
            format!("Native Office external hyperlink URI is invalid: {error}"),
        )
    })?;
    let valid = match parsed.scheme() {
        "http" | "https" => {
            parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
        }
        "mailto" => !parsed.path().is_empty(),
        _ => false,
    };
    if !valid {
        return Err(super::editor_error(
            "use.office.hyperlink_uri_invalid",
            "Native Office external hyperlinks require an absolute HTTP, HTTPS, or mailto URI without embedded credentials.",
        ));
    }
    Ok(())
}

fn validate_hyperlink_text(
    value: &str,
    max_bytes: usize,
    code: &str,
    label: &str,
) -> UseResult<()> {
    if value.is_empty()
        || value.len() > max_bytes
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(super::editor_error(
            code,
            format!(
                "{label} must contain 1-{max_bytes} non-control UTF-8 bytes without surrounding whitespace."
            ),
        ));
    }
    Ok(())
}

/// Typed Spreadsheet cell content written without a shared-string dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SpreadsheetCellValue {
    Text { value: String },
    Number { value: String },
    Boolean { value: bool },
    Formula { expression: String },
}

/// Zero-based insertion selector shared by native move and copy operations.
///
/// `Index` is evaluated after removing the source for a move. `Before` and
/// `After` use stable semantic paths and are resolved before the mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NativeOfficeInsertPosition {
    Index { index: usize },
    Before { path: String },
    After { path: String },
}

impl NativeOfficeInsertPosition {
    pub fn at_index(index: usize) -> Self {
        Self::Index { index }
    }

    pub fn before(path: impl Into<String>) -> Self {
        Self::Before { path: path.into() }
    }

    pub fn after(path: impl Into<String>) -> Self {
        Self::After { path: path.into() }
    }
}

/// Raster formats that can be embedded without an external Office runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeOfficeImageFormat {
    Png,
    #[serde(alias = "jpg")]
    Jpeg,
    Gif,
}

/// Validated dimensions and format of a native raster image byte slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeOfficeImageMetadata {
    pub format: NativeOfficeImageFormat,
    pub width_px: u32,
    pub height_px: u32,
}

impl NativeOfficeImageFormat {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Gif => "gif",
        }
    }

    pub(crate) fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
        }
    }
}

/// A bounded, base64-serializable raster image for a native Office mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeOfficeImage {
    pub(super) format: NativeOfficeImageFormat,
    pub(super) data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) alt_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) width_px: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) height_px: Option<u32>,
}

impl NativeOfficeImage {
    /// Validates PNG, JPEG, or GIF bytes without base64-encoding or mutating them.
    pub fn inspect_bytes(bytes: impl AsRef<[u8]>) -> UseResult<NativeOfficeImageMetadata> {
        let metadata = super::image::inspect_image(bytes.as_ref(), None)?;
        Ok(NativeOfficeImageMetadata {
            format: metadata.format,
            width_px: metadata.width_px,
            height_px: metadata.height_px,
        })
    }

    /// Detects and validates PNG, JPEG, or GIF bytes before serializing them.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> UseResult<Self> {
        let bytes = bytes.as_ref();
        let metadata = Self::inspect_bytes(bytes)?;
        Ok(Self {
            format: metadata.format,
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
            name: None,
            alt_text: None,
            width_px: None,
            height_px: None,
        })
    }

    pub fn format(&self) -> NativeOfficeImageFormat {
        self.format
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn with_alt_text(mut self, alt_text: impl Into<String>) -> Self {
        self.alt_text = Some(alt_text.into());
        self
    }

    pub fn with_width_px(mut self, width_px: u32) -> Self {
        self.width_px = Some(width_px);
        self
    }

    pub fn with_height_px(mut self, height_px: u32) -> Self {
        self.height_px = Some(height_px);
        self
    }
}

/// Receipt for an image embedded into a semantic Office document location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeCreatedImage {
    pub path: String,
    pub part: String,
    pub parent: String,
    pub owner_part: String,
    pub relationship_id: String,
    pub format: NativeOfficeImageFormat,
    pub width_px: u32,
    pub height_px: u32,
}

/// Typed in-process mutation supported by an atomic native batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NativeOfficeMutation {
    SetText {
        path: String,
        text: String,
    },
    SetTextFormat {
        path: String,
        format: NativeOfficeTextFormat,
    },
    SetHyperlink {
        path: String,
        hyperlink: NativeOfficeHyperlink,
    },
    SetTableColumnWidth {
        path: String,
        #[serde(rename = "widthEmu")]
        width_emu: u64,
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
    AddTableColumn {
        parent: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
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
        image: NativeOfficeImage,
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
    Move {
        path: String,
        #[serde(rename = "to", default, skip_serializing_if = "Option::is_none")]
        target_parent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<NativeOfficeInsertPosition>,
    },
    Copy {
        path: String,
        #[serde(rename = "to", default, skip_serializing_if = "Option::is_none")]
        target_parent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<NativeOfficeInsertPosition>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeOfficeSwapResult {
    pub first: String,
    pub second: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeBatchResult {
    pub applied: usize,
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swaps: Vec<NativeOfficeSwapResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_parts: Vec<NativeCreatedPart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_images: Vec<NativeCreatedImage>,
}
