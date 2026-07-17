use a3s_use_core::UseResult;

use crate::{
    DocumentNode, NativeSpreadsheetTable, NativeSpreadsheetTableColumn,
    NativeSpreadsheetTableStyle, OfficeNodeType,
};

impl NativeSpreadsheetTable {
    /// Reconstructs one complete typed table value from a mutable semantic
    /// Spreadsheet table node.
    pub fn from_semantic_node(node: &DocumentNode) -> UseResult<Self> {
        if node.node_type != OfficeNodeType::Table
            || node.tag != "table"
            || node.format.get("nativeMutable").map(String::as_str) != Some("true")
        {
            return Err(semantic_table_error(
                node,
                "is not a natively mutable Spreadsheet table node",
            ));
        }
        let name = required(node, "name")?.to_string();
        let display_name = required(node, "displayName")?;
        let range = required(node, "ref")?.to_string();
        let header_row = boolean(node, "headerRow")?;
        let totals_row = boolean(node, "totalsRow")?;
        let show_first_column = boolean(node, "showFirstColumn")?;
        let show_last_column = boolean(node, "showLastColumn")?;
        let show_row_stripes = boolean(node, "showRowStripes")?;
        let show_column_stripes = boolean(node, "showColumnStripes")?;
        let style = match required(node, "styleFamily")? {
            "none" => NativeSpreadsheetTableStyle::None,
            "light" => NativeSpreadsheetTableStyle::Light {
                number: style_number(node)?,
            },
            "medium" => NativeSpreadsheetTableStyle::Medium {
                number: style_number(node)?,
            },
            "dark" => NativeSpreadsheetTableStyle::Dark {
                number: style_number(node)?,
            },
            family => {
                return Err(semantic_table_error(
                    node,
                    format!("has unsupported style family '{family}'"),
                ))
            }
        };
        let columns = node
            .children
            .iter()
            .map(|column| {
                if column.node_type != OfficeNodeType::TableColumn
                    || column.tag != "column"
                    || !column.children.is_empty()
                {
                    return Err(semantic_table_error(
                        node,
                        "contains an unsupported table-column node",
                    ));
                }
                Ok(NativeSpreadsheetTableColumn {
                    name: required(column, "name")?.to_string(),
                })
            })
            .collect::<UseResult<Vec<_>>>()?;
        let table = Self {
            name: name.clone(),
            display_name: (display_name != name).then(|| display_name.to_string()),
            range,
            columns,
            header_row,
            totals_row,
            style,
            show_first_column,
            show_last_column,
            show_row_stripes,
            show_column_stripes,
        };
        table.validate()?;
        Ok(table)
    }
}

fn required<'a>(node: &'a DocumentNode, key: &str) -> UseResult<&'a str> {
    node.format
        .get(key)
        .map(String::as_str)
        .ok_or_else(|| semantic_table_error(node, format!("has no '{key}' property")))
}

fn boolean(node: &DocumentNode, key: &str) -> UseResult<bool> {
    match required(node, key)? {
        "true" => Ok(true),
        "false" => Ok(false),
        value => Err(semantic_table_error(
            node,
            format!("has non-boolean '{key}' value '{value}'"),
        )),
    }
}

fn style_number(node: &DocumentNode) -> UseResult<u8> {
    required(node, "styleNumber")?
        .parse::<u8>()
        .map_err(|error| semantic_table_error(node, format!("has invalid style number: {error}")))
}

fn semantic_table_error(node: &DocumentNode, reason: impl Into<String>) -> a3s_use_core::UseError {
    crate::discovery::office_error(
        "use.office.spreadsheet_table_semantic_invalid",
        format!("Spreadsheet table node '{}' {}.", node.path, reason.into()),
    )
    .with_detail("path", node.path.clone())
}
