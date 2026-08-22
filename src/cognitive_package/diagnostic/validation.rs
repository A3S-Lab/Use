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
            || !valid_machine_id(&self.scope.id)
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
            || !valid_machine_id(&self.scope.id)
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
            || !valid_machine_id(&self.scope.id)
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
            || !valid_machine_id(&self.scope.id)
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

impl PluginPendingOperationDiagnostic {
    fn validate(&self, registry: &PluginRegistryOperationDiagnostic) -> UseResult<()> {
        a3s_use_core::PluginOperationPlan::validate_operation_id(&self.operation_id)
            .map_err(|_| diagnostic_error("The diagnostic operation identity is invalid."))?;
        let phase_timestamps_valid = match self.phase {
            PluginOperationDiagnosticPhase::Planned => {
                self.admitted_at_ms.is_none() && self.cancelled_at_ms.is_none()
            }
            PluginOperationDiagnosticPhase::Admitted => {
                self.admitted_at_ms
                    .is_some_and(|value| value >= self.planned_at_ms)
                    && self.cancelled_at_ms.is_none()
            }
            PluginOperationDiagnosticPhase::Cancelled => {
                self.cancelled_at_ms
                    .is_some_and(|value| value >= self.planned_at_ms)
                    && self.admitted_at_ms.is_none()
            }
        };
        if !valid_sha256(&self.plan_digest)
            || self.created_at_ms == 0
            || self.expires_at_ms <= self.created_at_ms
            || self.planned_at_ms == 0
            || !phase_timestamps_valid
            || self.package_count == 0
            || self.package_count as usize > MAX_PLUGIN_PLAN_ITEMS
            || self.changed_package_count == 0
            || self.changed_package_count > self.package_count
            || self.source_count as usize != self.sources.len()
            || self.provider_count as usize != self.providers.len()
            || self.lifecycle_unit_count == 0
            || self.lifecycle_unit_count as usize > MAX_DIAGNOSTIC_LIFECYCLE_UNITS
            || self.observed_lifecycle_unit_count as usize != self.lifecycle.len()
            || self.observed_lifecycle_unit_count > self.lifecycle_unit_count
            || self.download_target_count as usize != self.downloads.len()
            || self.planning_target_count as usize != self.planning_targets.len()
            || self.planning_target_count > self.package_count
            || self.sources.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.downloads.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.planning_targets.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.providers.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.lifecycle.len() > MAX_DIAGNOSTIC_LIFECYCLE_UNITS
            || self
                .package_lock_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
            || self
                .prior_package_lock_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
            || !lock_status_matches(
                self.action,
                self.package_lock_digest.as_deref(),
                self.prior_package_lock_digest.as_deref(),
            )
            || !download_projection_matches(self)
            || !confirmation_status_matches(self.phase, self.authority_decision, self.confirmation)
            || !recovery_guidance_matches(
                self.phase,
                registry.operation_cutover.status,
                self.recovery,
            )
        {
            return Err(diagnostic_error(
                "The operation diagnostic is internally inconsistent.",
            ));
        }
        let mut source_packages = BTreeSet::new();
        for source in &self.sources {
            source.validate()?;
            if !source_packages.insert(source.package_id()) {
                return Err(diagnostic_error(
                    "The operation diagnostic contains duplicate source evidence.",
                ));
            }
        }
        if self
            .downloads
            .windows(2)
            .any(|pair| pair[0].package_id >= pair[1].package_id)
        {
            return Err(diagnostic_error(
                "The download diagnostic inventory is not canonical.",
            ));
        }
        for download in &self.downloads {
            download.validate()?;
            let matches_source = self.sources.iter().any(|source| {
                matches!(
                    source,
                    PluginOperationSourceDiagnostic::Registry {
                        package_id,
                        registry_name,
                        archive_digest,
                        ..
                    } if package_id == &download.package_id
                        && registry_name == &download.registry_name
                        && archive_digest == &download.archive_digest
                )
            });
            if !matches_source {
                return Err(diagnostic_error(
                    "A download diagnostic does not match its exact Registry source.",
                ));
            }
        }
        if self
            .planning_targets
            .windows(2)
            .any(|pair| pair[0].package_id >= pair[1].package_id)
        {
            return Err(diagnostic_error(
                "The planning-target diagnostic inventory is not canonical.",
            ));
        }
        for target in &self.planning_targets {
            target.validate()?;
            let matches_source = self.sources.iter().any(|source| {
                matches!(
                    source,
                    PluginOperationSourceDiagnostic::Registry {
                        package_id,
                        registry_name,
                        ..
                    } if package_id == &target.package_id
                        && registry_name == &target.registry_name
                )
            });
            let matches_download = self.downloads.iter().any(|download| {
                download.package_id == target.package_id
                    && download.registry_name == target.registry_name
            });
            if !matches_source || !matches_download {
                return Err(diagnostic_error(
                    "A planning-target diagnostic does not match its exact Registry source.",
                ));
            }
        }
        if self
            .providers
            .windows(2)
            .any(|pair| pair[0].surface >= pair[1].surface)
        {
            return Err(diagnostic_error(
                "The provider diagnostic inventory is not canonical.",
            ));
        }
        for provider in &self.providers {
            provider.validate()?;
        }
        let mut lifecycle_units = BTreeSet::new();
        for lifecycle in &self.lifecycle {
            lifecycle.validate()?;
            if !lifecycle_units.insert((
                lifecycle.package_id.as_str(),
                lifecycle_action_key(lifecycle.action),
            )) {
                return Err(diagnostic_error(
                    "The operation diagnostic contains duplicate lifecycle evidence.",
                ));
            }
        }
        self.grant.validate()?;
        Ok(())
    }
}

impl PluginDownloadTargetDiagnostic {
    fn validate(&self) -> UseResult<()> {
        let status_valid = match self.status {
            PluginDownloadTargetDiagnosticStatus::Missing => self.retained_bytes == 0,
            PluginDownloadTargetDiagnosticStatus::Partial => {
                self.retained_bytes < self.expected_bytes
            }
            PluginDownloadTargetDiagnosticStatus::Complete => {
                self.retained_bytes == self.expected_bytes
            }
        };
        if PluginPackageId::parse(self.package_id.clone()).is_err()
            || !valid_segment(&self.registry_name)
            || !valid_sha256(&self.archive_digest)
            || self.expected_bytes == 0
            || self.retained_bytes > self.expected_bytes
            || !status_valid
        {
            return Err(diagnostic_error(
                "A download target diagnostic is internally inconsistent.",
            ));
        }
        Ok(())
    }
}

impl PluginPlanningTargetDiagnostic {
    fn validate(&self) -> UseResult<()> {
        if PluginPackageId::parse(self.package_id.clone()).is_err()
            || !valid_segment(&self.registry_name)
            || !valid_sha256(&self.target_digest)
            || !valid_target_progress(self.expected_bytes, self.retained_bytes, self.status)
        {
            return Err(diagnostic_error(
                "A planning target diagnostic is internally inconsistent.",
            ));
        }
        Ok(())
    }
}

impl PluginOperationSourceDiagnostic {
    fn package_id(&self) -> &str {
        match self {
            Self::Registry { package_id, .. }
            | Self::ReleaseBundle { package_id, .. }
            | Self::LocalReviewed { package_id, .. } => package_id,
        }
    }

    fn validate(&self) -> UseResult<()> {
        PluginPackageId::parse(self.package_id().to_owned())
            .map_err(|_| diagnostic_error("A diagnostic source package identity is invalid."))?;
        let valid = match self {
            Self::Registry {
                registry_name,
                root_version,
                timestamp_version,
                snapshot_version,
                targets_version,
                catalog_record_digest,
                archive_digest,
                ..
            } => {
                valid_segment(registry_name)
                    && *root_version > 0
                    && *timestamp_version > 0
                    && *snapshot_version > 0
                    && *targets_version > 0
                    && valid_sha256(catalog_record_digest)
                    && valid_sha256(archive_digest)
            }
            Self::ReleaseBundle {
                bundle_digest,
                package_digest,
                ..
            } => valid_sha256(bundle_digest) && valid_sha256(package_digest),
            Self::LocalReviewed {
                source_digest,
                package_digest,
                ..
            } => valid_sha256(source_digest) && valid_sha256(package_digest),
        };
        if !valid {
            return Err(diagnostic_error(
                "A diagnostic source evidence record is invalid.",
            ));
        }
        Ok(())
    }
}

impl PluginProviderOperationDiagnostic {
    fn validate(&self) -> UseResult<()> {
        if PluginPackageId::parse(self.surface.package_id.clone()).is_err()
            || !valid_segment(&self.surface.surface.id)
            || !valid_machine_id(&self.provider_id)
            || !valid_machine_id(&self.provider_build_id)
            || !valid_sha256(&self.capability_digest)
            || !valid_sha256(&self.semantics_profile_digest)
        {
            return Err(diagnostic_error(
                "A provider readiness diagnostic is invalid.",
            ));
        }
        Ok(())
    }
}

impl PluginLifecycleOperationSummary {
    fn validate(&self) -> UseResult<()> {
        let terminal_valid = match self.status {
            PluginLifecycleOperationStatus::Completed => {
                self.completed_checkpoints == self.total_checkpoints
                    && self.completed_at_ms.is_some_and(|time| time > 0)
                    && self.rollback_evidence_digest.is_none()
                    && self.current_checkpoint.is_none()
            }
            PluginLifecycleOperationStatus::RolledBack => {
                self.completed_checkpoints < self.total_checkpoints
                    && self.completed_at_ms.is_some_and(|time| time > 0)
                    && self
                        .rollback_evidence_digest
                        .as_deref()
                        .is_some_and(valid_sha256)
            }
            PluginLifecycleOperationStatus::Applying
            | PluginLifecycleOperationStatus::RollingBack => {
                self.completed_checkpoints <= self.total_checkpoints
                    && self.completed_at_ms.is_none()
                    && self.rollback_evidence_digest.is_none()
            }
        };
        if PluginPackageId::parse(self.package_id.clone()).is_err()
            || self.generation == 0
            || !valid_sha256(&self.intent_digest)
            || self.total_checkpoints == 0
            || !terminal_valid
            || self.current_checkpoint.as_ref().is_some_and(|checkpoint| {
                !valid_current_checkpoint(checkpoint, self.total_checkpoints)
            })
        {
            return Err(diagnostic_error(
                "A lifecycle operation summary is invalid.",
            ));
        }
        Ok(())
    }
}

impl PluginGrantOperationDiagnostic {
    fn validate(&self) -> UseResult<()> {
        let counts = self.candidate_count as usize + self.retirement_count as usize;
        let empty = self.change_set_digest.is_none()
            && self.intent_digest.is_none()
            && self.state_revision_before.is_none()
            && self.state_revision_after.is_none()
            && self.capability_generation_before.is_none()
            && self.capability_generation_after.is_none()
            && self.transitioned_at_ms.is_none()
            && self.cutover_snapshot_digest.is_none()
            && self.cutover_committed_at_ms.is_none()
            && self.rollback_evidence_digest.is_none()
            && self.rolled_back_at_ms.is_none();
        if counts > MAX_PLUGIN_PLAN_ITEMS {
            return Err(diagnostic_error(
                "The Grant operation diagnostic exceeds its item bound.",
            ));
        }
        if matches!(
            self.status,
            PluginGrantDiagnosticStatus::NotRequired
                | PluginGrantDiagnosticStatus::AwaitingAdmission
                | PluginGrantDiagnosticStatus::Cancelled
        ) {
            let status_valid = match self.status {
                PluginGrantDiagnosticStatus::NotRequired => !self.required,
                PluginGrantDiagnosticStatus::AwaitingAdmission => self.required,
                PluginGrantDiagnosticStatus::Cancelled => true,
                _ => false,
            };
            if !status_valid || counts != 0 || !empty {
                return Err(diagnostic_error(
                    "The inactive Grant diagnostic is internally inconsistent.",
                ));
            }
            return Ok(());
        }

        let base_valid = self.required
            && counts > 0
            && self.change_set_digest.as_deref().is_some_and(valid_sha256)
            && self.state_revision_before.is_some_and(|value| value > 0)
            && self.state_revision_after
                == self
                    .state_revision_before
                    .and_then(|value| value.checked_add(1))
            && self.capability_generation_after
                == self
                    .capability_generation_before
                    .and_then(|value| value.checked_add(1))
            && self.transitioned_at_ms.is_some_and(|value| value > 0);
        let journal_status = !matches!(self.status, PluginGrantDiagnosticStatus::Authorized);
        let cutover_status = matches!(
            self.status,
            PluginGrantDiagnosticStatus::CutoverCommitted
                | PluginGrantDiagnosticStatus::Retiring
                | PluginGrantDiagnosticStatus::Completed
        );
        let rollback_status = matches!(
            self.status,
            PluginGrantDiagnosticStatus::RollingBack | PluginGrantDiagnosticStatus::RolledBack
        );
        let intent_valid =
            journal_status == self.intent_digest.as_deref().is_some_and(valid_sha256);
        let cutover_valid = cutover_status
            == (self
                .cutover_snapshot_digest
                .as_deref()
                .is_some_and(valid_sha256)
                && self.cutover_committed_at_ms.is_some_and(|time| {
                    self.transitioned_at_ms.is_some_and(|start| time >= start)
                }));
        let rollback_valid = rollback_status
            == (self
                .rollback_evidence_digest
                .as_deref()
                .is_some_and(valid_sha256)
                && self.rolled_back_at_ms.is_some_and(|time| {
                    self.transitioned_at_ms.is_some_and(|start| time >= start)
                }));
        if !base_valid || !intent_valid || !cutover_valid || !rollback_valid {
            return Err(diagnostic_error(
                "The active Grant diagnostic is internally inconsistent.",
            ));
        }
        Ok(())
    }
}

fn download_projection_matches(operation: &PluginPendingOperationDiagnostic) -> bool {
    if !planning_projection_matches(
        operation.planning_bytes,
        operation.planning_retained_bytes,
        operation.planning_target_count,
        operation.planning,
        &operation.planning_targets,
    ) {
        return false;
    }
    if !matches!(
        operation.action,
        PluginOperationAction::Install | PluginOperationAction::Upgrade
    ) {
        return operation.download_bytes == 0
            && operation.download_retained_bytes == 0
            && operation.download_target_count == 0
            && operation.downloads.is_empty()
            && operation.download == PluginDownloadDiagnosticStatus::NotRequired
            && operation.planning_bytes == 0
            && operation.planning_retained_bytes == 0
            && operation.planning_target_count == 0
            && operation.planning_targets.is_empty()
            && operation.planning == PluginDownloadDiagnosticStatus::NotRequired;
    }
    if operation.download_bytes == 0
        || operation
            .downloads
            .iter()
            .any(|target| target.retained_bytes > target.expected_bytes)
    {
        return false;
    }
    let Some(expected_bytes) = operation.downloads.iter().try_fold(0u64, |total, target| {
        total.checked_add(target.expected_bytes)
    }) else {
        return false;
    };
    let Some(retained_bytes) = operation.downloads.iter().try_fold(0u64, |total, target| {
        total.checked_add(target.retained_bytes)
    }) else {
        return false;
    };
    if retained_bytes != operation.download_retained_bytes
        || expected_bytes > operation.download_bytes
    {
        return false;
    }
    let unavailable_source = operation
        .sources
        .iter()
        .any(|source| !matches!(source, PluginOperationSourceDiagnostic::Registry { .. }));
    match operation.download {
        PluginDownloadDiagnosticStatus::Unavailable => unavailable_source,
        PluginDownloadDiagnosticStatus::Missing => {
            !unavailable_source
                && expected_bytes == operation.download_bytes
                && !operation.downloads.is_empty()
                && operation
                    .downloads
                    .iter()
                    .any(|target| target.status == PluginDownloadTargetDiagnosticStatus::Missing)
                && operation
                    .downloads
                    .iter()
                    .all(|target| target.status != PluginDownloadTargetDiagnosticStatus::Partial)
        }
        PluginDownloadDiagnosticStatus::InProgress => {
            !unavailable_source
                && expected_bytes == operation.download_bytes
                && operation
                    .downloads
                    .iter()
                    .any(|target| target.status == PluginDownloadTargetDiagnosticStatus::Partial)
        }
        PluginDownloadDiagnosticStatus::Complete => {
            !unavailable_source
                && expected_bytes == operation.download_bytes
                && retained_bytes == operation.download_bytes
                && !operation.downloads.is_empty()
                && operation
                    .downloads
                    .iter()
                    .all(|target| target.status == PluginDownloadTargetDiagnosticStatus::Complete)
        }
        PluginDownloadDiagnosticStatus::NotRequired => false,
    }
}

fn planning_projection_matches(
    expected_bytes: u64,
    retained_bytes: u64,
    target_count: u32,
    status: PluginDownloadDiagnosticStatus,
    targets: &[PluginPlanningTargetDiagnostic],
) -> bool {
    if target_count as usize != targets.len()
        || targets.len() > MAX_PLUGIN_PLAN_ITEMS
        || retained_bytes > expected_bytes
    {
        return false;
    }
    if targets.is_empty() {
        return expected_bytes == 0
            && retained_bytes == 0
            && status == PluginDownloadDiagnosticStatus::NotRequired;
    }
    if expected_bytes == 0 {
        return false;
    }
    let Some(observed_expected) = targets.iter().try_fold(0u64, |total, target| {
        total.checked_add(target.expected_bytes)
    }) else {
        return false;
    };
    let Some(observed_retained) = targets.iter().try_fold(0u64, |total, target| {
        total.checked_add(target.retained_bytes)
    }) else {
        return false;
    };
    if observed_expected != expected_bytes || observed_retained != retained_bytes {
        return false;
    }
    match status {
        PluginDownloadDiagnosticStatus::Missing => {
            targets
                .iter()
                .any(|target| target.status == PluginDownloadTargetDiagnosticStatus::Missing)
                && targets
                    .iter()
                    .all(|target| target.status != PluginDownloadTargetDiagnosticStatus::Partial)
        }
        PluginDownloadDiagnosticStatus::InProgress => targets
            .iter()
            .any(|target| target.status == PluginDownloadTargetDiagnosticStatus::Partial),
        PluginDownloadDiagnosticStatus::Complete => targets
            .iter()
            .all(|target| target.status == PluginDownloadTargetDiagnosticStatus::Complete),
        PluginDownloadDiagnosticStatus::NotRequired
        | PluginDownloadDiagnosticStatus::Unavailable => false,
    }
}

fn valid_target_progress(
    expected_bytes: u64,
    retained_bytes: u64,
    status: PluginDownloadTargetDiagnosticStatus,
) -> bool {
    if expected_bytes == 0 || retained_bytes > expected_bytes {
        return false;
    }
    match status {
        PluginDownloadTargetDiagnosticStatus::Missing => retained_bytes == 0,
        PluginDownloadTargetDiagnosticStatus::Partial => retained_bytes < expected_bytes,
        PluginDownloadTargetDiagnosticStatus::Complete => retained_bytes == expected_bytes,
    }
}

fn lock_status_matches(
    action: PluginOperationAction,
    package_lock_digest: Option<&str>,
    prior_package_lock_digest: Option<&str>,
) -> bool {
    match action {
        PluginOperationAction::Install | PluginOperationAction::Uninstall => {
            package_lock_digest.is_some() && prior_package_lock_digest.is_none()
        }
        PluginOperationAction::Upgrade => {
            package_lock_digest.is_some() && prior_package_lock_digest.is_some()
        }
        PluginOperationAction::Enable | PluginOperationAction::Disable => {
            package_lock_digest.is_none() && prior_package_lock_digest.is_none()
        }
    }
}

fn confirmation_status_matches(
    phase: PluginOperationDiagnosticPhase,
    decision: PlanPolicyDecision,
    status: PluginOperationConfirmationDiagnosticStatus,
) -> bool {
    match (phase, decision) {
        (PluginOperationDiagnosticPhase::Cancelled, _) => {
            status == PluginOperationConfirmationDiagnosticStatus::Cancelled
        }
        (PluginOperationDiagnosticPhase::Planned, PlanPolicyDecision::Ask) => {
            status == PluginOperationConfirmationDiagnosticStatus::AwaitingConfirmation
        }
        (PluginOperationDiagnosticPhase::Admitted, PlanPolicyDecision::Ask) => {
            status == PluginOperationConfirmationDiagnosticStatus::Confirmed
        }
        (
            PluginOperationDiagnosticPhase::Planned | PluginOperationDiagnosticPhase::Admitted,
            PlanPolicyDecision::Allow,
        )
        | (PluginOperationDiagnosticPhase::Planned, PlanPolicyDecision::Deny) => {
            status == PluginOperationConfirmationDiagnosticStatus::NotRequired
        }
        (PluginOperationDiagnosticPhase::Admitted, PlanPolicyDecision::Deny) => false,
    }
}

fn recovery_guidance_matches(
    phase: PluginOperationDiagnosticPhase,
    cutover: PluginRegistryCutoverDiagnosticStatus,
    guidance: PluginOperationRecoveryGuidance,
) -> bool {
    if cutover == PluginRegistryCutoverDiagnosticStatus::GenerationDrift {
        return guidance == PluginOperationRecoveryGuidance::OperatorReviewRequired;
    }
    match phase {
        PluginOperationDiagnosticPhase::Planned => {
            guidance == PluginOperationRecoveryGuidance::ReviewAndApplyExactPlan
        }
        PluginOperationDiagnosticPhase::Admitted => {
            guidance == PluginOperationRecoveryGuidance::ResumeExactPlan
        }
        PluginOperationDiagnosticPhase::Cancelled => {
            guidance == PluginOperationRecoveryGuidance::ObserveCancellation
        }
    }
}

fn lifecycle_action_key(action: PluginLifecycleAction) -> u8 {
    match action {
        PluginLifecycleAction::Install => 0,
        PluginLifecycleAction::Upgrade => 1,
        PluginLifecycleAction::Uninstall => 2,
        PluginLifecycleAction::Enable => 3,
        PluginLifecycleAction::Disable => 4,
    }
}

fn valid_current_checkpoint(
    checkpoint: &PluginLifecycleCheckpointDiagnostic,
    total_checkpoints: u32,
) -> bool {
    if checkpoint.sequence == 0
        || checkpoint.sequence > total_checkpoints
        || checkpoint
            .surface
            .as_ref()
            .is_some_and(|surface| !valid_segment(&surface.id))
    {
        return false;
    }
    match checkpoint.status {
        PluginLifecycleCheckpointDiagnosticStatus::Pending => {
            checkpoint.evidence_digest.is_none()
                && checkpoint.error_code.is_none()
                && checkpoint.observed_at_ms.is_none()
        }
        PluginLifecycleCheckpointDiagnosticStatus::Failed => {
            checkpoint
                .evidence_digest
                .as_deref()
                .is_some_and(valid_sha256)
                && checkpoint
                    .error_code
                    .as_deref()
                    .is_some_and(valid_machine_id)
                && checkpoint.observed_at_ms.is_some_and(|time| time > 0)
        }
        PluginLifecycleCheckpointDiagnosticStatus::Applied
        | PluginLifecycleCheckpointDiagnosticStatus::OptionalFailed => false,
    }
}

fn resolution_active_states_valid(states: &[PluginRegistryResolutionStatus]) -> bool {
    states.iter().all(|status| {
        matches!(
            status,
            PluginRegistryResolutionStatus::Verified
                | PluginRegistryResolutionStatus::Verifying
                | PluginRegistryResolutionStatus::Pending
        )
    }) && states
        .windows(2)
        .all(|pair| resolution_active_rank(pair[0]) <= resolution_active_rank(pair[1]))
        && states
            .iter()
            .filter(|status| **status == PluginRegistryResolutionStatus::Verifying)
            .count()
            <= 1
}

fn resolution_failed_states_valid(states: &[PluginRegistryResolutionStatus]) -> bool {
    states.iter().all(|status| {
        matches!(
            status,
            PluginRegistryResolutionStatus::Verified
                | PluginRegistryResolutionStatus::Failed
                | PluginRegistryResolutionStatus::Pending
        )
    }) && states
        .windows(2)
        .all(|pair| resolution_failed_rank(pair[0]) <= resolution_failed_rank(pair[1]))
        && states
            .iter()
            .filter(|status| **status == PluginRegistryResolutionStatus::Failed)
            .count()
            <= 1
}

fn resolution_active_rank(status: PluginRegistryResolutionStatus) -> u8 {
    match status {
        PluginRegistryResolutionStatus::Verified => 0,
        PluginRegistryResolutionStatus::Verifying => 1,
        PluginRegistryResolutionStatus::Pending => 2,
        PluginRegistryResolutionStatus::Failed => 3,
    }
}

fn resolution_failed_rank(status: PluginRegistryResolutionStatus) -> u8 {
    match status {
        PluginRegistryResolutionStatus::Verified => 0,
        PluginRegistryResolutionStatus::Failed => 1,
        PluginRegistryResolutionStatus::Pending => 2,
        PluginRegistryResolutionStatus::Verifying => 3,
    }
}

fn valid_resolution_terminal_time(started_at_ms: u64, completed_at_ms: Option<u64>) -> bool {
    completed_at_ms.is_some_and(|completed| completed >= started_at_ms)
}
