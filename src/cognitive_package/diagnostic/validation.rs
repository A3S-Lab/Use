use super::*;

impl PluginOperationDiagnostic {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        if input.is_empty() || input.len() > MAX_PLUGIN_OPERATION_DIAGNOSTIC_BYTES {
            return Err(diagnostic_error(
                "The cognitive-package operation diagnostic exceeds its input bound.",
            ));
        }
        let diagnostic: Self = serde_json::from_slice(input).map_err(|_| {
            diagnostic_error("The cognitive-package operation diagnostic is invalid JSON.")
        })?;
        diagnostic.validate()?;
        Ok(diagnostic)
    }

    pub fn validate(&self) -> UseResult<()> {
        PluginPackageId::parse(self.package_id.clone())
            .map_err(|_| diagnostic_error("The diagnostic package identity is invalid."))?;
        if self.schema != PLUGIN_OPERATION_DIAGNOSTIC_SCHEMA
            || self.observed_at_ms == 0
            || self.scope.validate().is_err()
        {
            return Err(diagnostic_error(
                "The cognitive-package operation diagnostic is invalid.",
            ));
        }
        self.registry.validate()?;
        self.operation.validate(&self.registry)?;
        let bytes = serde_json::to_vec(self).map_err(|_| {
            diagnostic_error("Failed to encode the cognitive-package operation diagnostic.")
        })?;
        if bytes.len() > MAX_PLUGIN_OPERATION_DIAGNOSTIC_BYTES {
            return Err(diagnostic_error(
                "The cognitive-package operation diagnostic exceeds its output bound.",
            ));
        }
        Ok(())
    }
}

impl PluginOperationHistoryDiagnostic {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        if input.is_empty() || input.len() > MAX_PLUGIN_OPERATION_HISTORY_BYTES {
            return Err(diagnostic_error(
                "The cognitive-package operation history exceeds its input bound.",
            ));
        }
        let diagnostic: Self = serde_json::from_slice(input).map_err(|_| {
            diagnostic_error("The cognitive-package operation history is invalid JSON.")
        })?;
        diagnostic.validate()?;
        Ok(diagnostic)
    }

    pub fn validate(&self) -> UseResult<()> {
        PluginPackageId::parse(self.package_id.clone())
            .map_err(|_| diagnostic_error("The history package identity is invalid."))?;
        if self.schema != PLUGIN_OPERATION_HISTORY_DIAGNOSTIC_SCHEMA
            || self.observed_at_ms == 0
            || self.scope.validate().is_err()
            || self.retention_limit as usize != MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS
            || self.retention_byte_limit as usize != MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES
            || self.retained_operation_count as usize != self.operations.len()
            || self.operations.len() > MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS
        {
            return Err(diagnostic_error(
                "The cognitive-package operation history is invalid.",
            ));
        }
        let mut operation_occurrences = BTreeSet::new();
        for retained in &self.operations {
            retained.validate()?;
            let operation = &retained.diagnostic;
            if operation.scope != self.scope
                || operation.package_id != self.package_id
                || !operation_occurrences.insert((
                    operation.operation.operation_id.as_str(),
                    operation.operation.plan_digest.as_str(),
                ))
            {
                return Err(diagnostic_error(
                    "The cognitive-package operation history is internally inconsistent.",
                ));
            }
        }
        let bytes = serde_json::to_vec(self).map_err(|_| {
            diagnostic_error("Failed to encode the cognitive-package operation history.")
        })?;
        if bytes.len() > MAX_PLUGIN_OPERATION_HISTORY_BYTES {
            return Err(diagnostic_error(
                "The cognitive-package operation history exceeds its output bound.",
            ));
        }
        Ok(())
    }
}

impl PluginRetainedOperationDiagnostic {
    pub(in crate::cognitive_package) fn validate(&self) -> UseResult<()> {
        self.diagnostic.validate()?;
        if self.retained_at_ms == 0
            || self.retained_at_ms != self.diagnostic.observed_at_ms
            || !retained_outcome_matches(self.outcome, &self.diagnostic)
        {
            return Err(diagnostic_error(
                "The retained cognitive-package operation outcome is inconsistent.",
            ));
        }
        Ok(())
    }
}

fn retained_outcome_matches(
    outcome: PluginRetainedOperationOutcome,
    diagnostic: &PluginOperationDiagnostic,
) -> bool {
    let operation = &diagnostic.operation;
    match outcome {
        PluginRetainedOperationOutcome::Completed => {
            operation.phase == PluginOperationDiagnosticPhase::Admitted
                && operation.observed_lifecycle_unit_count == operation.lifecycle_unit_count
                && !operation.lifecycle.is_empty()
                && operation.lifecycle.iter().all(|unit| {
                    unit.status == PluginLifecycleOperationStatus::Completed
                        && unit.completed_at_ms.is_some()
                })
                && matches!(
                    operation.grant.status,
                    PluginGrantDiagnosticStatus::NotRequired
                        | PluginGrantDiagnosticStatus::Completed
                )
                && matches!(
                    diagnostic.registry.operation_cutover.status,
                    PluginRegistryCutoverDiagnosticStatus::Acknowledged
                        | PluginRegistryCutoverDiagnosticStatus::Superseded
                )
        }
        PluginRetainedOperationOutcome::RolledBack => {
            operation.phase == PluginOperationDiagnosticPhase::Admitted
                && !operation.lifecycle.is_empty()
                && operation.lifecycle.iter().all(|unit| {
                    unit.status == PluginLifecycleOperationStatus::RolledBack
                        && unit.completed_at_ms.is_some()
                })
                && matches!(
                    operation.grant.status,
                    PluginGrantDiagnosticStatus::NotRequired
                        | PluginGrantDiagnosticStatus::RolledBack
                )
                && diagnostic.registry.operation_cutover.status
                    == PluginRegistryCutoverDiagnosticStatus::NotObserved
        }
        PluginRetainedOperationOutcome::Cancelled => {
            operation.phase == PluginOperationDiagnosticPhase::Cancelled
                && operation.cancelled_at_ms.is_some()
                && operation.lifecycle.is_empty()
                && operation.observed_lifecycle_unit_count == 0
                && operation.grant.status == PluginGrantDiagnosticStatus::Cancelled
                && diagnostic.registry.operation_cutover.status
                    == PluginRegistryCutoverDiagnosticStatus::NotObserved
        }
    }
}

impl PluginDownloadAttemptDiagnostic {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        if input.is_empty() || input.len() > MAX_PLUGIN_OPERATION_DIAGNOSTIC_BYTES {
            return Err(diagnostic_error(
                "The package download attempt diagnostic exceeds its input bound.",
            ));
        }
        let diagnostic: Self = serde_json::from_slice(input).map_err(|_| {
            diagnostic_error("The package download attempt diagnostic is invalid JSON.")
        })?;
        diagnostic.validate()?;
        Ok(diagnostic)
    }

    pub fn validate(&self) -> UseResult<()> {
        PluginPackageId::parse(self.package_id.clone())
            .map_err(|_| diagnostic_error("The download package identity is invalid."))?;
        if self.schema != PLUGIN_DOWNLOAD_ATTEMPT_DIAGNOSTIC_SCHEMA
            || self.observed_at_ms == 0
            || self.scope.validate().is_err()
        {
            return Err(diagnostic_error(
                "The package download attempt diagnostic is invalid.",
            ));
        }
        self.attempt.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| {
            diagnostic_error("Failed to encode the package download attempt diagnostic.")
        })?;
        if bytes.len() > MAX_PLUGIN_OPERATION_DIAGNOSTIC_BYTES {
            return Err(diagnostic_error(
                "The package download attempt diagnostic exceeds its output bound.",
            ));
        }
        Ok(())
    }
}

impl PluginPendingDownloadAttemptDiagnostic {
    fn validate(&self) -> UseResult<()> {
        if !matches!(
            self.action,
            PluginOperationAction::Install | PluginOperationAction::Upgrade
        ) || self.phase != PluginDownloadAttemptPhase::PrePlan
            || self.started_at_ms == 0
            || !valid_sha256(&self.package_lock_digest)
            || self.package_count == 0
            || self.package_count as usize > MAX_PLUGIN_PLAN_ITEMS
            || self.download_target_count == 0
            || self.download_target_count > self.package_count
            || self.download_target_count as usize != self.downloads.len()
            || self.download_bytes == 0
            || self.download_retained_bytes > self.download_bytes
            || self.downloads.len() > MAX_PLUGIN_PLAN_ITEMS
            || self
                .downloads
                .windows(2)
                .any(|pair| pair[0].package_id >= pair[1].package_id)
            || self.planning_target_count > self.package_count
            || !planning_projection_matches(
                self.planning_bytes,
                self.planning_retained_bytes,
                self.planning_target_count,
                self.planning,
                &self.planning_targets,
            )
            || self
                .planning_targets
                .windows(2)
                .any(|pair| pair[0].package_id >= pair[1].package_id)
        {
            return Err(diagnostic_error(
                "The pending package download diagnostic is invalid.",
            ));
        }
        for target in &self.downloads {
            target.validate()?;
        }
        for target in &self.planning_targets {
            target.validate()?;
            if !self.downloads.iter().any(|download| {
                download.package_id == target.package_id
                    && download.registry_name == target.registry_name
            }) {
                return Err(diagnostic_error(
                    "A planning target does not match its exact package download.",
                ));
            }
        }
        let expected_bytes = self
            .downloads
            .iter()
            .try_fold(0u64, |total, target| {
                total.checked_add(target.expected_bytes)
            })
            .ok_or_else(|| diagnostic_error("The download byte total is exhausted."))?;
        let retained_bytes = self
            .downloads
            .iter()
            .try_fold(0u64, |total, target| {
                total.checked_add(target.retained_bytes)
            })
            .ok_or_else(|| diagnostic_error("The retained download byte total is exhausted."))?;
        let status_valid = match self.download {
            PluginDownloadDiagnosticStatus::Missing => {
                self.downloads
                    .iter()
                    .any(|target| target.status == PluginDownloadTargetDiagnosticStatus::Missing)
                    && self.downloads.iter().all(|target| {
                        target.status != PluginDownloadTargetDiagnosticStatus::Partial
                    })
            }
            PluginDownloadDiagnosticStatus::InProgress => self
                .downloads
                .iter()
                .any(|target| target.status == PluginDownloadTargetDiagnosticStatus::Partial),
            PluginDownloadDiagnosticStatus::Complete => self
                .downloads
                .iter()
                .all(|target| target.status == PluginDownloadTargetDiagnosticStatus::Complete),
            PluginDownloadDiagnosticStatus::NotRequired
            | PluginDownloadDiagnosticStatus::Unavailable => false,
        };
        if expected_bytes != self.download_bytes
            || retained_bytes != self.download_retained_bytes
            || !status_valid
        {
            return Err(diagnostic_error(
                "The package download byte projection is inconsistent.",
            ));
        }
        Ok(())
    }
}

impl PluginResolutionAttemptDiagnostic {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        if input.is_empty() || input.len() > MAX_PLUGIN_OPERATION_DIAGNOSTIC_BYTES {
            return Err(diagnostic_error(
                "The Registry resolution diagnostic exceeds its input bound.",
            ));
        }
        let diagnostic: Self = serde_json::from_slice(input)
            .map_err(|_| diagnostic_error("The Registry resolution diagnostic is invalid JSON."))?;
        diagnostic.validate()?;
        Ok(diagnostic)
    }

    pub fn validate(&self) -> UseResult<()> {
        PluginPackageId::parse(self.package_id.clone())
            .map_err(|_| diagnostic_error("The resolution package identity is invalid."))?;
        if self.schema != PLUGIN_RESOLUTION_ATTEMPT_DIAGNOSTIC_SCHEMA
            || self.observed_at_ms == 0
            || self.scope.validate().is_err()
            || self.attempt.started_at_ms > self.observed_at_ms
            || self
                .attempt
                .completed_at_ms
                .is_some_and(|completed| completed > self.observed_at_ms)
            || self.attempt.registries.iter().any(|registry| {
                registry
                    .observed_at_ms
                    .is_some_and(|observed| observed > self.observed_at_ms)
            })
        {
            return Err(diagnostic_error(
                "The Registry resolution diagnostic is invalid.",
            ));
        }
        self.attempt.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| {
            diagnostic_error("Failed to encode the Registry resolution diagnostic.")
        })?;
        if bytes.len() > MAX_PLUGIN_OPERATION_DIAGNOSTIC_BYTES {
            return Err(diagnostic_error(
                "The Registry resolution diagnostic exceeds its output bound.",
            ));
        }
        Ok(())
    }
}

impl PluginPendingResolutionAttemptDiagnostic {
    fn validate(&self) -> UseResult<()> {
        if !matches!(
            self.action,
            PluginOperationAction::Install | PluginOperationAction::Upgrade
        ) || self.phase != PluginResolutionAttemptPhase::PreLock
            || self.started_at_ms == 0
            || self.requested_version.as_deref().is_some_and(|version| {
                semver::Version::parse(version)
                    .map(|parsed| parsed.to_string() != version)
                    .unwrap_or(true)
            })
            || self.registry_count == 0
            || self.registry_count as usize != self.registries.len()
            || self.registry_count as usize > a3s_use_extension::MAX_CONFIGURED_REGISTRY_SOURCES
            || self.verified_registry_count
                != self
                    .registries
                    .iter()
                    .filter(|registry| registry.status == PluginRegistryResolutionStatus::Verified)
                    .count() as u32
            || self
                .registries
                .windows(2)
                .any(|pair| pair[0].registry_name >= pair[1].registry_name)
            || self
                .registries
                .iter()
                .filter(|registry| registry.role == PluginRegistryResolutionRole::Root)
                .count()
                != 1
        {
            return Err(diagnostic_error(
                "The pending Registry resolution diagnostic is invalid.",
            ));
        }
        for registry in &self.registries {
            registry.validate()?;
        }
        let states = self
            .registries
            .iter()
            .map(|registry| registry.status)
            .collect::<Vec<_>>();
        let valid = match self.status {
            PluginResolutionDiagnosticStatus::Resolving => {
                self.completed_at_ms.is_none()
                    && self.package_lock_digest.is_none()
                    && self.package_count.is_none()
                    && self.error_code.is_none()
                    && resolution_active_states_valid(&states)
            }
            PluginResolutionDiagnosticStatus::Resolved => {
                valid_resolution_terminal_time(self.started_at_ms, self.completed_at_ms)
                    && self
                        .package_lock_digest
                        .as_deref()
                        .is_some_and(valid_sha256)
                    && self
                        .package_count
                        .is_some_and(|count| count > 0 && count as usize <= MAX_PLUGIN_PLAN_ITEMS)
                    && self.error_code.is_none()
                    && states
                        .iter()
                        .all(|status| *status == PluginRegistryResolutionStatus::Verified)
            }
            PluginResolutionDiagnosticStatus::Failed => {
                valid_resolution_terminal_time(self.started_at_ms, self.completed_at_ms)
                    && self.package_lock_digest.is_none()
                    && self.package_count.is_none()
                    && self.error_code.as_deref().is_some_and(valid_machine_id)
                    && resolution_failed_states_valid(&states)
            }
        };
        if !valid {
            return Err(diagnostic_error(
                "The Registry resolution state projection is inconsistent.",
            ));
        }
        Ok(())
    }
}

impl PluginRegistryResolutionDiagnostic {
    fn validate(&self) -> UseResult<()> {
        if !valid_segment(&self.registry_name)
            || !valid_sha256(&self.source_identity_digest)
            || !valid_sha256(&self.trust_root_digest)
        {
            return Err(diagnostic_error(
                "A Registry resolution entry has invalid identity evidence.",
            ));
        }
        let versions = [
            self.root_version,
            self.timestamp_version,
            self.snapshot_version,
            self.targets_version,
        ];
        let valid = match self.status {
            PluginRegistryResolutionStatus::Pending => {
                versions.iter().all(Option::is_none)
                    && self.package_targets.is_none()
                    && self.observed_at_ms.is_none()
                    && self.error_code.is_none()
            }
            PluginRegistryResolutionStatus::Verifying => {
                versions.iter().all(Option::is_none)
                    && self.package_targets.is_none()
                    && self.observed_at_ms.is_some_and(|time| time > 0)
                    && self.error_code.is_none()
            }
            PluginRegistryResolutionStatus::Verified => {
                versions
                    .iter()
                    .all(|version| version.is_some_and(|value| value > 0))
                    && self.package_targets.is_some_and(|count| count <= 10_000)
                    && self.observed_at_ms.is_some_and(|time| time > 0)
                    && self.error_code.is_none()
            }
            PluginRegistryResolutionStatus::Failed => {
                versions.iter().all(Option::is_none)
                    && self.package_targets.is_none()
                    && self.observed_at_ms.is_some_and(|time| time > 0)
                    && self.error_code.as_deref().is_some_and(valid_machine_id)
            }
        };
        if !valid {
            return Err(diagnostic_error(
                "A Registry resolution entry is inconsistent.",
            ));
        }
        Ok(())
    }
}

impl PluginRegistryOperationDiagnostic {
    fn validate(&self) -> UseResult<()> {
        if !valid_sha256(&self.snapshot_digest)
            || self.pending_cutover_count as usize
                > a3s_use_extension::MAX_PENDING_REGISTRY_CUTOVERS
        {
            return Err(diagnostic_error(
                "The Registry operation diagnostic is invalid.",
            ));
        }
        self.operation_cutover
            .validate(self.generation, self.pending_cutover_count)
    }
}

impl PluginRegistryCutoverDiagnostic {
    fn validate(&self, current_generation: u64, pending_count: u32) -> UseResult<()> {
        let Some(expected_after) = self.expected_generation_before.checked_add(1) else {
            return Err(diagnostic_error(
                "The diagnostic Registry generation is exhausted.",
            ));
        };
        let digest_valid = self
            .recorded_snapshot_digest
            .as_deref()
            .is_none_or(valid_sha256);
        let coherent = match self.status {
            PluginRegistryCutoverDiagnosticStatus::NotObserved => {
                current_generation == self.expected_generation_before
                    && self.recorded_generation_after.is_none()
                    && self.recorded_snapshot_digest.is_none()
            }
            PluginRegistryCutoverDiagnosticStatus::Recorded => {
                pending_count > 0
                    && current_generation >= expected_after
                    && self.recorded_generation_after == Some(expected_after)
                    && self.recorded_snapshot_digest.is_some()
            }
            PluginRegistryCutoverDiagnosticStatus::Acknowledged => {
                current_generation == expected_after
                    && self.recorded_generation_after == Some(expected_after)
            }
            PluginRegistryCutoverDiagnosticStatus::Superseded => {
                current_generation > expected_after
                    && self.recorded_generation_after == Some(expected_after)
            }
            PluginRegistryCutoverDiagnosticStatus::GenerationDrift => {
                current_generation > self.expected_generation_before
                    && self.recorded_generation_after.is_none()
                    && self.recorded_snapshot_digest.is_none()
            }
        };
        if self.expected_generation_after != expected_after || !digest_valid || !coherent {
            return Err(diagnostic_error(
                "The Registry cutover diagnostic is internally inconsistent.",
            ));
        }
        Ok(())
    }
}

include!("validation_pending.rs");
