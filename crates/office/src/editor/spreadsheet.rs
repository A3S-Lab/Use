use a3s_use_core::UseResult;

use super::{editor_error, escape_attribute, prefix, qualified};
use crate::semantic::{NativeOfficeDocument, OfficeNodeType};
use crate::xml_edit::{index_xml, insert_child};
use crate::{DocumentKind, NativeOfficePackage};

pub(super) fn add_worksheet(package: &mut NativeOfficePackage, name: &str) -> UseResult<String> {
    if package.kind() != DocumentKind::Spreadsheet {
        return Err(editor_error(
            "use.office.mutation_type_unsupported",
            "Native add-worksheet is available only for Spreadsheet documents.",
        ));
    }
    validate_worksheet_name(name)?;
    let snapshot = NativeOfficeDocument::from_package(package.clone())?;
    if snapshot.root().children.iter().any(|sheet| {
        sheet.node_type == OfficeNodeType::Worksheet && sheet.path[1..].eq_ignore_ascii_case(name)
    }) {
        return Err(editor_error(
            "use.office.spreadsheet_sheet_exists",
            format!("Spreadsheet already contains a worksheet named '{name}'."),
        ));
    }
    let number = (1..=package.limits().max_entries.saturating_add(1))
        .find(|number| !package.contains_part(&format!("xl/worksheets/sheet{number}.xml")))
        .ok_or_else(|| {
            editor_error(
                "use.office.spreadsheet_sheet_limit",
                "Spreadsheet has no available native worksheet part number.",
            )
        })?;
    let sheet_part = format!("xl/worksheets/sheet{number}.xml");
    let workbook = package.xml_part("xl/workbook.xml")?;
    let index = index_xml(&workbook)?;
    let sheets = index.child("sheets", 1).ok_or_else(|| {
        editor_error(
            "use.office.spreadsheet_sheets_missing",
            "Spreadsheet workbook has no sheets collection.",
        )
    })?;
    let sheet_id = sheets
        .children
        .iter()
        .filter(|child| child.local_name == "sheet")
        .filter_map(|child| child.qualified_attributes.get("sheetId"))
        .filter_map(|id| id.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| {
            editor_error(
                "use.office.spreadsheet_sheet_limit",
                "Spreadsheet worksheet IDs are exhausted.",
            )
        })?;

    crate::opc_edit::add_content_type_override(
        package,
        &sheet_part,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml",
    )?;
    package.set_part(&sheet_part, blank_worksheet_xml().as_bytes().to_vec())?;
    let relationship_id = crate::opc_edit::add_relationship(
        package,
        "xl/_rels/workbook.xml.rels",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet",
        &format!("worksheets/sheet{number}.xml"),
    )?;
    let tag = qualified(prefix(&sheets.qualified_name), "sheet");
    let fragment = format!(
        "<{tag} xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" name=\"{}\" sheetId=\"{sheet_id}\" r:id=\"{}\"/>",
        escape_attribute(name),
        escape_attribute(&relationship_id)
    );
    let edited = insert_child(&workbook, sheets, fragment)?;
    package.set_part("xl/workbook.xml", edited)?;
    Ok(format!("/{name}"))
}

pub(super) fn remove_worksheet(package: &mut NativeOfficePackage, path: &str) -> UseResult<()> {
    if package.kind() != DocumentKind::Spreadsheet {
        return Err(editor_error(
            "use.office.mutation_type_unsupported",
            "Native worksheet removal is available only for Spreadsheet documents.",
        ));
    }
    let requested_name = path
        .strip_prefix('/')
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            editor_error(
                "use.office.mutation_path_unsupported",
                "Native worksheet removal requires a path such as /Sheet2.",
            )
        })?;
    let snapshot = NativeOfficeDocument::from_package(package.clone())?;
    let worksheets = snapshot
        .root()
        .children
        .iter()
        .filter(|node| node.node_type == OfficeNodeType::Worksheet)
        .collect::<Vec<_>>();
    if worksheets.len() <= 1 {
        return Err(editor_error(
            "use.office.spreadsheet_last_sheet",
            "A Spreadsheet document must retain at least one worksheet.",
        ));
    }
    let requested = worksheets
        .into_iter()
        .find(|sheet| sheet.path[1..].eq_ignore_ascii_case(requested_name))
        .ok_or_else(|| {
            editor_error(
                "use.office.node_not_found",
                format!("Office semantic path '{path}' does not exist."),
            )
        })?;
    let part_name = requested.format.get("part").cloned().ok_or_else(|| {
        editor_error(
            "use.office.spreadsheet_sheet_invalid",
            "Spreadsheet semantic sheet has no source part.",
        )
    })?;
    let workbook = package.xml_part("xl/workbook.xml")?;
    let index = index_xml(&workbook)?;
    let sheets = index.child("sheets", 1).ok_or_else(|| {
        editor_error(
            "use.office.spreadsheet_sheets_missing",
            "Spreadsheet workbook has no sheets collection.",
        )
    })?;
    let sheet = sheets
        .children
        .iter()
        .filter(|child| child.local_name == "sheet")
        .find(|child| {
            child
                .qualified_attributes
                .get("name")
                .is_some_and(|name| name.eq_ignore_ascii_case(requested_name))
        })
        .ok_or_else(|| {
            editor_error(
                "use.office.node_not_found",
                format!("Office semantic path '{path}' does not exist."),
            )
        })?;
    let relationship_id = sheet
        .qualified_attributes
        .iter()
        .find(|(name, _)| name.ends_with(":id"))
        .map(|(_, value)| value.clone())
        .ok_or_else(|| {
            editor_error(
                "use.office.spreadsheet_sheet_invalid",
                format!("Worksheet '{requested_name}' has no relationship ID."),
            )
        })?;
    let edited = crate::xml_edit::apply_patches(
        &workbook,
        vec![crate::xml_edit::XmlPatch::new(
            sheet.full_range.clone(),
            Vec::new(),
        )],
    )?;
    crate::opc_edit::remove_relationship(package, "xl/_rels/workbook.xml.rels", &relationship_id)?;
    crate::opc_edit::remove_content_type_override(package, &part_name)?;
    let (directory, file_name) = part_name.rsplit_once('/').ok_or_else(|| {
        editor_error(
            "use.office.spreadsheet_sheet_invalid",
            format!("Worksheet part '{part_name}' has an invalid path."),
        )
    })?;
    package.remove_part(&format!("{directory}/_rels/{file_name}.rels"))?;
    if !package.remove_part(&part_name)? {
        return Err(editor_error(
            "use.office.node_not_found",
            format!("Office semantic path '{path}' does not exist."),
        ));
    }
    package.set_part("xl/workbook.xml", edited)
}

fn validate_worksheet_name(name: &str) -> UseResult<()> {
    if name.is_empty()
        || name.chars().count() > 31
        || name.chars().any(char::is_control)
        || name
            .chars()
            .any(|character| matches!(character, '[' | ']' | ':' | '*' | '?' | '/' | '\\'))
        || name.starts_with('\'')
        || name.ends_with('\'')
    {
        return Err(editor_error(
            "use.office.spreadsheet_sheet_name_invalid",
            "Worksheet names must be 1-31 characters and exclude control characters, []:*?/\\, and edge apostrophes.",
        ));
    }
    Ok(())
}

fn blank_worksheet_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><dimension ref="A1"/><sheetViews><sheetView workbookViewId="0"/></sheetViews><sheetFormatPr defaultRowHeight="15"/><sheetData/><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/></worksheet>"#
}
