use crate::error::ToolAcquireError;

/// Where a tool payload comes from. Bare names stay unresolved until a later
/// curated table maps them to a GitHub release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolBackend {
    Catalog,
    Github { owner: String, repo: String },
    Npm { package: String },
    Pipx { package: String },
    Aqua { owner: String, repo: String },
}

/// One parsed install spec. `raw` is the string the host sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    raw: String,
    backend: ToolBackend,
}

impl ToolSpec {
    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn backend(&self) -> &ToolBackend {
        &self.backend
    }
}

/// Parse a Studio tool spec. Accepted backends are catalog (no prefix),
/// `github:`, `npm:`, `pipx:`, and `aqua:`.
pub fn parse_tool_spec(raw: &str) -> Result<ToolSpec, ToolAcquireError> {
    if !is_tool_key(raw) {
        return Err(ToolAcquireError::InvalidSpec(raw.to_string()));
    }
    let backend = match raw.split_once(':') {
        None => {
            parse_tool_name(raw)?;
            ToolBackend::Catalog
        }
        Some((backend, rest)) => parse_backend(backend, rest)?,
    };
    Ok(ToolSpec {
        raw: raw.to_string(),
        backend,
    })
}

pub(crate) fn parse_tool_name(raw: &str) -> Result<(), ToolAcquireError> {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return Err(ToolAcquireError::InvalidName(raw.to_string()));
    };
    if !first.is_ascii_alphabetic()
        || !chars.all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        return Err(ToolAcquireError::InvalidName(raw.to_string()));
    }
    Ok(())
}

/// A version that can be one directory name under the install root.
pub(crate) fn parse_tool_version(raw: &str) -> Result<(), ToolAcquireError> {
    if !is_tool_key(raw) || raw.contains('/') || raw.contains(':') || raw.contains('\\') {
        return Err(ToolAcquireError::InvalidVersion(raw.to_string()));
    }
    Ok(())
}

fn parse_backend(backend: &str, rest: &str) -> Result<ToolBackend, ToolAcquireError> {
    match backend {
        "github" => {
            let (owner, repo) = split_owner_repo(rest)?;
            Ok(ToolBackend::Github { owner, repo })
        }
        "aqua" => {
            let (owner, repo) = split_owner_repo(rest)?;
            Ok(ToolBackend::Aqua { owner, repo })
        }
        "npm" => Ok(ToolBackend::Npm {
            package: parse_package_name(rest)?,
        }),
        "pipx" => {
            if rest.contains('/') {
                return Err(ToolAcquireError::InvalidSpec(format!("pipx:{rest}")));
            }
            Ok(ToolBackend::Pipx {
                package: parse_package_name(rest)?,
            })
        }
        other => Err(ToolAcquireError::UnknownBackend(other.to_string())),
    }
}

fn split_owner_repo(rest: &str) -> Result<(String, String), ToolAcquireError> {
    let Some((owner, repo)) = rest.split_once('/') else {
        return Err(ToolAcquireError::InvalidSpec(rest.to_string()));
    };
    if owner.is_empty()
        || repo.is_empty()
        || owner.contains('/')
        || repo.contains('/')
        || owner == "."
        || owner == ".."
        || repo == "."
        || repo == ".."
    {
        return Err(ToolAcquireError::InvalidSpec(rest.to_string()));
    }
    Ok((owner.to_string(), repo.to_string()))
}

fn parse_package_name(rest: &str) -> Result<String, ToolAcquireError> {
    if rest.is_empty() || rest == "." || rest == ".." {
        return Err(ToolAcquireError::InvalidSpec(rest.to_string()));
    }
    if let Some(scoped) = rest.strip_prefix('@') {
        let Some((scope, name)) = scoped.split_once('/') else {
            return Err(ToolAcquireError::InvalidSpec(rest.to_string()));
        };
        if scope.is_empty() || name.is_empty() || name.contains('/') || scope.contains('/') {
            return Err(ToolAcquireError::InvalidSpec(rest.to_string()));
        }
    }
    Ok(rest.to_string())
}

fn is_tool_key(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() && first != '@' {
        return false;
    }
    value.chars().all(is_tool_key_char) && !value.contains("..") && !value.contains("//")
}

fn is_tool_key_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '@' | ':' | '/' | '_' | '.' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_specs_studio_already_uses() {
        assert!(matches!(
            parse_tool_spec("uv").unwrap().backend(),
            ToolBackend::Catalog
        ));
        assert!(matches!(
            parse_tool_spec("github:larksuite/cli").unwrap().backend(),
            ToolBackend::Github { .. }
        ));
        assert!(matches!(
            parse_tool_spec("npm:@openai/codex").unwrap().backend(),
            ToolBackend::Npm { .. }
        ));
        assert!(matches!(
            parse_tool_spec("npm:openclaw").unwrap().backend(),
            ToolBackend::Npm { .. }
        ));
        assert!(matches!(
            parse_tool_spec("pipx:babeldoc-stream").unwrap().backend(),
            ToolBackend::Pipx { .. }
        ));
        assert!(matches!(
            parse_tool_spec("aqua:google-antigravity/antigravity-cli")
                .unwrap()
                .backend(),
            ToolBackend::Aqua { .. }
        ));
    }

    #[test]
    fn rejects_unknown_backends_and_path_escape() {
        assert!(matches!(
            parse_tool_spec("conda:foo"),
            Err(ToolAcquireError::UnknownBackend(_))
        ));
        assert!(matches!(
            parse_tool_spec("../uv"),
            Err(ToolAcquireError::InvalidSpec(_))
        ));
        assert!(matches!(
            parse_tool_spec("github:only-owner"),
            Err(ToolAcquireError::InvalidSpec(_))
        ));
        assert!(matches!(
            parse_tool_version("1.0.0/../../etc"),
            Err(ToolAcquireError::InvalidVersion(_))
        ));
    }
}
