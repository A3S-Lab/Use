use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{CapabilityGatewayCatalog, PluginSurfaceRef, UseResult};

use crate::control_store::model::{
    ControlCapabilityEffectAuthority, ControlCapabilitySurfaceState, ControlEffectSubject,
};

use super::index_error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CatalogSurfaceIncarnation {
    package_id: String,
    lifecycle_generation: u64,
    surface: PluginSurfaceRef,
}

/// Revalidate the host-produced Agent projection against the exact committed
/// generation and its terminal surface evidence.
///
/// A host may intentionally expose a subset because negotiated A3S extension
/// metadata is not yet part of the universal catalog. Every descriptor it does
/// expose must nevertheless belong to an enabled package incarnation and a
/// successfully prepared selected surface; degraded optional surfaces cannot
/// leak into the Gateway catalog.
pub(super) fn validate_projected_catalog(
    authority: &ControlCapabilityEffectAuthority,
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<()> {
    catalog.validate()?;
    let generation = &authority.generation;
    if catalog.installation() != &generation.snapshot.installation
        || catalog.generation() != generation.capability.generation
    {
        return Err(index_error(
            "The Capability Gateway catalog belongs to another Control generation.",
        ));
    }

    let lifecycles = generation
        .package_lifecycles
        .iter()
        .map(|lifecycle| {
            (
                lifecycle.package_id.as_str(),
                lifecycle.lifecycle_generation,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let prepared = authority
        .materializations
        .iter()
        .filter_map(|materialization| {
            if !matches!(
                materialization.state,
                ControlCapabilitySurfaceState::Prepared { .. }
            ) {
                return None;
            }
            let ControlEffectSubject::Surface {
                package_id,
                lifecycle_generation,
                surface,
                ..
            } = &materialization.intent.subject
            else {
                return None;
            };
            Some(CatalogSurfaceIncarnation {
                package_id: package_id.clone(),
                lifecycle_generation: *lifecycle_generation,
                surface: surface.clone(),
            })
        })
        .collect::<BTreeSet<_>>();

    for descriptor in &catalog.descriptors {
        let package = generation
            .snapshot
            .package_selection(descriptor.package_id.as_str())
            .ok_or_else(|| {
                index_error(
                    "A Capability Gateway descriptor package is absent from Control authority.",
                )
            })?;
        let lifecycle_generation =
            lifecycles
                .get(package.package_id())
                .copied()
                .ok_or_else(|| {
                    index_error(
                        "A Capability Gateway descriptor package has no lifecycle generation.",
                    )
                })?;
        let surface = CatalogSurfaceIncarnation {
            package_id: package.package_id().to_owned(),
            lifecycle_generation,
            surface: descriptor.surface.clone(),
        };
        if !package.enabled
            || descriptor.generation != lifecycle_generation
            || package.package.catalog.record.package.sha256.as_deref()
                != Some(descriptor.package_digest.as_str())
            || package
                .package
                .catalog
                .record
                .package
                .manifest_sha256
                .as_deref()
                != Some(descriptor.manifest_digest.as_str())
            || package.package.catalog.provenance.catalog_record_digest
                != descriptor.publication.catalog_record_digest
            || !package.selected_surfaces.contains(&descriptor.surface)
            || !prepared.contains(&surface)
            || descriptor.dependencies.iter().any(|dependency| {
                !package.selected_surfaces.contains(dependency)
                    || !prepared.contains(&CatalogSurfaceIncarnation {
                        package_id: package.package_id().to_owned(),
                        lifecycle_generation,
                        surface: dependency.clone(),
                    })
            })
        {
            return Err(index_error(
                "A Capability Gateway descriptor is not bound to an enabled, prepared Control surface.",
            ));
        }
    }
    Ok(())
}
