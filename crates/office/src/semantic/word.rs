use std::collections::BTreeMap;

use a3s_use_core::UseResult;

use super::{semantic_error, DocumentNode, OfficeNodeType};
use crate::xml_tree::{parse_xml_tree, XmlElement, XmlNode};
use crate::{NativeOfficePackage, OpcPackageModel, RelationshipSource, RelationshipTarget};

const WORD_NAMESPACE: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const STRICT_WORD_NAMESPACE: &str = "http://purl.oclc.org/ooxml/wordprocessingml/main";

pub(super) fn read(
    package: &NativeOfficePackage,
    opc: &OpcPackageModel,
) -> UseResult<DocumentNode> {
    let part = package.xml_part("word/document.xml")?;
    let document = parse_xml_tree(&part)?;
    require_word_element(&document, "document", part.name())?;
    let body = document.child("body").ok_or_else(|| {
        semantic_error(
            "use.office.word_body_missing",
            "Word document has no w:body element.",
        )
    })?;
    let styles = read_styles(package)?;

    let mut root = DocumentNode::new("/", "document", OfficeNodeType::Document);
    let mut body_node = DocumentNode::new("/body", "body", OfficeNodeType::Body);
    read_block_children(body, "/body", &styles, &mut body_node.children);
    body_node.text = join_block_text(&body_node.children);
    root.text = body_node.text.clone();
    root.children.push(body_node);
    read_headers_and_footers(package, opc, &styles, &mut root)?;
    Ok(root)
}

fn read_headers_and_footers(
    package: &NativeOfficePackage,
    opc: &OpcPackageModel,
    styles: &BTreeMap<String, String>,
    root: &mut DocumentNode,
) -> UseResult<()> {
    let source = RelationshipSource::Part {
        part_name: "word/document.xml".to_string(),
    };
    let mut header_index = 0_usize;
    let mut footer_index = 0_usize;
    for relationship in opc.relationships().relationships_from(&source) {
        let (tag, node_type, index) = if relationship.relationship_type.ends_with("/header") {
            header_index += 1;
            ("header", OfficeNodeType::Header, header_index)
        } else if relationship.relationship_type.ends_with("/footer") {
            footer_index += 1;
            ("footer", OfficeNodeType::Footer, footer_index)
        } else {
            continue;
        };
        let RelationshipTarget::Internal { part_name, .. } = &relationship.target else {
            continue;
        };
        if !package.contains_part(part_name) {
            continue;
        }
        let part = package.xml_part(part_name)?;
        let container = parse_xml_tree(&part)?;
        let path = format!("/{tag}[{index}]");
        let mut node = DocumentNode::new(&path, tag, node_type);
        read_block_children(&container, &path, styles, &mut node.children);
        node.text = join_block_text(&node.children);
        root.children.push(node);
    }
    Ok(())
}

fn read_styles(package: &NativeOfficePackage) -> UseResult<BTreeMap<String, String>> {
    if !package.contains_part("word/styles.xml") {
        return Ok(BTreeMap::new());
    }
    let part = package.xml_part("word/styles.xml")?;
    let root = parse_xml_tree(&part)?;
    require_word_element(&root, "styles", part.name())?;
    let mut styles = BTreeMap::new();
    for style in root.children_named("style") {
        let Some(style_id) = style.attribute("styleId") else {
            continue;
        };
        let name = style
            .child("name")
            .and_then(|name| name.attribute("val"))
            .unwrap_or(style_id);
        styles.insert(style_id.to_string(), name.to_string());
    }
    Ok(styles)
}

fn read_block_children(
    container: &XmlElement,
    parent_path: &str,
    styles: &BTreeMap<String, String>,
    output: &mut Vec<DocumentNode>,
) {
    let mut paragraph_index = output
        .iter()
        .filter(|node| node.node_type == OfficeNodeType::Paragraph)
        .count();
    let mut table_index = output
        .iter()
        .filter(|node| node.node_type == OfficeNodeType::Table)
        .count();
    for element in container.child_elements() {
        match element.local_name.as_str() {
            "p" => {
                paragraph_index += 1;
                output.push(read_paragraph(
                    element,
                    &format!("{parent_path}/p[{paragraph_index}]"),
                    styles,
                ));
            }
            "tbl" => {
                table_index += 1;
                output.push(read_table(
                    element,
                    &format!("{parent_path}/tbl[{table_index}]"),
                    styles,
                ));
            }
            "sdt" | "customXml" | "ins" | "del" | "moveFrom" | "moveTo" => {
                let nested = element.child("sdtContent").unwrap_or(element);
                read_block_children(nested, parent_path, styles, output);
                paragraph_index = output
                    .iter()
                    .filter(|node| node.node_type == OfficeNodeType::Paragraph)
                    .count();
                table_index = output
                    .iter()
                    .filter(|node| node.node_type == OfficeNodeType::Table)
                    .count();
            }
            _ => {}
        }
    }
}

fn read_paragraph(
    paragraph: &XmlElement,
    path: &str,
    styles: &BTreeMap<String, String>,
) -> DocumentNode {
    let mut node = DocumentNode::new(path, "p", OfficeNodeType::Paragraph);
    if let Some(properties) = paragraph.child("pPr") {
        apply_paragraph_properties(properties, styles, &mut node);
    }
    let mut run_index = 0_usize;
    let mut hyperlink_index = 0_usize;
    for child in paragraph.child_elements() {
        match child.local_name.as_str() {
            "r" => {
                run_index += 1;
                node.children
                    .push(read_run(child, &format!("{path}/r[{run_index}]")));
            }
            "hyperlink" => {
                hyperlink_index += 1;
                let hyperlink_path = format!("{path}/hyperlink[{hyperlink_index}]");
                let mut hyperlink =
                    DocumentNode::new(&hyperlink_path, "hyperlink", OfficeNodeType::Hyperlink);
                if let Some(id) = child.attribute("id") {
                    hyperlink.format.insert("relationshipId".into(), id.into());
                }
                for run in child.children_named("r") {
                    run_index += 1;
                    hyperlink
                        .children
                        .push(read_run(run, &format!("{hyperlink_path}/r[{run_index}]")));
                }
                hyperlink.text = hyperlink
                    .children
                    .iter()
                    .map(|child| child.text.as_str())
                    .collect();
                node.children.push(hyperlink);
            }
            "fldSimple" | "sdt" | "smartTag" | "ins" | "del" => {
                append_nested_runs(child, path, &mut run_index, &mut node.children);
            }
            _ => {}
        }
    }
    node.text = node
        .children
        .iter()
        .map(|child| child.text.as_str())
        .collect();
    node
}

fn append_nested_runs(
    element: &XmlElement,
    paragraph_path: &str,
    run_index: &mut usize,
    output: &mut Vec<DocumentNode>,
) {
    for child in element.child_elements() {
        if child.local_name == "r" {
            *run_index += 1;
            output.push(read_run(child, &format!("{paragraph_path}/r[{run_index}]")));
        } else {
            append_nested_runs(child, paragraph_path, run_index, output);
        }
    }
}

fn read_run(run: &XmlElement, path: &str) -> DocumentNode {
    let mut node = DocumentNode::new(path, "r", OfficeNodeType::Run);
    if let Some(properties) = run.child("rPr") {
        apply_run_properties(properties, &mut node);
    }
    node.text = word_text(run);
    node
}

fn read_table(table: &XmlElement, path: &str, styles: &BTreeMap<String, String>) -> DocumentNode {
    let mut node = DocumentNode::new(path, "tbl", OfficeNodeType::Table);
    if let Some(style) = table
        .child("tblPr")
        .and_then(|properties| properties.child("tblStyle"))
        .and_then(|style| style.attribute("val"))
    {
        node.style = Some(style.to_string());
        if let Some(name) = styles.get(style) {
            node.format.insert("styleName".into(), name.clone());
        }
    }
    for (row_offset, row) in table.children_named("tr").enumerate() {
        let row_path = format!("{path}/tr[{}]", row_offset + 1);
        let mut row_node = DocumentNode::new(&row_path, "tr", OfficeNodeType::TableRow);
        for (cell_offset, cell) in row.children_named("tc").enumerate() {
            let cell_path = format!("{row_path}/tc[{}]", cell_offset + 1);
            let mut cell_node = DocumentNode::new(&cell_path, "tc", OfficeNodeType::TableCell);
            read_block_children(cell, &cell_path, styles, &mut cell_node.children);
            cell_node.text = join_block_text(&cell_node.children);
            row_node.children.push(cell_node);
        }
        row_node.text = row_node
            .children
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<Vec<_>>()
            .join("\t");
        node.children.push(row_node);
    }
    node.text = node
        .children
        .iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    node
}

fn apply_paragraph_properties(
    properties: &XmlElement,
    styles: &BTreeMap<String, String>,
    node: &mut DocumentNode,
) {
    if let Some(style_id) = properties
        .child("pStyle")
        .and_then(|style| style.attribute("val"))
    {
        node.style = Some(style_id.to_string());
        if let Some(name) = styles.get(style_id) {
            node.format.insert("styleName".into(), name.clone());
        }
    }
    copy_child_value(properties, "jc", "alignment", node);
    if let Some(numbering) = properties.child("numPr") {
        copy_child_value(numbering, "numId", "numId", node);
        copy_child_value(numbering, "ilvl", "numLevel", node);
    }
    if let Some(direction) = properties.child("bidi") {
        let direction = if bool_value(direction) { "rtl" } else { "ltr" };
        node.format.insert("direction".into(), direction.into());
    }
}

fn apply_run_properties(properties: &XmlElement, node: &mut DocumentNode) {
    for (child, key) in [
        ("b", "bold"),
        ("i", "italic"),
        ("strike", "strike"),
        ("vanish", "hidden"),
        ("rtl", "rtl"),
    ] {
        if let Some(property) = properties.child(child) {
            node.format
                .insert(key.into(), bool_value(property).to_string());
        }
    }
    if let Some(fonts) = properties.child("rFonts") {
        for (attribute, key) in [
            ("ascii", "font"),
            ("eastAsia", "font.eastAsia"),
            ("cs", "font.complexScript"),
        ] {
            if let Some(value) = fonts.attribute(attribute) {
                node.format.insert(key.into(), value.into());
            }
        }
    }
    if let Some(size) = properties
        .child("sz")
        .and_then(|size| size.attribute("val"))
        .and_then(|value| value.parse::<f64>().ok())
    {
        node.format
            .insert("size".into(), format!("{}pt", size / 2.0));
    }
    copy_child_value(properties, "color", "color", node);
    if let Some(language) = properties.child("lang") {
        for (attribute, key) in [
            ("val", "language"),
            ("eastAsia", "language.eastAsia"),
            ("bidi", "language.complexScript"),
        ] {
            if let Some(value) = language.attribute(attribute) {
                node.format.insert(key.into(), value.into());
            }
        }
    }
}

fn copy_child_value(parent: &XmlElement, child_name: &str, key: &str, node: &mut DocumentNode) {
    if let Some(value) = parent
        .child(child_name)
        .and_then(|child| child.attribute("val"))
    {
        node.format.insert(key.to_string(), value.to_string());
    }
}

fn bool_value(element: &XmlElement) -> bool {
    !matches!(
        element
            .attribute("val")
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("false" | "0" | "off" | "no")
    )
}

fn word_text(element: &XmlElement) -> String {
    let mut output = String::new();
    append_word_text(element, &mut output);
    output
}

fn append_word_text(element: &XmlElement, output: &mut String) {
    match element.local_name.as_str() {
        "t" | "delText" | "instrText" => output.push_str(&direct_text(element)),
        "tab" => output.push('\t'),
        "br" | "cr" => output.push('\n'),
        "noBreakHyphen" => output.push('\u{2011}'),
        "softHyphen" => output.push('\u{00ad}'),
        _ => {
            for child in element.child_elements() {
                append_word_text(child, output);
            }
        }
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

fn join_block_text(children: &[DocumentNode]) -> String {
    children
        .iter()
        .map(|child| child.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn require_word_element(element: &XmlElement, local_name: &str, part: &str) -> UseResult<()> {
    if element.local_name == local_name
        && matches!(
            element.namespace.as_deref(),
            Some(WORD_NAMESPACE | STRICT_WORD_NAMESPACE)
        )
    {
        return Ok(());
    }
    Err(semantic_error(
        "use.office.word_xml_invalid",
        format!("Word part '{part}' has an unexpected root element."),
    ))
}
