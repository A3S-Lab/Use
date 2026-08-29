use std::collections::BTreeSet;
use std::path::Path;

use a3s_use_core::{
    McpReleaseDescriptor, PlanQualifiedSurfaceRef, PlanScope, PluginSurfaceKind, PluginSurfaceRef,
    UseError, UseResult, MAX_RELEASE_DESCRIPTOR_BYTES,
};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, InstalledExtension, PluginMcpLaunch, SurfaceActivation,
};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::{
    McpLaunchProjection, McpRuntimeProjection, McpServerProjection, McpSurfaceActivation,
    ProjectedLifecycleIdentity,
};
use crate::plugin_runtime::{
    RuntimeBindingReceipt, RuntimeBindingStore, RuntimeServiceReadinessEvidence,
    RuntimeSurfaceContract,
};
use crate::surface_reconciler::{SurfaceObservations, SurfaceObservedState};

const MCP_RUNTIME_BINDING_DIGEST_SCHEMA: &[u8] = b"a3s.use.mcp-runtime-binding-projection.v1\0";

pub(super) struct McpEvidence {
    pub(super) projections: Vec<McpServerProjection>,
    pub(super) observations: SurfaceObservations,
}

impl McpEvidence {
    fn empty() -> Self {
        Self {
            projections: Vec::new(),
            observations: SurfaceObservations::new(),
        }
    }
}

/// Project exact, package-qualified MCP launch evidence without opening a
/// connection or resolving an endpoint URL.
///
/// Stdio launchers are immutable package files and become `Prepared` after
/// reinspection. Streamable HTTP launchers require one exact healthy Runtime
/// binding for the selected lifecycle generation; the projection retains only
/// its opaque Gateway endpoint reference and non-secret readiness evidence.
pub(super) async fn mcp_evidence_from_store(
    extension: &InstalledExtension,
    store: &RuntimeBindingStore,
    scope: &PlanScope,
) -> UseResult<McpEvidence> {
    let Some(generation) = extension.receipt.lifecycle_generation else {
        return Ok(McpEvidence::empty());
    };
    let Some(package_sha256) = extension.receipt.package_sha256.as_deref() else {
        return Ok(McpEvidence::empty());
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
    for surface in &extension.manifest.mcp_servers {
        let reference = PluginSurfaceRef {
            kind: PluginSurfaceKind::Mcp,
            id: surface.id.clone(),
        };
        let file_evidence = match a3s_use_extension::inspect_mcp_surface_files(
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
        let server_name = mcp_server_name(&extension.receipt.package_id, &surface.id);
        if !names.insert(server_name.clone()) {
            return Err(UseError::new(
                "use.capability.mcp_name_conflict",
                "Two MCP surfaces resolve to the same host server identity.",
            ));
        }
        let activation = match surface.activation {
            SurfaceActivation::Eager => McpSurfaceActivation::Eager,
            SurfaceActivation::Lazy => McpSurfaceActivation::Lazy,
        };

        let launch = match &surface.launch {
            PluginMcpLaunch::Stdio { executable, args } => {
                observations.insert(reference, SurfaceObservedState::Prepared);
                McpLaunchProjection::Stdio {
                    executable: executable.clone(),
                    args: args.clone(),
                }
            }
            PluginMcpLaunch::StreamableHttp { release } => {
                let descriptor = match read_mcp_descriptor(
                    &extension.receipt.package_root.join(release),
                )
                .await
                {
                    Ok(descriptor) => descriptor,
                    Err(_) => {
                        observations.insert(reference, SurfaceObservedState::Failed);
                        continue;
                    }
                };
                let qualified = PlanQualifiedSurfaceRef {
                    package_id: extension.receipt.package_id.clone(),
                    surface: reference.clone(),
                };
                let Some(receipt) = store.get_generation(scope, &qualified, generation).await?
                else {
                    continue;
                };
                receipt.validate()?;
                let RuntimeBindingReceipt::Service(binding) = receipt else {
                    observations.insert(reference, SurfaceObservedState::Failed);
                    continue;
                };
                let descriptor_digest = descriptor.descriptor_digest()?;
                let (
                    RuntimeSurfaceContract::McpService {
                        endpoint_path,
                        protocol_version,
                        ..
                    },
                    RuntimeServiceReadinessEvidence::McpInitialized { initialize },
                ) = (&binding.contract, &binding.readiness)
                else {
                    observations.insert(reference, SurfaceObservedState::Failed);
                    continue;
                };
                if binding.surface != qualified
                    || binding.scope != *scope
                    || binding.package_digest != lifecycle_identity.package_digest()
                    || binding.generation != generation
                    || binding.descriptor_digest != descriptor_digest
                {
                    observations.insert(reference, SurfaceObservedState::Failed);
                    continue;
                }

                let binding_digest = runtime_binding_digest(&binding)?;
                observations.insert(reference, SurfaceObservedState::Healthy);
                McpLaunchProjection::StreamableHttp {
                    release: release.clone(),
                    runtime: McpRuntimeProjection {
                        scope: binding.scope,
                        endpoint_ref: binding.endpoint_ref.as_str().to_string(),
                        endpoint_path: endpoint_path.clone(),
                        protocol_version: protocol_version.clone(),
                        initialized_at_ms: initialize.initialized_at_ms,
                        provider_id: binding.provider_id,
                        provider_build_id: binding.provider_build_id,
                        runtime_generation: binding.generation,
                        descriptor_digest: binding.descriptor_digest,
                        binding_digest,
                    },
                }
            }
        };
        projections.push(McpServerProjection {
            id: surface.id.clone(),
            server_name,
            activation,
            lifecycle_identity: projected_identity.clone(),
            file_evidence_digest: file_evidence.digest().to_string(),
            launch,
        });
    }
    projections.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(McpEvidence {
        projections,
        observations,
    })
}

fn mcp_server_name(package_id: &str, surface_id: &str) -> String {
    let identity = format!("{package_id}\0{surface_id}");
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    let suffix = digest.get(..16).unwrap_or(&digest);
    format!(
        "use_mcp_{}_{}_{}",
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

async fn read_mcp_descriptor(path: &Path) -> UseResult<McpReleaseDescriptor> {
    let file = tokio::fs::File::open(path).await.map_err(|error| {
        UseError::new(
            "use.capability.mcp_descriptor_unreadable",
            format!(
                "Failed to open projected MCP release descriptor '{}': {error}",
                path.display()
            ),
        )
    })?;
    let mut bytes = Vec::new();
    file.take((MAX_RELEASE_DESCRIPTOR_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| {
            UseError::new(
                "use.capability.mcp_descriptor_unreadable",
                format!(
                    "Failed to read projected MCP release descriptor '{}': {error}",
                    path.display()
                ),
            )
        })?;
    if bytes.is_empty() || bytes.len() > MAX_RELEASE_DESCRIPTOR_BYTES {
        return Err(UseError::new(
            "use.capability.mcp_descriptor_invalid",
            "The projected MCP release descriptor exceeds its bounded contract.",
        ));
    }
    McpReleaseDescriptor::from_json(&bytes).map_err(|error| {
        UseError::new(
            "use.capability.mcp_descriptor_invalid",
            format!(
                "Projected MCP release descriptor '{}' is invalid: {}",
                path.display(),
                error.message
            ),
        )
    })
}

fn runtime_binding_digest(
    binding: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
) -> UseResult<String> {
    let bytes =
        serde_json::to_vec(&RuntimeBindingReceipt::Service(binding.clone())).map_err(|error| {
            UseError::new(
                "use.capability.mcp_binding_invalid",
                format!("Failed to encode exact MCP Runtime binding evidence: {error}"),
            )
        })?;
    let mut hasher = Sha256::new();
    hasher.update(MCP_RUNTIME_BINDING_DIGEST_SCHEMA);
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use a3s_use_core::PlanEnforcementProfile;
    use a3s_use_extension::{ExtensionManifest, ExtensionReceipt, ExtensionTrust};

    use crate::plugin_runtime::{
        RuntimeEndpointRef, RuntimeMcpInitializeEvidence, RuntimeServiceBindingReceipt,
        RUNTIME_SERVICE_BINDING_SCHEMA,
    };

    use super::*;

    const STDIO_PLUGIN: &str = r#"
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

  mcp "catalog" {
    transport  = "stdio"
    executable = "bin/catalog"
    args       = ["--mode", "catalog"]
    activation = "lazy"
    optional   = false
  }

  mcp "library" {
    transport  = "stdio"
    executable = "bin/library"
    args       = []
    activation = "lazy"
    optional   = false
  }
}
"#;

    const HTTP_PLUGIN: &str = r#"
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

  mcp "library" {
    transport  = "streamable-http"
    release    = "releases/mcp.json"
    activation = "eager"
    optional   = false
  }
}
"#;

    fn installed_extension(source: &str, package_root: &Path) -> InstalledExtension {
        let manifest = ExtensionManifest::parse_acl(source).unwrap();
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
            manifest_sha256: format!("{:x}", Sha256::digest(source.as_bytes())),
            package_sha256: Some("a".repeat(64)),
            trust: ExtensionTrust::RegistryTuf,
            registry: None,
            verified_catalog: None,
            planning_bundle: None,
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

    async fn write_executable(path: &Path, bytes: &[u8]) {
        tokio::fs::write(path, bytes).await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).unwrap();
        }
    }

    async fn write_mcp_descriptor(package_root: &Path) -> McpReleaseDescriptor {
        let bytes = include_bytes!("../../crates/core/fixtures/releases/mcp-release-v1.json");
        tokio::fs::create_dir_all(package_root.join("releases"))
            .await
            .unwrap();
        tokio::fs::write(package_root.join("releases/mcp.json"), bytes)
            .await
            .unwrap();
        McpReleaseDescriptor::from_json(bytes).unwrap()
    }

    fn service_binding(
        descriptor: &McpReleaseDescriptor,
        package_id: &str,
        surface_id: &str,
        package_digest: &str,
        endpoint_ref: &str,
    ) -> RuntimeServiceBindingReceipt {
        RuntimeServiceBindingReceipt {
            schema: RUNTIME_SERVICE_BINDING_SCHEMA.to_string(),
            surface: PlanQualifiedSurfaceRef {
                package_id: package_id.to_string(),
                surface: PluginSurfaceRef {
                    kind: PluginSurfaceKind::Mcp,
                    id: surface_id.to_string(),
                },
            },
            package_digest: package_digest.to_string(),
            scope: scope(),
            descriptor_digest: descriptor.descriptor_digest().unwrap(),
            provider_id: "test-runtime".to_string(),
            provider_build_id: "build-1".to_string(),
            capability_digest: format!("sha256:{}", "b".repeat(64)),
            enforcement: PlanEnforcementProfile::Container,
            unit_id: format!("service/{}-{surface_id}/7", package_id.replace('/', "-")),
            generation: 7,
            spec_digest: format!("sha256:{}", "c".repeat(64)),
            semantics_profile_digest: format!("sha256:{}", "d".repeat(64)),
            endpoint_ref: RuntimeEndpointRef::parse(endpoint_ref).unwrap(),
            runtime_started_at_ms: 10,
            observation_revision: 20,
            last_healthy_at_ms: 20,
            contract: RuntimeSurfaceContract::McpService {
                port_name: descriptor.service.port_name.clone(),
                endpoint_path: descriptor.service.endpoint_path.clone(),
                protocol_version: descriptor.service.protocol_version.clone(),
                shutdown_grace_ms: descriptor.service.shutdown_grace_ms,
            },
            readiness: RuntimeServiceReadinessEvidence::McpInitialized {
                initialize: RuntimeMcpInitializeEvidence::new(
                    descriptor.service.protocol_version.clone(),
                    20,
                )
                .unwrap(),
            },
        }
    }

    #[tokio::test]
    async fn multiple_stdio_surfaces_keep_exact_ids_and_distinct_host_names() {
        let temporary = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(temporary.path().join("bin"))
            .await
            .unwrap();
        write_executable(&temporary.path().join("bin/catalog"), b"catalog-v1").await;
        write_executable(&temporary.path().join("bin/library"), b"library-v1").await;
        let extension = installed_extension(STDIO_PLUGIN, temporary.path());
        let store = RuntimeBindingStore::new(temporary.path().join("state"), scope()).unwrap();

        let evidence = mcp_evidence_from_store(&extension, &store, &scope())
            .await
            .unwrap();

        assert_eq!(
            evidence
                .projections
                .iter()
                .map(|projection| projection.id.as_str())
                .collect::<Vec<_>>(),
            ["catalog", "library"]
        );
        assert_ne!(
            evidence.projections[0].server_name,
            evidence.projections[1].server_name
        );
        assert!(evidence
            .projections
            .iter()
            .all(|projection| projection.file_evidence_digest.starts_with("sha256:")));
        assert!(evidence
            .observations
            .values()
            .all(|observation| { *observation == SurfaceObservedState::Prepared }));

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
                runtime_tasks: &[],
                mcp_projections: &evidence.projections,
            },
        )
        .await
        .unwrap();
        assert!(capability.enabled);
        assert_eq!(capability.mcp_servers, evidence.projections);
        let json = serde_json::to_value(capability).unwrap();
        assert_eq!(json["mcpServers"][0]["id"], "catalog");
        assert_eq!(json["mcpServers"][1]["launch"]["transport"], "stdio");
    }

    #[tokio::test]
    async fn exact_http_runtime_binding_projects_opaque_ready_evidence() {
        let temporary = tempfile::tempdir().unwrap();
        let descriptor = write_mcp_descriptor(temporary.path()).await;
        let extension = installed_extension(HTTP_PLUGIN, temporary.path());
        let store = RuntimeBindingStore::new(temporary.path().join("state"), scope()).unwrap();
        let binding = service_binding(
            &descriptor,
            "acme/research",
            "library",
            &format!("sha256:{}", "a".repeat(64)),
            "gateway:managed-services/acme-library-generation-7",
        );
        store
            .put(&RuntimeBindingReceipt::Service(binding))
            .await
            .unwrap();

        let evidence = mcp_evidence_from_store(&extension, &store, &scope())
            .await
            .unwrap();

        assert_eq!(evidence.projections.len(), 1);
        assert_eq!(
            evidence.observations.get(&PluginSurfaceRef {
                kind: PluginSurfaceKind::Mcp,
                id: "library".to_string(),
            }),
            Some(&SurfaceObservedState::Healthy)
        );
        let McpLaunchProjection::StreamableHttp { runtime, .. } = &evidence.projections[0].launch
        else {
            panic!("fixture must project Streamable HTTP MCP")
        };
        assert_eq!(runtime.runtime_generation, 7);
        assert_eq!(runtime.provider_id, "test-runtime");
        assert_eq!(
            runtime.endpoint_ref,
            "gateway:managed-services/acme-library-generation-7"
        );
        assert!(runtime.binding_digest.starts_with("sha256:"));

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
                runtime_tasks: &[],
                mcp_projections: &evidence.projections,
            },
        )
        .await
        .unwrap();
        assert!(capability.enabled);
        assert_eq!(capability.mcp_servers.len(), 1);
    }

    #[tokio::test]
    async fn mismatched_http_package_binding_fails_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let descriptor = write_mcp_descriptor(temporary.path()).await;
        let extension = installed_extension(HTTP_PLUGIN, temporary.path());
        let store = RuntimeBindingStore::new(temporary.path().join("state"), scope()).unwrap();
        let binding = service_binding(
            &descriptor,
            "acme/research",
            "library",
            &format!("sha256:{}", "e".repeat(64)),
            "gateway:managed-services/acme-library-generation-7",
        );
        store
            .put(&RuntimeBindingReceipt::Service(binding))
            .await
            .unwrap();

        let evidence = mcp_evidence_from_store(&extension, &store, &scope())
            .await
            .unwrap();

        assert!(evidence.projections.is_empty());
        assert_eq!(
            evidence.observations.get(&PluginSurfaceRef {
                kind: PluginSurfaceKind::Mcp,
                id: "library".to_string(),
            }),
            Some(&SurfaceObservedState::Failed)
        );
    }

    #[tokio::test]
    async fn mhs_bridge_publishes_only_after_its_exact_gateway_and_dependency_graph_are_ready() {
        let package_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("crates/extension/fixtures/packages/plugin-v3-mhs-bridge/package");
        let source = tokio::fs::read_to_string(package_root.join("a3s-use-extension.acl"))
            .await
            .unwrap();
        let extension = installed_extension(&source, &package_root);
        let descriptor = read_mcp_descriptor(&package_root.join("releases/mhs-gateway-v1.json"))
            .await
            .unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let store = RuntimeBindingStore::new(temporary.path().join("state"), scope()).unwrap();

        let missing = mcp_evidence_from_store(&extension, &store, &scope())
            .await
            .unwrap();
        let mut missing_observations = missing.observations;
        missing_observations.insert(
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Flow,
                id: "monitor".to_string(),
            },
            SurfaceObservedState::Prepared,
        );
        let unpublished = super::super::project_extension_for_host_with_evidence(
            &extension,
            extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
            super::super::CapabilityHostProjectionContext {
                desired_enabled: extension.receipt.enabled,
                host_version: "0.3.0",
                host_observations: &missing_observations,
                knowledge_bindings: &[],
                runtime_tasks: &[],
                mcp_projections: &missing.projections,
            },
        )
        .await
        .unwrap();
        assert!(!unpublished.enabled);
        assert!(unpublished.mcp_servers.is_empty());
        assert!(unpublished.flows.is_empty());
        assert!(unpublished.skills.is_empty());
        assert!(unpublished.activity_bar.is_empty());

        let binding = service_binding(
            &descriptor,
            "acme/mhs-bridge",
            "fleet",
            &format!("sha256:{}", "a".repeat(64)),
            "gateway:managed-services/acme-mhs-bridge-fleet-generation-7",
        );
        store
            .put(&RuntimeBindingReceipt::Service(binding))
            .await
            .unwrap();

        let evidence = mcp_evidence_from_store(&extension, &store, &scope())
            .await
            .unwrap();
        let mut observations = evidence.observations.clone();
        observations.insert(
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Flow,
                id: "monitor".to_string(),
            },
            SurfaceObservedState::Prepared,
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
                host_observations: &observations,
                knowledge_bindings: &[],
                runtime_tasks: &[],
                mcp_projections: &evidence.projections,
            },
        )
        .await
        .unwrap();

        assert!(capability.enabled);
        assert_eq!(capability.mcp_servers.len(), 1);
        assert_eq!(capability.mcp_servers[0].id, "fleet");
        assert_eq!(capability.flows[0].requires_mcp, ["fleet"]);
        assert_eq!(capability.skills[0].id, "operator");
        let devices = &capability.activity_bar[0];
        assert_eq!(devices.id, "devices");
        assert_eq!(
            devices
                .dependencies
                .iter()
                .map(|dependency| (dependency.kind, dependency.id.as_str()))
                .collect::<Vec<_>>(),
            [
                (PluginSurfaceKind::Flow, "monitor"),
                (PluginSurfaceKind::Mcp, "fleet"),
                (PluginSurfaceKind::Skill, "operator"),
            ]
        );
    }
}
