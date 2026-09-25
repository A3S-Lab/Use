//! Fail-closed expiry monitoring for a published Registry tree.
//!
//! Operators run this against a staged or mirrored tree to detect root and
//! online-role expiry before clients start failing closed at refresh time.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::UseResult;
use serde_json::Value;

use crate::tools_error;

/// CLI entry: report role expiry and fail when any role is inside the warn window.
pub(crate) fn check_expiry_command(options: &crate::Options) -> UseResult<()> {
    let registry = PathBuf::from(options.require("registry")?);
    let warn_within_hours: u64 = options
        .optional("warn-within-hours")
        .map(|value| {
            value.parse().map_err(|_| {
                tools_error(
                    "registry_tools.arguments_invalid",
                    "--warn-within-hours must be a non-negative integer.",
                )
            })
        })
        .transpose()?
        .unwrap_or(72);
    let report = check_expiry(&registry, warn_within_hours)?;
    println!(
        "{}",
        serde_json::json!({
            "registry": registry.display().to_string(),
            "warnWithinHours": warn_within_hours,
            "roles": report.roles,
            "expiringSoon": report.expiring_soon,
            "expired": report.expired,
        })
    );
    if !report.expired.is_empty() {
        return Err(tools_error(
            "registry_tools.expiry_failed",
            &format!("Expired Registry roles: {}.", report.expired.join(", ")),
        ));
    }
    if !report.expiring_soon.is_empty() {
        return Err(tools_error(
            "registry_tools.expiry_warning",
            &format!(
                "Registry roles expire within {warn_within_hours}h: {}.",
                report.expiring_soon.join(", ")
            ),
        ));
    }
    Ok(())
}

struct ExpiryReport {
    roles: Vec<serde_json::Value>,
    expiring_soon: Vec<String>,
    expired: Vec<String>,
}

fn check_expiry(registry: &Path, warn_within_hours: u64) -> UseResult<ExpiryReport> {
    let metadata = registry.join("metadata");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_secs() as i64;
    let warn_deadline = now + (warn_within_hours as i64) * 3600;
    let mut roles = Vec::new();
    let mut expiring_soon = Vec::new();
    let mut expired = Vec::new();
    for role in ["root", "targets", "snapshot", "timestamp"] {
        let path = metadata.join(format!("{role}.json"));
        let document = read_json(&path)?;
        let expires = document
            .pointer("/signed/expires")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                tools_error(
                    "registry_tools.expiry_failed",
                    &format!("{} lacks signed.expires.", path.display()),
                )
            })?;
        let expires_at = parse_rfc3339_utc(expires)?;
        let status = if expires_at <= now {
            expired.push(role.to_owned());
            "expired"
        } else if expires_at <= warn_deadline {
            expiring_soon.push(role.to_owned());
            "expiring_soon"
        } else {
            "ok"
        };
        roles.push(serde_json::json!({
            "role": role,
            "expires": expires,
            "expiresAtUnix": expires_at,
            "status": status,
        }));
    }
    Ok(ExpiryReport {
        roles,
        expiring_soon,
        expired,
    })
}

fn read_json(path: &Path) -> UseResult<Value> {
    let bytes = std::fs::read(path).map_err(|error| {
        tools_error(
            "registry_tools.expiry_failed",
            &format!("Failed to read '{}': {error}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        tools_error(
            "registry_tools.expiry_failed",
            &format!("'{}' is not valid JSON: {error}", path.display()),
        )
    })
}

fn parse_rfc3339_utc(value: &str) -> UseResult<i64> {
    // Accept the assemble/rotate layout: YYYY-MM-DDTHH:MM:SSZ
    let trimmed = value.trim();
    if !trimmed.ends_with('Z') || trimmed.len() != 20 {
        return Err(tools_error(
            "registry_tools.expiry_failed",
            &format!("Unsupported expires timestamp '{value}' (expected YYYY-MM-DDTHH:MM:SSZ)."),
        ));
    }
    let year: i32 = trimmed[0..4].parse().map_err(|_| invalid_expires(value))?;
    let month: u32 = trimmed[5..7].parse().map_err(|_| invalid_expires(value))?;
    let day: u32 = trimmed[8..10].parse().map_err(|_| invalid_expires(value))?;
    let hour: u32 = trimmed[11..13]
        .parse()
        .map_err(|_| invalid_expires(value))?;
    let minute: u32 = trimmed[14..16]
        .parse()
        .map_err(|_| invalid_expires(value))?;
    let second: u32 = trimmed[17..19]
        .parse()
        .map_err(|_| invalid_expires(value))?;
    if trimmed.as_bytes()[4] != b'-'
        || trimmed.as_bytes()[7] != b'-'
        || trimmed.as_bytes()[10] != b'T'
        || trimmed.as_bytes()[13] != b':'
        || trimmed.as_bytes()[16] != b':'
    {
        return Err(invalid_expires(value));
    }
    let days = days_from_civil(year, month, day)?;
    Ok(days * 86_400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second))
}

fn invalid_expires(value: &str) -> a3s_use_core::UseError {
    tools_error(
        "registry_tools.expiry_failed",
        &format!("Unsupported expires timestamp '{value}' (expected YYYY-MM-DDTHH:MM:SSZ)."),
    )
}

fn days_from_civil(year: i32, month: u32, day: u32) -> UseResult<i64> {
    if !(1..=12).contains(&month) || day == 0 || day > 31 {
        return Err(tools_error(
            "registry_tools.expiry_failed",
            "expires timestamp has an invalid calendar date.",
        ));
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 }.div_euclid(400);
    let yoe = (y - era * 400) as u64;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + u64::from(doy);
    Ok(era as i64 * 146_097 + doe as i64 - 719_468)
}
