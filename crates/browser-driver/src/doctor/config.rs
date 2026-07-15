//! Validate canonical A3S ACL Browser configuration and report legacy JSON.

use std::path::{Path, PathBuf};

use super::{Check, Status};

pub(super) fn check(checks: &mut Vec<Check>) {
    let category = "Config";

    check_canonical(
        checks,
        "config.user",
        category,
        &crate::product::user_config_path(),
    );
    check_canonical(
        checks,
        "config.project",
        category,
        &crate::product::project_config_path(),
    );

    if let Some(custom) = std::env::var_os("A3S_USE_BROWSER_CONFIG") {
        check_explicit(
            checks,
            "config.custom",
            category,
            "A3S_USE_BROWSER_CONFIG",
            &PathBuf::from(custom),
        );
    } else if let Some(custom) = std::env::var_os("AGENT_BROWSER_CONFIG") {
        check_explicit(
            checks,
            "config.custom_legacy",
            category,
            "AGENT_BROWSER_CONFIG (legacy)",
            &PathBuf::from(custom),
        );
    }

    if let Some(path) = crate::product::legacy_user_config_path().filter(|path| path.is_file()) {
        check_legacy(checks, "config.user_legacy", category, &path);
    }
    let path = crate::product::legacy_project_config_path();
    if path.is_file() {
        check_legacy(checks, "config.project_legacy", category, &path);
    }
}

fn check_canonical(checks: &mut Vec<Check>, id: &'static str, category: &'static str, path: &Path) {
    if !path.exists() {
        return;
    }
    match crate::flags::validate_config_file(path) {
        Ok(format) => checks.push(Check::new(
            id,
            category,
            Status::Pass,
            format!("{} ({format})", path.display()),
        )),
        Err(error) => checks.push(
            Check::new(id, category, Status::Fail, error)
                .with_fix(format!("edit {}", path.display())),
        ),
    }
}

fn check_explicit(
    checks: &mut Vec<Check>,
    id: &'static str,
    category: &'static str,
    variable: &str,
    path: &Path,
) {
    if !path.exists() {
        checks.push(
            Check::new(
                id,
                category,
                Status::Fail,
                format!("{variable} points to missing file: {}", path.display()),
            )
            .with_fix(format!("update or unset {variable}")),
        );
        return;
    }
    match crate::flags::validate_config_file(path) {
        Ok(format) => checks.push(Check::new(
            id,
            category,
            Status::Pass,
            format!("{variable}: {} ({format})", path.display()),
        )),
        Err(error) => checks.push(
            Check::new(id, category, Status::Fail, error)
                .with_fix(format!("edit {}", path.display())),
        ),
    }
}

fn check_legacy(checks: &mut Vec<Check>, id: &'static str, category: &'static str, path: &Path) {
    let detail = match crate::flags::validate_config_file(path) {
        Ok(_) => format!(
            "Legacy Browser JSON config {}; migrate to {}",
            path.display(),
            crate::product::display_path(&crate::product::user_config_path())
        ),
        Err(error) => error,
    };
    checks.push(
        Check::new(id, category, Status::Warn, detail)
            .with_fix("rewrite the configuration as an A3S ACL browser block"),
    );
}
