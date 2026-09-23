//! Package-local Executable Tool projection for CapabilityRegistry.
//!
//! These surfaces are integrity-bound package files (not Runtime BindingStore
//! receipts). Lifecycle prepares them with static launcher evidence; the
//! registry must reinspect the same files, emit `Prepared` observations, and
//! project `ExecutableToolProjection` so hosts can list UI and invoke tools
//! without inventing a provider_id.

use std::collections::BTreeSet;

use a3s_use_core::{PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, InstalledExtension, ToolTaskSource, ToolWorkload,
};
use sha2::{Digest, Sha256};

use super::{ExecutableToolProjection, ProjectedLifecycleIdentity};
use crate::surface_reconciler::{SurfaceObservations, SurfaceObservedState};

pub(super) struct ExecutableToolEvidence {
    pub(super) projections: Vec<ExecutableToolProjection>,
    pub(super) observations: SurfaceObservations,
}

impl ExecutableToolEvidence {
    fn empty() -> Self {
        Self {
            projections: Vec::new(),
            observations: SurfaceObservations::new(),
        }
    }
}

/// Project package-local Executable Tool Tasks from file evidence only.
pub(super) async fn executable_tool_evidence_from_package(
    extension: &InstalledExtension,
) -> UseResult<ExecutableToolEvidence> {
    let Some(generation) = extension.receipt.lifecycle_generation else {
        return Ok(ExecutableToolEvidence::empty());
    };
    let Some(package_sha256) = extension.receipt.package_sha256.as_deref() else {
        return Ok(ExecutableToolEvidence::empty());
    };
    let lifecycle_identity = ExtensionLifecycleIdentity::new(
        &extension.receipt.package_id,
        format!("sha256:{package_sha256}"),
        format!("sha256:{}", extension.receipt.manifest_sha256),
        generation,
    )?;
    let projected_identity = ProjectedLifecycleIdentity {
        package_id: lifecycle_identity.package_id().to_string(),
        package_digest: lifecycle_identity.package_digest().to_string(),
        manifest_digest: lifecycle_identity.manifest_digest().to_string(),
        generation: lifecycle_identity.generation(),
    };

    let mut projections = Vec::new();
    let mut observations = SurfaceObservations::new();
    let mut names = BTreeSet::new();
    for surface in &extension.manifest.tools {
        let ToolWorkload::Task(task) = &surface.workload else {
            continue;
        };
        let ToolTaskSource::Executable { executable } = &task.source else {
            continue;
        };
        if task.interactive {
            continue;
        }

        let reference = PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: surface.id.clone(),
        };
        let file_evidence = match a3s_use_extension::inspect_tool_surface_files(
            surface,
            &extension.receipt.package_root,
        )
        .await
        {
            Ok(evidence) => evidence,
            Err(_) => {
                observations.insert(reference, SurfaceObservedState::Failed);
                continue;
            }
        };

        let tool_name = tool_name(&extension.receipt.package_id, &surface.id);
        if !names.insert(tool_name.clone()) {
            return Err(UseError::new(
                "use.capability.executable_tool_name_conflict",
                "Two package-local Executable Tools resolve to the same host tool identity.",
            ));
        }
        if executable.as_os_str().is_empty() || executable.is_absolute() {
            observations.insert(reference, SurfaceObservedState::Failed);
            continue;
        }

        observations.insert(reference, SurfaceObservedState::Prepared);
        projections.push(ExecutableToolProjection {
            tool_name,
            surface_id: surface.id.clone(),
            command: task.command.clone(),
            json_output: task.json_output,
            timeout_ms: task.timeout_ms,
            scope: extension.receipt.installation.clone(),
            lifecycle_identity: projected_identity.clone(),
            file_evidence_digest: file_evidence.digest().to_string(),
            executable: executable.clone(),
        });
    }
    projections.sort_by(|left, right| left.tool_name.cmp(&right.tool_name));
    Ok(ExecutableToolEvidence {
        projections,
        observations,
    })
}

fn tool_name(package_id: &str, surface_id: &str) -> String {
    let identity = format!("{package_id}\0{surface_id}");
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    let suffix = digest.get(..16).unwrap_or(&digest);
    format!(
        "use_tool_{}_{}_{}",
        readable_segment(package_id.rsplit('/').next().unwrap_or(package_id)),
        readable_segment(surface_id),
        suffix
    )
}

fn readable_segment(value: &str) -> String {
    value
        .chars()
        .take(10)
        .map(|character| if character == '-' { '_' } else { character })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use a3s_use_extension::{ExtensionManifest, ExtensionReceipt, ExtensionTrust};
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    use super::super::{project_extension_for_host_with_evidence, CapabilityHostProjectionContext};
    use super::*;

    const EXECUTABLE_PLUGIN: &str = r#"
extension "a3s/applet-demo" {
  schema_version = 3
  version        = "0.1.0"
  route          = "applet-demo"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read", "execute"]

  repository {
    url      = "https://github.com/A3S-Lab/Use-Registry"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "echo" {
    workload    = "task"
    interface   = "cli"
    executable  = "tools/echo"
    command     = "applet-demo-echo"
    json_output = true
    interactive = false
    timeout_ms  = 30000
    activation  = "lazy"
    optional    = false
  }

  mcp "context" {
    transport  = "stdio"
    executable = "mcp/context"
    args       = ["--stdio"]
    activation = "lazy"
    optional   = false
  }

  skill "applet-demo" {
    path          = "skills/applet-demo/SKILL.md"
    requires_tool = ["echo"]
    requires_mcp  = ["context"]
    optional      = false
  }

  ui "panel" {
    title       = "Applet Demo"
    description = "Signed example Applet UI surface."
    icon        = "layout"
    entry       = "ui/panel/index.html"
    styles      = ["ui/panel/index.css"]
    scripts     = ["ui/panel/index.js"]
    skill       = "applet-demo"
    bind_tool   = ["echo"]
    bind_mcp    = ["context"]
    order       = 20
    optional    = false
  }
}
"#;

    fn installed_extension(package_root: &std::path::Path) -> InstalledExtension {
        let manifest = ExtensionManifest::parse_acl(EXECUTABLE_PLUGIN).unwrap();
        let mut selected_surfaces = manifest
            .plugin_surfaces()
            .unwrap()
            .into_iter()
            .map(|surface| surface.surface)
            .collect::<Vec<_>>();
        selected_surfaces.sort();
        let receipt = ExtensionReceipt {
            schema_version: a3s_use_extension::EXTENSION_RECEIPT_SCHEMA_VERSION,
            installation: crate::test_installation(),
            package_id: manifest.package_id.clone(),
            component_id: format!("use/{}", manifest.package_id),
            route_alias: manifest.route_alias.clone(),
            version: manifest.version.clone(),
            package_root: package_root.to_path_buf(),
            manifest_sha256: format!("{:x}", Sha256::digest(EXECUTABLE_PLUGIN.as_bytes())),
            package_sha256: Some("a".repeat(64)),
            trust: ExtensionTrust::LocalExplicit,
            registry: None,
            verified_catalog: None,
            planning_bundle: None,
            selected_surfaces,
            installed_at_unix: 1,
            enabled: true,
            lifecycle_generation: Some(3),
        };
        InstalledExtension { receipt, manifest }
    }

    async fn write_executable(path: &std::path::Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        let mut file = tokio::fs::File::create(path).await.unwrap();
        file.write_all(bytes).await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = tokio::fs::metadata(path).await.unwrap().permissions();
            permissions.set_mode(0o755);
            tokio::fs::set_permissions(path, permissions).await.unwrap();
        }
    }

    async fn write_package_tree(root: &std::path::Path) {
        write_executable(root.join("tools/echo").as_path(), b"#!/bin/sh\nexec cat\n").await;
        write_executable(root.join("mcp/context").as_path(), b"#!/bin/sh\necho mcp\n").await;
        tokio::fs::create_dir_all(root.join("skills/applet-demo"))
            .await
            .unwrap();
        tokio::fs::write(
            root.join("skills/applet-demo/SKILL.md"),
            "---\nname: applet-demo\ndescription: demo\n---\n\n# Demo\n",
        )
        .await
        .unwrap();
        tokio::fs::create_dir_all(root.join("ui/panel"))
            .await
            .unwrap();
        tokio::fs::write(
            root.join("ui/panel/index.html"),
            "<!doctype html><title>Applet Demo</title>",
        )
        .await
        .unwrap();
        tokio::fs::write(root.join("ui/panel/index.css"), "body{}")
            .await
            .unwrap();
        tokio::fs::write(root.join("ui/panel/index.js"), "console.log(1)")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn package_local_echo_projects_prepared_observation_and_executable_tool() {
        let temporary = tempfile::tempdir().unwrap();
        write_package_tree(temporary.path()).await;
        let extension = installed_extension(temporary.path());

        let evidence = executable_tool_evidence_from_package(&extension)
            .await
            .unwrap();
        assert_eq!(evidence.projections.len(), 1);
        assert_eq!(evidence.projections[0].surface_id, "echo");
        assert_eq!(
            evidence.projections[0].executable,
            PathBuf::from("tools/echo")
        );
        assert!(evidence.projections[0]
            .file_evidence_digest
            .starts_with("sha256:"));
        assert_eq!(
            evidence.observations.get(&PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "echo".to_owned(),
            }),
            Some(&SurfaceObservedState::Prepared)
        );
    }

    #[tokio::test]
    async fn package_local_echo_admits_ui_panel_without_runtime_binding_store() {
        let temporary = tempfile::tempdir().unwrap();
        write_package_tree(temporary.path()).await;
        let extension = installed_extension(temporary.path());
        let executable = executable_tool_evidence_from_package(&extension)
            .await
            .unwrap();
        let mcp = super::super::managed_mcp::mcp_evidence_from_store(
            &extension,
            &crate::plugin_runtime::RuntimeBindingStore::new(
                temporary.path().join("state"),
                crate::test_installation(),
            )
            .unwrap(),
            &crate::test_installation(),
        )
        .await
        .unwrap();

        let mut host_observations = executable.observations.clone();
        for (surface, state) in mcp.observations {
            host_observations.insert(surface, state);
        }

        let capability = project_extension_for_host_with_evidence(
            &extension,
            extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
            CapabilityHostProjectionContext {
                desired_enabled: true,
                host_version: "0.3.0",
                host_observations: &host_observations,
                knowledge_bindings: &[],
                runtime_tasks: &[],
                mcp_projections: &mcp.projections,
                executable_tools: &executable.projections,
            },
        )
        .await
        .unwrap();

        assert!(capability.enabled);
        assert!(capability
            .reconciliation
            .as_ref()
            .is_some_and(|snapshot| snapshot.capability_ready));
        assert_eq!(capability.executable_tools.len(), 1);
        assert_eq!(capability.executable_tools[0].surface_id, "echo");
        assert_eq!(capability.activity_bar.len(), 1);
        assert_eq!(capability.activity_bar[0].id, "panel");
    }
}
