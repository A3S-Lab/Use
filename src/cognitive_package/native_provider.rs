use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    CatalogMcpTransport, CatalogSurface, ExecutablePlanningSurface, PlanEnforcementProfile,
    PlanQualifiedSurfaceRef, PlannedPackageState, PlannedPackageTransition,
    PlannedProviderEvidence, PluginPlanningBundle, PluginSurfaceKind, ToolWorkloadClass, UseResult,
};
use sha2::{Digest, Sha256};

use super::{current_host_target, package_manager_error};

/// Produce the exact built-in provider evidence for package-local Tool Tasks
/// and stdio MCP launchers described by signed planning targets.
///
/// Release-backed Tool Tasks, Tool Services, and HTTP MCP Services are
/// validated but skipped here. They must be selected separately through an
/// explicit Runtime/Gateway provider. This lets one package safely mix native
/// and managed executable surfaces without either provider claiming the
/// other's workload.
pub fn plan_native_provider_evidence(
    packages: &[PlannedPackageTransition],
    planning_bundles: &BTreeMap<String, PluginPlanningBundle>,
) -> UseResult<Vec<PlannedProviderEvidence>> {
    let mut providers = Vec::new();
    let mut seen = BTreeSet::new();
    for package in packages {
        let Some(state) = package.after.as_ref() else {
            continue;
        };
        let executable = state
            .release
            .surfaces
            .iter()
            .filter(|surface| {
                matches!(
                    surface.kind,
                    PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
                )
            })
            .collect::<Vec<_>>();
        if executable.is_empty() {
            continue;
        }
        let bundle = planning_bundles.get(&package.package_id).ok_or_else(|| {
            native_provider_error(format!(
                "Cognitive package '{}' omitted its signed native planning bundle.",
                package.package_id
            ))
        })?;
        validate_bundle_state(bundle, state)?;
        validate_bundle_surface_set(bundle, &executable)?;
        for surface in executable {
            let reference = PlanQualifiedSurfaceRef {
                package_id: package.package_id.clone(),
                surface: surface.reference(),
            };
            if !seen.insert(reference.clone()) {
                return Err(native_provider_error(
                    "Native provider planning contains a duplicate package surface.",
                ));
            }
            let planning = bundle
                .surfaces
                .iter()
                .find(|planning| planning.reference() == reference.surface)
                .ok_or_else(|| {
                    native_provider_error(format!(
                        "Signed planning evidence omitted '{}/{}'.",
                        package.package_id, surface.id
                    ))
                })?;
            let permission = state
                .permissions
                .surfaces
                .iter()
                .find(|permission| permission.surface == reference.surface)
                .ok_or_else(|| {
                    native_provider_error(format!(
                        "Native surface '{}/{}' omitted its permission ceiling.",
                        package.package_id, surface.id
                    ))
                })?;
            if !planning_matches_catalog(planning, surface) {
                return Err(native_provider_error(format!(
                    "Executable planning evidence for '{}/{}' does not match its selected catalog workload.",
                    package.package_id, surface.id
                )));
            }
            if !is_native_planning_surface(planning) {
                continue;
            }
            if !permission.native_execution || permission.private_service {
                return Err(native_provider_error(format!(
                    "Native executable surface '{}/{}' lacks exact native-launcher authority.",
                    package.package_id, surface.id
                )));
            }
            providers.push(native_provider_evidence(
                reference,
                &state.release.package_sha256,
            )?);
        }
    }
    providers.sort_by(|left, right| left.surface.cmp(&right.surface));
    Ok(providers)
}

fn validate_bundle_surface_set(
    bundle: &PluginPlanningBundle,
    executable: &[&CatalogSurface],
) -> UseResult<()> {
    let selected = executable
        .iter()
        .map(|surface| surface.reference())
        .collect::<Vec<_>>();
    let planned = bundle
        .surfaces
        .iter()
        .map(ExecutablePlanningSurface::reference)
        .collect::<Vec<_>>();
    if planned != selected {
        return Err(native_provider_error(
            "The signed planning bundle does not cover the exact selected executable surfaces.",
        ));
    }
    Ok(())
}

fn is_native_planning_surface(surface: &ExecutablePlanningSurface) -> bool {
    matches!(
        surface,
        ExecutablePlanningSurface::ToolTaskNative { .. }
            | ExecutablePlanningSurface::McpStdio { .. }
    )
}

fn planning_matches_catalog(
    planning: &ExecutablePlanningSurface,
    catalog: &CatalogSurface,
) -> bool {
    if planning.reference() != catalog.reference() {
        return false;
    }
    match planning {
        ExecutablePlanningSurface::ToolTaskNative { .. }
        | ExecutablePlanningSurface::ToolTask { .. } => {
            catalog.workload == Some(ToolWorkloadClass::Task) && catalog.mcp_transport.is_none()
        }
        ExecutablePlanningSurface::ToolService { .. } => {
            catalog.workload == Some(ToolWorkloadClass::Service) && catalog.mcp_transport.is_none()
        }
        ExecutablePlanningSurface::McpService { .. } => {
            catalog.workload.is_none()
                && catalog.mcp_transport == Some(CatalogMcpTransport::StreamableHttp)
        }
        ExecutablePlanningSurface::McpStdio { .. } => {
            catalog.workload.is_none() && catalog.mcp_transport == Some(CatalogMcpTransport::Stdio)
        }
    }
}

pub(super) fn native_provider_evidence(
    surface: PlanQualifiedSurfaceRef,
    package_sha256: &str,
) -> UseResult<PlannedProviderEvidence> {
    let target = current_host_target()?;
    Ok(PlannedProviderEvidence {
        semantics_profile_digest: digest(&format!(
            "a3s-use-static-surface-v1\n{}\n{:?}\n{}\n{}",
            surface.package_id, surface.surface.kind, surface.surface.id, package_sha256
        )),
        surface,
        provider_id: "a3s-use-native-launcher".to_owned(),
        provider_build_id: format!("a3s-use:{}:{target}", env!("CARGO_PKG_VERSION")),
        capability_digest: digest(&format!(
            "a3s-use-native-launcher-v1\n{}\n{target}",
            env!("CARGO_PKG_VERSION")
        )),
        enforcement: PlanEnforcementProfile::NativeUnconfined,
    })
}

fn validate_bundle_state(
    bundle: &PluginPlanningBundle,
    state: &PlannedPackageState,
) -> UseResult<()> {
    bundle.validate()?;
    if bundle.package_id != state.release.package_id
        || bundle.version != state.release.version
        || bundle.channel != state.release.channel
        || bundle.target != state.release.target
        || bundle.package_sha256 != state.release.package_sha256
        || bundle.manifest_sha256 != state.release.manifest_sha256
        || bundle.permission_ceiling_digest != state.release.permission_ceiling_digest
        || state.permissions.descriptor_digest()? != state.release.permission_ceiling_digest
    {
        return Err(native_provider_error(
            "The signed native planning bundle does not match the selected package state.",
        ));
    }
    Ok(())
}

fn native_provider_error(message: impl Into<String>) -> a3s_use_core::UseError {
    package_manager_error("use.plugin.runtime_provider_required", message)
}

fn digest(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use a3s_use_core::{
        CatalogMcpTransport, PlanPackageChangeKind, PlanPackageRole, PlannedPluginRelease,
        PluginCatalogRecord, PLUGIN_PLANNING_BUNDLE_SCHEMA,
    };

    use super::*;

    #[test]
    fn signed_native_launchers_produce_complete_deterministic_provider_evidence() {
        let (transition, bundle) = native_transition();
        let providers = plan_native_provider_evidence(
            &[transition],
            &BTreeMap::from([("acme/research".to_owned(), bundle.clone())]),
        )
        .unwrap();

        assert_eq!(providers.len(), 2);
        assert!(providers
            .iter()
            .all(|provider| provider.provider_id == "a3s-use-native-launcher"));
        assert!(providers
            .iter()
            .all(|provider| provider.enforcement == PlanEnforcementProfile::NativeUnconfined));

        let mut release_backed = bundle;
        release_backed.surfaces[1] = planning_release_task();
        let providers = plan_native_provider_evidence(
            &[native_transition().0],
            &BTreeMap::from([("acme/research".to_owned(), release_backed)]),
        )
        .unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].surface.surface.id, "library");
    }

    fn native_transition() -> (PlannedPackageTransition, PluginPlanningBundle) {
        let mut record = PluginCatalogRecord::from_json(include_bytes!(
            "../../crates/core/fixtures/plugins/catalog-record-v3.json"
        ))
        .unwrap();
        record.surfaces.retain(|surface| {
            (surface.kind == PluginSurfaceKind::Mcp && surface.id == "library")
                || (surface.kind == PluginSurfaceKind::Tool && surface.id == "convert")
        });
        record.surfaces[0].mcp_transport = Some(CatalogMcpTransport::Stdio);
        record.permission_ceiling.surfaces.retain(|permission| {
            (permission.surface.kind == PluginSurfaceKind::Mcp
                && permission.surface.id == "library")
                || (permission.surface.kind == PluginSurfaceKind::Tool
                    && permission.surface.id == "convert")
        });
        record.permission_ceiling.surfaces[0].native_execution = true;
        record.permission_ceiling.surfaces[0].private_service = false;
        record.permission_ceiling_digest = record.permission_ceiling.descriptor_digest().unwrap();
        record.validate().unwrap();
        let state = PlannedPackageState {
            release: PlannedPluginRelease {
                package_id: record.package_id.clone(),
                version: record.version.clone(),
                channel: record.channel,
                target: record.target.clone(),
                package_sha256: record.package.sha256.clone().unwrap(),
                manifest_sha256: record.package.manifest_sha256.clone().unwrap(),
                permission_ceiling_digest: record.permission_ceiling_digest.clone(),
                surfaces: record.surfaces.clone(),
            },
            permissions: record.permission_ceiling.clone(),
        };
        let bundle = PluginPlanningBundle {
            schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
            package_id: record.package_id.clone(),
            version: record.version.clone(),
            channel: record.channel,
            target: record.target.clone(),
            archive_sha256: record.archive.sha256,
            package_sha256: record.package.sha256.unwrap(),
            manifest_sha256: record.package.manifest_sha256.unwrap(),
            permission_ceiling_digest: record.permission_ceiling_digest,
            surfaces: vec![
                ExecutablePlanningSurface::McpStdio {
                    id: "library".to_owned(),
                    activation: a3s_use_core::PlanningSurfaceActivation::Lazy,
                    executable: "bin/acme-research".to_owned(),
                    args: vec!["serve".to_owned(), "--mcp".to_owned()],
                },
                ExecutablePlanningSurface::ToolTaskNative {
                    id: "convert".to_owned(),
                    activation: a3s_use_core::PlanningSurfaceActivation::Lazy,
                    executable: "bin/acme-research".to_owned(),
                    command: "acme-convert".to_owned(),
                    json_output: true,
                    timeout_ms: 120_000,
                },
            ],
        };
        (
            PlannedPackageTransition {
                package_id: "acme/research".to_owned(),
                role: PlanPackageRole::Root,
                change: PlanPackageChangeKind::Add,
                before: None,
                after: Some(state),
                source: None,
                surfaces: Vec::new(),
            },
            bundle,
        )
    }

    fn planning_release_task() -> ExecutablePlanningSurface {
        let descriptor = a3s_use_core::ToolReleaseDescriptor::from_json(include_bytes!(
            "../../crates/core/fixtures/releases/tool-task-release-v1.json"
        ))
        .unwrap();
        ExecutablePlanningSurface::ToolTask {
            id: "convert".to_owned(),
            activation: a3s_use_core::PlanningSurfaceActivation::Lazy,
            command: "acme-convert".to_owned(),
            json_output: true,
            timeout_ms: 120_000,
            artifact: a3s_use_core::PlanningArtifactRef {
                uri: format!(
                    "oci://registry.example/acme/convert@{}",
                    descriptor.artifact.digest
                ),
                digest: descriptor.artifact.digest.clone(),
                media_type: descriptor.artifact.media_type.clone(),
            },
            descriptor,
        }
    }
}
