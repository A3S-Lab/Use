use std::collections::BTreeMap;

use a3s_use_core::UseResult;

use super::{
    editor_error, escape_attribute, index_xml, node_not_found, validate_mutation_path,
    validate_worksheet_name, XmlPatch,
};
use crate::semantic::{NativeOfficeDocument, OfficeNodeType};
use crate::spreadsheet_formula::rewrite_formula_sheet_name;
use crate::xml_edit::{apply_patches, escape_text, IndexedXmlElement};
use crate::{DocumentKind, LosslessXmlPart, NativeOfficePackage};

pub(crate) fn rename_worksheet(
    package: &mut NativeOfficePackage,
    path: &str,
    name: &str,
) -> UseResult<String> {
    require_spreadsheet(package)?;
    validate_mutation_path(path)?;
    validate_worksheet_name(name)?;
    let snapshot = NativeOfficeDocument::from_package(package.clone())?;
    let worksheets = snapshot
        .root()
        .children
        .iter()
        .filter(|node| node.node_type == OfficeNodeType::Worksheet)
        .collect::<Vec<_>>();
    let requested = worksheets
        .iter()
        .copied()
        .find(|sheet| sheet.path.eq_ignore_ascii_case(path))
        .ok_or_else(|| node_not_found(path))?;
    let old_name = requested.path.trim_start_matches('/');
    if old_name == name {
        return Ok(format!("/{name}"));
    }
    if worksheets.iter().any(|sheet| {
        sheet
            .path
            .trim_start_matches('/')
            .eq_ignore_ascii_case(name)
    }) {
        return Err(editor_error(
            "use.office.spreadsheet_sheet_exists",
            format!("Spreadsheet already contains a worksheet named '{name}'."),
        ));
    }

    rewrite_formula_part_names(package, old_name, name)?;
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
        .filter(|sheet| sheet.local_name == "sheet")
        .find(|sheet| {
            sheet
                .attributes
                .get("name")
                .is_some_and(|value| value.eq_ignore_ascii_case(old_name))
        })
        .ok_or_else(|| node_not_found(path))?;
    let updates = BTreeMap::from([("name".to_string(), name.to_string())]);
    let mut patches = vec![XmlPatch::new(
        sheet.start_tag_range.clone(),
        updated_start_tag(sheet, &updates),
    )];
    let mut defined_names = Vec::new();
    index.descendants_named("definedName", &mut defined_names);
    for defined_name in defined_names {
        let text = decoded_text(&workbook, defined_name)?;
        let rewritten = rewrite_formula_sheet_name(&text, old_name, name)?;
        if rewritten != text {
            patches.push(XmlPatch::new(
                defined_name.content_range.clone(),
                escape_text(&rewritten),
            ));
        }
    }
    package.set_part("xl/workbook.xml", apply_patches(&workbook, patches)?)?;
    Ok(format!("/{name}"))
}

pub(crate) fn move_worksheet(
    package: &mut NativeOfficePackage,
    path: &str,
    position: usize,
) -> UseResult<String> {
    require_spreadsheet(package)?;
    validate_mutation_path(path)?;
    let workbook = package.xml_part("xl/workbook.xml")?;
    let index = index_xml(&workbook)?;
    let sheets = index.child("sheets", 1).ok_or_else(|| {
        editor_error(
            "use.office.spreadsheet_sheets_missing",
            "Spreadsheet workbook has no sheets collection.",
        )
    })?;
    let sheet_elements = sheets
        .children
        .iter()
        .filter(|sheet| sheet.local_name == "sheet")
        .collect::<Vec<_>>();
    if position == 0 || position > sheet_elements.len() {
        return Err(editor_error(
            "use.office.spreadsheet_sheet_position_invalid",
            format!(
                "Worksheet position must be within 1-{}.",
                sheet_elements.len()
            ),
        )
        .with_detail("position", position));
    }
    let old_index = sheet_elements
        .iter()
        .position(|sheet| {
            sheet
                .attributes
                .get("name")
                .is_some_and(|name| format!("/{name}").eq_ignore_ascii_case(path))
        })
        .ok_or_else(|| node_not_found(path))?;
    let new_index = position - 1;
    if old_index == new_index {
        return Ok(path.to_string());
    }

    let mut order = (0..sheet_elements.len()).collect::<Vec<_>>();
    let moved = order.remove(old_index);
    order.insert(new_index, moved);
    let parse_bytes = workbook.parse_bytes();
    let content = order
        .iter()
        .map(|old| {
            parse_bytes
                .get(sheet_elements[*old].full_range.clone())
                .ok_or_else(|| {
                    editor_error(
                        "use.office.spreadsheet_sheet_invalid",
                        "Worksheet XML range is invalid.",
                    )
                })
        })
        .collect::<UseResult<Vec<_>>>()?
        .concat();
    let mut patches = vec![XmlPatch::new(sheets.content_range.clone(), content)];
    let new_index_for_old = |old: usize| order.iter().position(|candidate| *candidate == old);

    let mut defined_names = Vec::new();
    index.descendants_named("definedName", &mut defined_names);
    for name in defined_names {
        let Some(old) = name
            .attributes
            .get("localSheetId")
            .and_then(|value| value.parse::<usize>().ok())
        else {
            continue;
        };
        let Some(new) = new_index_for_old(old) else {
            return Err(editor_error(
                "use.office.spreadsheet_defined_name_invalid",
                format!("Defined name localSheetId {old} has no worksheet."),
            ));
        };
        if new != old {
            patches.push(XmlPatch::new(
                name.start_tag_range.clone(),
                updated_start_tag(
                    name,
                    &BTreeMap::from([("localSheetId".to_string(), new.to_string())]),
                ),
            ));
        }
    }

    let mut views = Vec::new();
    index.descendants_named("workbookView", &mut views);
    for view in views {
        let mut updates = BTreeMap::new();
        for attribute in ["activeTab", "firstSheet"] {
            let Some(old) = view
                .attributes
                .get(attribute)
                .and_then(|value| value.parse::<usize>().ok())
            else {
                continue;
            };
            if let Some(new) = new_index_for_old(old) {
                if new != old {
                    updates.insert(attribute.to_string(), new.to_string());
                }
            }
        }
        if !updates.is_empty() {
            patches.push(XmlPatch::new(
                view.start_tag_range.clone(),
                updated_start_tag(view, &updates),
            ));
        }
    }

    package.set_part("xl/workbook.xml", apply_patches(&workbook, patches)?)?;
    Ok(path.to_string())
}

fn rewrite_formula_part_names(
    package: &mut NativeOfficePackage,
    old: &str,
    new: &str,
) -> UseResult<()> {
    let parts = package
        .part_names()
        .filter(|part| {
            part.starts_with("xl/")
                && part.ends_with(".xml")
                && *part != "xl/workbook.xml"
                && (part.starts_with("xl/worksheets/")
                    || part.starts_with("xl/charts/")
                    || part.starts_with("xl/tables/"))
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    for part_name in parts {
        let part = package.xml_part(&part_name)?;
        let index = index_xml(&part)?;
        let mut candidates = Vec::new();
        collect_formula_elements(&index, &mut candidates);
        let mut patches = Vec::new();
        for formula in candidates {
            let text = decoded_text(&part, formula)?;
            let rewritten = rewrite_formula_sheet_name(&text, old, new)?;
            if rewritten != text {
                patches.push(XmlPatch::new(
                    formula.content_range.clone(),
                    escape_text(&rewritten),
                ));
            }
        }
        if !patches.is_empty() {
            package.set_part(&part_name, apply_patches(&part, patches)?)?;
        }
    }
    Ok(())
}

fn collect_formula_elements<'a>(
    element: &'a IndexedXmlElement,
    output: &mut Vec<&'a IndexedXmlElement>,
) {
    for child in &element.children {
        if matches!(
            child.local_name.as_str(),
            "f" | "formula" | "calculatedColumnFormula" | "totalsRowFormula"
        ) {
            output.push(child);
        }
        collect_formula_elements(child, output);
    }
}

fn updated_start_tag(element: &IndexedXmlElement, updates: &BTreeMap<String, String>) -> String {
    let mut attributes = element.qualified_attributes.clone();
    attributes.extend(updates.clone());
    let attributes = attributes
        .into_iter()
        .map(|(name, value)| format!(" {name}=\"{}\"", escape_attribute(&value)))
        .collect::<String>();
    let terminator = if element.empty { "/>" } else { ">" };
    format!("<{}{attributes}{terminator}", element.qualified_name)
}

fn decoded_text(part: &LosslessXmlPart, element: &IndexedXmlElement) -> UseResult<String> {
    let bytes = part
        .parse_bytes()
        .get(element.content_range.clone())
        .ok_or_else(|| {
            editor_error(
                "use.office.spreadsheet_formula_invalid",
                format!("Formula range in '{}' is invalid.", part.name()),
            )
        })?;
    let text = std::str::from_utf8(bytes).map_err(|error| {
        editor_error(
            "use.office.spreadsheet_formula_invalid",
            format!("Formula in '{}' is not UTF-8: {error}", part.name()),
        )
    })?;
    quick_xml::escape::unescape(text)
        .map(|value| value.into_owned())
        .map_err(|error| {
            editor_error(
                "use.office.spreadsheet_formula_invalid",
                format!(
                    "Formula in '{}' contains invalid XML escapes: {error}",
                    part.name()
                ),
            )
        })
}

fn require_spreadsheet(package: &NativeOfficePackage) -> UseResult<()> {
    if package.kind() == DocumentKind::Spreadsheet {
        Ok(())
    } else {
        Err(editor_error(
            "use.office.mutation_type_unsupported",
            "Worksheet rename and move are available only for Spreadsheet documents.",
        ))
    }
}
