use a3s_use_core::UseResult;

use super::{
    editor_error, escape_attribute, node_not_found, parse_segments, prefix,
    preserve_space_attribute, qualified,
};
use crate::semantic::{NativeOfficeDocument, OfficeNodeType};
use crate::xml_edit::{
    escape_text, index_xml, insert_child, replace_text_descendants, IndexedXmlElement,
};
use crate::{DocumentKind, NativeOfficePackage};

pub(super) fn add_slide(
    package: &mut NativeOfficePackage,
    parent: &str,
    title: &str,
) -> UseResult<String> {
    if package.kind() != DocumentKind::Presentation {
        return Err(editor_error(
            "use.office.mutation_type_unsupported",
            "Native add-slide is available only for Presentation documents.",
        ));
    }
    if parent != "/" {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Native Presentation slides can currently be added only to /.",
        ));
    }
    let layout_part = package
        .part_names()
        .find(|name| name.starts_with("ppt/slideLayouts/slideLayout") && name.ends_with(".xml"))
        .map(str::to_string)
        .ok_or_else(|| {
            editor_error(
                "use.office.presentation_layout_missing",
                "Presentation has no slide layout for a native slide mutation.",
            )
        })?;
    let number = (1..=package.limits().max_entries.saturating_add(1))
        .find(|number| !package.contains_part(&format!("ppt/slides/slide{number}.xml")))
        .ok_or_else(|| {
            editor_error(
                "use.office.presentation_slide_limit",
                "Presentation has no available native slide part number.",
            )
        })?;
    let slide_part = format!("ppt/slides/slide{number}.xml");
    let slide_relationship_part = format!("ppt/slides/_rels/slide{number}.xml.rels");
    let presentation = package.xml_part("ppt/presentation.xml")?;
    let index = index_xml(&presentation)?;
    let slide_list = index
        .child("sldIdLst", 1)
        .ok_or_else(|| node_not_found("/"))?;
    let position = slide_list
        .children
        .iter()
        .filter(|child| child.local_name == "sldId")
        .count()
        + 1;
    let slide_id = slide_list
        .children
        .iter()
        .filter(|child| child.local_name == "sldId")
        .filter_map(|child| child.qualified_attributes.get("id"))
        .filter_map(|id| id.parse::<u32>().ok())
        .max()
        .unwrap_or(255)
        .max(255)
        .checked_add(1)
        .ok_or_else(|| {
            editor_error(
                "use.office.presentation_slide_limit",
                "Presentation slide IDs are exhausted.",
            )
        })?;

    crate::opc_edit::add_content_type_override(
        package,
        &slide_part,
        "application/vnd.openxmlformats-officedocument.presentationml.slide+xml",
    )?;
    package.set_part(&slide_part, slide_xml(title).into_bytes())?;
    let layout_target = format!(
        "../{}",
        layout_part.strip_prefix("ppt/").unwrap_or(&layout_part)
    );
    crate::opc_edit::add_relationship(
        package,
        &slide_relationship_part,
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout",
        &layout_target,
    )?;
    let relationship_id = crate::opc_edit::add_relationship(
        package,
        "ppt/_rels/presentation.xml.rels",
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide",
        &format!("slides/slide{number}.xml"),
    )?;
    let slide_tag = qualified(prefix(&slide_list.qualified_name), "sldId");
    let fragment = format!(
        "<{slide_tag} xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" id=\"{slide_id}\" r:id=\"{}\"/>",
        escape_attribute(&relationship_id)
    );
    let edited = insert_child(&presentation, slide_list, fragment)?;
    package.set_part("ppt/presentation.xml", edited)?;
    Ok(format!("/slide[{position}]"))
}

pub(super) fn add_shape(
    package: &mut NativeOfficePackage,
    parent: &str,
    text: &str,
) -> UseResult<String> {
    if package.kind() != DocumentKind::Presentation {
        return Err(editor_error(
            "use.office.mutation_type_unsupported",
            "Native add-shape is available only for Presentation documents.",
        ));
    }
    let snapshot = NativeOfficeDocument::from_package(package.clone())?;
    let slide = snapshot.get(parent, 0)?;
    if slide.node_type != OfficeNodeType::Slide {
        return Err(editor_error(
            "use.office.mutation_path_unsupported",
            "Native Presentation shapes require a slide parent.",
        ));
    }
    let part_name = slide.format.get("part").ok_or_else(|| {
        editor_error(
            "use.office.presentation_slide_invalid",
            "Presentation semantic slide has no source part.",
        )
    })?;
    let part = package.xml_part(part_name)?;
    let index = index_xml(&part)?;
    let shape_tree = index
        .descendant("spTree")
        .ok_or_else(|| node_not_found(parent))?;
    let position = shape_tree
        .children
        .iter()
        .filter(|child| child.local_name == "sp")
        .count()
        + 1;
    let mut non_visual = Vec::new();
    shape_tree.descendants_named("cNvPr", &mut non_visual);
    let id = non_visual
        .into_iter()
        .filter_map(|element| element.attributes.get("id"))
        .filter_map(|id| id.parse::<u32>().ok())
        .max()
        .unwrap_or(1)
        .checked_add(1)
        .ok_or_else(|| {
            editor_error(
                "use.office.presentation_shape_limit",
                "Presentation shape IDs are exhausted.",
            )
        })?;
    let offset = u32::try_from(position.saturating_sub(1)).map_err(|_| {
        editor_error(
            "use.office.presentation_shape_limit",
            "Presentation shape position does not fit the supported coordinate range.",
        )
    })? % 4;
    let y = 1_828_800_i64 + i64::from(offset) * 1_143_000;
    let fragment = text_shape_xml(TextShape {
        id,
        name: &format!("TextBox {id}"),
        text,
        x: 914_400,
        y,
        width: 10_363_200,
        height: 914_400,
        font_size: 2_000,
    });
    let edited = insert_child(&part, shape_tree, fragment)?;
    package.set_part(part_name, edited)?;
    Ok(format!("{}/shape[{position}]", slide.path))
}

pub(super) fn set_text(package: &mut NativeOfficePackage, path: &str, text: &str) -> UseResult<()> {
    let snapshot = NativeOfficeDocument::from_package(package.clone())?;
    let requested = snapshot.get(path, 0)?;
    let slide_path = path
        .split('/')
        .find(|segment| segment.starts_with("slide["))
        .map(|segment| format!("/{segment}"))
        .ok_or_else(|| {
            editor_error(
                "use.office.mutation_path_unsupported",
                "Presentation set-text requires a slide element path.",
            )
        })?;
    let slide = snapshot.get(&slide_path, 0)?;
    let part_name = slide.format.get("part").ok_or_else(|| {
        editor_error(
            "use.office.presentation_slide_invalid",
            "Presentation semantic slide has no source part.",
        )
    })?;
    let part = package.xml_part(part_name)?;
    let index = index_xml(&part)?;
    let target = locate_path(&index, &requested.path)?;
    let edited = replace_text_descendants(&part, target, "t", text, None)?;
    package.set_part(part_name, edited)
}

pub(super) fn remove(package: &mut NativeOfficePackage, path: &str) -> UseResult<()> {
    let snapshot = NativeOfficeDocument::from_package(package.clone())?;
    let requested = snapshot.get(path, 0)?;
    match requested.node_type {
        OfficeNodeType::Shape => remove_shape(package, &snapshot, &requested.path),
        OfficeNodeType::Slide => remove_slide(package, &requested.path),
        _ => Err(editor_error(
            "use.office.mutation_type_unsupported",
            "Native Presentation remove currently supports slides and shapes.",
        )),
    }
}

fn remove_shape(
    package: &mut NativeOfficePackage,
    snapshot: &NativeOfficeDocument,
    path: &str,
) -> UseResult<()> {
    let slide_path = path
        .split('/')
        .find(|segment| segment.starts_with("slide["))
        .map(|segment| format!("/{segment}"))
        .ok_or_else(|| node_not_found(path))?;
    let slide = snapshot.get(&slide_path, 0)?;
    let part_name = slide.format.get("part").ok_or_else(|| {
        editor_error(
            "use.office.presentation_slide_invalid",
            "Presentation semantic slide has no source part.",
        )
    })?;
    let part = package.xml_part(part_name)?;
    let index = index_xml(&part)?;
    let target = locate_path(&index, path)?;
    let edited = crate::xml_edit::apply_patches(
        &part,
        vec![crate::xml_edit::XmlPatch::new(
            target.full_range.clone(),
            Vec::new(),
        )],
    )?;
    package.set_part(part_name, edited)
}

fn remove_slide(package: &mut NativeOfficePackage, path: &str) -> UseResult<()> {
    let position = path
        .strip_prefix("/slide[")
        .and_then(|position| position.strip_suffix(']'))
        .and_then(|position| position.parse::<usize>().ok())
        .filter(|position| *position > 0)
        .ok_or_else(|| node_not_found(path))?;
    let snapshot = NativeOfficeDocument::from_package(package.clone())?;
    let slide = snapshot.get(path, 0)?;
    let part_name = slide.format.get("part").cloned().ok_or_else(|| {
        editor_error(
            "use.office.presentation_slide_invalid",
            "Presentation semantic slide has no source part.",
        )
    })?;
    let presentation = package.xml_part("ppt/presentation.xml")?;
    let index = index_xml(&presentation)?;
    let slide_list = index
        .child("sldIdLst", 1)
        .ok_or_else(|| node_not_found(path))?;
    let slide_id = slide_list
        .child("sldId", position)
        .ok_or_else(|| node_not_found(path))?;
    let relationship_id = slide_id
        .qualified_attributes
        .iter()
        .find(|(name, _)| name.ends_with(":id"))
        .map(|(_, value)| value.clone())
        .ok_or_else(|| {
            editor_error(
                "use.office.presentation_slide_invalid",
                format!("Presentation slide '{path}' has no relationship ID."),
            )
        })?;
    let edited = crate::xml_edit::apply_patches(
        &presentation,
        vec![crate::xml_edit::XmlPatch::new(
            slide_id.full_range.clone(),
            Vec::new(),
        )],
    )?;
    crate::opc_edit::remove_relationship(
        package,
        "ppt/_rels/presentation.xml.rels",
        &relationship_id,
    )?;
    crate::opc_edit::remove_content_type_override(package, &part_name)?;
    let (directory, file_name) = part_name.rsplit_once('/').ok_or_else(|| {
        editor_error(
            "use.office.presentation_slide_invalid",
            format!("Presentation slide part '{part_name}' has an invalid path."),
        )
    })?;
    let relationship_part = format!("{directory}/_rels/{file_name}.rels");
    package.remove_part(&relationship_part)?;
    if !package.remove_part(&part_name)? {
        return Err(node_not_found(path));
    }
    package.set_part("ppt/presentation.xml", edited)
}

fn slide_xml(title: &str) -> String {
    let shape = if title.is_empty() {
        String::new()
    } else {
        text_shape_xml(TextShape {
            id: 2,
            name: "Title 1",
            text: title,
            x: 914_400,
            y: 457_200,
            width: 10_363_200,
            height: 1_143_000,
            font_size: 3_200,
        })
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr>{shape}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
    )
}

struct TextShape<'a> {
    id: u32,
    name: &'a str,
    text: &'a str,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    font_size: u32,
}

fn text_shape_xml(shape: TextShape<'_>) -> String {
    let text = escape_text(shape.text);
    let space = preserve_space_attribute(&text);
    format!(
        "<p:sp><p:nvSpPr><p:cNvPr id=\"{}\" name=\"{}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\" sz=\"{}\"/><a:t{space}>{text}</a:t></a:r><a:endParaRPr lang=\"en-US\"/></a:p></p:txBody></p:sp>",
        shape.id,
        escape_attribute(shape.name),
        shape.x,
        shape.y,
        shape.width,
        shape.height,
        shape.font_size
    )
}

fn locate_path<'a>(root: &'a IndexedXmlElement, path: &str) -> UseResult<&'a IndexedXmlElement> {
    let segments = parse_segments(path)?;
    let mut segments = segments.into_iter();
    let slide = segments.next().ok_or_else(|| node_not_found(path))?;
    if slide.name != "slide" {
        return Err(node_not_found(path));
    }
    let mut current = root
        .descendant("spTree")
        .ok_or_else(|| node_not_found(path))?;
    for segment in segments {
        current = match segment.name.as_str() {
            "shape" => current
                .child("sp", segment.position.unwrap_or(1))
                .ok_or_else(|| node_not_found(path))?,
            "group" => current
                .child("grpSp", segment.position.unwrap_or(1))
                .ok_or_else(|| node_not_found(path))?,
            "paragraph" | "p" => current
                .descendant("txBody")
                .and_then(|body| body.child("p", segment.position.unwrap_or(1)))
                .ok_or_else(|| node_not_found(path))?,
            "run" | "r" => current
                .child_any(&["r", "fld"], segment.position.unwrap_or(1))
                .ok_or_else(|| node_not_found(path))?,
            name => {
                return Err(editor_error(
                    "use.office.mutation_path_unsupported",
                    format!(
                        "Presentation path element '{name}' is not supported for native mutation."
                    ),
                ));
            }
        };
    }
    Ok(current)
}
