use std::collections::BTreeMap;

use a3s_use_core::UseResult;

use super::{semantic_error, DocumentNode, OfficeNodeType};
use crate::spreadsheet_reference::{
    column_name as a1_column_name, parse_column, CellRange, CellReference, MAX_COLUMNS,
};
use crate::xml_tree::{parse_xml_tree, XmlElement, XmlNode};
use crate::{NativeOfficePackage, OpcPackageModel, RelationshipSource, RelationshipTarget};

const SPREADSHEET_NAMESPACE: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const STRICT_SPREADSHEET_NAMESPACE: &str = "http://purl.oclc.org/ooxml/spreadsheetml/main";
const SPREADSHEET_DRAWING_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const STRICT_SPREADSHEET_DRAWING_NAMESPACE: &str =
    "http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing";
const RELATIONSHIPS_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const STRICT_RELATIONSHIPS_NAMESPACE: &str =
    "http://purl.oclc.org/ooxml/officeDocument/relationships";

pub(super) fn read(
    package: &NativeOfficePackage,
    opc: &OpcPackageModel,
) -> UseResult<DocumentNode> {
    let workbook_part = package.xml_part("xl/workbook.xml")?;
    let workbook = parse_xml_tree(&workbook_part)?;
    require_spreadsheet_element(&workbook, "workbook", workbook_part.name())?;
    let shared_strings = read_shared_strings(package)?;
    let styles = read_styles(package)?;
    let source = RelationshipSource::Part {
        part_name: "xl/workbook.xml".to_string(),
    };

    let mut root = DocumentNode::new("/", "workbook", OfficeNodeType::Workbook);
    let sheets = workbook.child("sheets").ok_or_else(|| {
        semantic_error(
            "use.office.spreadsheet_sheets_missing",
            "Spreadsheet workbook has no sheets collection.",
        )
    })?;
    for sheet in sheets.children_named("sheet") {
        let name = sheet.attribute("name").ok_or_else(|| {
            semantic_error(
                "use.office.spreadsheet_sheet_invalid",
                "Spreadsheet sheet is missing its name.",
            )
        })?;
        validate_sheet_name(name)?;
        let relationship_id = relationship_attribute(sheet, "id").ok_or_else(|| {
            semantic_error(
                "use.office.spreadsheet_sheet_invalid",
                format!("Spreadsheet sheet '{name}' has no relationship ID."),
            )
        })?;
        let relationship = opc
            .relationships()
            .relationship(&source, relationship_id)
            .ok_or_else(|| {
                semantic_error(
                    "use.office.spreadsheet_sheet_missing",
                    format!(
                        "Spreadsheet sheet '{name}' references missing relationship '{relationship_id}'."
                    ),
                )
            })?;
        let RelationshipTarget::Internal { part_name, .. } = &relationship.target else {
            return Err(semantic_error(
                "use.office.spreadsheet_sheet_invalid",
                format!("Spreadsheet sheet '{name}' cannot use an external relationship."),
            ));
        };
        let mut sheet_node =
            read_worksheet(package, opc, part_name, name, &shared_strings, &styles)?;
        if let Some(sheet_id) = sheet.attribute("sheetId") {
            sheet_node.format.insert("sheetId".into(), sheet_id.into());
        }
        if let Some(state) = sheet.attribute("state") {
            sheet_node.format.insert("state".into(), state.into());
        }
        root.children.push(sheet_node);
    }
    root.text = root
        .children
        .iter()
        .map(|sheet| sheet.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(root)
}

pub(super) fn virtual_get(
    root: &DocumentNode,
    path: &str,
    depth: usize,
) -> UseResult<Option<DocumentNode>> {
    let Some((requested_sheet, target)) =
        path.strip_prefix('/').and_then(|path| path.split_once('/'))
    else {
        return Ok(None);
    };
    let Some(sheet) = root.children.iter().find(|node| {
        node.node_type == OfficeNodeType::Worksheet
            && node.path[1..].eq_ignore_ascii_case(requested_sheet)
    }) else {
        return Ok(None);
    };
    if let Some(column) = target
        .strip_prefix("col[")
        .and_then(|value| value.strip_suffix(']'))
    {
        return virtual_column(sheet, column, depth).map(Some);
    }
    let Some((start, end)) = target.split_once(':') else {
        return Ok(None);
    };
    virtual_range(sheet, start, end, depth).map(Some)
}

fn virtual_column(sheet: &DocumentNode, requested: &str, depth: usize) -> UseResult<DocumentNode> {
    let column = if requested
        .chars()
        .all(|character| character.is_ascii_digit())
    {
        let number = requested.parse::<u32>().map_err(|error| {
            semantic_error(
                "use.office.spreadsheet_column_invalid",
                format!("Spreadsheet column '{requested}' is invalid: {error}"),
            )
        })?;
        column_name(number)?
    } else {
        let normalized = requested.to_ascii_uppercase();
        if column_number(&format!("{normalized}1")).is_none() {
            return Err(semantic_error(
                "use.office.spreadsheet_column_invalid",
                format!("Spreadsheet column '{requested}' is outside A:XFD."),
            ));
        }
        normalized
    };
    let path = format!("{}/col[{column}]", sheet.path);
    let mut node = DocumentNode::new(path, "col", OfficeNodeType::Column);
    node.format.insert("column".into(), column.clone());
    let cells = cells(sheet)
        .into_iter()
        .filter(|cell| {
            cell.format
                .get("column")
                .is_some_and(|value| value == &column)
        })
        .collect::<Vec<_>>();
    node.child_count = cells.len();
    node.text = cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if depth > 0 {
        node.children = cells
            .into_iter()
            .map(|cell| cell.clone_to_depth(depth - 1))
            .collect();
    }
    Ok(node)
}

fn virtual_range(
    sheet: &DocumentNode,
    start: &str,
    end: &str,
    depth: usize,
) -> UseResult<DocumentNode> {
    let (start_column, start_row, start_reference) = cell_coordinates(start)?;
    let (end_column, end_row, end_reference) = cell_coordinates(end)?;
    let min_column = start_column.min(end_column);
    let max_column = start_column.max(end_column);
    let min_row = start_row.min(end_row);
    let max_row = start_row.max(end_row);
    let path = format!("{}/{}:{}", sheet.path, start_reference, end_reference);
    let mut node = DocumentNode::new(path, "range", OfficeNodeType::Range);
    node.format.insert(
        "normalizedRef".into(),
        format!(
            "{}{}:{}{}",
            column_name(min_column)?,
            min_row,
            column_name(max_column)?,
            max_row
        ),
    );
    let matching = cells(sheet)
        .into_iter()
        .filter(|cell| {
            cell.path
                .rsplit('/')
                .next()
                .and_then(|reference| cell_coordinates(reference).ok())
                .is_some_and(|(column, row, _)| {
                    (min_column..=max_column).contains(&column)
                        && (min_row..=max_row).contains(&row)
                })
        })
        .collect::<Vec<_>>();
    node.child_count = matching.len();
    node.text = matching
        .iter()
        .map(|cell| format!("{}={}", cell.path, cell.text))
        .collect::<Vec<_>>()
        .join("\n");
    if depth > 0 {
        node.children = matching
            .into_iter()
            .map(|cell| cell.clone_to_depth(depth - 1))
            .collect();
    }
    Ok(node)
}

fn cells(sheet: &DocumentNode) -> Vec<&DocumentNode> {
    sheet
        .children
        .iter()
        .flat_map(|row| row.children.iter())
        .filter(|node| node.node_type == OfficeNodeType::Cell)
        .collect()
}

fn cell_coordinates(reference: &str) -> UseResult<(u32, u32, String)> {
    let reference = CellReference::parse(reference)?;
    Ok((reference.column, reference.row, reference.a1()))
}

fn read_worksheet(
    package: &NativeOfficePackage,
    opc: &OpcPackageModel,
    part_name: &str,
    sheet_name: &str,
    shared_strings: &[String],
    styles: &[BTreeMap<String, String>],
) -> UseResult<DocumentNode> {
    let part = package.xml_part(part_name)?;
    let worksheet = parse_xml_tree(&part)?;
    require_spreadsheet_element(&worksheet, "worksheet", part.name())?;
    let sheet_path = format!("/{sheet_name}");
    let mut sheet_node = DocumentNode::new(&sheet_path, "sheet", OfficeNodeType::Worksheet);
    sheet_node.format.insert("part".into(), part_name.into());
    if let Some(sheet_data) = worksheet.child("sheetData") {
        let mut inferred_row = 0_u32;
        for row in sheet_data.children_named("row") {
            let row_number = row
                .attribute("r")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or_else(|| inferred_row.saturating_add(1));
            if row_number == 0 {
                return Err(semantic_error(
                    "use.office.spreadsheet_row_invalid",
                    format!("Worksheet '{sheet_name}' contains row zero."),
                ));
            }
            inferred_row = row_number;
            let row_path = format!("{sheet_path}/row[{row_number}]");
            let mut row_node = DocumentNode::new(&row_path, "row", OfficeNodeType::Row);
            copy_attribute(row, "ht", "height", &mut row_node);
            copy_attribute(row, "hidden", "hidden", &mut row_node);
            copy_attribute(row, "outlineLevel", "outlineLevel", &mut row_node);
            let mut inferred_column = 0_u32;
            for cell in row.children_named("c") {
                let reference = match cell.attribute("r") {
                    Some(reference) => normalize_cell_reference(reference, row_number)?,
                    None => {
                        inferred_column = inferred_column.saturating_add(1);
                        format!("{}{row_number}", column_name(inferred_column)?)
                    }
                };
                inferred_column = column_number(&reference).unwrap_or(inferred_column);
                row_node.children.push(read_cell(
                    cell,
                    &sheet_path,
                    &reference,
                    shared_strings,
                    styles,
                )?);
            }
            row_node.text = row_node
                .children
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>()
                .join("\t");
            sheet_node.children.push(row_node);
        }
    }
    append_worksheet_hyperlinks(opc, &worksheet, part_name, &sheet_path, &mut sheet_node)?;
    append_worksheet_comments(package, opc, part_name, &sheet_path, &mut sheet_node)?;
    append_worksheet_pictures(
        package,
        opc,
        &worksheet,
        part_name,
        &sheet_path,
        &mut sheet_node,
    )?;
    sheet_node.text = sheet_node
        .children
        .iter()
        .filter(|child| child.node_type == OfficeNodeType::Row)
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(sheet_node)
}

fn append_worksheet_comments(
    package: &NativeOfficePackage,
    opc: &OpcPackageModel,
    worksheet_part: &str,
    sheet_path: &str,
    sheet: &mut DocumentNode,
) -> UseResult<()> {
    let source = RelationshipSource::Part {
        part_name: worksheet_part.to_string(),
    };
    let Some(relationship) = opc
        .relationships()
        .relationships_from(&source)
        .iter()
        .find(|relationship| relationship.relationship_type.ends_with("/comments"))
    else {
        return Ok(());
    };
    let RelationshipTarget::Internal { part_name, .. } = &relationship.target else {
        return Err(semantic_error(
            "use.office.comment_relationship_invalid",
            format!("Worksheet '{sheet_path}' comments relationship must be internal."),
        ));
    };
    let part = package.xml_part(part_name)?;
    let comments = parse_xml_tree(&part)?;
    require_spreadsheet_element(&comments, "comments", part.name())?;
    let authors = comments
        .child("authors")
        .map(|authors| {
            authors
                .children_named("author")
                .map(direct_text)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let Some(list) = comments.child("commentList") else {
        return Ok(());
    };
    for comment in list.children_named("comment") {
        let reference = comment.attribute("ref").ok_or_else(|| {
            semantic_error(
                "use.office.comment_reference_missing",
                format!("Worksheet '{sheet_path}' contains a comment without a cell reference."),
            )
        })?;
        let reference = CellReference::parse(reference)?.a1();
        let mut node = DocumentNode::new(
            format!("{sheet_path}/{reference}/comment"),
            "comment",
            OfficeNodeType::Comment,
        );
        node.text = comment
            .child("text")
            .map(spreadsheet_text)
            .unwrap_or_default();
        node.format.insert("ref".into(), reference.clone());
        node.format.insert("part".into(), part_name.clone());
        node.format
            .insert("ownerPart".into(), worksheet_part.to_string());
        if let Some(author_id) = comment.attribute("authorId") {
            node.format.insert("authorId".into(), author_id.into());
            if let Some(author) = author_id
                .parse::<usize>()
                .ok()
                .and_then(|author_id| authors.get(author_id))
            {
                node.format.insert("author".into(), author.clone());
            }
        }
        if let Some(cell) = find_cell_mut(sheet, &reference) {
            cell.children.push(node);
        } else {
            sheet.children.push(node);
        }
    }
    Ok(())
}

fn append_worksheet_hyperlinks(
    opc: &OpcPackageModel,
    worksheet: &XmlElement,
    worksheet_part: &str,
    sheet_path: &str,
    sheet: &mut DocumentNode,
) -> UseResult<()> {
    let Some(hyperlinks) = worksheet.child("hyperlinks") else {
        return Ok(());
    };
    let source = RelationshipSource::Part {
        part_name: worksheet_part.to_string(),
    };
    let mut fallback_index = 0_usize;
    for hyperlink in hyperlinks.children_named("hyperlink") {
        let reference = hyperlink.attribute("ref").ok_or_else(|| {
            semantic_error(
                "use.office.spreadsheet_hyperlink_invalid",
                format!("Worksheet '{sheet_path}' contains a hyperlink without a cell reference."),
            )
        })?;
        let range = CellRange::parse(reference)?;
        let normalized_reference = range.a1();
        let single_cell = range.is_single_cell().then(|| range.start.a1());
        let cell = single_cell
            .as_deref()
            .and_then(|reference| find_cell_mut(sheet, reference));
        fallback_index += usize::from(cell.is_none());
        let path = cell.as_ref().map_or_else(
            || format!("{sheet_path}/hyperlink[{fallback_index}]"),
            |cell| {
                if cell
                    .children
                    .iter()
                    .any(|child| child.node_type == OfficeNodeType::Hyperlink)
                {
                    format!("{}/hyperlink[2]", cell.path)
                } else {
                    format!("{}/hyperlink", cell.path)
                }
            },
        );
        let mut node = DocumentNode::new(path, "hyperlink", OfficeNodeType::Hyperlink);
        node.format.insert("ref".into(), normalized_reference);
        for (attribute, key) in [("display", "display"), ("tooltip", "tooltip")] {
            if let Some(value) = hyperlink.attribute(attribute) {
                node.format.insert(key.into(), value.into());
            }
        }
        if let Some(location) = hyperlink.attribute("location") {
            node.format.insert("targetKind".into(), "internal".into());
            node.format.insert("target".into(), location.into());
        } else if let Some(id) = hyperlink.attribute("id") {
            node.format.insert("relationshipId".into(), id.into());
            if let Some(relationship) = opc
                .relationships()
                .relationship(&source, id)
                .filter(|relationship| relationship.relationship_type.ends_with("/hyperlink"))
            {
                match &relationship.target {
                    RelationshipTarget::External { uri } => {
                        node.format.insert("targetKind".into(), "external".into());
                        node.format.insert("target".into(), uri.clone());
                    }
                    RelationshipTarget::Internal {
                        part_name,
                        fragment,
                    } => {
                        node.format.insert("targetKind".into(), "internal".into());
                        let target = fragment.as_ref().map_or_else(
                            || part_name.clone(),
                            |fragment| format!("{part_name}#{fragment}"),
                        );
                        node.format.insert("target".into(), target);
                    }
                }
            }
        }
        node.text = node
            .format
            .get("display")
            .cloned()
            .or_else(|| cell.as_ref().map(|cell| cell.text.clone()))
            .unwrap_or_default();
        if let Some(cell) = cell {
            cell.children.push(node);
        } else {
            sheet.children.push(node);
        }
    }
    Ok(())
}

fn find_cell_mut<'a>(sheet: &'a mut DocumentNode, reference: &str) -> Option<&'a mut DocumentNode> {
    let path = format!("{}/{reference}", sheet.path);
    sheet
        .children
        .iter_mut()
        .flat_map(|row| row.children.iter_mut())
        .find(|cell| cell.node_type == OfficeNodeType::Cell && cell.path == path)
}

fn append_worksheet_pictures(
    package: &NativeOfficePackage,
    opc: &OpcPackageModel,
    worksheet: &XmlElement,
    worksheet_part: &str,
    sheet_path: &str,
    sheet: &mut DocumentNode,
) -> UseResult<()> {
    let drawings = worksheet.children_named("drawing").collect::<Vec<_>>();
    if drawings.len() > 1 {
        return Err(semantic_error(
            "use.office.spreadsheet_drawing_invalid",
            format!("Worksheet '{sheet_path}' contains more than one drawing element."),
        ));
    }
    let Some(drawing) = drawings.first() else {
        return Ok(());
    };
    let relationship_id = relationship_attribute(drawing, "id").ok_or_else(|| {
        semantic_error(
            "use.office.spreadsheet_drawing_invalid",
            format!("Worksheet '{sheet_path}' drawing has no relationship ID."),
        )
    })?;
    let worksheet_source = RelationshipSource::Part {
        part_name: worksheet_part.to_string(),
    };
    let relationship = opc
        .relationships()
        .relationship(&worksheet_source, relationship_id)
        .filter(|relationship| relationship.relationship_type.ends_with("/drawing"))
        .ok_or_else(|| {
            semantic_error(
                "use.office.spreadsheet_drawing_invalid",
                format!("Worksheet '{sheet_path}' drawing relationship is missing or invalid."),
            )
        })?;
    let RelationshipTarget::Internal {
        part_name: drawing_part,
        ..
    } = &relationship.target
    else {
        return Err(semantic_error(
            "use.office.spreadsheet_drawing_invalid",
            "Spreadsheet drawings must use internal relationships.",
        ));
    };
    let drawing_xml = parse_xml_tree(&package.xml_part(drawing_part)?)?;
    if drawing_xml.local_name != "wsDr"
        || !matches!(
            drawing_xml.namespace.as_deref(),
            Some(SPREADSHEET_DRAWING_NAMESPACE | STRICT_SPREADSHEET_DRAWING_NAMESPACE)
        )
    {
        return Err(semantic_error(
            "use.office.spreadsheet_drawing_invalid",
            format!("Spreadsheet drawing '/{drawing_part}' has an invalid root element."),
        ));
    }
    let drawing_source = RelationshipSource::Part {
        part_name: drawing_part.clone(),
    };
    let mut picture_index = 0_usize;
    for anchor in drawing_xml.child_elements().filter(|element| {
        matches!(
            element.local_name.as_str(),
            "oneCellAnchor" | "twoCellAnchor" | "absoluteAnchor"
        )
    }) {
        let Some(picture) = find_descendant(anchor, "pic") else {
            continue;
        };
        picture_index += 1;
        let mut node = DocumentNode::new(
            format!("{sheet_path}/picture[{picture_index}]"),
            "picture",
            OfficeNodeType::Picture,
        );
        node.format
            .insert("ownerPart".into(), format!("/{drawing_part}"));
        if let Some(properties) = find_descendant(picture, "cNvPr") {
            copy_attribute(properties, "id", "id", &mut node);
            copy_attribute(properties, "name", "name", &mut node);
            if let Some(alt) = properties
                .attribute("descr")
                .or_else(|| properties.attribute("title"))
            {
                node.format.insert("alt".into(), alt.into());
            }
        }
        if let Some(blip) = find_descendant(picture, "blip") {
            if let Some(embed) = blip.attribute("embed") {
                node.format.insert("relationshipId".into(), embed.into());
                if let Some(media) = opc
                    .relationships()
                    .relationship(&drawing_source, embed)
                    .and_then(|relationship| relationship.target.internal_part_name())
                {
                    node.format.insert("part".into(), format!("/{media}"));
                }
            }
        }
        let extent = anchor
            .child("ext")
            .or_else(|| find_descendant(picture, "xfrm").and_then(|xfrm| xfrm.child("ext")));
        if let Some(extent) = extent {
            copy_pixel_extent(extent, "cx", "widthPx", &mut node);
            copy_pixel_extent(extent, "cy", "heightPx", &mut node);
        }
        if let Some(from) = anchor.child("from") {
            let column = from
                .child("col")
                .and_then(|value| direct_text(value).parse::<u32>().ok());
            let row = from
                .child("row")
                .and_then(|value| direct_text(value).parse::<u32>().ok());
            if let (Some(column), Some(row)) = (column, row) {
                if let Ok(column) = column_name(column.saturating_add(1)) {
                    node.format
                        .insert("anchorCell".into(), format!("{column}{}", row + 1));
                }
            }
        }
        sheet.children.push(node);
    }
    Ok(())
}

fn find_descendant<'a>(element: &'a XmlElement, local_name: &str) -> Option<&'a XmlElement> {
    for child in element.child_elements() {
        if child.local_name == local_name {
            return Some(child);
        }
        if let Some(found) = find_descendant(child, local_name) {
            return Some(found);
        }
    }
    None
}

fn copy_pixel_extent(element: &XmlElement, attribute: &str, key: &str, node: &mut DocumentNode) {
    if let Some(value) = element
        .attribute(attribute)
        .and_then(|value| value.parse::<u64>().ok())
    {
        node.format
            .insert(key.into(), ((value + 4_762) / 9_525).to_string());
    }
}

fn read_cell(
    cell: &XmlElement,
    sheet_path: &str,
    reference: &str,
    shared_strings: &[String],
    styles: &[BTreeMap<String, String>],
) -> UseResult<DocumentNode> {
    let mut node = DocumentNode::new(
        format!("{sheet_path}/{reference}"),
        "cell",
        OfficeNodeType::Cell,
    );
    let column = reference
        .chars()
        .take_while(|character| character.is_ascii_alphabetic())
        .collect::<String>();
    node.format.insert("column".into(), column);
    node.format.insert(
        "row".into(),
        reference
            .chars()
            .skip_while(|character| character.is_ascii_alphabetic())
            .collect(),
    );
    let value_type = cell.attribute("t").unwrap_or("n");
    let raw_value = cell.child("v").map(direct_text).unwrap_or_default();
    node.text = match value_type {
        "s" => {
            let index = raw_value.parse::<usize>().map_err(|error| {
                semantic_error(
                    "use.office.spreadsheet_shared_string_invalid",
                    format!("Cell '{reference}' has invalid shared-string index: {error}"),
                )
            })?;
            shared_strings.get(index).cloned().ok_or_else(|| {
                semantic_error(
                    "use.office.spreadsheet_shared_string_invalid",
                    format!("Cell '{reference}' references missing shared string {index}."),
                )
            })?
        }
        "inlineStr" => cell.child("is").map(spreadsheet_text).unwrap_or_default(),
        "b" => match raw_value.as_str() {
            "1" => "true".to_string(),
            "0" => "false".to_string(),
            _ => raw_value,
        },
        _ => raw_value,
    };
    node.format
        .insert("valueType".into(), value_type_name(value_type).into());
    node.format
        .insert("empty".into(), node.text.is_empty().to_string());
    if let Some(formula) = cell.child("f") {
        node.format.insert("formula".into(), direct_text(formula));
        if let Some(formula_type) = formula.attribute("t") {
            node.format
                .insert("formulaType".into(), formula_type.into());
        }
        if let Some(reference) = formula.attribute("ref") {
            node.format.insert("formulaRef".into(), reference.into());
        }
    }
    if let Some(style_index) = cell
        .attribute("s")
        .and_then(|value| value.parse::<usize>().ok())
    {
        node.format
            .insert("styleIndex".into(), style_index.to_string());
        let style = styles.get(style_index).ok_or_else(|| {
            semantic_error(
                "use.office.spreadsheet_style_invalid",
                format!("Cell '{reference}' references missing style {style_index}."),
            )
        })?;
        node.format.extend(style.clone());
    }
    Ok(node)
}

fn read_shared_strings(package: &NativeOfficePackage) -> UseResult<Vec<String>> {
    if !package.contains_part("xl/sharedStrings.xml") {
        return Ok(Vec::new());
    }
    let part = package.xml_part("xl/sharedStrings.xml")?;
    let root = parse_xml_tree(&part)?;
    require_spreadsheet_element(&root, "sst", part.name())?;
    Ok(root.children_named("si").map(spreadsheet_text).collect())
}

fn read_styles(package: &NativeOfficePackage) -> UseResult<Vec<BTreeMap<String, String>>> {
    if !package.contains_part("xl/styles.xml") {
        return Ok(vec![BTreeMap::new()]);
    }
    let part = package.xml_part("xl/styles.xml")?;
    let root = parse_xml_tree(&part)?;
    require_spreadsheet_element(&root, "styleSheet", part.name())?;
    let custom_formats = root
        .child("numFmts")
        .map(|formats| {
            formats
                .children_named("numFmt")
                .filter_map(|format| {
                    Some((
                        format.attribute("numFmtId")?.to_string(),
                        format.attribute("formatCode")?.to_string(),
                    ))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    // Some legacy producers omit the required fonts collection while still
    // using fontId=0. Retain the prior tolerant read for that default only;
    // explicit out-of-range font references fail closed.
    let fonts = root
        .child("fonts")
        .map(|fonts| fonts.children_named("font").map(read_font).collect())
        .unwrap_or_else(|| vec![BTreeMap::new()]);
    let mut styles = Vec::new();
    if let Some(cell_xfs) = root.child("cellXfs") {
        for style in cell_xfs.children_named("xf") {
            let mut values = BTreeMap::new();
            for (attribute, key) in [
                ("fontId", "fontId"),
                ("fillId", "fillId"),
                ("borderId", "borderId"),
                ("xfId", "baseStyleId"),
            ] {
                if let Some(value) = style.attribute(attribute) {
                    values.insert(key.to_string(), value.to_string());
                }
            }
            if let Some(font_id) = style.attribute("fontId") {
                let index = font_id.parse::<usize>().map_err(|error| {
                    semantic_error(
                        "use.office.spreadsheet_style_invalid",
                        format!("Spreadsheet style has invalid fontId '{font_id}': {error}"),
                    )
                })?;
                let font = fonts.get(index).ok_or_else(|| {
                    semantic_error(
                        "use.office.spreadsheet_style_invalid",
                        format!("Spreadsheet style references missing font {index}."),
                    )
                })?;
                values.extend(font.clone());
            }
            if let Some(number_format_id) = style.attribute("numFmtId") {
                values.insert("numberFormatId".into(), number_format_id.into());
                if let Some(format) = custom_formats
                    .get(number_format_id)
                    .map(String::as_str)
                    .or_else(|| built_in_number_format(number_format_id))
                {
                    values.insert("numberFormat".into(), format.into());
                }
            }
            if let Some(alignment) = style.child("alignment") {
                for (attribute, key) in [
                    ("horizontal", "alignment"),
                    ("vertical", "verticalAlignment"),
                    ("wrapText", "wrapText"),
                    ("textRotation", "textRotation"),
                ] {
                    if let Some(value) = alignment.attribute(attribute) {
                        values.insert(key.into(), value.into());
                    }
                }
            }
            styles.push(values);
        }
    }
    if styles.is_empty() {
        Ok(vec![BTreeMap::new()])
    } else {
        Ok(styles)
    }
}

fn read_font(font: &XmlElement) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for (child_name, key) in [("b", "bold"), ("i", "italic")] {
        if let Some(property) = font.child(child_name) {
            values.insert(key.into(), spreadsheet_bool_value(property).to_string());
        }
    }
    if let Some(name) = font.child("name").and_then(|name| name.attribute("val")) {
        values.insert("font".into(), name.into());
    }
    if let Some(size) = font
        .child("sz")
        .and_then(|size| size.attribute("val"))
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
    {
        values.insert("size".into(), format!("{size}pt"));
    }
    if let Some(rgb) = font
        .child("color")
        .and_then(|color| color.attribute("rgb"))
        .and_then(normalize_font_rgb)
    {
        values.insert("color".into(), rgb);
    }
    values
}

fn spreadsheet_bool_value(element: &XmlElement) -> bool {
    !matches!(
        element
            .attribute("val")
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("false" | "0" | "off" | "no")
    )
}

fn normalize_font_rgb(value: &str) -> Option<String> {
    if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    match value.len() {
        6 => Some(value.to_ascii_uppercase()),
        8 => Some(value[2..].to_ascii_uppercase()),
        _ => None,
    }
}

fn spreadsheet_text(element: &XmlElement) -> String {
    let mut output = String::new();
    append_spreadsheet_text(element, &mut output);
    output
}

fn append_spreadsheet_text(element: &XmlElement, output: &mut String) {
    if element.local_name == "t" {
        output.push_str(&direct_text(element));
        return;
    }
    if element.local_name == "rPh" {
        return;
    }
    for child in element.child_elements() {
        append_spreadsheet_text(child, output);
    }
}

fn direct_text(element: &XmlElement) -> String {
    element
        .children
        .iter()
        .filter_map(|child| match child {
            XmlNode::Text(text) => Some(text.as_str()),
            XmlNode::Element(_) => None,
        })
        .collect()
}

fn normalize_cell_reference(reference: &str, expected_row: u32) -> UseResult<String> {
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
        || reference[column_length..].parse::<u32>().ok() != Some(expected_row)
        || column_number(&reference).is_none()
    {
        return Err(semantic_error(
            "use.office.spreadsheet_cell_reference_invalid",
            format!("Spreadsheet cell reference '{reference}' is invalid for row {expected_row}."),
        ));
    }
    Ok(reference)
}

fn column_number(reference: &str) -> Option<u32> {
    let column = reference
        .chars()
        .take_while(|character| character.is_ascii_alphabetic())
        .collect::<String>();
    parse_column(&column).ok()
}

fn column_name(number: u32) -> UseResult<String> {
    if !(1..=MAX_COLUMNS).contains(&number) {
        return Err(semantic_error(
            "use.office.spreadsheet_column_invalid",
            format!("Spreadsheet column number {number} is outside XFD."),
        ));
    }
    Ok(a1_column_name(number))
}

fn validate_sheet_name(name: &str) -> UseResult<()> {
    if name.is_empty()
        || name.chars().count() > 31
        || name.chars().any(char::is_control)
        || name.contains(['/', '\\', '[', ']', ':', '*', '?'])
    {
        return Err(semantic_error(
            "use.office.spreadsheet_sheet_name_invalid",
            format!("Spreadsheet sheet name '{name}' is invalid."),
        ));
    }
    Ok(())
}

fn copy_attribute(element: &XmlElement, attribute: &str, key: &str, node: &mut DocumentNode) {
    if let Some(value) = element.attribute(attribute) {
        node.format.insert(key.into(), value.into());
    }
}

fn value_type_name(value_type: &str) -> &'static str {
    match value_type {
        "s" | "inlineStr" | "str" => "String",
        "b" => "Boolean",
        "e" => "Error",
        "d" => "Date",
        _ => "Number",
    }
}

fn built_in_number_format(id: &str) -> Option<&'static str> {
    match id {
        "0" => Some("General"),
        "1" => Some("0"),
        "2" => Some("0.00"),
        "9" => Some("0%"),
        "10" => Some("0.00%"),
        "14" => Some("mm-dd-yy"),
        "49" => Some("@"),
        _ => None,
    }
}

fn relationship_attribute<'a>(element: &'a XmlElement, local_name: &str) -> Option<&'a str> {
    element
        .attribute_ns(RELATIONSHIPS_NAMESPACE, local_name)
        .or_else(|| element.attribute_ns(STRICT_RELATIONSHIPS_NAMESPACE, local_name))
}

fn require_spreadsheet_element(
    element: &XmlElement,
    local_name: &str,
    part: &str,
) -> UseResult<()> {
    if element.local_name == local_name
        && matches!(
            element.namespace.as_deref(),
            Some(SPREADSHEET_NAMESPACE | STRICT_SPREADSHEET_NAMESPACE)
        )
    {
        return Ok(());
    }
    Err(semantic_error(
        "use.office.spreadsheet_xml_invalid",
        format!("Spreadsheet part '{part}' has an unexpected root element."),
    ))
}
