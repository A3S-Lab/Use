use super::*;

const MANIFEST: &str = r#"
extension "acme/slack" {
  schema_version = 1
  version        = "1.2.0"
  route          = "slack"
  actions        = ["read", "mutate"]

  cli {
    executable  = "bin/a3s-use-acme-slack"
    json_output = true
  }

  mcp {
    executable = "bin/a3s-use-acme-slack"
    args       = ["serve", "--mcp"]
    transport  = "stdio"
  }

  skill {
    path = "skills/slack/SKILL.md"
  }

  contributes {
    activity_bar "inbox" {
      title       = "Slack Inbox"
      description = "Review Slack activity with the installed Slack capability."
      icon        = "messages-square"
      entry       = "web/activity.html"
      styles      = ["web/activity.css"]
      scripts     = ["web/activity.js"]
      skill       = "slack"
      order       = 120
    }
  }
}
"#;

#[test]
fn parses_acl_into_native_surfaces() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    assert_eq!(manifest.package_id, "acme/slack");
    assert!(manifest.cli.is_some());
    assert!(manifest.mcp.is_some());
    assert!(manifest.skill.is_some());
    assert!(manifest.tools.is_empty());
    assert!(manifest.mcp_servers.is_empty());
    assert!(manifest.skills.is_empty());
    assert!(manifest.ui.is_empty());
    assert_eq!(manifest.surface_kinds(), ["cli", "mcp", "skill"]);
    assert_eq!(manifest.contributes.activity_bar.len(), 1);
    let activity = &manifest.contributes.activity_bar[0];
    assert_eq!(activity.id, "inbox");
    assert_eq!(activity.title, "Slack Inbox");
    assert_eq!(activity.entry, PathBuf::from("web/activity.html"));
    assert_eq!(activity.styles, [PathBuf::from("web/activity.css")]);
    assert_eq!(activity.scripts, [PathBuf::from("web/activity.js")]);
    assert_eq!(activity.skill, "slack");
    assert_eq!(activity.order, 120);
}

#[test]
fn parses_external_repository_identity_and_host_compatibility() {
    let manifest = MANIFEST
        .replace("schema_version = 1", "schema_version = 2")
        .replace(
            "route          = \"slack\"",
            concat!(
                "route          = \"slack\"\n",
                "  requires_use   = \">=0.2.0, <0.3.0\"\n\n",
                "  repository {\n",
                "    url      = \"https://github.com/acme/slack\"\n",
                "    revision = \"0123456789abcdef0123456789abcdef01234567\"\n",
                "  }"
            ),
        );
    let manifest = ExtensionManifest::parse_acl(&manifest).unwrap();

    assert_eq!(manifest.schema_version, 2);
    assert!(manifest.supports_use_version("0.2.0").unwrap());
    assert!(!manifest.supports_use_version("0.3.0").unwrap());
    assert_eq!(
        manifest.repository.unwrap(),
        ExtensionRepository {
            url: "https://github.com/acme/slack".to_string(),
            revision: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
        }
    );
}

#[test]
fn rejects_incomplete_or_unsafe_repository_manifests() {
    let schema_two = MANIFEST.replace("schema_version = 1", "schema_version = 2");
    assert!(ExtensionManifest::parse_acl(&schema_two).is_err());

    let unsafe_repository = schema_two.replace(
        "route          = \"slack\"",
        concat!(
            "route          = \"slack\"\n",
            "  requires_use   = \">=0.2.0\"\n\n",
            "  repository {\n",
            "    url = \"https://user@example.com/acme/slack?ref=main\"\n",
            "  }"
        ),
    );
    assert!(ExtensionManifest::parse_acl(&unsafe_repository).is_err());
}

#[test]
fn rejects_custom_rpc_fields_and_path_escape() {
    let custom_rpc = MANIFEST.replace(
        "json_output = true",
        "json_output = true\n    jsonrpc = \"2.0\"",
    );
    assert!(ExtensionManifest::parse_acl(&custom_rpc).is_err());
    let escaping = MANIFEST.replace("bin/a3s-use-acme-slack", "../a3s-use-acme-slack");
    assert!(ExtensionManifest::parse_acl(&escaping).is_err());
}

#[test]
fn rejects_reserved_routes() {
    for route in ["browser", "box", "ocr"] {
        let manifest = MANIFEST.replace(
            "route          = \"slack\"",
            &format!("route = \"{route}\""),
        );
        assert!(ExtensionManifest::parse_acl(&manifest).is_err());
    }
}

#[test]
fn rejects_activity_contributions_without_a_skill_surface() {
    let manifest = MANIFEST.replace("  skill {\n    path = \"skills/slack/SKILL.md\"\n  }\n", "");
    let error = ExtensionManifest::parse_acl(&manifest).unwrap_err();
    assert!(error
        .message
        .contains("Activity Bar contributions require a Skill surface"));
}

#[test]
fn rejects_escaping_or_non_html_activity_assets() {
    let escaping = MANIFEST.replace("web/activity.html", "../activity.html");
    assert!(ExtensionManifest::parse_acl(&escaping).is_err());
    let script = MANIFEST.replace("web/activity.html", "web/activity.js");
    assert!(ExtensionManifest::parse_acl(&script).is_err());
    let wrong_style = MANIFEST.replace("web/activity.css", "web/activity.js");
    assert!(ExtensionManifest::parse_acl(&wrong_style).is_err());
    let duplicate = MANIFEST.replace(
        "scripts     = [\"web/activity.js\"]",
        "scripts     = [\"web/activity.css\"]",
    );
    assert!(ExtensionManifest::parse_acl(&duplicate).is_err());
}
