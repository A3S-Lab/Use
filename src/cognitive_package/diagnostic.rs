use std::collections::BTreeSet;

use a3s_use_core::{
    PlanActor, PlanEnforcementProfile, PlanPackageChangeKind, PlanPolicyDecision, PlanScope,
    PlannedPackageState, PluginOperationAction, PluginOperationPlanEnvelope, PluginPackageId,
    PluginPlanSource, UseError, UseResult, MAX_PLUGIN_PLAN_ITEMS,
};
use a3s_use_extension::{
    ExtensionRegistryCutoverRecord, RegistrySourceStore, WorkspaceGrantLifecyclePhase,
    WorkspaceGrantOperationJournal,
};
use serde::{Deserialize, Serialize};

use crate::plugin_lifecycle::{
    operation_cutover_key, PluginLifecycleAction, PluginLifecycleCheckpointDiagnostic,
    PluginLifecycleCheckpointDiagnosticStatus, PluginLifecycleCheckpointKind,
    PluginLifecycleJournalStore, PluginLifecycleOperationDiagnostic,
    PluginLifecycleOperationStatus,
};

use super::download_attempt::PendingPackageDownloadAttempt;
use super::host_store::PluginHostProtocolStore;
use super::resolution_attempt::{
    PackageRegistryResolutionRole, PackageRegistryResolutionStatus, PackageResolutionAccess,
    PackageResolutionAttemptStatus,
};
use super::store::{PackageGraphOperationPhase, PendingPackageGraphOperation};
use super::CognitivePackageManager;

mod enablement;
mod projection;
#[cfg(test)]
pub(super) mod tests;
mod validation;

pub(super) use enablement::diagnose_enablement_operation;
use enablement::{diagnose_reviewed_enablement_operation, pending_enablement};
use projection::{
    confirmation_status, diagnostic_phase, expected_lifecycle_units, observe_grant,
    observe_lifecycle, project_download_attempt, project_downloads, project_providers,
    project_registry_cutover, project_sources,
};

pub const PLUGIN_OPERATION_DIAGNOSTIC_SCHEMA: &str = "a3s.use.plugin-operation-diagnostic.v1";
pub const PLUGIN_OPERATION_HISTORY_DIAGNOSTIC_SCHEMA: &str =
    "a3s.use.plugin-operation-history-diagnostic.v1";
pub const PLUGIN_DOWNLOAD_ATTEMPT_DIAGNOSTIC_SCHEMA: &str =
    "a3s.use.plugin-download-attempt-diagnostic.v1";
pub const PLUGIN_RESOLUTION_ATTEMPT_DIAGNOSTIC_SCHEMA: &str =
    "a3s.use.plugin-resolution-attempt-diagnostic.v1";
pub const MAX_PLUGIN_OPERATION_DIAGNOSTIC_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PLUGIN_OPERATION_HISTORY_BYTES: usize =
    MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES + 64 * 1024;
pub const MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS: usize = 16;
const MAX_DIAGNOSTIC_LIFECYCLE_UNITS: usize = MAX_PLUGIN_PLAN_ITEMS * 2;

/// Read-only cross-product evidence for one exact retained graph or active
/// enablement operation.
///
/// The projection intentionally excludes paths, idempotency keys, Registry
/// URLs, credentials, tokens, secret names and values, package content, and
/// arbitrary package-authored text. It is observation only and cannot be used
/// as apply or recovery authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationDiagnostic {
    pub schema: String,
    pub observed_at_ms: u64,
    pub scope: PlanScope,
    pub package_id: String,
    pub registry: PluginRegistryOperationDiagnostic,
    pub operation: PluginPendingOperationDiagnostic,
}

/// Bounded, newest-first history of completed or otherwise retired operation
/// diagnostics for one explicit package and scope.
///
/// Entries are immutable observations, not lifecycle journals or recovery
/// authority. Active operation and pre-plan download evidence remain available
/// through the default single-operation diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationHistoryDiagnostic {
    pub schema: String,
    pub observed_at_ms: u64,
    pub scope: PlanScope,
    pub package_id: String,
    pub retention_limit: u32,
    pub retention_byte_limit: u64,
    pub retained_operation_count: u32,
    pub operations: Vec<PluginRetainedOperationDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRetainedOperationDiagnostic {
    pub retained_at_ms: u64,
    pub outcome: PluginRetainedOperationOutcome,
    pub diagnostic: PluginOperationDiagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginRetainedOperationOutcome {
    Completed,
    RolledBack,
    Cancelled,
}

/// Read-only cache evidence retained before package validation can produce a
/// reviewed operation plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDownloadAttemptDiagnostic {
    pub schema: String,
    pub observed_at_ms: u64,
    pub scope: PlanScope,
    pub package_id: String,
    pub attempt: PluginPendingDownloadAttemptDiagnostic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPendingDownloadAttemptDiagnostic {
    pub action: PluginOperationAction,
    pub phase: PluginDownloadAttemptPhase,
    pub started_at_ms: u64,
    pub package_lock_digest: String,
    pub package_count: u32,
    pub download_bytes: u64,
    pub download_retained_bytes: u64,
    pub download_target_count: u32,
    pub download: PluginDownloadDiagnosticStatus,
    pub downloads: Vec<PluginDownloadTargetDiagnostic>,
    pub planning_bytes: u64,
    pub planning_retained_bytes: u64,
    pub planning_target_count: u32,
    pub planning: PluginDownloadDiagnosticStatus,
    pub planning_targets: Vec<PluginPlanningTargetDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginDownloadAttemptPhase {
    PrePlan,
}

/// Read-only Registry/TUF evidence retained before an exact package lock can
/// exist. It contains trust digests and signed role versions, never URLs,
/// paths, metadata bytes, credentials, or arbitrary transport errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginResolutionAttemptDiagnostic {
    pub schema: String,
    pub observed_at_ms: u64,
    pub scope: PlanScope,
    pub package_id: String,
    pub attempt: PluginPendingResolutionAttemptDiagnostic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPendingResolutionAttemptDiagnostic {
    pub action: PluginOperationAction,
    pub phase: PluginResolutionAttemptPhase,
    pub access: PluginRegistryResolutionAccess,
    pub status: PluginResolutionDiagnosticStatus,
    pub started_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_version: Option<String>,
    pub channel: a3s_use_core::PluginReleaseChannel,
    pub registry_count: u32,
    pub verified_registry_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_lock_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub registries: Vec<PluginRegistryResolutionDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginResolutionAttemptPhase {
    PreLock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginRegistryResolutionAccess {
    Refreshed,
    Cached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginResolutionDiagnosticStatus {
    Resolving,
    Resolved,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRegistryResolutionDiagnostic {
    pub registry_name: String,
    pub role: PluginRegistryResolutionRole,
    pub source_identity_digest: String,
    pub trust_root_digest: String,
    pub status: PluginRegistryResolutionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub targets_version: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_targets: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginRegistryResolutionRole {
    Root,
    Dependency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginRegistryResolutionStatus {
    Pending,
    Verifying,
    Verified,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRegistryOperationDiagnostic {
    pub generation: u64,
    pub snapshot_digest: String,
    pub pending_cutover_count: u32,
    pub operation_cutover: PluginRegistryCutoverDiagnostic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginRegistryCutoverDiagnostic {
    pub status: PluginRegistryCutoverDiagnosticStatus,
    pub expected_generation_before: u64,
    pub expected_generation_after: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_generation_after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_snapshot_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginRegistryCutoverDiagnosticStatus {
    NotObserved,
    Recorded,
    Acknowledged,
    Superseded,
    GenerationDrift,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPendingOperationDiagnostic {
    pub operation_id: String,
    pub action: PluginOperationAction,
    pub phase: PluginOperationDiagnosticPhase,
    pub plan_digest: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub planned_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admitted_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_lock_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior_package_lock_digest: Option<String>,
    pub authority_actor: PlanActor,
    pub authority_decision: PlanPolicyDecision,
    pub confirmation: PluginOperationConfirmationDiagnosticStatus,
    pub package_count: u32,
    pub changed_package_count: u32,
    pub source_count: u32,
    pub provider_count: u32,
    pub lifecycle_unit_count: u32,
    pub observed_lifecycle_unit_count: u32,
    pub download_bytes: u64,
    pub download_retained_bytes: u64,
    pub download_target_count: u32,
    pub download: PluginDownloadDiagnosticStatus,
    pub plan_drain_required: bool,
    pub downloads: Vec<PluginDownloadTargetDiagnostic>,
    pub planning_bytes: u64,
    pub planning_retained_bytes: u64,
    pub planning_target_count: u32,
    pub planning: PluginDownloadDiagnosticStatus,
    pub planning_targets: Vec<PluginPlanningTargetDiagnostic>,
    pub sources: Vec<PluginOperationSourceDiagnostic>,
    pub providers: Vec<PluginProviderOperationDiagnostic>,
    pub grant: PluginGrantOperationDiagnostic,
    pub lifecycle: Vec<PluginLifecycleOperationSummary>,
    pub recovery: PluginOperationRecoveryGuidance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginOperationDiagnosticPhase {
    Planned,
    Admitted,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginOperationConfirmationDiagnosticStatus {
    NotRequired,
    AwaitingConfirmation,
    Confirmed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginDownloadDiagnosticStatus {
    NotRequired,
    Unavailable,
    Missing,
    InProgress,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginDownloadTargetDiagnostic {
    pub package_id: String,
    pub registry_name: String,
    pub archive_digest: String,
    pub expected_bytes: u64,
    pub retained_bytes: u64,
    pub status: PluginDownloadTargetDiagnosticStatus,
}

/// Path-free byte evidence for one exact separately signed executable-planning
/// target. It is cache observation only and never planning or apply authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPlanningTargetDiagnostic {
    pub package_id: String,
    pub registry_name: String,
    pub target_digest: String,
    pub expected_bytes: u64,
    pub retained_bytes: u64,
    pub status: PluginDownloadTargetDiagnosticStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginDownloadTargetDiagnosticStatus {
    Missing,
    Partial,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginOperationSourceDiagnostic {
    Registry {
        package_id: String,
        registry_name: String,
        root_version: u64,
        timestamp_version: u64,
        snapshot_version: u64,
        targets_version: u64,
        catalog_record_digest: String,
        archive_digest: String,
    },
    ReleaseBundle {
        package_id: String,
        bundle_digest: String,
        package_digest: String,
    },
    LocalReviewed {
        package_id: String,
        source_digest: String,
        package_digest: String,
        unsigned: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginProviderOperationDiagnostic {
    pub surface: a3s_use_core::PlanQualifiedSurfaceRef,
    pub provider_id: String,
    pub provider_build_id: String,
    pub capability_digest: String,
    pub semantics_profile_digest: String,
    pub enforcement: PlanEnforcementProfile,
    pub readiness: PluginProviderDiagnosticReadiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginProviderDiagnosticReadiness {
    Selected,
    Preparing,
    Ready,
    OptionalFailed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginGrantOperationDiagnostic {
    pub required: bool,
    pub status: PluginGrantDiagnosticStatus,
    pub candidate_count: u32,
    pub retirement_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_set_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_revision_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_revision_after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_generation_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_generation_after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transitioned_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutover_snapshot_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutover_committed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_evidence_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rolled_back_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginGrantDiagnosticStatus {
    NotRequired,
    AwaitingAdmission,
    Cancelled,
    Authorized,
    IntentRecorded,
    Preparing,
    Prepared,
    CutoverCommitted,
    Retiring,
    Completed,
    RollingBack,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleOperationSummary {
    pub package_id: String,
    pub action: PluginLifecycleAction,
    pub status: PluginLifecycleOperationStatus,
    pub generation: u64,
    pub intent_digest: String,
    pub completed_checkpoints: u32,
    pub total_checkpoints: u32,
    pub publication: PluginLifecyclePublicationDiagnosticStatus,
    pub drain: PluginLifecycleDrainDiagnosticStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_checkpoint: Option<PluginLifecycleCheckpointDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_evidence_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginLifecyclePublicationDiagnosticStatus {
    Pending,
    Published,
    Hidden,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginLifecycleDrainDiagnosticStatus {
    NotRequired,
    Pending,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginOperationRecoveryGuidance {
    ReviewAndApplyExactPlan,
    ResumeExactPlan,
    ObserveCancellation,
    OperatorReviewRequired,
}

#[derive(Debug, Clone)]
struct ExpectedLifecycleUnit {
    package_id: String,
    action: PluginLifecycleAction,
    generation: u64,
    package_digest: String,
    manifest_digest: String,
    total_checkpoints: u32,
}

#[derive(Debug, Clone)]
struct ObservedLifecycleUnit {
    raw: PluginLifecycleOperationDiagnostic,
    summary: PluginLifecycleOperationSummary,
}

#[derive(Debug, Clone)]
struct DownloadProjection {
    expected_bytes: u64,
    retained_bytes: u64,
    status: PluginDownloadDiagnosticStatus,
    targets: Vec<PluginDownloadTargetDiagnostic>,
    planning_expected_bytes: u64,
    planning_retained_bytes: u64,
    planning_status: PluginDownloadDiagnosticStatus,
    planning_targets: Vec<PluginPlanningTargetDiagnostic>,
}

impl DownloadProjection {
    fn not_required() -> Self {
        Self {
            expected_bytes: 0,
            retained_bytes: 0,
            status: PluginDownloadDiagnosticStatus::NotRequired,
            targets: Vec::new(),
            planning_expected_bytes: 0,
            planning_retained_bytes: 0,
            planning_status: PluginDownloadDiagnosticStatus::NotRequired,
            planning_targets: Vec::new(),
        }
    }
}

impl CognitivePackageManager {
    /// Diagnose one exact retained graph or active enablement operation without
    /// applying, recovering, cancelling, reconciling, or otherwise mutating it.
    pub async fn diagnose_operation(
        &self,
        package_id: &str,
    ) -> UseResult<PluginOperationDiagnostic> {
        let parsed_package_id = PluginPackageId::parse(package_id.to_owned()).map_err(|_| {
            diagnostic_error("The operation diagnostic package identity is invalid.")
        })?;
        let _maintenance = self
            .maintenance_lock()
            .acquire_shared()
            .await
            .map_err(|_| diagnostic_state_error())?;
        let graph = self
            .pending_store()
            .get_for_package(package_id)
            .await
            .map_err(|_| diagnostic_state_error())?;
        let enablement = pending_enablement(self, &parsed_package_id).await?;
        match (graph, enablement) {
            (Some(pending), None) => self.diagnose_graph_operation(package_id, pending).await,
            (None, Some(active)) => diagnose_enablement_operation(self, package_id, active).await,
            (Some(_), Some(_)) => Err(diagnostic_state_error()),
            (None, None) => {
                let reviewed = PluginHostProtocolStore::new(self.registry.paths().state_root())
                    .get_enablement_diagnostic(&self.scope, &parsed_package_id)
                    .await
                    .map_err(|_| diagnostic_state_error())?;
                if let Some((record, cancellation)) = reviewed {
                    if let Some(diagnostic) = diagnose_reviewed_enablement_operation(
                        self,
                        &parsed_package_id,
                        record,
                        cancellation,
                    )
                    .await?
                    {
                        return Ok(diagnostic);
                    }
                }
                Err(UseError::new(
                    "use.plugin.operation_diagnostic_not_found",
                    "No diagnosable cognitive-package operation exists for this package and scope.",
                )
                .with_suggestion(
                    "Use 'a3s-use extension inspect <publisher/name> --json' for installed lifecycle history.",
                ))
            }
        }
    }

    /// Diagnose an exact retained target set before a reviewed graph exists.
    ///
    /// This projection observes cache state only. The retained package lock
    /// selects historical Registry datastores but is never exposed as apply or
    /// recovery authority.
    pub async fn diagnose_download_attempt(
        &self,
        package_id: &str,
    ) -> UseResult<PluginDownloadAttemptDiagnostic> {
        PluginPackageId::parse(package_id.to_owned()).map_err(|_| {
            diagnostic_error("The download diagnostic package identity is invalid.")
        })?;
        let _maintenance = self
            .maintenance_lock()
            .acquire_shared()
            .await
            .map_err(|_| diagnostic_state_error())?;
        let attempt = self
            .download_attempt_store()
            .get_for_package(package_id)
            .await
            .map_err(|_| diagnostic_state_error())?
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.download_attempt_diagnostic_not_found",
                    "No retained pre-plan package download exists for this package and scope.",
                )
            })?;
        if attempt.scope != self.scope || attempt.root_package_id != package_id {
            return Err(diagnostic_state_error());
        }
        let download = project_download_attempt(self, &attempt).await?;
        let diagnostic = PluginDownloadAttemptDiagnostic {
            schema: PLUGIN_DOWNLOAD_ATTEMPT_DIAGNOSTIC_SCHEMA.to_owned(),
            observed_at_ms: super::plan::now_ms().map_err(|_| diagnostic_state_error())?,
            scope: attempt.scope,
            package_id: attempt.root_package_id,
            attempt: PluginPendingDownloadAttemptDiagnostic {
                action: attempt.action,
                phase: PluginDownloadAttemptPhase::PrePlan,
                started_at_ms: attempt.started_at_ms,
                package_lock_digest: attempt.package_lock_digest,
                package_count: bounded_count(
                    attempt.package_lock.packages.len(),
                    "download package",
                )?,
                download_bytes: download.expected_bytes,
                download_retained_bytes: download.retained_bytes,
                download_target_count: bounded_count(download.targets.len(), "download target")?,
                download: download.status,
                downloads: download.targets,
                planning_bytes: download.planning_expected_bytes,
                planning_retained_bytes: download.planning_retained_bytes,
                planning_target_count: bounded_count(
                    download.planning_targets.len(),
                    "planning target",
                )?,
                planning: download.planning_status,
                planning_targets: download.planning_targets,
            },
        };
        diagnostic
            .validate()
            .map_err(|_| diagnostic_state_error())?;
        Ok(diagnostic)
    }

    /// Diagnose the exact Registry/TUF phase before an immutable package lock
    /// exists. Reading never acquires Registry locks, contacts a Registry, or
    /// changes retained planning evidence.
    pub async fn diagnose_resolution_attempt(
        &self,
        package_id: &str,
    ) -> UseResult<PluginResolutionAttemptDiagnostic> {
        PluginPackageId::parse(package_id.to_owned()).map_err(|_| {
            diagnostic_error("The resolution diagnostic package identity is invalid.")
        })?;
        let _maintenance = self
            .maintenance_lock()
            .acquire_shared()
            .await
            .map_err(|_| diagnostic_state_error())?;
        let attempt = self
            .resolution_attempt_store()
            .get_for_package(package_id)
            .await
            .map_err(|_| diagnostic_state_error())?
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.resolution_attempt_diagnostic_not_found",
                    "No retained pre-lock Registry resolution exists for this package and scope.",
                )
            })?;
        if attempt.scope != self.scope || attempt.root_package_id != package_id {
            return Err(diagnostic_state_error());
        }
        let registries = attempt
            .registries
            .into_iter()
            .map(|registry| PluginRegistryResolutionDiagnostic {
                registry_name: registry.registry_name,
                role: match registry.role {
                    PackageRegistryResolutionRole::Root => PluginRegistryResolutionRole::Root,
                    PackageRegistryResolutionRole::Dependency => {
                        PluginRegistryResolutionRole::Dependency
                    }
                },
                source_identity_digest: registry.source_identity_digest,
                trust_root_digest: registry.trust_root_digest,
                status: match registry.status {
                    PackageRegistryResolutionStatus::Pending => {
                        PluginRegistryResolutionStatus::Pending
                    }
                    PackageRegistryResolutionStatus::Verifying => {
                        PluginRegistryResolutionStatus::Verifying
                    }
                    PackageRegistryResolutionStatus::Verified => {
                        PluginRegistryResolutionStatus::Verified
                    }
                    PackageRegistryResolutionStatus::Failed => {
                        PluginRegistryResolutionStatus::Failed
                    }
                },
                root_version: registry.root_version,
                timestamp_version: registry.timestamp_version,
                snapshot_version: registry.snapshot_version,
                targets_version: registry.targets_version,
                package_targets: registry.package_targets,
                observed_at_ms: registry.observed_at_ms,
                error_code: registry.error_code,
            })
            .collect::<Vec<_>>();
        let diagnostic = PluginResolutionAttemptDiagnostic {
            schema: PLUGIN_RESOLUTION_ATTEMPT_DIAGNOSTIC_SCHEMA.to_owned(),
            observed_at_ms: super::plan::now_ms().map_err(|_| diagnostic_state_error())?,
            scope: attempt.scope,
            package_id: attempt.root_package_id,
            attempt: PluginPendingResolutionAttemptDiagnostic {
                action: attempt.action,
                phase: PluginResolutionAttemptPhase::PreLock,
                access: match attempt.access {
                    PackageResolutionAccess::Refreshed => PluginRegistryResolutionAccess::Refreshed,
                    PackageResolutionAccess::Cached => PluginRegistryResolutionAccess::Cached,
                },
                status: match attempt.status {
                    PackageResolutionAttemptStatus::Resolving => {
                        PluginResolutionDiagnosticStatus::Resolving
                    }
                    PackageResolutionAttemptStatus::Resolved => {
                        PluginResolutionDiagnosticStatus::Resolved
                    }
                    PackageResolutionAttemptStatus::Failed => {
                        PluginResolutionDiagnosticStatus::Failed
                    }
                },
                started_at_ms: attempt.started_at_ms,
                completed_at_ms: attempt.completed_at_ms,
                requested_version: attempt.requested_version,
                channel: attempt.channel,
                registry_count: bounded_count(registries.len(), "resolution Registry")?,
                verified_registry_count: bounded_count(
                    registries
                        .iter()
                        .filter(|registry| {
                            registry.status == PluginRegistryResolutionStatus::Verified
                        })
                        .count(),
                    "verified Registry",
                )?,
                package_lock_digest: attempt.package_lock_digest,
                package_count: attempt.package_count,
                error_code: attempt.error_code,
                registries,
            },
        };
        diagnostic
            .validate()
            .map_err(|_| diagnostic_state_error())?;
        Ok(diagnostic)
    }

    pub(super) async fn diagnose_graph_operation(
        &self,
        package_id: &str,
        pending: PendingPackageGraphOperation,
    ) -> UseResult<PluginOperationDiagnostic> {
        if pending.envelope.plan.scope != self.scope {
            return Err(diagnostic_state_error());
        }

        let snapshot = self
            .registry
            .published_snapshot()
            .await
            .map_err(|_| diagnostic_state_error())?;
        let expected = expected_lifecycle_units(&pending)?;
        let phase = diagnostic_phase(pending.phase());
        let observed = observe_lifecycle(
            self,
            &pending.envelope.plan.operation_id,
            &pending.envelope.plan_digest,
            phase,
            &expected,
        )
        .await?;
        let grant = observe_grant(self, &pending.envelope, &pending.authorization, phase).await?;
        let cutover_key =
            operation_cutover_key(&pending.envelope).map_err(|_| diagnostic_state_error())?;
        let operation_cutover = project_registry_cutover(
            &pending.envelope,
            phase,
            &cutover_key,
            &snapshot.pending_cutovers,
            snapshot.generation,
            &observed,
            &grant,
        )?;
        let registry = PluginRegistryOperationDiagnostic {
            generation: snapshot.generation,
            snapshot_digest: snapshot
                .descriptor_digest()
                .map_err(|_| diagnostic_state_error())?,
            pending_cutover_count: bounded_count(
                snapshot.pending_cutovers.len(),
                "Registry cutover",
            )?,
            operation_cutover,
        };
        let sources = project_sources(&pending.envelope.plan)?;
        let downloads = project_downloads(self, &pending.envelope).await?;
        let providers = project_providers(&pending.envelope.plan, &observed)?;
        let lifecycle = observed
            .iter()
            .map(|unit| unit.summary.clone())
            .collect::<Vec<_>>();
        let recovery = if registry.operation_cutover.status
            == PluginRegistryCutoverDiagnosticStatus::GenerationDrift
        {
            PluginOperationRecoveryGuidance::OperatorReviewRequired
        } else {
            match phase {
                PluginOperationDiagnosticPhase::Planned => {
                    PluginOperationRecoveryGuidance::ReviewAndApplyExactPlan
                }
                PluginOperationDiagnosticPhase::Admitted => {
                    PluginOperationRecoveryGuidance::ResumeExactPlan
                }
                PluginOperationDiagnosticPhase::Cancelled => {
                    PluginOperationRecoveryGuidance::ObserveCancellation
                }
            }
        };
        let operation = pending_operation_diagnostic(
            &pending.envelope,
            phase,
            pending.planned_at_ms,
            (pending.admitted_at_ms > 0).then_some(pending.admitted_at_ms),
            (pending.cancelled_at_ms > 0).then_some(pending.cancelled_at_ms),
            confirmation_status(&pending.envelope, &pending.authorization, phase),
            sources,
            providers,
            grant,
            lifecycle,
            expected.len(),
            downloads,
            recovery,
        )?;
        let diagnostic = PluginOperationDiagnostic {
            schema: PLUGIN_OPERATION_DIAGNOSTIC_SCHEMA.to_owned(),
            observed_at_ms: super::plan::now_ms().map_err(|_| diagnostic_state_error())?,
            scope: self.scope.clone(),
            package_id: package_id.to_owned(),
            registry,
            operation,
        };
        diagnostic.validate()?;
        Ok(diagnostic)
    }
}

#[allow(clippy::too_many_arguments)]
fn pending_operation_diagnostic(
    envelope: &PluginOperationPlanEnvelope,
    phase: PluginOperationDiagnosticPhase,
    planned_at_ms: u64,
    admitted_at_ms: Option<u64>,
    cancelled_at_ms: Option<u64>,
    confirmation: PluginOperationConfirmationDiagnosticStatus,
    sources: Vec<PluginOperationSourceDiagnostic>,
    providers: Vec<PluginProviderOperationDiagnostic>,
    grant: PluginGrantOperationDiagnostic,
    lifecycle: Vec<PluginLifecycleOperationSummary>,
    lifecycle_unit_count: usize,
    download: DownloadProjection,
    recovery: PluginOperationRecoveryGuidance,
) -> UseResult<PluginPendingOperationDiagnostic> {
    let plan = &envelope.plan;
    let changed_package_count = if matches!(
        plan.action,
        PluginOperationAction::Enable | PluginOperationAction::Disable
    ) {
        plan.packages.len()
    } else {
        plan.packages
            .iter()
            .filter(|package| package.change != PlanPackageChangeKind::Retain)
            .count()
    };
    Ok(PluginPendingOperationDiagnostic {
        operation_id: plan.operation_id.clone(),
        action: plan.action,
        phase,
        plan_digest: envelope.plan_digest.clone(),
        created_at_ms: plan.created_at_ms,
        expires_at_ms: plan.expires_at_ms,
        planned_at_ms,
        admitted_at_ms,
        cancelled_at_ms,
        package_lock_digest: plan.package_lock_digest.clone(),
        prior_package_lock_digest: plan.prior_package_lock_digest.clone(),
        authority_actor: plan.authority.actor,
        authority_decision: plan.authority.decision,
        confirmation,
        package_count: bounded_count(plan.packages.len(), "package")?,
        changed_package_count: bounded_count(changed_package_count, "changed package")?,
        source_count: bounded_count(sources.len(), "source")?,
        provider_count: bounded_count(providers.len(), "provider")?,
        lifecycle_unit_count: bounded_count(lifecycle_unit_count, "lifecycle unit")?,
        observed_lifecycle_unit_count: bounded_count(lifecycle.len(), "observed lifecycle unit")?,
        download_bytes: plan.impact.download_bytes,
        download_retained_bytes: download.retained_bytes,
        download_target_count: bounded_count(download.targets.len(), "download target")?,
        download: download.status,
        plan_drain_required: plan.impact.drain_required,
        downloads: download.targets,
        planning_bytes: download.planning_expected_bytes,
        planning_retained_bytes: download.planning_retained_bytes,
        planning_target_count: bounded_count(download.planning_targets.len(), "planning target")?,
        planning: download.planning_status,
        planning_targets: download.planning_targets,
        sources,
        providers,
        grant,
        lifecycle,
        recovery,
    })
}

pub(super) fn bounded_count(count: usize, kind: &str) -> UseResult<u32> {
    u32::try_from(count).map_err(|_| {
        diagnostic_error(format!(
            "The {kind} diagnostic count exceeds its public bound."
        ))
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

fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn diagnostic_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.operation_diagnostic_invalid", message)
}

pub(super) fn diagnostic_state_error() -> UseError {
    UseError::new(
        "use.plugin.operation_diagnostic_state_invalid",
        "The retained cognitive-package evidence is unsupported, damaged, or internally inconsistent.",
    )
    .with_suggestion(
        "Preserve the unsupported state for incident review, remove it only with an approved cleanup procedure, then reinstall the package from a trusted Registry.",
    )
}
