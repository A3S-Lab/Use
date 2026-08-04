use std::collections::{BTreeMap, BTreeSet};

use crate::UseResult;

use super::catalog::{catalog_error, CatalogSurface, PluginCatalogRecord, MAX_CATALOG_SURFACES};
use super::validation::strictly_sorted_unique;
use super::{PluginSurfaceKind, PluginSurfaceRef};

impl PluginCatalogRecord {
    /// Resolve mandatory surfaces plus the exact transitive dependency closure
    /// of an explicit optional-surface selection.
    pub fn resolve_surfaces(
        &self,
        requested: &[PluginSurfaceRef],
    ) -> UseResult<Vec<CatalogSurface>> {
        self.validate()?;
        if requested.len() > MAX_CATALOG_SURFACES {
            return Err(catalog_error(
                "The requested plugin surface selection is too large.",
            ));
        }
        let available = self
            .surfaces
            .iter()
            .map(|surface| (surface.reference(), surface))
            .collect::<BTreeMap<_, _>>();
        let requested_count = requested.len();
        let requested = requested.iter().cloned().collect::<BTreeSet<_>>();
        if requested.len() != requested_count
            || requested
                .iter()
                .any(|surface| !available.contains_key(surface))
        {
            return Err(catalog_error(
                "The requested plugin surface selection is invalid.",
            ));
        }
        let mut selected = self
            .surfaces
            .iter()
            .filter(|surface| !surface.optional)
            .map(CatalogSurface::reference)
            .chain(requested)
            .collect::<BTreeSet<_>>();
        loop {
            let before = selected.len();
            let dependencies = selected
                .iter()
                .filter_map(|surface| available.get(surface))
                .flat_map(|surface| surface.requires.iter().cloned())
                .collect::<Vec<_>>();
            selected.extend(dependencies);
            if selected.len() == before {
                break;
            }
        }
        Ok(self
            .surfaces
            .iter()
            .filter(|surface| selected.contains(&surface.reference()))
            .cloned()
            .collect())
    }
}

pub(super) fn validate_surface_dependencies(
    surfaces: &[CatalogSurface],
    surface_refs: &BTreeSet<PluginSurfaceRef>,
    schema_v2: bool,
) -> UseResult<()> {
    if !schema_v2 && surfaces.iter().any(|surface| !surface.requires.is_empty()) {
        return Err(catalog_error(
            "Catalog surface dependencies require schema version 2.",
        ));
    }
    for surface in surfaces {
        if surface.requires.len() > MAX_CATALOG_SURFACES
            || !strictly_sorted_unique(&surface.requires)
            || surface.requires.iter().any(|required| {
                required == &surface.reference()
                    || !surface_refs.contains(required)
                    || !valid_surface_dependency(surface.kind, required.kind)
            })
        {
            return Err(catalog_error(
                "A catalog surface dependency set is invalid.",
            ));
        }
    }
    let mut resolved = BTreeSet::new();
    loop {
        let before = resolved.len();
        for surface in surfaces {
            let reference = surface.reference();
            if surface
                .requires
                .iter()
                .all(|required| resolved.contains(required))
            {
                resolved.insert(reference);
            }
        }
        if resolved.len() == surfaces.len() {
            return Ok(());
        }
        if resolved.len() == before {
            return Err(catalog_error(
                "Catalog surface dependencies must be acyclic.",
            ));
        }
    }
}

fn valid_surface_dependency(owner: PluginSurfaceKind, required: PluginSurfaceKind) -> bool {
    match owner {
        PluginSurfaceKind::Flow => matches!(
            required,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp | PluginSurfaceKind::Okf
        ),
        PluginSurfaceKind::Skill => {
            matches!(
                required,
                PluginSurfaceKind::Flow
                    | PluginSurfaceKind::Tool
                    | PluginSurfaceKind::Mcp
                    | PluginSurfaceKind::Okf
            )
        }
        PluginSurfaceKind::Ui => matches!(
            required,
            PluginSurfaceKind::Flow
                | PluginSurfaceKind::Skill
                | PluginSurfaceKind::Tool
                | PluginSurfaceKind::Mcp
        ),
        PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp | PluginSurfaceKind::Okf => false,
    }
}
