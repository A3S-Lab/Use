use std::collections::{BTreeMap, BTreeSet};

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
    confirmation_status, diagnostic_phase, empty_grant_diagnostic, expected_lifecycle_units,
    expected_lifecycle_units_from_envelope, observe_grant, observe_lifecycle,
    project_download_attempt, project_downloads, project_providers, project_registry_cutover,
    project_sources,
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

include!("diagnostic_queries.rs");
