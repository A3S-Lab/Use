use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{UseError, UseResult};

pub(super) const MAX_SESSION_TIMEOUT_MS: u64 = 120_000;
const MIN_AUTHORIZATION_RECHECK_INTERVAL_MS: u64 = 10;
const MAX_AUTHORIZATION_RECHECK_INTERVAL_MS: u64 = 10_000;

pub(super) fn valid_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

#[cfg(not(windows))]
pub(super) fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(windows)]
pub(super) fn paths_overlap(left: &Path, right: &Path) -> bool {
    let components = |path: &Path| {
        path.components()
            .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
            .collect::<Vec<_>>()
    };
    let left = components(left);
    let right = components(right);
    left.starts_with(&right) || right.starts_with(&left)
}

pub(super) fn valid_timeout(value: u64) -> bool {
    value > 0 && value <= MAX_SESSION_TIMEOUT_MS
}

pub(super) fn valid_authorization_recheck_interval(value: u64) -> bool {
    (MIN_AUTHORIZATION_RECHECK_INTERVAL_MS..=MAX_AUTHORIZATION_RECHECK_INTERVAL_MS).contains(&value)
}

pub(super) fn valid_package_id(value: &str) -> bool {
    value.len() <= 128 && value.split('/').count() == 2 && value.split('/').all(valid_segment)
}

pub(super) fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(super) fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(super) fn input_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.stdio_mcp.input_invalid", message)
}

pub(super) fn host_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.stdio_mcp.host_invalid", message)
}

pub(super) fn valid_protocol(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_'))
}

pub(super) fn valid_server_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

pub(super) fn unix_time_ms() -> UseResult<u64> {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            UseError::new(
                "use.plugin.stdio_mcp.clock_invalid",
                "The host clock is before the Unix epoch.",
            )
        })?
        .as_millis();
    u64::try_from(milliseconds).map_err(|_| {
        UseError::new(
            "use.plugin.stdio_mcp.clock_invalid",
            "The host time cannot be represented by the stdio MCP lifecycle.",
        )
    })
}
