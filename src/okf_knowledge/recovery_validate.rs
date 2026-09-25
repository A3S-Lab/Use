// Knowledge restore validation helpers (included into recovery).

fn selected_from_inventory(
    bindings: &[OkfKnowledgeBinding],
) -> UseResult<Vec<(PlanQualifiedSurfaceRef, u64)>> {
    let mut selected = Vec::new();
    for surface in bindings
        .iter()
        .map(|binding| binding.receipt.surface.clone())
        .collect::<BTreeSet<_>>()
    {
        let records = bindings
            .iter()
            .filter(|binding| binding.receipt.surface == surface)
            .cloned()
            .collect::<Vec<_>>();
        let snapshot = super::store::snapshot_from_records(&records)?;
        match (snapshot.selected, snapshot.projection) {
            (Some(binding), Some(_)) => {
                selected.push((surface, binding.receipt.generation));
            }
            (None, None) => {}
            _ => return Err(selection_mismatch()),
        }
    }
    Ok(selected)
}

fn validate_authority_for_plan(
    authority: &AuthorityResult,
    plan: &OkfKnowledgeRestorePlan,
) -> UseResult<()> {
    if authority.digest != plan.authority_digest
        || authority.registry_generation != plan.registry_generation
        || authority.retained_projections != plan.retained_projections
        || authority.removed_tombstones != plan.removed_tombstones
        || authority.selected_projections != plan.selected_projections
        || authority.missing_bindings > plan.missing_bindings
        || authority.missing_bindings == plan.missing_bindings
            && authority.binding_state_digest != plan.binding_state_digest
    {
        return Err(restore_error(
            "use.okf.knowledge_restore_authority_changed",
            "Knowledge package, binding, lifecycle, Registry, or Grant authority changed after restore review.",
        ));
    }
    Ok(())
}

fn restore_in_progress(marker: Option<&journal::ActiveRestoreMarker>) -> UseError {
    let mut error = restore_error(
        "use.okf.knowledge_restore_in_progress",
        "Another durable Knowledge restore must reach its exact terminal result before planning or applying a different restore.",
    )
    .with_suggestion(
        "Resume the active restore with its reviewed plan digest; do not remove its maintenance marker or retained files.",
    );
    if let Some(marker) = marker {
        error = error.with_detail("activePlanDigest", serde_json::json!(marker.plan_digest));
    }
    error
}

fn restore_in_progress_operation(operation: Option<&RestoreOperation>) -> UseError {
    let mut error = restore_error(
        "use.okf.knowledge_restore_in_progress",
        "A nonterminal Knowledge restore must be resumed before another restore can start.",
    )
    .with_suggestion("Resume the existing restore with its exact reviewed plan digest.");
    if let Some(operation) = operation {
        error = error.with_detail("activePlanDigest", serde_json::json!(operation.plan_digest));
    }
    error
}

#[cfg(test)]
const RESTORE_CRASH_CHECKPOINT_ENV: &str = "A3S_USE_TEST_OKF_RESTORE_CHECKPOINT";

#[cfg(test)]
fn maybe_test_crash(status: RestoreOperationStatus) {
    let checkpoint = match status {
        RestoreOperationStatus::Planned => "planned",
        RestoreOperationStatus::Staged => "staged",
        RestoreOperationStatus::BindingsRestored => "bindings-restored",
        RestoreOperationStatus::PriorMoved => "prior-moved",
        RestoreOperationStatus::Published => "published",
        RestoreOperationStatus::Completed => "completed",
    };
    if std::env::var(RESTORE_CRASH_CHECKPOINT_ENV).as_deref() == Ok(checkpoint) {
        std::process::exit(86);
    }
}

#[cfg(test)]
fn maybe_test_crash_binding_restore() {
    if std::env::var(RESTORE_CRASH_CHECKPOINT_ENV).as_deref() == Ok("binding-file-restored") {
        std::process::exit(86);
    }
}

#[cfg(test)]
fn maybe_test_crash_marker() {
    if std::env::var(RESTORE_CRASH_CHECKPOINT_ENV).as_deref() == Ok("marker-active") {
        std::process::exit(86);
    }
}

#[cfg(not(test))]
fn maybe_test_crash(_status: RestoreOperationStatus) {}

#[cfg(not(test))]
fn maybe_test_crash_marker() {}

#[cfg(not(test))]
fn maybe_test_crash_binding_restore() {}

fn validate_inventory_selections(
    bindings: &[OkfKnowledgeBinding],
    selected: &[(PlanQualifiedSurfaceRef, u64)],
) -> UseResult<()> {
    if selected_from_inventory(bindings)? != selected {
        return Err(selection_mismatch());
    }
    Ok(())
}

fn validate_current_binding_subset(
    current: &[OkfKnowledgeBinding],
    expected: &[OkfKnowledgeBinding],
) -> UseResult<usize> {
    let expected_by_key = expected
        .iter()
        .map(|binding| {
            (
                (binding.receipt.surface.clone(), binding.receipt.generation),
                binding,
            )
        })
        .collect::<BTreeMap<_, _>>();
    if expected_by_key.len() != expected.len() {
        return Err(restore_error(
            "use.okf.knowledge_restore_backup_invalid",
            "The Knowledge backup contains duplicate binding identities.",
        ));
    }
    let mut retained = BTreeSet::new();
    for binding in current {
        let key = (binding.receipt.surface.clone(), binding.receipt.generation);
        if expected_by_key.get(&key).copied() != Some(binding) || !retained.insert(key) {
            return Err(restore_error(
                "use.okf.knowledge_restore_binding_conflict",
                "The current Knowledge binding inventory contains changed or newer evidence outside the reviewed backup.",
            )
            .with_suggestion(
                "Preserve the current state and restore from a coordinated backup that contains this exact binding inventory.",
            ));
        }
    }
    Ok(expected.len().saturating_sub(current.len()))
}

fn binding_state_digest(bindings: &[OkfKnowledgeBinding]) -> UseResult<String> {
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json(
            bindings,
            "encode the current Knowledge binding inventory"
        )?)
    ))
}

fn validate_installed_binding(
    installed: &InstalledExtension,
    binding: &OkfKnowledgeBinding,
) -> UseResult<()> {
    let receipt = &binding.receipt;
    if installed.receipt.package_id != receipt.surface.package_id
        || installed.receipt.lifecycle_generation != Some(receipt.generation)
        || installed.receipt.package_sha256.as_deref()
            != receipt.package_digest.strip_prefix("sha256:")
        || installed.receipt.manifest_sha256
            != receipt
                .manifest_digest
                .strip_prefix("sha256:")
                .unwrap_or_default()
        || installed
            .manifest
            .okf
            .iter()
            .find(|surface| surface.id == receipt.surface.surface.id)
            .is_none_or(|surface| surface.bundle != receipt.bundle)
    {
        return Err(restore_error(
            "use.okf.knowledge_restore_registry_mismatch",
            "A Knowledge restore binding does not match its exact immutable package and OKF surface.",
        ));
    }
    Ok(())
}

fn validate_backup_policy(
    manifest: &OkfKnowledgeBackupManifest,
    policy: &super::OkfKnowledgeStoragePolicy,
) -> UseResult<()> {
    let storage = &manifest.storage;
    if storage.max_scope_expanded_bytes != policy.max_scope_expanded_bytes()
        || storage.max_scope_projections != policy.max_scope_projections()
        || storage.max_surface_generations != policy.max_surface_generations()
        || storage.max_scope_tombstones != policy.max_scope_tombstones()
    {
        return Err(restore_error(
            "use.okf.knowledge_restore_policy_mismatch",
            "The Knowledge backup storage policy differs from the current host policy.",
        ));
    }
    Ok(())
}

fn selection_mismatch() -> UseError {
    restore_error(
        "use.okf.knowledge_restore_selection_mismatch",
        "The backup selection differs from the exact durable Knowledge binding projection.",
    )
}

fn canonical_json(value: &(impl Serialize + ?Sized), action: &str) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        restore_error(
            "use.okf.knowledge_restore_plan_invalid",
            format!("Failed to {action}: {error}"),
        )
    })?;
    Ok(bytes)
}

fn now_ms() -> UseResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            restore_error(
                "use.okf.knowledge_restore_clock_invalid",
                format!("The system clock is before the Unix epoch: {error}"),
            )
        })?
        .as_millis();
    u64::try_from(millis)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            restore_error(
                "use.okf.knowledge_restore_clock_invalid",
                "The system clock exceeds the Knowledge restore timestamp range.",
            )
        })
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn restore_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
