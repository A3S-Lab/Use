use a3s_use_core::UseError;
use rmcp::model::CallToolResult;

/// Error code used when a manager boundary failure cannot be represented by a
/// stable A3S error code. The boundary deliberately does not forward provider
/// error identifiers, because a provider is allowed to contain implementation
/// or deployment-specific names.
pub(super) const MCP_OPERATION_ERROR: &str = "use.plugin.manager_mcp_failed";

/// Project an internal error into the small, agent-visible manager contract.
///
/// `UseError` is useful inside the host because it carries diagnostics for
/// operators, but its message, suggestion, and details may contain paths,
/// URLs, command output, or credentials. None of those fields cross the MCP
/// boundary. Contract-shaped `use.*` codes remain available as stable machine
/// identifiers; malformed or provider-owned codes are collapsed to one safe
/// code.
pub(super) fn tool_error(error: UseError) -> CallToolResult {
    let code = public_code(&error.code);
    CallToolResult::structured_error(serde_json::json!({
        "code": code,
        "message": public_message(code),
    }))
}

pub(super) fn invalid_input() -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_params("Plugin Manager tool input is invalid.", None)
}

pub(super) fn startup_error(message: &'static str) -> UseError {
    UseError::new("use.plugin.manager_mcp_invalid", message)
}

pub(super) fn operation_error() -> UseError {
    UseError::new(
        MCP_OPERATION_ERROR,
        "The Plugin Manager could not complete the request.",
    )
}

fn public_code(code: &str) -> &str {
    if code.len() <= 128 && is_contract_code(code) {
        code
    } else {
        MCP_OPERATION_ERROR
    }
}

fn is_contract_code(code: &str) -> bool {
    let Some(namespace) = code.strip_prefix("use.") else {
        return false;
    };
    !namespace.is_empty()
        && namespace.split('.').all(|segment| {
            !segment.is_empty()
                && segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
        })
}

fn public_message(code: &str) -> &'static str {
    if code.contains("scope") || code.contains("authorization") {
        "The request is not authorized for the managed scope."
    } else if code.contains("confirmation") || code.ends_with("_denied") {
        "The requested operation was not authorized."
    } else if code.contains("stale") || code.contains("generation") || code.contains("conflict") {
        "Managed package state changed; refresh the plan and retry."
    } else if code.contains("missing") || code.contains("not_installed") {
        "The requested package or state was not found."
    } else if code.contains("invalid") || code.contains("malformed") {
        "The request does not satisfy the Plugin Manager contract."
    } else {
        "The Plugin Manager could not complete the request."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_drops_private_error_fields_and_provider_code() {
        let result = tool_error(
            UseError::new("provider.internal_secret", "token=/srv/a3s/secrets/token")
                .with_suggestion("read /srv/a3s/secrets/token")
                .with_detail("path", "/srv/a3s/secrets/token"),
        );
        let value = result
            .structured_content
            .expect("manager errors use structured content");
        assert_eq!(value["code"], MCP_OPERATION_ERROR);
        assert_eq!(
            value["message"],
            "The Plugin Manager could not complete the request."
        );
        assert_eq!(value.as_object().map(|object| object.len()), Some(2));
        let serialized = value.to_string();
        assert!(!serialized.contains("internal_secret"));
        assert!(!serialized.contains("/srv/a3s/secrets"));
        assert!(!serialized.contains("token="));
    }

    #[test]
    fn projection_keeps_only_valid_stable_use_codes() {
        let result = tool_error(UseError::new(
            "use.plugin.manager_scope_mismatch",
            "private scope path /srv/a3s/workspace",
        ));
        let value = result
            .structured_content
            .expect("manager errors use structured content");
        assert_eq!(value["code"], "use.plugin.manager_scope_mismatch");
        assert_eq!(
            value["message"],
            "The request is not authorized for the managed scope."
        );
        assert!(!value.to_string().contains("/srv/a3s/workspace"));
    }

    #[test]
    fn projection_collapses_malformed_use_codes() {
        let result = tool_error(UseError::new(
            "use..secret",
            "private diagnostic /srv/a3s/secrets/token",
        ));
        let value = result
            .structured_content
            .expect("manager errors use structured content");
        assert_eq!(value["code"], MCP_OPERATION_ERROR);
        assert!(!value.to_string().contains("secret"));
    }

    #[test]
    fn invalid_input_message_is_constant() {
        let error = invalid_input();
        assert_eq!(error.message, "Plugin Manager tool input is invalid.");
    }
}
