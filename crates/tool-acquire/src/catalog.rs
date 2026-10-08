/// One bare name Studio already installs. The spec is the concrete backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogTool {
    pub name: &'static str,
    pub spec: &'static str,
    pub executable: &'static str,
    /// Optional pipx extra. The stored spec stays `pipx:<package>`.
    pub pipx_extras: Option<&'static str>,
}

const CATALOG: &[CatalogTool] = &[
    CatalogTool {
        name: "uv",
        spec: "github:astral-sh/uv",
        executable: "uv",
        pipx_extras: None,
    },
    CatalogTool {
        name: "bun",
        spec: "github:oven-sh/bun",
        executable: "bun",
        pipx_extras: None,
    },
    CatalogTool {
        name: "fd",
        spec: "github:sharkdp/fd",
        executable: "fd",
        pipx_extras: None,
    },
    CatalogTool {
        name: "rg",
        spec: "github:BurntSushi/ripgrep",
        executable: "rg",
        pipx_extras: None,
    },
    CatalogTool {
        name: "rtk",
        spec: "github:rtk-ai/rtk",
        executable: "rtk",
        pipx_extras: None,
    },
    CatalogTool {
        name: "lark-cli",
        spec: "github:larksuite/cli",
        executable: "lark-cli",
        pipx_extras: None,
    },
    CatalogTool {
        name: "gh",
        spec: "github:cli/cli",
        executable: "gh",
        pipx_extras: None,
    },
    CatalogTool {
        name: "ntn",
        spec: "npm:ntn",
        executable: "ntn",
        pipx_extras: None,
    },
    CatalogTool {
        name: "babeldoc-stream",
        spec: "pipx:babeldoc-stream",
        executable: "babeldoc-stream",
        pipx_extras: None,
    },
    CatalogTool {
        name: "a3s-box",
        spec: "github:A3S-Lab/Box",
        executable: "a3s-box",
        pipx_extras: None,
    },
    CatalogTool {
        name: "claude",
        spec: "npm:@anthropic-ai/claude-code",
        executable: "claude",
        pipx_extras: None,
    },
    CatalogTool {
        name: "codex",
        spec: "npm:@openai/codex",
        executable: "codex",
        pipx_extras: None,
    },
    CatalogTool {
        name: "opencode",
        spec: "npm:opencode-ai",
        executable: "opencode",
        pipx_extras: None,
    },
    CatalogTool {
        name: "agy",
        spec: "aqua:google-antigravity/antigravity-cli",
        executable: "agy",
        pipx_extras: None,
    },
    CatalogTool {
        name: "openclaw",
        spec: "npm:openclaw",
        executable: "openclaw",
        pipx_extras: None,
    },
    CatalogTool {
        name: "dsh",
        spec: "npm:@deepseek-ai/dsh",
        executable: "dsh",
        pipx_extras: None,
    },
    CatalogTool {
        name: "gemini",
        spec: "npm:@google/gemini-cli",
        executable: "gemini",
        pipx_extras: None,
    },
    CatalogTool {
        name: "qwen",
        spec: "npm:@qwen-code/qwen-code",
        executable: "qwen",
        pipx_extras: None,
    },
    CatalogTool {
        name: "kimi",
        spec: "npm:@moonshot-ai/kimi-code",
        executable: "kimi",
        pipx_extras: None,
    },
    CatalogTool {
        name: "qoderclicn",
        spec: "npm:@qodercn-ai/qoderclicn",
        executable: "qoderclicn",
        pipx_extras: None,
    },
    CatalogTool {
        name: "copilot",
        spec: "npm:@github/copilot",
        executable: "copilot",
        pipx_extras: None,
    },
    CatalogTool {
        name: "pi",
        spec: "npm:@earendil-works/pi-coding-agent",
        executable: "pi",
        pipx_extras: None,
    },
    CatalogTool {
        name: "hermes",
        spec: "pipx:hermes-agent",
        executable: "hermes",
        pipx_extras: Some("web"),
    },
    CatalogTool {
        name: "mcode",
        spec: "npm:@minimax-ai/code",
        executable: "mcode",
        pipx_extras: None,
    },
];

pub fn catalog() -> &'static [CatalogTool] {
    CATALOG
}

pub fn catalog_by_name(name: &str) -> Option<&'static CatalogTool> {
    CATALOG.iter().find(|entry| entry.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::parse_tool_spec;

    #[test]
    fn every_catalog_spec_parses_and_bare_names_are_unique() {
        let mut names = std::collections::BTreeSet::new();
        for entry in catalog() {
            assert!(
                names.insert(entry.name),
                "duplicate catalog name {}",
                entry.name
            );
            parse_tool_spec(entry.spec).expect(entry.spec);
            assert!(entry.executable == entry.name || !entry.executable.is_empty());
        }
        assert_eq!(
            catalog_by_name("uv").map(|entry| entry.spec),
            Some("github:astral-sh/uv")
        );
        assert_eq!(
            catalog_by_name("agy").map(|entry| entry.spec),
            Some("aqua:google-antigravity/antigravity-cli")
        );
        assert!(catalog_by_name("python").is_none());
    }
}
