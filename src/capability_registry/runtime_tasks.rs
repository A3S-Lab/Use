use std::collections::BTreeSet;

use a3s_use_core::{
    PlanQualifiedSurfaceRef, PlanScope, PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult,
};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, InstalledExtension, ToolTaskSource, ToolWorkload,
};
use sha2::{Digest, Sha256};

use super::{ProjectedLifecycleIdentity, ToolTaskProjection};
use crate::plugin_runtime::{RuntimeBindingReceipt, RuntimeBindingStore, RuntimeSurfaceContract};
use crate::surface_reconciler::{SurfaceObservations, SurfaceObservedState};

pub(super) struct RuntimeTaskEvidence {
    pub(super) projections: Vec<ToolTaskProjection>,
    pub(super) observations: SurfaceObservations,
}

impl RuntimeTaskEvidence {
    fn empty() -> Self {
        Self {
            projections: Vec::new(),
            observations: SurfaceObservations::new(),
        }
    }
}

pub(super) async fn runtime_task_evidence_from_store(
    extension: &InstalledExtension,
    store: &RuntimeBindingStore,
    scope: &PlanScope,
) -> UseResult<RuntimeTaskEvidence> {
    let Some(generation) = extension.receipt.lifecycle_generation else {
        return Ok(RuntimeTaskEvidence::empty());
    };
    let Some(package_sha256) = extension.receipt.package_sha256.as_deref() else {
        return Ok(RuntimeTaskEvidence::empty());
    };
    let lifecycle_identity = ExtensionLifecycleIdentity::new(
        &extension.receipt.package_id,
        format!("sha256:{package_sha256}"),
        format!("sha256:{}", extension.receipt.manifest_sha256),
        generation,
    )?;

    let mut projections = Vec::new();
    let mut observations = SurfaceObservations::new();
    let mut names = BTreeSet::new();
    for surface in &extension.manifest.tools {
        let ToolWorkload::Task(task) = &surface.workload else {
            continue;
        };
        if !matches!(&task.source, ToolTaskSource::Release { .. }) || task.interactive {
            continue;
        }

        let reference = PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: surface.id.clone(),
        };
        let qualified = PlanQualifiedSurfaceRef {
            package_id: extension.receipt.package_id.clone(),
            surface: reference.clone(),
        };
        let Some(receipt) = store.get_generation(scope, &qualified, generation).await? else {
            continue;
        };
        receipt.validate()?;
        let RuntimeBindingReceipt::Task(binding) = receipt else {
            observations.insert(reference, SurfaceObservedState::Failed);
            continue;
        };
        if crate::plugin_runtime::validate_task_descriptor_binding(extension, &surface.id, &binding)
            .is_err()
        {
            observations.insert(reference, SurfaceObservedState::Failed);
            continue;
        }
        let contract_matches = matches!(
            &binding.contract,
            RuntimeSurfaceContract::ToolTask {
                command_name,
                json_output,
                ..
            } if command_name == &task.command && json_output == &task.json_output
        );
        if binding.surface != qualified
            || binding.scope != *scope
            || binding.package_digest != lifecycle_identity.package_digest()
            || binding.generation() != generation
            || !contract_matches
        {
            observations.insert(reference, SurfaceObservedState::Failed);
            continue;
        }

        let tool_name = tool_name(&extension.receipt.package_id, &surface.id);
        if !names.insert(tool_name.clone()) {
            return Err(UseError::new(
                "use.capability.runtime_task_name_conflict",
                "Two Runtime Tool Tasks resolve to the same host tool identity.",
            ));
        }
        observations.insert(reference, SurfaceObservedState::Prepared);
        projections.push(ToolTaskProjection {
            tool_name,
            surface_id: surface.id.clone(),
            command: task.command.clone(),
            json_output: task.json_output,
            timeout_ms: task.timeout_ms,
            scope: scope.clone(),
            lifecycle_identity: ProjectedLifecycleIdentity {
                package_id: lifecycle_identity.package_id().to_string(),
                package_digest: lifecycle_identity.package_digest().to_string(),
                manifest_digest: lifecycle_identity.manifest_digest().to_string(),
                generation: lifecycle_identity.generation(),
            },
            provider_id: binding.provider_id,
        });
    }
    projections.sort_by(|left, right| left.tool_name.cmp(&right.tool_name));
    Ok(RuntimeTaskEvidence {
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
    use a3s_runtime::contract::NetworkMode;
    use a3s_use_core::{
        ExecutablePlanningSurface, PlanEnforcementProfile, PlannedProviderEvidence,
        PlanningArtifactRef, PlanningSurfaceActivation, PluginPlanningBundle, PluginReleaseChannel,
        PLUGIN_PLANNING_BUNDLE_SCHEMA,
    };
    use a3s_use_extension::{ExtensionManifest, ExtensionReceipt, ExtensionTrust};

    use crate::plugin_runtime::{
        plan_tool_task_release, RuntimePreparedTaskBinding, RuntimeSurfaceContext,
        RuntimeTaskInvocation,
    };

    use super::*;

    const TASK_PLUGIN: &str = r#"
extension "acme/research" {
  schema_version = 3
  version        = "1.0.0"
  route          = "research"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["execute"]

  repository {
    url      = "https://github.com/acme/research"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "convert" {
    workload    = "task"
    interface   = "cli"
    release     = "releases/convert.json"
    command     = "acme-convert"
    json_output = true
    interactive = false
    timeout_ms  = 120000
    activation  = "lazy"
    optional    = false
  }
}
"#;

    fn installed_extension(package_root: &std::path::Path) -> InstalledExtension {
        let manifest = ExtensionManifest::parse_acl(TASK_PLUGIN).unwrap();
        let mut selected_surfaces = manifest
            .plugin_surfaces()
            .unwrap()
            .into_iter()
            .map(|surface| surface.surface)
            .collect::<Vec<_>>();
        selected_surfaces.sort();
        let descriptor = crate::plugin_runtime::test_support::task_descriptor();
        let receipt = ExtensionReceipt {
            schema_version: a3s_use_extension::EXTENSION_RECEIPT_SCHEMA_VERSION,
            installation: crate::test_installation(),
            package_id: manifest.package_id.clone(),
            component_id: "use/acme/research".to_string(),
            route_alias: manifest.route_alias.clone(),
            version: manifest.version.clone(),
            package_root: package_root.to_path_buf(),
            manifest_sha256: format!("{:x}", Sha256::digest(TASK_PLUGIN.as_bytes())),
            package_sha256: Some("a".repeat(64)),
            trust: ExtensionTrust::LocalExplicit,
            registry: None,
            verified_catalog: None,
            planning_bundle: Some(PluginPlanningBundle {
                schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
                package_id: manifest.package_id.clone(),
                version: manifest.version.clone(),
                channel: PluginReleaseChannel::Stable,
                target: "linux-x86_64".to_owned(),
                archive_sha256: format!("sha256:{}", "c".repeat(64)),
                package_sha256: format!("sha256:{}", "a".repeat(64)),
                manifest_sha256: format!(
                    "sha256:{}",
                    Sha256::digest(TASK_PLUGIN.as_bytes())
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                ),
                permission_ceiling_digest: format!("sha256:{}", "d".repeat(64)),
                surfaces: vec![ExecutablePlanningSurface::ToolTask {
                    id: "convert".to_owned(),
                    activation: PlanningSurfaceActivation::Lazy,
                    command: "acme-convert".to_owned(),
                    json_output: true,
                    timeout_ms: 120_000,
                    artifact: PlanningArtifactRef {
                        uri: format!(
                            "oci://registry.example/acme/research@{}",
                            descriptor.artifact.digest
                        ),
                        digest: descriptor.artifact.digest.clone(),
                        media_type: descriptor.artifact.media_type.clone(),
                    },
                    descriptor,
                }],
            }),
            selected_surfaces,
            installed_at_unix: 1,
            enabled: true,
            lifecycle_generation: Some(7),
        };
        InstalledExtension { receipt, manifest }
    }

    fn scope() -> PlanScope {
        crate::test_installation()
    }

    fn task_binding(
        extension: &InstalledExtension,
        scope: PlanScope,
        package_digest: &str,
        generation: u64,
    ) -> RuntimePreparedTaskBinding {
        let surface = extension.manifest.tools[0].clone();
        let ToolWorkload::Task(task) = surface.workload else {
            panic!("fixture must contain a Tool Task")
        };
        let context = RuntimeSurfaceContext::new(
            extension.receipt.package_id.clone(),
            package_digest,
            scope,
            format!("sha256:{}", "b".repeat(64)),
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: surface.id,
            },
            generation,
        )
        .unwrap();
        let descriptor = crate::plugin_runtime::test_support::task_descriptor();
        let artifact = crate::plugin_runtime::test_support::artifact(
            &descriptor.artifact.digest,
            &descriptor.artifact.media_type,
        );
        let plan = plan_tool_task_release(
            context,
            &task,
            &descriptor,
            artifact,
            RuntimeTaskInvocation::new("projection-test", Vec::new()).unwrap(),
            crate::plugin_runtime::test_support::policy(),
            NetworkMode::None,
        )
        .unwrap();
        let capabilities = crate::plugin_runtime::test_support::capabilities(&plan);
        let evidence = PlannedProviderEvidence {
            enforcement: PlanEnforcementProfile::Container,
            ..crate::plugin_runtime::test_support::evidence(&plan, &capabilities)
        };
        RuntimePreparedTaskBinding::from_plan(&plan, &evidence).unwrap()
    }

    #[tokio::test]
    async fn exact_binding_projects_one_stable_runtime_task() {
        let temporary = tempfile::tempdir().unwrap();
        let extension = installed_extension(temporary.path());
        let store = RuntimeBindingStore::new(temporary.path().join("state"), scope()).unwrap();
        let scope = scope();
        let package_digest = format!("sha256:{}", "a".repeat(64));
        let binding = task_binding(&extension, scope.clone(), &package_digest, 7);
        store
            .put(&RuntimeBindingReceipt::Task(binding))
            .await
            .unwrap();

        let evidence = runtime_task_evidence_from_store(&extension, &store, &scope)
            .await
            .unwrap();

        assert_eq!(evidence.projections.len(), 1);
        let projection = &evidence.projections[0];
        assert!(projection
            .tool_name
            .starts_with("use_tool_research_convert_"));
        assert_eq!(projection.command, "acme-convert");
        assert_eq!(projection.surface_id, "convert");
        assert_eq!(projection.scope, scope);
        assert_eq!(projection.lifecycle_identity.generation, 7);
        assert_eq!(projection.provider_id, "test-runtime");
        assert_eq!(
            evidence.observations.get(&PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_string(),
            }),
            Some(&SurfaceObservedState::Prepared)
        );

        let capability = super::super::project_extension_for_host_with_evidence(
            &extension,
            extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
            super::super::CapabilityHostProjectionContext {
                desired_enabled: extension.receipt.enabled,
                host_version: "0.3.0",
                host_observations: &evidence.observations,
                knowledge_bindings: &[],
                runtime_tasks: &evidence.projections,
                mcp_projections: &[],
            },
        )
        .await
        .unwrap();
        assert!(capability.enabled);
        assert_eq!(capability.tool_tasks, evidence.projections);
        let json = serde_json::to_value(&capability).unwrap();
        assert_eq!(json["toolTasks"][0]["lifecycleIdentity"]["generation"], 7);
        assert_eq!(json["toolTasks"][0]["scope"]["kind"], "user");

        let duplicate =
            super::super::validate_unique_tool_task_names(&[capability.clone(), capability])
                .unwrap_err();
        assert_eq!(duplicate.code, "use.capability.runtime_task_name_conflict");
    }

    #[tokio::test]
    async fn mismatched_package_binding_fails_closed_without_projection() {
        let temporary = tempfile::tempdir().unwrap();
        let extension = installed_extension(temporary.path());
        let store = RuntimeBindingStore::new(temporary.path().join("state"), scope()).unwrap();
        let scope = scope();
        let binding = task_binding(
            &extension,
            scope.clone(),
            &format!("sha256:{}", "c".repeat(64)),
            7,
        );
        store
            .put(&RuntimeBindingReceipt::Task(binding))
            .await
            .unwrap();

        let evidence = runtime_task_evidence_from_store(&extension, &store, &scope)
            .await
            .unwrap();

        assert!(evidence.projections.is_empty());
        assert_eq!(
            evidence.observations.get(&PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_string(),
            }),
            Some(&SurfaceObservedState::Failed)
        );

        let capability = super::super::project_extension_for_host_with_evidence(
            &extension,
            extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
            super::super::CapabilityHostProjectionContext {
                desired_enabled: extension.receipt.enabled,
                host_version: "0.3.0",
                host_observations: &evidence.observations,
                knowledge_bindings: &[],
                runtime_tasks: &evidence.projections,
                mcp_projections: &[],
            },
        )
        .await
        .unwrap();
        assert!(!capability.enabled);
        assert!(capability.tool_tasks.is_empty());
    }

    #[tokio::test]
    async fn self_consistent_binding_with_replaced_signed_descriptor_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let mut extension = installed_extension(temporary.path());
        let bundle = extension.receipt.planning_bundle.as_mut().unwrap();
        let ExecutablePlanningSurface::ToolTask { descriptor, .. } = &mut bundle.surfaces[0] else {
            panic!("fixture must contain a Runtime Tool Task planning surface")
        };
        descriptor
            .provenance
            .build_operation_id
            .push_str("-tampered");
        descriptor.validate().unwrap();

        let store = RuntimeBindingStore::new(temporary.path().join("state"), scope()).unwrap();
        let scope = scope();
        let package_digest = format!("sha256:{}", "a".repeat(64));
        let binding = task_binding(&extension, scope.clone(), &package_digest, 7);
        store
            .put(&RuntimeBindingReceipt::Task(binding))
            .await
            .unwrap();

        let evidence = runtime_task_evidence_from_store(&extension, &store, &scope)
            .await
            .unwrap();
        assert!(evidence.projections.is_empty());
        assert_eq!(
            evidence.observations.get(&PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_owned(),
            }),
            Some(&SurfaceObservedState::Failed)
        );
    }

    #[tokio::test]
    async fn registry_runtime_task_without_planning_evidence_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let mut extension = installed_extension(temporary.path());
        extension.receipt.trust = ExtensionTrust::RegistryTuf;
        extension.receipt.planning_bundle = None;
        let store = RuntimeBindingStore::new(temporary.path().join("state"), scope()).unwrap();
        let scope = scope();
        let package_digest = format!("sha256:{}", "a".repeat(64));
        let binding = task_binding(&extension, scope.clone(), &package_digest, 7);
        store
            .put(&RuntimeBindingReceipt::Task(binding))
            .await
            .unwrap();

        let evidence = runtime_task_evidence_from_store(&extension, &store, &scope)
            .await
            .unwrap();
        assert!(evidence.projections.is_empty());
        assert_eq!(
            evidence.observations.get(&PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_owned(),
            }),
            Some(&SurfaceObservedState::Failed)
        );
    }
}
