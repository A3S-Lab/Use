use a3s_use_extension::ExtensionManifest;

use super::{PluginLifecycleAction, PluginLifecycleIntent, PluginLifecycleIntentSpec};

pub(super) const ALL_SURFACES: &str = r#"
extension "acme/research" {
  schema_version = 3
  version        = "1.0.0"
  route          = "research"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/research"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "query" {
    workload   = "task"
    interface  = "cli"
    executable = "bin/query"
    command    = "query"
    optional   = true
  }

  mcp "catalog" {
    transport  = "stdio"
    executable = "bin/catalog-mcp"
    optional   = false
  }

  okf "papers" {
    format_version         = "0.2"
    root                   = "knowledge/papers"
    content_digest         = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    concept_count          = 1
    file_count             = 2
    expanded_bytes         = 128
    max_files              = 16
    max_concepts           = 16
    max_expanded_bytes     = 4096
    max_document_bytes     = 2048
    max_links_per_document = 16
    optional               = true
  }

  flow "review" {
    engine        = "a3s-flow"
    runtime       = "native-ts"
    source        = "flows/review.ts"
    export        = "run"
    requires_tool = ["query"]
    requires_mcp  = ["catalog"]
    requires_okf  = ["papers"]
    optional      = false
  }

  skill "review" {
    path          = "skills/review/SKILL.md"
    requires_tool = ["query"]
    requires_mcp  = ["catalog"]
    requires_okf  = ["papers"]
    requires_flow = ["review"]
    optional      = false
  }

  ui "review" {
    entry     = "web/review.html"
    styles    = []
    scripts   = []
    skill     = "review"
    bind_tool = ["query"]
    bind_mcp  = ["catalog"]
    bind_flow = ["review"]
    optional  = false
  }
}
"#;

pub(super) fn manifest() -> ExtensionManifest {
    ExtensionManifest::parse_acl(ALL_SURFACES).unwrap()
}

pub(super) fn intent(action: PluginLifecycleAction) -> PluginLifecycleIntent {
    let manifest = manifest();
    PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: format!("{action:?}:acme-research:1").to_lowercase(),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope_id: "workspace:research".to_string(),
            package_id: "acme/research".to_string(),
            package_digest: format!("sha256:{}", "2".repeat(64)),
            manifest_digest: format!("sha256:{}", "3".repeat(64)),
            generation: 7,
            action,
        },
        &manifest,
    )
    .unwrap()
}
