use std::collections::BTreeMap;
use std::sync::Arc;

use a3s_use_core::{inspect_okf_bundle_files, OkfBundleFile, UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::okf_knowledge::OkfKnowledgeStageSpec;

pub(super) const INDEX_SCHEMA: &str = "a3s-knowledge-okf-fts5-v1";
const INDEX_DESCRIPTOR_SCHEMA: &str = "a3s.use.okf-sqlite-index.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IndexedDocument {
    pub concept_id: String,
    pub path: String,
    pub type_name: String,
    pub title: String,
    pub search_text: String,
    pub source_digest: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedIndex {
    pub digest: String,
    pub build_id: String,
    pub documents: Vec<IndexedDocument>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexDescriptor<'a> {
    schema: &'static str,
    bundle_digest: &'a str,
    documents: Vec<IndexDocumentDescriptor<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexDocumentDescriptor<'a> {
    concept_id: &'a str,
    path: &'a str,
    #[serde(rename = "type")]
    type_name: &'a str,
    title: &'a str,
    search_text: &'a str,
    source_digest: &'a str,
}

pub(super) fn prepare(
    spec: OkfKnowledgeStageSpec,
    files: Arc<[OkfBundleFile]>,
) -> UseResult<PreparedIndex> {
    spec.validate()?;
    let inspection = inspect_okf_bundle_files(
        spec.bundle.format_version,
        spec.bundle.limits.clone(),
        &files,
    )?;
    spec.bundle.verify_inspection(&inspection)?;

    let files = files
        .iter()
        .map(|file| (file.path.as_str(), file.content.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let mut documents = Vec::with_capacity(inspection.concepts.len());
    for concept in inspection.concepts {
        let content = files.get(concept.path.as_str()).ok_or_else(|| {
            index_error(format!(
                "The inspected OKF concept '{}' disappeared before indexing.",
                concept.path
            ))
        })?;
        let markdown = std::str::from_utf8(content).map_err(|error| {
            index_error(format!(
                "The inspected OKF concept '{}' is not UTF-8: {error}",
                concept.path
            ))
        })?;
        let body = strip_frontmatter(markdown);
        let title = first_heading(body).unwrap_or_else(|| fallback_title(&concept.id));
        let plain_text = markdown_text(body);
        let search_text =
            normalize_whitespace(&format!("{} {} {}", concept.type_name, title, plain_text));
        if search_text.is_empty() {
            return Err(index_error(format!(
                "The OKF concept '{}' produced an empty cited-search document.",
                concept.path
            )));
        }
        documents.push(IndexedDocument {
            concept_id: concept.id,
            path: concept.path,
            type_name: concept.type_name,
            title,
            search_text,
            source_digest: format!("sha256:{:x}", Sha256::digest(content)),
            content: content.to_vec(),
        });
    }
    documents.sort_by(|left, right| left.path.cmp(&right.path));

    let digest = descriptor_digest(&spec.bundle.content_digest, &documents)?;
    let build_id = format!("fts5-{}", &digest[7..23]);
    Ok(PreparedIndex {
        digest,
        build_id,
        documents,
    })
}

/// Rebuild the immutable descriptor retained by a projection. Audit uses the
/// same implementation as staging so changes to source digests or derived
/// search fields cannot be hidden behind a structurally healthy FTS table.
pub(super) fn descriptor_digest(
    bundle_digest: &str,
    documents: &[IndexedDocument],
) -> UseResult<String> {
    let descriptor = IndexDescriptor {
        schema: INDEX_DESCRIPTOR_SCHEMA,
        bundle_digest,
        documents: documents
            .iter()
            .map(|document| IndexDocumentDescriptor {
                concept_id: &document.concept_id,
                path: &document.path,
                type_name: &document.type_name,
                title: &document.title,
                search_text: &document.search_text,
                source_digest: &document.source_digest,
            })
            .collect(),
    };
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    descriptor.serialize(&mut serializer).map_err(|error| {
        index_error(format!(
            "Failed to encode the canonical OKF search index: {error}"
        ))
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(&bytes)))
}

fn strip_frontmatter(markdown: &str) -> &str {
    let mut offset = 0_usize;
    let mut lines = markdown.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return markdown;
    };
    if trim_line(first) != "---" {
        return markdown;
    }
    offset += first.len();
    for line in lines {
        offset += line.len();
        if trim_line(line) == "---" {
            return &markdown[offset..];
        }
    }
    markdown
}

fn trim_line(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn first_heading(markdown: &str) -> Option<String> {
    let mut heading = None;
    let mut current = None;
    for event in Parser::new_ext(markdown, Options::all()) {
        match event {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            }) if heading.is_none() => current = Some(String::new()),
            Event::End(TagEnd::Heading(HeadingLevel::H1)) if current.is_some() => {
                heading = current.take().map(|value| normalize_whitespace(&value));
            }
            Event::Text(value) | Event::Code(value) if current.is_some() => {
                if let Some(value_buffer) = current.as_mut() {
                    value_buffer.push_str(&value);
                    value_buffer.push(' ');
                }
            }
            _ => {}
        }
    }
    heading.filter(|value| !value.is_empty())
}

fn markdown_text(markdown: &str) -> String {
    let mut output = String::new();
    for event in Parser::new_ext(markdown, Options::all()) {
        match event {
            Event::Text(value)
            | Event::Code(value)
            | Event::InlineMath(value)
            | Event::DisplayMath(value)
            | Event::FootnoteReference(value) => {
                output.push_str(&value);
                output.push(' ');
            }
            Event::SoftBreak | Event::HardBreak => output.push(' '),
            _ => {}
        }
    }
    normalize_whitespace(&output)
}

fn fallback_title(concept_id: &str) -> String {
    concept_id
        .rsplit('/')
        .next()
        .unwrap_or(concept_id)
        .replace(['-', '_'], " ")
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn index_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_index_invalid", message)
}
