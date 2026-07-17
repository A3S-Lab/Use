use a3s_use_office::{
    NativeOfficeRgbColor, NativeSpreadsheetCellFormat, NativeSpreadsheetFill,
    NativeSpreadsheetReadingOrder, NativeSpreadsheetVerticalAlignment,
};
use serde::{Deserialize, Serialize};

use super::OfficeRgbColor;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum OfficeCellFill {
    /// Explicitly remove the cell fill.
    None,
    /// Apply a solid 24-bit RGB fill.
    Solid { color: OfficeRgbColor },
}

impl From<OfficeCellFill> for NativeSpreadsheetFill {
    fn from(value: OfficeCellFill) -> Self {
        match value {
            OfficeCellFill::None => Self::None,
            OfficeCellFill::Solid { color } => Self::Solid {
                color: NativeOfficeRgbColor::new(color.red, color.green, color.blue),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum OfficeCellVerticalAlignment {
    Top,
    Center,
    Bottom,
    Justify,
    Distributed,
}

impl From<OfficeCellVerticalAlignment> for NativeSpreadsheetVerticalAlignment {
    fn from(value: OfficeCellVerticalAlignment) -> Self {
        match value {
            OfficeCellVerticalAlignment::Top => Self::Top,
            OfficeCellVerticalAlignment::Center => Self::Center,
            OfficeCellVerticalAlignment::Bottom => Self::Bottom,
            OfficeCellVerticalAlignment::Justify => Self::Justify,
            OfficeCellVerticalAlignment::Distributed => Self::Distributed,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum OfficeCellReadingOrder {
    Context,
    LeftToRight,
    RightToLeft,
}

impl From<OfficeCellReadingOrder> for NativeSpreadsheetReadingOrder {
    fn from(value: OfficeCellReadingOrder) -> Self {
        match value {
            OfficeCellReadingOrder::Context => Self::Context,
            OfficeCellReadingOrder::LeftToRight => Self::LeftToRight,
            OfficeCellReadingOrder::RightToLeft => Self::RightToLeft,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::mcp::office) struct OfficeCellFormat {
    /// Built-in alias or explicit Excel number format code, up to 255 characters.
    number_format: Option<String>,
    /// Solid RGB fill or an explicit fill clear.
    fill: Option<OfficeCellFill>,
    /// Vertical cell alignment.
    vertical_alignment: Option<OfficeCellVerticalAlignment>,
    /// Explicitly enable or disable wrapped text.
    wrap_text: Option<bool>,
    /// Text rotation from 0 through 180, or 255 for stacked vertical text.
    text_rotation: Option<u16>,
    /// Alignment indentation from 0 through 255.
    indent: Option<u8>,
    /// Explicitly enable or disable shrink-to-fit.
    shrink_to_fit: Option<bool>,
    /// Contextual, left-to-right, or right-to-left cell reading order.
    reading_order: Option<OfficeCellReadingOrder>,
}

impl From<OfficeCellFormat> for NativeSpreadsheetCellFormat {
    fn from(value: OfficeCellFormat) -> Self {
        Self {
            number_format: value.number_format,
            fill: value.fill.map(Into::into),
            vertical_alignment: value.vertical_alignment.map(Into::into),
            wrap_text: value.wrap_text,
            text_rotation: value.text_rotation,
            indent: value.indent,
            shrink_to_fit: value.shrink_to_fit,
            reading_order: value.reading_order.map(Into::into),
        }
    }
}
