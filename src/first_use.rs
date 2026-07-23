use a3s_use_core::{FirstUseInstallPolicy, UseError, UseResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoInstallAction {
    Ready,
    Install,
}

fn automatic_install_action(
    domain: &str,
    asset: &str,
    available: bool,
    explicit_invalid: bool,
    policy: FirstUseInstallPolicy,
) -> UseResult<AutoInstallAction> {
    if explicit_invalid {
        return Err(UseError::new(
            format!("use.{domain}.explicit_provider_invalid"),
            format!("The explicit {domain} provider is not usable."),
        )
        .with_suggestion(format!(
            "Fix or unset the explicit {domain} provider before retrying."
        )));
    }
    if available {
        return Ok(AutoInstallAction::Ready);
    }
    if let Some(block) = policy.blocked_by() {
        return Err(UseError::new(
            format!("use.{domain}.auto_install_disabled"),
            format!(
                "{asset} is not ready and first-use installation is disabled by {}.",
                block.reason()
            ),
        )
        .with_suggestion(format!(
            "Enable first-use installation or prepare {asset} explicitly while online."
        ))
        .with_detail("reason", block.reason()));
    }
    Ok(AutoInstallAction::Install)
}

#[cfg(feature = "browser")]
pub(crate) async fn ensure_browser_ready() -> UseResult<a3s_use_browser::BrowserRuntimeStatus> {
    use a3s_use_browser::{BrowserInstallSource, ManagedBrowser};

    let status = a3s_use_browser::browser_status(ManagedBrowser::Chrome);
    let explicit_configured = explicit_environment_value(&["A3S_BROWSER_EXECUTABLE", "CHROME"]);
    let explicit_invalid = explicit_configured
        && !(status.available && status.source == BrowserInstallSource::Environment);
    match automatic_install_action(
        "browser",
        "the shared A3S Use Browser runtime",
        status.available,
        explicit_invalid,
        FirstUseInstallPolicy::from_env()?,
    )? {
        AutoInstallAction::Ready => Ok(status),
        AutoInstallAction::Install => {
            a3s_use_browser::install_browser(ManagedBrowser::Chrome).await
        }
    }
}

#[cfg(feature = "browser")]
fn explicit_environment_value(names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_provider_never_installs_or_fails_policy() {
        let action = automatic_install_action(
            "browser",
            "Browser",
            true,
            false,
            FirstUseInstallPolicy::new(true, true),
        )
        .unwrap();
        assert_eq!(action, AutoInstallAction::Ready);
    }

    #[test]
    fn missing_provider_installs_when_policy_allows_it() {
        let action = automatic_install_action(
            "browser",
            "Browser",
            false,
            false,
            FirstUseInstallPolicy::new(false, false),
        )
        .unwrap();
        assert_eq!(action, AutoInstallAction::Install);
    }

    #[test]
    fn policy_and_explicit_provider_boundaries_are_typed() {
        let explicit = automatic_install_action(
            "browser",
            "Browser",
            false,
            true,
            FirstUseInstallPolicy::new(false, false),
        )
        .unwrap_err();
        assert_eq!(explicit.code, "use.browser.explicit_provider_invalid");

        for policy in [
            FirstUseInstallPolicy::new(true, false),
            FirstUseInstallPolicy::new(false, true),
        ] {
            let error =
                automatic_install_action("browser", "Browser", false, false, policy).unwrap_err();
            assert_eq!(error.code, "use.browser.auto_install_disabled");
        }
    }
}
