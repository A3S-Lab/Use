// Extension registry binding/validation helpers (included into registry).

async fn verify_package_integrity(extension: &InstalledExtension) -> UseResult<()> {
    let actual = package_sha256(&extension.receipt.package_root).await?;
    if extension.receipt.package_sha256.as_deref() != Some(actual.as_str()) {
        return Err(UseError::new(
            "use.extension.package_digest_mismatch",
            format!(
                "Installed package '{}' no longer matches its recorded digest.",
                extension.receipt.package_id
            ),
        )
        .with_suggestion("Reinstall the extension from its trusted source."));
    }
    Ok(())
}

fn package_bindings(installed: &[InstalledExtension]) -> Vec<ExtensionPackageBinding> {
    installed
        .iter()
        .map(|extension| ExtensionPackageBinding {
            package_id: extension.receipt.package_id.clone(),
            component_id: extension.receipt.component_id.clone(),
            route_alias: extension.receipt.route_alias.clone(),
            version: extension.receipt.version.clone(),
            package_root: extension.receipt.package_root.clone(),
            manifest_sha256: extension.receipt.manifest_sha256.clone(),
            package_sha256: extension.receipt.package_sha256.clone(),
            lifecycle_generation: extension.receipt.lifecycle_generation,
            enabled: extension.receipt.enabled,
            surfaces: extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
        })
        .collect()
}

pub(super) fn validate_surface_selection(
    manifest: &ExtensionManifest,
    catalog: Option<&VerifiedPluginCatalogRecord>,
    selected_surfaces: &[PluginSurfaceRef],
) -> UseResult<()> {
    let selected = selected_surfaces
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let manifest_surfaces = manifest.plugin_surfaces()?;
    let available = manifest_surfaces
        .iter()
        .map(|surface| (surface.surface.clone(), surface))
        .collect::<BTreeMap<_, _>>();
    if selected.is_empty()
        || selected.len() != selected_surfaces.len()
        || selected_surfaces.windows(2).any(|pair| pair[0] >= pair[1])
        || selected
            .iter()
            .any(|surface| !available.contains_key(surface))
        || available
            .values()
            .any(|surface| !surface.optional && !selected.contains(&surface.surface))
        || selected.iter().any(|reference| {
            available.get(reference).is_some_and(|surface| {
                surface
                    .dependencies
                    .iter()
                    .any(|dependency| !selected.contains(dependency))
            })
        })
    {
        return Err(UseError::new(
            "use.extension.receipt_invalid",
            "The extension receipt surface selection is not the manifest's required dependency closure.",
        ));
    }
    if let Some(catalog) = catalog {
        let mut expected = catalog
            .record
            .resolve_surfaces(selected_surfaces)?
            .into_iter()
            .map(|surface| surface.reference())
            .collect::<Vec<_>>();
        expected.sort();
        if expected != selected_surfaces {
            return Err(UseError::new(
                "use.extension.receipt_invalid",
                "The extension receipt surface selection does not match its signed catalog.",
            ));
        }
    }
    Ok(())
}

fn lifecycle_bindings(
    packages: &[ExtensionPackageBinding],
) -> BTreeMap<&str, (u64, &str, &str, &str, bool)> {
    packages
        .iter()
        .filter_map(|binding| {
            let generation = binding.lifecycle_generation?;
            let package_sha256 = binding.package_sha256.as_deref()?;
            Some((
                binding.package_id.as_str(),
                (
                    generation,
                    binding.manifest_sha256.as_str(),
                    package_sha256,
                    binding.version.as_str(),
                    binding.enabled,
                ),
            ))
        })
        .collect()
}

fn published_binding_matches_extension(
    binding: &ExtensionPackageBinding,
    extension: &InstalledExtension,
) -> bool {
    binding == &package_bindings(std::slice::from_ref(extension))[0]
}

fn published_binding_matches_generation(
    binding: &ExtensionPackageBinding,
    extension: &InstalledExtension,
) -> bool {
    binding.enabled
        && binding.package_id == extension.receipt.package_id
        && binding.package_root == extension.receipt.package_root
        && binding.manifest_sha256 == extension.receipt.manifest_sha256
        && binding.package_sha256 == extension.receipt.package_sha256
        && binding.lifecycle_generation == extension.receipt.lifecycle_generation
}

fn unique_published_alias_binding<'a>(
    snapshot: &'a ExtensionRegistrySnapshot,
    alias: &str,
) -> UseResult<Option<&'a ExtensionPackageBinding>> {
    if !crate::valid_route_alias(alias) {
        return Err(UseError::new(
            "use.extension.alias_invalid",
            "Extension aliases must be lowercase, non-reserved identifier segments.",
        ));
    }
    let matches = snapshot
        .packages
        .iter()
        .filter(|binding| binding.enabled && binding.route_alias.as_deref() == Some(alias))
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err(UseError::new(
            "use.extension.alias_ambiguous",
            format!("Extension alias '{alias}' resolves to multiple packages."),
        )
        .with_detail(
            "packageIds",
            matches
                .iter()
                .map(|binding| binding.package_id.clone())
                .collect::<Vec<_>>(),
        )
        .with_suggestion("Select the cognitive package by its canonical publisher/name ID."));
    }
    Ok(matches.into_iter().next())
}

fn control_extension_matches_identity(
    extension: &InstalledExtension,
    identity: &ExtensionLifecycleIdentity,
) -> bool {
    let receipt = &extension.receipt;
    receipt.package_id == identity.package_id()
        && receipt.lifecycle_generation == Some(identity.generation())
        && receipt.package_sha256.as_deref() == identity.package_digest().strip_prefix("sha256:")
        && receipt.manifest_sha256
            == identity
                .manifest_digest()
                .strip_prefix("sha256:")
                .unwrap_or_default()
}

fn lifecycle_generation_lock_path(
    paths: &ExtensionPaths,
    receipt: &ExtensionReceipt,
) -> UseResult<PathBuf> {
    if receipt.schema_version != EXTENSION_RECEIPT_SCHEMA_VERSION {
        return Err(UseError::new(
            "use.extension.lifecycle_receipt_invalid",
            "An extension receipt has inconsistent generation-lease evidence.",
        ));
    }
    let generation = receipt.lifecycle_generation.ok_or_else(|| {
        UseError::new(
            "use.extension.lifecycle_receipt_invalid",
            "A cognitive-package receipt omitted its generation-lease identity.",
        )
    })?;
    Ok(paths.lifecycle_package_lock_path(&receipt.package_id, generation))
}

fn installed_dependents(installed: &[InstalledExtension], package_id: &str) -> Vec<String> {
    installed
        .iter()
        .filter(|extension| {
            extension.receipt.package_id != package_id
                && extension
                    .manifest
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.package_id == package_id)
        })
        .map(|extension| extension.receipt.package_id.clone())
        .collect()
}

fn ensure_no_installed_dependents(
    installed: &[InstalledExtension],
    package_id: &str,
) -> UseResult<()> {
    let required_by = installed_dependents(installed, package_id);
    if required_by.is_empty() {
        return Ok(());
    }
    Err(UseError::new(
        "use.extension.package_required",
        format!("Cognitive package '{package_id}' is still required by another installed package."),
    )
    .with_detail("packageId", package_id.to_string())
    .with_detail("requiredBy", required_by)
    .with_suggestion(
        "Review and apply a cascade uninstall plan that removes dependents before dependencies.",
    ))
}

fn normalize_package_id(value: &str) -> UseResult<String> {
    let value = value.strip_prefix("use/").unwrap_or(value);
    if !super::valid_package_id(value) {
        return Err(UseError::new(
            "use.extension.id_invalid",
            "Extension IDs must be '<publisher>/<name>' lowercase identifiers.",
        ));
    }
    Ok(value.to_string())
}

fn validate_catalog_package(
    catalog: Option<&VerifiedPluginCatalogRecord>,
    registry: Option<&ResolvedRemotePackage>,
    manifest: &ExtensionManifest,
    manifest_bytes: &[u8],
    package_digest: &str,
) -> UseResult<()> {
    let Some(catalog) = catalog else {
        return Ok(());
    };
    let manifest_digest = sha256(manifest_bytes);
    validate_catalog_binding(
        catalog,
        registry,
        manifest,
        &manifest_digest,
        package_digest,
    )
}

fn validate_catalog_binding(
    catalog: &VerifiedPluginCatalogRecord,
    registry: Option<&ResolvedRemotePackage>,
    manifest: &ExtensionManifest,
    manifest_digest: &str,
    package_digest: &str,
) -> UseResult<()> {
    catalog.validate().map_err(|error| {
        catalog_package_error(format!(
            "The verified catalog evidence is invalid: {}",
            error.message
        ))
    })?;
    if !catalog.record.is_package_plan_ready() {
        return Err(catalog_package_error(
            "Only complete catalog evidence can be persisted as plan-ready installation state.",
        ));
    }
    let resolved = ResolvedRemotePackage::from_verified_catalog(catalog).map_err(|error| {
        catalog_package_error(format!(
            "The verified catalog cannot reconstruct its registry target: {}",
            error.message
        ))
    })?;
    if registry != Some(&resolved) {
        return Err(catalog_package_error(
            "The verified catalog does not match the selected registry target.",
        ));
    }
    let record = &catalog.record;
    validate_catalog_manifest_binding(record, manifest)?;
    let expected_package_digest = record
        .package
        .sha256
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"));
    let expected_manifest_digest = record
        .package
        .manifest_sha256
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"));
    if expected_package_digest != Some(package_digest)
        || expected_manifest_digest != Some(manifest_digest)
    {
        return Err(catalog_package_error(
            "The verified catalog does not match the installed package, manifest, or dependency graph.",
        ));
    }
    Ok(())
}

/// Validate the manifest fields that drive lifecycle side effects against one
/// signed catalog record. This is intentionally independent of package bytes
/// so durable operation journals can reject a changed replay manifest before
/// touching a retained generation.
pub fn validate_catalog_manifest_binding(
    record: &PluginCatalogRecord,
    manifest: &ExtensionManifest,
) -> UseResult<()> {
    record.validate().map_err(|error| {
        catalog_package_error(format!(
            "The catalog record is invalid during manifest binding: {}",
            error.message
        ))
    })?;
    if record.package_id != manifest.package_id
        || record.version != manifest.version
        || record.dependencies != manifest.dependencies
    {
        return Err(catalog_package_error(
            "The catalog does not match the manifest package, version, or dependency graph.",
        ));
    }
    if manifest.schema_version == 3 {
        validate_surface_catalog_binding(record, manifest)?;
    }
    Ok(())
}

fn validate_surface_catalog_binding(
    record: &a3s_use_core::PluginCatalogRecord,
    manifest: &ExtensionManifest,
) -> UseResult<()> {
    let manifest_surfaces = manifest.plugin_surfaces()?;
    if record.surfaces.len() != manifest_surfaces.len() {
        return Err(catalog_package_error(
            "The verified catalog surface inventory does not match the installed manifest.",
        ));
    }
    for surface in &manifest_surfaces {
        let Some(catalog) = record
            .surfaces
            .iter()
            .find(|catalog| catalog.reference() == surface.surface)
        else {
            return Err(catalog_package_error(
                "The verified catalog omitted a manifest-declared surface.",
            ));
        };
        if catalog.optional != surface.optional || catalog.requires != surface.dependencies {
            return Err(catalog_package_error(
                "The verified catalog surface dependency graph does not match the installed manifest.",
            ));
        }
    }
    for surface in &manifest.okf {
        let Some(catalog) = record
            .surfaces
            .iter()
            .find(|catalog| catalog.kind == PluginSurfaceKind::Okf && catalog.id == surface.id)
        else {
            return Err(catalog_package_error(
                "The verified catalog omitted a manifest-declared OKF surface.",
            ));
        };
        if catalog.okf_bundle.as_ref() != Some(&surface.bundle) {
            return Err(catalog_package_error(
                "The verified catalog OKF contract does not match the installed manifest.",
            ));
        }
    }
    Ok(())
}

fn catalog_package_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.catalog_package_mismatch", message)
}

fn plan_evidence_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.plan_evidence_missing", message)
}
