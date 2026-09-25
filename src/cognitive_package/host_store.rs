use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    PlanScope, PluginHostEnablementPlanRequest, PluginHostEnablementPlanResult,
    PluginHostEnablementPlanStatus, PluginHostPackageState, PluginHostPlanRequest,
    PluginHostPlanResult, PluginManagedScope, PluginOperationPlan, PluginPackageId, UseError,
    UseResult,
};
use fs2::FileExt;
use olpc_cjson::CanonicalFormatter;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const HOST_REQUEST_RECORD_SCHEMA: &str = "a3s.use.plugin-host-request-record.v1";
const HOST_OPERATION_INDEX_SCHEMA: &str = "a3s.use.plugin-host-operation-index.v1";
const HOST_ENABLEMENT_DIAGNOSTIC_INDEX_SCHEMA: &str =
    "a3s.use.plugin-host-enablement-diagnostic-index.v1";
const HOST_CANCELLATION_RECORD_SCHEMA: &str = "a3s.use.plugin-host-cancellation-record.v1";
pub(super) const MAX_HOST_RECORD_BYTES: u64 = 4 * 1024 * 1024;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum StoredPluginHostPlan {
    Graph {
        request: Box<PluginHostPlanRequest>,
        result: Box<PluginHostPlanResult>,
    },
    Enablement {
        request: Box<PluginHostEnablementPlanRequest>,
        result: Box<PluginHostEnablementPlanResult>,
    },
}

impl StoredPluginHostPlan {
    pub fn graph(
        request: PluginHostPlanRequest,
        mut result: PluginHostPlanResult,
    ) -> UseResult<Self> {
        result.replayed = false;
        let plan = Self::Graph {
            request: Box::new(request),
            result: Box::new(result),
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn enablement(
        request: PluginHostEnablementPlanRequest,
        mut result: PluginHostEnablementPlanResult,
    ) -> UseResult<Self> {
        result.replayed = false;
        let plan = Self::Enablement {
            request: Box::new(request),
            result: Box::new(result),
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn request_id(&self) -> &str {
        match self {
            Self::Graph { request, .. } => &request.request_id,
            Self::Enablement { request, .. } => &request.request_id,
        }
    }

    pub fn scope(&self) -> &PluginManagedScope {
        match self {
            Self::Graph { request, .. } => &request.scope,
            Self::Enablement { request, .. } => &request.scope,
        }
    }

    pub fn request_digest(&self) -> UseResult<String> {
        match self {
            Self::Graph { request, .. } => request.descriptor_digest(),
            Self::Enablement { request, .. } => request.descriptor_digest(),
        }
    }

    pub fn operation_binding(&self) -> Option<(&str, &str)> {
        match self {
            Self::Graph { result, .. } => Some((
                result.plan.plan.operation_id.as_str(),
                result.plan.plan_digest.as_str(),
            )),
            Self::Enablement { result, .. }
                if result.status == PluginHostEnablementPlanStatus::Planned =>
            {
                result
                    .plan
                    .as_ref()
                    .map(|plan| (plan.plan.operation_id.as_str(), plan.plan_digest.as_str()))
            }
            Self::Enablement { .. } => None,
        }
    }

    pub fn envelope(&self) -> Option<&a3s_use_core::PluginOperationPlanEnvelope> {
        match self {
            Self::Graph { result, .. } => Some(&result.plan),
            Self::Enablement { result, .. } => result.plan.as_ref(),
        }
    }

    pub fn graph_parts(&self) -> Option<(&PluginHostPlanRequest, &PluginHostPlanResult)> {
        match self {
            Self::Graph { request, result } => Some((request.as_ref(), result.as_ref())),
            Self::Enablement { .. } => None,
        }
    }

    pub fn enablement_parts(
        &self,
    ) -> Option<(
        &PluginHostEnablementPlanRequest,
        &PluginHostEnablementPlanResult,
    )> {
        match self {
            Self::Enablement { request, result } => Some((request.as_ref(), result.as_ref())),
            Self::Graph { .. } => None,
        }
    }

    fn validate(&self) -> UseResult<()> {
        match self {
            Self::Graph { request, result } => {
                request.validate()?;
                result.validate()?;
                if result.replayed
                    || result.request_id != request.request_id
                    || result.assignment_generation != request.assignment_generation
                    || result.capabilities_digest != request.capabilities_digest
                    || result.scope != request.scope
                    || result.package_id != request.package_id
                    || result.plan.plan.action != request.action
                {
                    return Err(store_invalid(
                        "A stored graph plan does not bind its exact Host request.",
                    ));
                }
            }
            Self::Enablement { request, result } => {
                request.validate()?;
                result.validate()?;
                if result.replayed
                    || result.request_id != request.request_id
                    || result.assignment_generation != request.assignment_generation
                    || result.capabilities_digest != request.capabilities_digest
                    || result.scope != request.scope
                    || result.package_id != request.package_id
                    || result.expected_package_generation != request.expected_package_generation
                    || result.enabled != request.enabled
                {
                    return Err(store_invalid(
                        "A stored enablement plan does not bind its exact Host request.",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredPluginHostOutcome {
    pub completed_at_ms: u64,
    pub operation_result_digest: String,
    pub state: PluginHostPackageState,
}

impl StoredPluginHostOutcome {
    pub fn new(
        completed_at_ms: u64,
        operation_result_digest: impl Into<String>,
        state: PluginHostPackageState,
    ) -> UseResult<Self> {
        let outcome = Self {
            completed_at_ms,
            operation_result_digest: operation_result_digest.into(),
            state,
        };
        outcome.validate()?;
        Ok(outcome)
    }

    fn validate(&self) -> UseResult<()> {
        self.state.validate()?;
        if self.completed_at_ms == 0 || !valid_sha256(&self.operation_result_digest) {
            return Err(store_invalid("A stored Host operation outcome is invalid."));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredPluginHostRequest {
    pub schema: String,
    pub record_digest: String,
    pub request_digest: String,
    pub plan: StoredPluginHostPlan,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<StoredPluginHostOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredPluginHostCancellation {
    pub schema: String,
    pub request_id: String,
    pub operation_id: String,
    pub plan_digest: String,
    pub cancelled_at_ms: u64,
}

impl StoredPluginHostCancellation {
    pub fn new(
        request_id: impl Into<String>,
        operation_id: impl Into<String>,
        plan_digest: impl Into<String>,
        cancelled_at_ms: u64,
    ) -> UseResult<Self> {
        let record = Self {
            schema: HOST_CANCELLATION_RECORD_SCHEMA.to_owned(),
            request_id: request_id.into(),
            operation_id: operation_id.into(),
            plan_digest: plan_digest.into(),
            cancelled_at_ms,
        };
        record.validate()?;
        Ok(record)
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        PluginOperationPlan::validate_operation_id(&self.operation_id)
            .map_err(|_| store_invalid("A Host cancellation operation ID is invalid."))?;
        if self.schema != HOST_CANCELLATION_RECORD_SCHEMA
            || self.request_id.is_empty()
            || self.request_id.len() > 256
            || !valid_sha256(&self.plan_digest)
            || self.cancelled_at_ms == 0
        {
            return Err(store_invalid("A Host cancellation record is invalid."));
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredPluginHostRequestPayload<'a> {
    schema: &'a str,
    request_digest: &'a str,
    plan: &'a StoredPluginHostPlan,
    outcome: &'a Option<StoredPluginHostOutcome>,
}

impl StoredPluginHostRequest {
    pub fn new(plan: StoredPluginHostPlan) -> UseResult<Self> {
        let request_digest = plan.request_digest()?;
        let mut record = Self {
            schema: HOST_REQUEST_RECORD_SCHEMA.to_string(),
            record_digest: String::new(),
            request_digest,
            plan,
            outcome: None,
        };
        record.record_digest = record.expected_digest()?;
        record.validate()?;
        Ok(record)
    }

    pub fn with_outcome(&self, outcome: StoredPluginHostOutcome) -> UseResult<Self> {
        self.validate()?;
        outcome.validate()?;
        if self.plan.operation_binding().is_none() {
            return Err(store_invalid(
                "A no-change Host request cannot retain an operation outcome.",
            ));
        }
        let mut completed = self.clone();
        completed.outcome = Some(outcome);
        completed.record_digest = completed.expected_digest()?;
        completed.validate()?;
        Ok(completed)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.plan.validate()?;
        if let Some(outcome) = &self.outcome {
            outcome.validate()?;
        }
        if self.schema != HOST_REQUEST_RECORD_SCHEMA
            || self.request_digest != self.plan.request_digest()?
            || self.record_digest != self.expected_digest()?
            || self.outcome.is_some() && self.plan.operation_binding().is_none()
        {
            return Err(store_invalid(
                "A durable Plugin Host request record is invalid.",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> UseResult<String> {
        digest_value(&StoredPluginHostRequestPayload {
            schema: &self.schema,
            request_digest: &self.request_digest,
            plan: &self.plan,
            outcome: &self.outcome,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredPluginHostOperationIndex {
    pub(super) schema: String,
    pub(super) record_digest: String,
    pub(super) request_id: String,
    pub(super) request_digest: String,
    pub(super) operation_id: String,
    pub(super) plan_digest: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredPluginHostOperationIndexPayload<'a> {
    schema: &'a str,
    request_id: &'a str,
    request_digest: &'a str,
    operation_id: &'a str,
    plan_digest: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredPluginHostEnablementDiagnosticIndex {
    pub(super) schema: String,
    pub(super) record_digest: String,
    pub(super) scope: PlanScope,
    pub(super) managed_scope: PluginManagedScope,
    pub(super) package_id: String,
    pub(super) request_id: String,
    pub(super) request_digest: String,
    pub(super) operation_id: String,
    pub(super) plan_digest: String,
    pub(super) planned_at_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredPluginHostEnablementDiagnosticIndexPayload<'a> {
    schema: &'a str,
    scope: &'a PlanScope,
    managed_scope: &'a PluginManagedScope,
    package_id: &'a str,
    request_id: &'a str,
    request_digest: &'a str,
    operation_id: &'a str,
    plan_digest: &'a str,
    planned_at_ms: u64,
}

impl StoredPluginHostOperationIndex {
    pub(super) fn from_request(record: &StoredPluginHostRequest) -> UseResult<Option<Self>> {
        record.validate()?;
        let Some((operation_id, plan_digest)) = record.plan.operation_binding() else {
            return Ok(None);
        };
        let mut index = Self {
            schema: HOST_OPERATION_INDEX_SCHEMA.to_string(),
            record_digest: String::new(),
            request_id: record.plan.request_id().to_string(),
            request_digest: record.request_digest.clone(),
            operation_id: operation_id.to_string(),
            plan_digest: plan_digest.to_string(),
        };
        index.record_digest = index.expected_digest()?;
        index.validate()?;
        Ok(Some(index))
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        PluginOperationPlan::validate_operation_id(&self.operation_id)
            .map_err(|_| store_invalid("A Host operation index identity is invalid."))?;
        if self.schema != HOST_OPERATION_INDEX_SCHEMA
            || !valid_sha256(&self.request_digest)
            || !valid_sha256(&self.plan_digest)
            || self.record_digest != self.expected_digest()?
        {
            return Err(store_invalid("A Host operation index is invalid."));
        }
        Ok(())
    }

    fn expected_digest(&self) -> UseResult<String> {
        digest_value(&StoredPluginHostOperationIndexPayload {
            schema: &self.schema,
            request_id: &self.request_id,
            request_digest: &self.request_digest,
            operation_id: &self.operation_id,
            plan_digest: &self.plan_digest,
        })
    }

    pub(super) fn matches(&self, record: &StoredPluginHostRequest) -> bool {
        record.plan.operation_binding().is_some_and(|binding| {
            self.request_id == record.plan.request_id()
                && self.request_digest == record.request_digest
                && self.operation_id == binding.0
                && self.plan_digest == binding.1
        })
    }
}

impl StoredPluginHostEnablementDiagnosticIndex {
    pub(super) fn from_request(record: &StoredPluginHostRequest) -> UseResult<Option<Self>> {
        record.validate()?;
        let Some((request, result)) = record.plan.enablement_parts() else {
            return Ok(None);
        };
        if result.status != PluginHostEnablementPlanStatus::Planned {
            return Ok(None);
        }
        let envelope = result.plan.as_ref().ok_or_else(|| {
            store_invalid("A reviewed Host enablement plan omitted its exact envelope.")
        })?;
        let mut index = Self {
            schema: HOST_ENABLEMENT_DIAGNOSTIC_INDEX_SCHEMA.to_owned(),
            record_digest: String::new(),
            scope: request.scope.plan_scope(),
            managed_scope: request.scope.clone(),
            package_id: request.package_id.to_string(),
            request_id: request.request_id.clone(),
            request_digest: record.request_digest.clone(),
            operation_id: envelope.plan.operation_id.clone(),
            plan_digest: envelope.plan_digest.clone(),
            planned_at_ms: result.planned_at_ms,
        };
        index.record_digest = index.expected_digest()?;
        index.validate()?;
        Ok(Some(index))
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        self.managed_scope.validate()?;
        PluginPackageId::parse(self.package_id.clone()).map_err(|_| {
            store_invalid("A Host enablement diagnostic package identity is invalid.")
        })?;
        PluginOperationPlan::validate_operation_id(&self.operation_id).map_err(|_| {
            store_invalid("A Host enablement diagnostic operation identity is invalid.")
        })?;
        if self.schema != HOST_ENABLEMENT_DIAGNOSTIC_INDEX_SCHEMA
            || self.scope != self.managed_scope.plan_scope()
            || self.request_id.is_empty()
            || self.request_id.len() > 256
            || !valid_sha256(&self.request_digest)
            || !valid_sha256(&self.plan_digest)
            || self.planned_at_ms == 0
            || self.record_digest != self.expected_digest()?
        {
            return Err(store_invalid(
                "A Host enablement diagnostic index is invalid.",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> UseResult<String> {
        digest_value(&StoredPluginHostEnablementDiagnosticIndexPayload {
            schema: &self.schema,
            scope: &self.scope,
            managed_scope: &self.managed_scope,
            package_id: &self.package_id,
            request_id: &self.request_id,
            request_digest: &self.request_digest,
            operation_id: &self.operation_id,
            plan_digest: &self.plan_digest,
            planned_at_ms: self.planned_at_ms,
        })
    }

    pub(super) fn matches(&self, record: &StoredPluginHostRequest) -> bool {
        let Some((request, result)) = record.plan.enablement_parts() else {
            return false;
        };
        let Some(envelope) = result.plan.as_ref() else {
            return false;
        };
        result.status == PluginHostEnablementPlanStatus::Planned
            && self.scope == request.scope.plan_scope()
            && self.managed_scope == request.scope
            && self.package_id == request.package_id.as_str()
            && self.request_id == request.request_id
            && self.request_digest == record.request_digest
            && self.operation_id == envelope.plan.operation_id
            && self.plan_digest == envelope.plan_digest
            && self.planned_at_ms == result.planned_at_ms
    }

    fn supersedes(&self, other: &Self) -> bool {
        (self.planned_at_ms, self.request_id.as_str())
            > (other.planned_at_ms, other.request_id.as_str())
    }
}

#[derive(Debug, Clone)]
pub(super) struct PluginHostProtocolStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl PluginHostProtocolStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("plugin-host-manager"),
            state_root,
        }
    }

    pub async fn lock_request(
        &self,
        scope: &PluginManagedScope,
        request_id: &str,
    ) -> UseResult<StdFile> {
        scope.validate()?;
        let directory = self.scope_root(scope)?.join("request-locks");
        ensure_owned_directory(&self.state_root, &directory).await?;
        acquire_lock(directory.join(format!("{}.lock", sha256_hex(request_id.as_bytes())))).await
    }

    pub async fn lock_operation(
        &self,
        scope: &PluginManagedScope,
        operation_id: &str,
    ) -> UseResult<StdFile> {
        scope.validate()?;
        PluginOperationPlan::validate_operation_id(operation_id)
            .map_err(|_| store_invalid("A Host operation lock identity is invalid."))?;
        let directory = self.scope_root(scope)?.join("operation-locks");
        ensure_owned_directory(&self.state_root, &directory).await?;
        acquire_lock(directory.join(format!("{}.lock", sha256_hex(operation_id.as_bytes())))).await
    }

    pub async fn get_by_request(
        &self,
        scope: &PluginManagedScope,
        request_id: &str,
    ) -> UseResult<Option<StoredPluginHostRequest>> {
        let path = self.request_path(scope, request_id)?;
        let Some(record) = read_optional(&self.state_root, &path).await? else {
            return Ok(None);
        };
        self.validate_request_path(scope, request_id, &record)?;
        Ok(Some(record))
    }

    pub async fn get_by_operation(
        &self,
        scope: &PluginManagedScope,
        operation_id: &str,
        plan_digest: &str,
    ) -> UseResult<Option<StoredPluginHostRequest>> {
        let path = self.operation_path(scope, operation_id, plan_digest)?;
        let Some(index) = read_optional::<StoredPluginHostOperationIndex>(&self.state_root, &path)
            .await?
        else {
            return Ok(None);
        };
        index.validate()?;
        if index.operation_id != operation_id || index.plan_digest != plan_digest {
            return Err(store_invalid(
                "A Host operation index does not match its exact operation binding path.",
            ));
        }
        let record = self
            .get_by_request(scope, &index.request_id)
            .await?
            .ok_or_else(|| {
                store_invalid("A Host operation index refers to a missing request record.")
            })?;
        if !index.matches(&record) {
            return Err(store_invalid(
                "A Host operation index disagrees with its immutable request record.",
            ));
        }
        Ok(Some(record))
    }

    /// Return the newest exact Host-reviewed enablement plan for one public
    /// package scope. This index is observation-only and is never accepted by
    /// apply or recovery.
    pub async fn get_enablement_diagnostic(
        &self,
        scope: &PlanScope,
        package_id: &PluginPackageId,
    ) -> UseResult<
        Option<(
            StoredPluginHostRequest,
            Option<StoredPluginHostCancellation>,
        )>,
    > {
        let path = self.enablement_diagnostic_path(scope, package_id.as_str())?;
        let index: Option<StoredPluginHostEnablementDiagnosticIndex> =
            read_optional(&self.state_root, &path).await?;
        let Some(index) = index else {
            return Ok(None);
        };
        index.validate()?;
        if index.scope != *scope || index.package_id != package_id.as_str() {
            return Err(store_invalid(
                "A Host enablement diagnostic index does not match its owned path.",
            ));
        }
        let record = self
            .get_by_request(&index.managed_scope, &index.request_id)
            .await?
            .ok_or_else(|| {
                store_invalid("A Host enablement diagnostic index refers to a missing request.")
            })?;
        if !index.matches(&record) {
            return Err(store_invalid(
                "A Host enablement diagnostic index disagrees with its reviewed request.",
            ));
        }
        if record.outcome.is_some() {
            return Ok(None);
        }
        let cancellation = self
            .get_cancellation(
                &index.managed_scope,
                &index.operation_id,
                &index.plan_digest,
            )
            .await?;
        Ok(Some((record, cancellation)))
    }

    pub async fn put_plan(&self, record: &StoredPluginHostRequest) -> UseResult<bool> {
        record.validate()?;
        let scope = record.plan.scope();
        let _lock = self.lock_store(scope).await?;
        let request_path = self.request_path(scope, record.plan.request_id())?;
        if let Some(current) = read_optional(&self.state_root, &request_path).await? {
            self.validate_request_path(scope, record.plan.request_id(), &current)?;
            if current != *record {
                return Err(store_conflict(
                    "The Host request ID already owns a different immutable plan.",
                ));
            }
            self.ensure_operation_index(&current).await?;
            self.ensure_enablement_diagnostic_index(&current).await?;
            return Ok(false);
        }
        write_new(&self.state_root, &request_path, record).await?;
        self.ensure_operation_index(record).await?;
        self.ensure_enablement_diagnostic_index(record).await?;
        Ok(true)
    }

    pub async fn put_outcome(
        &self,
        expected: &StoredPluginHostRequest,
        outcome: StoredPluginHostOutcome,
    ) -> UseResult<(StoredPluginHostRequest, bool)> {
        expected.validate()?;
        outcome.validate()?;
        let scope = expected.plan.scope();
        let _lock = self.lock_store(scope).await?;
        let path = self.request_path(scope, expected.plan.request_id())?;
        let current = read_optional(&self.state_root, &path)
            .await?
            .ok_or_else(|| store_invalid("The applied Host plan record disappeared."))?;
        self.validate_request_path(scope, expected.plan.request_id(), &current)?;
        if let Some(current_outcome) = &current.outcome {
            if current_outcome == &outcome {
                return Ok((current, false));
            }
            return Err(store_conflict(
                "The Host operation already owns a different durable outcome.",
            ));
        }
        if &current != expected {
            return Err(store_conflict(
                "The Host plan record changed before its operation outcome was stored.",
            ));
        }
        let completed = current.with_outcome(outcome)?;
        write_replace(&self.state_root, &path, &completed).await?;
        Ok((completed, true))
    }

    pub async fn get_cancellation(
        &self,
        scope: &PluginManagedScope,
        operation_id: &str,
        plan_digest: &str,
    ) -> UseResult<Option<StoredPluginHostCancellation>> {
        let path = self.cancellation_path(scope, operation_id, plan_digest)?;
        let value: Option<StoredPluginHostCancellation> =
            read_optional(&self.state_root, &path).await?;
        if let Some(value) = &value {
            value.validate()?;
            if value.operation_id != operation_id || value.plan_digest != plan_digest {
                return Err(store_invalid(
                    "A Host cancellation record does not match its exact operation binding path.",
                ));
            }
        }
        Ok(value)
    }

    pub async fn put_cancellation(
        &self,
        scope: &PluginManagedScope,
        cancellation: &StoredPluginHostCancellation,
    ) -> UseResult<bool> {
        cancellation.validate()?;
        let _lock = self.lock_store(scope).await?;
        let path =
            self.cancellation_path(scope, &cancellation.operation_id, &cancellation.plan_digest)?;
        let current: Option<StoredPluginHostCancellation> =
            read_optional(&self.state_root, &path).await?;
        let inserted = if let Some(current) = current {
            if current == *cancellation
                || current.operation_id == cancellation.operation_id
                    && current.plan_digest == cancellation.plan_digest
            {
                false
            } else {
                return Err(store_conflict(
                    "The Host operation already owns a different cancellation record.",
                ));
            }
        } else {
            write_new(&self.state_root, &path, cancellation).await?;
            true
        };
        Ok(inserted)
    }

    async fn ensure_operation_index(&self, record: &StoredPluginHostRequest) -> UseResult<()> {
        let Some(index) = StoredPluginHostOperationIndex::from_request(record)? else {
            return Ok(());
        };
        let path =
            self.operation_path(record.plan.scope(), &index.operation_id, &index.plan_digest)?;
        let current: Option<StoredPluginHostOperationIndex> =
            read_optional(&self.state_root, &path).await?;
        if let Some(current) = current {
            current.validate()?;
            if current != index {
                return Err(store_conflict(
                    "The Host operation binding already owns a different immutable plan.",
                ));
            }
            Ok(())
        } else {
            write_new(&self.state_root, &path, &index).await
        }
    }

    async fn ensure_enablement_diagnostic_index(
        &self,
        record: &StoredPluginHostRequest,
    ) -> UseResult<()> {
        let Some(index) = StoredPluginHostEnablementDiagnosticIndex::from_request(record)? else {
            return Ok(());
        };
        let path = self.enablement_diagnostic_path(&index.scope, &index.package_id)?;
        let directory = path.parent().ok_or_else(|| {
            store_invalid("A Host enablement diagnostic index path is incomplete.")
        })?;
        ensure_owned_directory(&self.state_root, directory).await?;
        let _lock = acquire_lock(directory.join(".store.lock")).await?;
        let current: Option<StoredPluginHostEnablementDiagnosticIndex> =
            read_optional(&self.state_root, &path).await?;
        let Some(current) = current else {
            return write_new(&self.state_root, &path, &index).await;
        };
        current.validate()?;
        if current.scope != index.scope || current.package_id != index.package_id {
            return Err(store_invalid(
                "A Host enablement diagnostic index moved across its owned package path.",
            ));
        }
        let current_record = self
            .get_by_request(&current.managed_scope, &current.request_id)
            .await?
            .ok_or_else(|| {
                store_invalid("A Host enablement diagnostic index lost its reviewed request.")
            })?;
        if !current.matches(&current_record) {
            return Err(store_invalid(
                "A Host enablement diagnostic index drifted from its reviewed request.",
            ));
        }
        if current == index || !index.supersedes(&current) {
            return Ok(());
        }
        write_replace(&self.state_root, &path, &index).await
    }

    async fn lock_store(&self, scope: &PluginManagedScope) -> UseResult<StdFile> {
        let directory = self.scope_root(scope)?;
        ensure_owned_directory(&self.state_root, &directory).await?;
        acquire_lock(directory.join(".store.lock")).await
    }

    fn validate_request_path(
        &self,
        scope: &PluginManagedScope,
        request_id: &str,
        record: &StoredPluginHostRequest,
    ) -> UseResult<()> {
        record.validate()?;
        if record.plan.scope() != scope || record.plan.request_id() != request_id {
            return Err(store_invalid(
                "A Host request record does not match its owned scope and request path.",
            ));
        }
        Ok(())
    }

    fn scope_root(&self, scope: &PluginManagedScope) -> UseResult<PathBuf> {
        scope.validate()?;
        let digest = scope.descriptor_digest()?;
        let digest = digest.strip_prefix("sha256:").ok_or_else(|| {
            store_invalid("The managed Host scope digest has an invalid encoding.")
        })?;
        Ok(self.root.join(digest))
    }

    fn request_path(&self, scope: &PluginManagedScope, request_id: &str) -> UseResult<PathBuf> {
        Ok(self
            .scope_root(scope)?
            .join("requests")
            .join(format!("{}.json", sha256_hex(request_id.as_bytes()))))
    }

    fn operation_path(
        &self,
        scope: &PluginManagedScope,
        operation_id: &str,
        plan_digest: &str,
    ) -> UseResult<PathBuf> {
        PluginOperationPlan::validate_operation_id(operation_id)
            .map_err(|_| store_invalid("A Host operation path identity is invalid."))?;
        if !valid_sha256(plan_digest) {
            return Err(store_invalid("A Host operation path digest is invalid."));
        }
        Ok(self.scope_root(scope)?.join("operations").join(format!(
            "{}.json",
            operation_binding_digest(operation_id, plan_digest)
        )))
    }

    fn cancellation_path(
        &self,
        scope: &PluginManagedScope,
        operation_id: &str,
        plan_digest: &str,
    ) -> UseResult<PathBuf> {
        PluginOperationPlan::validate_operation_id(operation_id)
            .map_err(|_| store_invalid("A Host cancellation path identity is invalid."))?;
        if !valid_sha256(plan_digest) {
            return Err(store_invalid("A Host cancellation path digest is invalid."));
        }
        Ok(self.scope_root(scope)?.join("cancellations").join(format!(
            "{}.json",
            operation_binding_digest(operation_id, plan_digest)
        )))
    }

    fn enablement_diagnostic_path(
        &self,
        scope: &PlanScope,
        package_id: &str,
    ) -> UseResult<PathBuf> {
        let scope_digest = scope.storage_key().map_err(|_| {
            store_invalid("A Host enablement diagnostic installation identity is invalid.")
        })?;
        Ok(self
            .root
            .join("diagnostics/enablement")
            .join(scope.kind.as_str())
            .join(scope_digest)
            .join(format!("{}.json", sha256_hex(package_id.as_bytes()))))
    }
}

include!("host_store_io.rs");
