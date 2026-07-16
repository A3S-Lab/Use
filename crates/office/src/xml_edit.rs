use std::collections::BTreeMap;
use std::ops::Range;

use a3s_use_core::{UseError, UseResult};
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::discovery::office_error;
use crate::xml::{LosslessXmlPart, XmlEncoding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexedXmlElement {
    pub qualified_name: String,
    pub local_name: String,
    pub attributes: BTreeMap<String, String>,
    pub qualified_attributes: BTreeMap<String, String>,
    pub full_range: Range<usize>,
    pub start_tag_range: Range<usize>,
    pub content_range: Range<usize>,
    pub children: Vec<IndexedXmlElement>,
    pub empty: bool,
}

impl IndexedXmlElement {
    pub fn child(&self, local_name: &str, position: usize) -> Option<&Self> {
        if position == 0 {
            return None;
        }
        self.children
            .iter()
            .filter(|child| child.local_name == local_name)
            .nth(position - 1)
    }

    pub fn child_any(&self, local_names: &[&str], position: usize) -> Option<&Self> {
        if position == 0 {
            return None;
        }
        self.children
            .iter()
            .filter(|child| local_names.contains(&child.local_name.as_str()))
            .nth(position - 1)
    }

    pub fn descendants_named<'a>(
        &'a self,
        local_name: &str,
        output: &mut Vec<&'a IndexedXmlElement>,
    ) {
        for child in &self.children {
            if child.local_name == local_name {
                output.push(child);
            }
            child.descendants_named(local_name, output);
        }
    }

    pub fn descendant(&self, local_name: &str) -> Option<&Self> {
        for child in &self.children {
            if child.local_name == local_name {
                return Some(child);
            }
            if let Some(found) = child.descendant(local_name) {
                return Some(found);
            }
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XmlPatch {
    range: Range<usize>,
    replacement: Vec<u8>,
}

impl XmlPatch {
    pub fn new(range: Range<usize>, replacement: impl Into<Vec<u8>>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
        }
    }
}

pub(crate) fn index_xml(part: &LosslessXmlPart) -> UseResult<IndexedXmlElement> {
    require_utf8(part)?;
    let mut reader = Reader::from_reader(part.parse_bytes());
    reader.config_mut().check_end_names = true;
    reader.config_mut().check_comments = true;
    let mut stack = Vec::<IndexedXmlElement>::new();
    let mut root = None;
    loop {
        let event_start = usize::try_from(reader.buffer_position()).map_err(|_| {
            edit_error(part.name(), "XML byte position does not fit this platform.")
        })?;
        let event = reader.read_event().map_err(|error| {
            edit_error(
                part.name(),
                format!("Failed to index XML for mutation: {error}"),
            )
        })?;
        let event_end = usize::try_from(reader.buffer_position()).map_err(|_| {
            edit_error(part.name(), "XML byte position does not fit this platform.")
        })?;
        match event {
            Event::Start(start) => {
                stack.push(indexed_element(
                    part.name(),
                    &start,
                    &reader,
                    event_start..event_end,
                    event_end..event_end,
                    false,
                )?);
            }
            Event::Empty(start) => {
                let element = indexed_element(
                    part.name(),
                    &start,
                    &reader,
                    event_start..event_end,
                    event_end..event_end,
                    true,
                )?;
                append_element(part.name(), &mut stack, &mut root, element)?;
            }
            Event::End(_) => {
                let mut element = stack.pop().ok_or_else(|| {
                    edit_error(
                        part.name(),
                        "XML mutation index found an unmatched end tag.",
                    )
                })?;
                element.content_range.end = event_start;
                element.full_range.end = event_end;
                append_element(part.name(), &mut stack, &mut root, element)?;
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(edit_error(
            part.name(),
            "XML mutation index ended with unclosed elements.",
        ));
    }
    root.ok_or_else(|| edit_error(part.name(), "XML mutation index has no root element."))
}

pub(crate) fn apply_patches(
    part: &LosslessXmlPart,
    mut patches: Vec<XmlPatch>,
) -> UseResult<Vec<u8>> {
    require_utf8(part)?;
    // Insertions at the same byte as a replacement must be emitted first.
    // Sorting by the empty range's smaller end makes that ordering explicit.
    patches.sort_by_key(|patch| (patch.range.start, patch.range.end));
    let parse_bytes = part.parse_bytes();
    let mut previous_end = 0_usize;
    for patch in &patches {
        if patch.range.start > patch.range.end
            || patch.range.end > parse_bytes.len()
            || patch.range.start < previous_end
        {
            return Err(edit_error(
                part.name(),
                "XML mutation patches overlap or fall outside the part.",
            ));
        }
        previous_end = patch.range.end;
    }

    let replacement_bytes = patches
        .iter()
        .try_fold(0_usize, |total, patch| {
            total.checked_add(patch.replacement.len())
        })
        .ok_or_else(|| edit_error(part.name(), "XML mutation output size overflowed."))?;
    let removed_bytes = patches
        .iter()
        .try_fold(0_usize, |total, patch| total.checked_add(patch.range.len()))
        .ok_or_else(|| edit_error(part.name(), "XML mutation range size overflowed."))?;
    let capacity = parse_bytes
        .len()
        .checked_sub(removed_bytes)
        .and_then(|size| size.checked_add(replacement_bytes))
        .ok_or_else(|| edit_error(part.name(), "XML mutation output size overflowed."))?;
    let mut edited = Vec::with_capacity(capacity + part.raw_prefix().len());
    edited.extend_from_slice(part.raw_prefix());
    let mut cursor = 0_usize;
    for patch in patches {
        edited.extend_from_slice(&parse_bytes[cursor..patch.range.start]);
        edited.extend_from_slice(&patch.replacement);
        cursor = patch.range.end;
    }
    edited.extend_from_slice(&parse_bytes[cursor..]);
    LosslessXmlPart::parse(part.name().to_string(), edited.clone()).map_err(|error| {
        edit_error(
            part.name(),
            format!("XML mutation produced an invalid part: {}", error.message),
        )
    })?;
    Ok(edited)
}

pub(crate) fn replace_text_descendants(
    part: &LosslessXmlPart,
    element: &IndexedXmlElement,
    text_element_name: &str,
    text: &str,
    insertion: Option<String>,
) -> UseResult<Vec<u8>> {
    let mut text_elements = Vec::new();
    element.descendants_named(text_element_name, &mut text_elements);
    if text_elements.is_empty() {
        let insertion = insertion.ok_or_else(|| {
            edit_error(
                part.name(),
                format!(
                    "XML element '{}' has no editable '{text_element_name}' descendant.",
                    element.local_name
                ),
            )
        })?;
        return insert_child(part, element, insertion);
    }
    let escaped = escape_text(text);
    let preserve_space =
        text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace);
    let mut patches = Vec::with_capacity(text_elements.len());
    for (index, text_element) in text_elements.into_iter().enumerate() {
        if index == 0 && preserve_space {
            let replacement = format!(
                "<{} xml:space=\"preserve\">{escaped}</{}>",
                text_element.qualified_name, text_element.qualified_name
            );
            patches.push(XmlPatch::new(text_element.full_range.clone(), replacement));
        } else {
            patches.push(XmlPatch::new(
                text_element.content_range.clone(),
                if index == 0 {
                    escaped.as_bytes().to_vec()
                } else {
                    Vec::new()
                },
            ));
        }
    }
    apply_patches(part, patches)
}

pub(crate) fn insert_child(
    part: &LosslessXmlPart,
    element: &IndexedXmlElement,
    child: impl AsRef<[u8]>,
) -> UseResult<Vec<u8>> {
    if !element.empty {
        return apply_patches(
            part,
            vec![XmlPatch::new(
                element.content_range.end..element.content_range.end,
                child.as_ref().to_vec(),
            )],
        );
    }

    let start_tag = part
        .parse_bytes()
        .get(element.start_tag_range.clone())
        .ok_or_else(|| edit_error(part.name(), "Empty XML element range is invalid."))?;
    let slash = start_tag
        .iter()
        .rposition(|byte| *byte == b'/')
        .filter(|slash| {
            start_tag.get(slash + 1..).is_some_and(|suffix| {
                suffix
                    .iter()
                    .all(|byte| *byte == b'>' || byte.is_ascii_whitespace())
            })
        })
        .ok_or_else(|| edit_error(part.name(), "Empty XML element has no '/>' terminator."))?;
    let mut replacement = Vec::with_capacity(
        start_tag
            .len()
            .saturating_add(child.as_ref().len())
            .saturating_add(element.qualified_name.len())
            .saturating_add(3),
    );
    replacement.extend_from_slice(&start_tag[..slash]);
    replacement.push(b'>');
    replacement.extend_from_slice(child.as_ref());
    replacement.extend_from_slice(b"</");
    replacement.extend_from_slice(element.qualified_name.as_bytes());
    replacement.push(b'>');
    apply_patches(
        part,
        vec![XmlPatch::new(element.full_range.clone(), replacement)],
    )
}

pub(crate) fn escape_text(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}

fn indexed_element(
    part_name: &str,
    start: &quick_xml::events::BytesStart<'_>,
    reader: &quick_xml::reader::Reader<&[u8]>,
    start_tag_range: Range<usize>,
    content_range: Range<usize>,
    empty: bool,
) -> UseResult<IndexedXmlElement> {
    let qualified_name = std::str::from_utf8(start.name().as_ref())
        .map(str::to_string)
        .map_err(|error| {
            edit_error(
                part_name,
                format!("XML mutation element name is not UTF-8: {error}"),
            )
        })?;
    let local_name = std::str::from_utf8(start.local_name().as_ref())
        .map(str::to_string)
        .map_err(|error| {
            edit_error(
                part_name,
                format!("XML mutation element name is not UTF-8: {error}"),
            )
        })?;
    let mut attributes = BTreeMap::new();
    let mut qualified_attributes = BTreeMap::new();
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|error| {
            edit_error(
                part_name,
                format!("Invalid XML mutation attribute: {error}"),
            )
        })?;
        let qualified_name = std::str::from_utf8(attribute.key.as_ref())
            .map(str::to_string)
            .map_err(|error| {
                edit_error(
                    part_name,
                    format!("XML mutation attribute name is not UTF-8: {error}"),
                )
            })?;
        let local_name = std::str::from_utf8(attribute.key.local_name().as_ref())
            .map(str::to_string)
            .map_err(|error| {
                edit_error(
                    part_name,
                    format!("XML mutation attribute name is not UTF-8: {error}"),
                )
            })?;
        let value = attribute
            .decode_and_unescape_value(reader.decoder())
            .map_err(|error| {
                edit_error(
                    part_name,
                    format!("XML mutation attribute cannot be decoded: {error}"),
                )
            })?
            .into_owned();
        qualified_attributes.insert(qualified_name, value.clone());
        attributes.entry(local_name).or_insert(value);
    }
    Ok(IndexedXmlElement {
        qualified_name,
        local_name,
        attributes,
        qualified_attributes,
        full_range: start_tag_range.start..start_tag_range.end,
        start_tag_range,
        content_range,
        children: Vec::new(),
        empty,
    })
}

fn append_element(
    part_name: &str,
    stack: &mut [IndexedXmlElement],
    root: &mut Option<IndexedXmlElement>,
    element: IndexedXmlElement,
) -> UseResult<()> {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(element);
    } else if root.replace(element).is_some() {
        return Err(edit_error(
            part_name,
            "XML mutation index contains more than one root element.",
        ));
    }
    Ok(())
}

fn require_utf8(part: &LosslessXmlPart) -> UseResult<()> {
    if part.encoding() == XmlEncoding::Utf8 {
        Ok(())
    } else {
        Err(edit_error(
            part.name(),
            "Loss-preserving mutation of UTF-16 XML is not available yet; read remains supported.",
        ))
    }
}

fn edit_error(part_name: &str, message: impl Into<String>) -> UseError {
    office_error("use.office.xml_edit_invalid", message).with_detail("part", part_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_patch_preserves_untouched_prefixes_attributes_and_siblings() {
        let source = br#"<?xml version="1.0"?><w:root xmlns:w="urn:w"><w:p odd="keep"><w:t>before</w:t></w:p><!--exact--><x:unknown xmlns:x="urn:x" x:a="1" /></w:root>"#;
        let part = LosslessXmlPart::parse("word/document.xml", source.as_slice()).unwrap();
        let index = index_xml(&part).unwrap();
        let paragraph = index.child("p", 1).unwrap();

        let edited = replace_text_descendants(&part, paragraph, "t", " after & < ", None).unwrap();

        let edited = String::from_utf8(edited).unwrap();
        assert!(edited.contains(
            r#"<w:p odd="keep"><w:t xml:space="preserve"> after &amp; &lt; </w:t></w:p>"#
        ));
        assert!(edited.contains(r#"<!--exact--><x:unknown xmlns:x="urn:x" x:a="1" />"#));
    }

    #[test]
    fn insert_child_expands_an_empty_element_without_moving_its_siblings() {
        let source = br#"<?xml version="1.0"?><w:root xmlns:w="urn:w"><w:p odd="keep" /><!--exact--><w:tail/></w:root>"#;
        let part = LosslessXmlPart::parse("word/document.xml", source.as_slice()).unwrap();
        let index = index_xml(&part).unwrap();
        let paragraph = index.child("p", 1).unwrap();

        let edited = insert_child(&part, paragraph, "<w:r><w:t>text</w:t></w:r>").unwrap();
        let edited = String::from_utf8(edited).unwrap();

        assert!(edited
            .contains("<w:p odd=\"keep\" ><w:r><w:t>text</w:t></w:r></w:p><!--exact--><w:tail/>"));
    }

    #[test]
    fn index_preserves_qualified_attributes_with_the_same_local_name() {
        let source =
            br#"<p:root xmlns:p="urn:p" xmlns:r="urn:r"><p:item id="256" r:id="rId1"/></p:root>"#;
        let part = LosslessXmlPart::parse("ppt/presentation.xml", source.as_slice()).unwrap();
        let index = index_xml(&part).unwrap();
        let item = index.child("item", 1).unwrap();

        assert_eq!(item.qualified_attributes["id"], "256");
        assert_eq!(item.qualified_attributes["r:id"], "rId1");
        assert_eq!(item.attributes["id"], "256");
    }

    #[test]
    fn patcher_rejects_overlaps_and_utf16_mutation() {
        let part = LosslessXmlPart::parse("part.xml", b"<root>text</root>".as_slice()).unwrap();
        assert_eq!(
            apply_patches(
                &part,
                vec![
                    XmlPatch::new(1..4, b"x".to_vec()),
                    XmlPatch::new(3..5, b"y".to_vec())
                ]
            )
            .unwrap_err()
            .code,
            "use.office.xml_edit_invalid"
        );

        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend("<root/>".encode_utf16().flat_map(u16::to_le_bytes));
        let part = LosslessXmlPart::parse("part.xml", utf16).unwrap();
        assert_eq!(
            index_xml(&part).unwrap_err().code,
            "use.office.xml_edit_invalid"
        );
    }
}
