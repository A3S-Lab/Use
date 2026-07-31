use percent_encoding::percent_decode_str;
use url::Url;

use crate::UseResult;

use super::{bundle_error, path_escape};

const MAX_OKF_PATH_BYTES: usize = 1_024;

pub(super) fn normalize_bundle_root(value: &str) -> UseResult<String> {
    normalize_declared_path(value, "bundle root")
}

pub(super) fn normalize_bundle_file_path(value: &str) -> UseResult<String> {
    normalize_declared_path(value, "bundle file")
}

fn normalize_declared_path(value: &str, label: &str) -> UseResult<String> {
    if value.is_empty()
        || value.len() > MAX_OKF_PATH_BYTES
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return Err(path_escape(
            value,
            format!("The OKF {label} '{value}' is not a canonical bundle-relative path."),
        ));
    }
    let mut segments = Vec::new();
    for segment in value.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") {
            return Err(path_escape(
                value,
                format!("The OKF {label} '{value}' is not a canonical bundle-relative path."),
            ));
        }
        segments.push(segment);
    }
    Ok(segments.join("/"))
}

pub(super) fn resolve_reference(
    source_path: &str,
    target: &str,
    allow_scope_descriptor: bool,
) -> UseResult<Option<String>> {
    let target = target.trim();
    if target.is_empty() || target.starts_with('#') {
        return Ok(None);
    }
    if target.starts_with("//") {
        return Ok(None);
    }
    if target.contains('\\') || target.chars().any(char::is_control) {
        return Err(path_escape(
            target,
            format!("OKF document '{source_path}' contains an unsafe path reference."),
        ));
    }
    if let Ok(url) = Url::parse(target) {
        if matches!(url.scheme(), "data" | "file" | "javascript") {
            return Err(bundle_error(format!(
                "OKF document '{source_path}' contains unsafe URI scheme '{}'.",
                url.scheme()
            )));
        }
        return Ok(None);
    }
    if allow_scope_descriptor
        && target.chars().any(char::is_whitespace)
        && !target.starts_with('.')
        && !target.starts_with('/')
    {
        return Ok(None);
    }

    let path_end = target.find(['?', '#']).unwrap_or(target.len());
    let encoded_path = &target[..path_end];
    if encoded_path.is_empty() {
        return Ok(None);
    }
    let decoded = percent_decode_str(encoded_path)
        .decode_utf8()
        .map_err(|_| {
            path_escape(
                target,
                format!("OKF document '{source_path}' contains a non-UTF-8 path reference."),
            )
        })?;
    if decoded.contains('\\') || decoded.chars().any(char::is_control) {
        return Err(path_escape(
            target,
            format!("OKF document '{source_path}' contains an unsafe path reference."),
        ));
    }

    let absolute = decoded.starts_with('/');
    let trailing_slash = decoded.ends_with('/');
    let mut segments = if absolute {
        Vec::new()
    } else {
        let mut source = source_path.split('/').collect::<Vec<_>>();
        source.pop();
        source
    };
    for segment in decoded.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() {
                    return Err(path_escape(
                        target,
                        format!(
                            "OKF document '{source_path}' contains a reference outside the bundle."
                        ),
                    ));
                }
            }
            value => segments.push(value),
        }
    }
    if trailing_slash || segments.is_empty() {
        segments.push("index.md");
    }
    let resolved = segments.join("/");
    if resolved.len() > MAX_OKF_PATH_BYTES {
        return Err(path_escape(
            target,
            format!("OKF document '{source_path}' contains an oversized path reference."),
        ));
    }
    Ok(Some(resolved))
}
