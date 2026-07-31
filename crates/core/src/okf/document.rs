use std::collections::BTreeSet;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_yaml_ng::{Mapping, Value};

use crate::UseResult;

use super::{
    bundle_error, limit_error, path, OkfBundleDiagnostic, OkfConceptSummary, OkfDiagnosticCode,
    OkfFormatVersion,
};

pub(super) struct DocumentInspection {
    pub(super) concept: Option<OkfConceptSummary>,
    pub(super) diagnostics: Vec<OkfBundleDiagnostic>,
}

pub(super) fn inspect_document(
    path: &str,
    content: &[u8],
    format_version: OkfFormatVersion,
    bundle_paths: &BTreeSet<String>,
    max_links: u64,
) -> UseResult<DocumentInspection> {
    let text = std::str::from_utf8(content).map_err(|error| {
        bundle_error(format!(
            "OKF Markdown document '{path}' must be UTF-8: {error}"
        ))
    })?;
    let parsed = parse_frontmatter(path, text)?;
    let file_name = path.rsplit('/').next().unwrap_or(path);

    match file_name {
        "index.md" => inspect_index(path, parsed, format_version, bundle_paths, max_links),
        "log.md" => inspect_log(path, parsed, bundle_paths, max_links),
        _ => inspect_concept(path, parsed, bundle_paths, max_links),
    }
}

fn inspect_index(
    path: &str,
    parsed: ParsedDocument<'_>,
    format_version: OkfFormatVersion,
    bundle_paths: &BTreeSet<String>,
    max_links: u64,
) -> UseResult<DocumentInspection> {
    if let Some(frontmatter) = &parsed.frontmatter {
        if path != "index.md" {
            return Err(bundle_error(format!(
                "Only the bundle-root index.md may contain frontmatter; '{path}' does not."
            )));
        }
        let mapping = yaml_mapping(path, frontmatter)?;
        if mapping.len() != 1 {
            return Err(bundle_error(
                "The bundle-root index.md frontmatter may contain only okf_version.",
            ));
        }
        let Some(version) = mapping_string(mapping, "okf_version") else {
            return Err(bundle_error(
                "The bundle-root index.md frontmatter requires string okf_version.",
            ));
        };
        if version != format_version.as_str() {
            return Err(bundle_error(format!(
                "The bundle-root okf_version '{version}' does not match the declared format version '{}'.",
                format_version.as_str()
            )));
        }
    }
    let (_, diagnostics) = inspect_references(
        path,
        parsed.body,
        parsed.frontmatter.as_ref(),
        bundle_paths,
        max_links,
    )?;
    Ok(DocumentInspection {
        concept: None,
        diagnostics,
    })
}

fn inspect_log(
    path: &str,
    parsed: ParsedDocument<'_>,
    bundle_paths: &BTreeSet<String>,
    max_links: u64,
) -> UseResult<DocumentInspection> {
    if let Some(frontmatter) = &parsed.frontmatter {
        yaml_mapping(path, frontmatter)?;
    }
    validate_log_dates(path, parsed.body)?;
    let (_, diagnostics) = inspect_references(
        path,
        parsed.body,
        parsed.frontmatter.as_ref(),
        bundle_paths,
        max_links,
    )?;
    Ok(DocumentInspection {
        concept: None,
        diagnostics,
    })
}

fn inspect_concept(
    path: &str,
    parsed: ParsedDocument<'_>,
    bundle_paths: &BTreeSet<String>,
    max_links: u64,
) -> UseResult<DocumentInspection> {
    let frontmatter = parsed.frontmatter.as_ref().ok_or_else(|| {
        bundle_error(format!(
            "OKF concept '{path}' requires a YAML frontmatter block."
        ))
    })?;
    let mapping = yaml_mapping(path, frontmatter)?;
    let type_name = mapping_string(mapping, "type").ok_or_else(|| {
        bundle_error(format!(
            "OKF concept '{path}' requires a non-empty string type."
        ))
    })?;
    if type_name.trim() != type_name
        || type_name.is_empty()
        || type_name.len() > 256
        || type_name.chars().any(char::is_control)
    {
        return Err(bundle_error(format!(
            "OKF concept '{path}' requires a non-empty string type."
        )));
    }
    let (link_count, diagnostics) = inspect_references(
        path,
        parsed.body,
        Some(frontmatter),
        bundle_paths,
        max_links,
    )?;
    let id = path
        .strip_suffix(".md")
        .ok_or_else(|| bundle_error(format!("OKF concept '{path}' is not Markdown.")))?;
    Ok(DocumentInspection {
        concept: Some(OkfConceptSummary {
            id: id.to_string(),
            path: path.to_string(),
            type_name: type_name.to_string(),
            link_count,
        }),
        diagnostics,
    })
}

fn inspect_references(
    source_path: &str,
    body: &str,
    frontmatter: Option<&Value>,
    bundle_paths: &BTreeSet<String>,
    max_links: u64,
) -> UseResult<(u64, Vec<OkfBundleDiagnostic>)> {
    let mut references = markdown_references(body)
        .into_iter()
        .map(|target| (target, false))
        .collect::<Vec<_>>();
    if let Some(Value::Mapping(mapping)) = frontmatter {
        references.extend(frontmatter_references(mapping));
    }
    if references.len() as u64 > max_links {
        return Err(limit_error());
    }

    let mut dangling = BTreeSet::new();
    for (target, allow_scope_descriptor) in &references {
        let Some(resolved) = path::resolve_reference(source_path, target, *allow_scope_descriptor)?
        else {
            continue;
        };
        if !bundle_paths.contains(&resolved) {
            dangling.insert(resolved);
        }
    }
    let diagnostics = dangling
        .into_iter()
        .map(|target| OkfBundleDiagnostic {
            code: OkfDiagnosticCode::DanglingLink,
            path: source_path.to_string(),
            message: format!(
                "OKF document '{source_path}' references missing bundle target '{target}'."
            ),
            target,
        })
        .collect();
    Ok((references.len() as u64, diagnostics))
}

fn markdown_references(body: &str) -> Vec<String> {
    Parser::new_ext(body, Options::all())
        .filter_map(|event| match event {
            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                Some(dest_url.into_string())
            }
            _ => None,
        })
        .collect()
}

fn frontmatter_references(mapping: &Mapping) -> Vec<(String, bool)> {
    let mut references = Vec::new();
    for name in ["resource", "computation"] {
        if let Some(value) = mapping_string(mapping, name) {
            references.push((value.to_string(), false));
        }
    }
    for name in ["executor", "attester"] {
        let Some(Value::Mapping(contract)) = mapping_value(mapping, name) else {
            continue;
        };
        if let Some(value) = mapping_string(contract, "resource") {
            references.push((value.to_string(), false));
        }
    }
    let Some(Value::Sequence(sources)) = mapping_value(mapping, "sources") else {
        return references;
    };
    for source in sources {
        let Value::Mapping(source) = source else {
            continue;
        };
        if let Some(value) = mapping_string(source, "resource") {
            references.push((value.to_string(), true));
        }
    }
    references
}

fn mapping_value<'a>(mapping: &'a Mapping, name: &str) -> Option<&'a Value> {
    mapping.iter().find_map(|(key, value)| match key {
        Value::String(key) if key == name => Some(value),
        _ => None,
    })
}

fn mapping_string<'a>(mapping: &'a Mapping, name: &str) -> Option<&'a str> {
    match mapping_value(mapping, name) {
        Some(Value::String(value)) => Some(value),
        _ => None,
    }
}

fn yaml_mapping<'a>(path: &str, value: &'a Value) -> UseResult<&'a Mapping> {
    match value {
        Value::Mapping(mapping) => Ok(mapping),
        _ => Err(bundle_error(format!(
            "OKF document '{path}' frontmatter must be a YAML mapping."
        ))),
    }
}

struct ParsedDocument<'a> {
    frontmatter: Option<Value>,
    body: &'a str,
}

fn parse_frontmatter<'a>(path: &str, text: &'a str) -> UseResult<ParsedDocument<'a>> {
    let mut lines = text.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok(ParsedDocument {
            frontmatter: None,
            body: text,
        });
    };
    if trim_line(first) != "---" {
        return Ok(ParsedDocument {
            frontmatter: None,
            body: text,
        });
    }

    let frontmatter_start = first.len();
    let mut offset = frontmatter_start;
    let mut closing = None;
    for line in lines {
        if trim_line(line) == "---" {
            closing = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let Some((frontmatter_end, body_start)) = closing else {
        return Err(bundle_error(format!(
            "OKF document '{path}' has an unterminated YAML frontmatter block."
        )));
    };
    let value = serde_yaml_ng::from_str::<Value>(&text[frontmatter_start..frontmatter_end])
        .map_err(|error| {
            bundle_error(format!(
                "OKF document '{path}' has invalid YAML frontmatter: {error}"
            ))
        })?;
    Ok(ParsedDocument {
        frontmatter: Some(value),
        body: &text[body_start..],
    })
}

fn trim_line(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn validate_log_dates(path: &str, body: &str) -> UseResult<()> {
    let dates = log_date_headings(body);
    if dates.iter().any(|date| !valid_iso_date(date))
        || dates.windows(2).any(|pair| pair[0] <= pair[1])
    {
        return Err(bundle_error(format!(
            "OKF log '{path}' requires unique ISO 8601 date headings in newest-first order."
        )));
    }
    Ok(())
}

fn log_date_headings(body: &str) -> Vec<String> {
    let mut headings = Vec::new();
    let mut current = None;
    for event in Parser::new_ext(body, Options::all()) {
        match event {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H2,
                ..
            }) => current = Some(String::new()),
            Event::End(TagEnd::Heading(HeadingLevel::H2)) => {
                if let Some(heading) = current.take() {
                    headings.push(heading.trim().to_string());
                }
            }
            Event::Text(value)
            | Event::Code(value)
            | Event::InlineMath(value)
            | Event::DisplayMath(value)
            | Event::Html(value)
            | Event::InlineHtml(value)
            | Event::FootnoteReference(value) => {
                if let Some(heading) = &mut current {
                    heading.push_str(&value);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(heading) = &mut current {
                    heading.push(' ');
                }
            }
            _ => {}
        }
    }
    headings
}

fn valid_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let Some(year) = parse_digits(&bytes[0..4]) else {
        return false;
    };
    let Some(month) = parse_digits(&bytes[5..7]) else {
        return false;
    };
    let Some(day) = parse_digits(&bytes[8..10]) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max_day).contains(&day)
}

fn parse_digits(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |value, byte| {
        byte.is_ascii_digit()
            .then(|| value * 10 + u32::from(byte - b'0'))
    })
}
