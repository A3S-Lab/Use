use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    PluginHostEnablementPlanRequest, PluginHostEnablementPlanResult,
    PluginHostEnablementPlanStatus, PluginHostPackageState, PluginHostPlanRequest,
    PluginHostPlanResult, PluginManagedScope, PluginOperationPlan, UseError, UseResult,
};
use fs2::FileExt;
use olpc_cjson::CanonicalFormatter;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const HOST_REQUEST_RECORD_SCHEMA: &str = "a3s.use.plugin-host-request-record.v1";
const HOST_OPERATION_INDEX_SCHEMA: &str = "a3s.use.plugin-host-operation-index.v1";
const MAX_HOST_RECORD_BYTES: u64 = 4 * 1024 * 1024;
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
struct StoredPluginHostOperationIndex {
    schema: String,
    record_digest: String,
    request_id: String,
    request_digest: String,
    operation_id: String,
    plan_digest: String,
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

impl StoredPluginHostOperationIndex {
    fn from_request(record: &StoredPluginHostRequest) -> UseResult<Option<Self>> {
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

    fn validate(&self) -> UseResult<()> {
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

    fn matches(&self, record: &StoredPluginHostRequest) -> bool {
        record.plan.operation_binding().is_some_and(|binding| {
            self.request_id == record.plan.request_id()
                && self.request_digest == record.request_digest
                && self.operation_id == binding.0
                && self.plan_digest == binding.1
        })
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
    ) -> UseResult<Option<StoredPluginHostRequest>> {
        let path = self.operation_path(scope, operation_id)?;
        let index: Option<StoredPluginHostOperationIndex> =
            read_optional(&self.state_root, &path).await?;
        let Some(index) = index else {
            return Ok(None);
        };
        index.validate()?;
        if index.operation_id != operation_id {
            return Err(store_invalid(
                "A Host operation index does not match its owned path.",
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
            return Ok(false);
        }
        write_new(&self.state_root, &request_path, record).await?;
        self.ensure_operation_index(record).await?;
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

    async fn ensure_operation_index(&self, record: &StoredPluginHostRequest) -> UseResult<()> {
        let Some(index) = StoredPluginHostOperationIndex::from_request(record)? else {
            return Ok(());
        };
        let path = self.operation_path(record.plan.scope(), &index.operation_id)?;
        let current: Option<StoredPluginHostOperationIndex> =
            read_optional(&self.state_root, &path).await?;
        if let Some(current) = current {
            current.validate()?;
            if current == index {
                return Ok(());
            }
            return Err(store_conflict(
                "The Host operation ID already owns a different immutable plan.",
            ));
        }
        write_new(&self.state_root, &path, &index).await
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

    fn operation_path(&self, scope: &PluginManagedScope, operation_id: &str) -> UseResult<PathBuf> {
        PluginOperationPlan::validate_operation_id(operation_id)
            .map_err(|_| store_invalid("A Host operation path identity is invalid."))?;
        Ok(self
            .scope_root(scope)?
            .join("operations")
            .join(format!("{}.json", sha256_hex(operation_id.as_bytes()))))
    }
}

async fn acquire_lock(lock_path: PathBuf) -> UseResult<StdFile> {
    match fs::symlink_metadata(&lock_path).await {
        Ok(metadata)
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                || !metadata.is_file() =>
        {
            return Err(store_invalid("A Plugin Host lock path is invalid."))
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect Plugin Host lock", &lock_path, error)),
    }
    let error_path = lock_path.clone();
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        Ok::<_, io::Error>(file)
    })
    .await
    .map_err(|error| {
        store_io(format!(
            "Failed to join the Plugin Host lock task '{}': {error}",
            error_path.display()
        ))
    })?
    .map_err(|error| path_error("acquire Plugin Host lock", &error_path, error))
}

async fn read_optional<T: DeserializeOwned>(
    state_root: &Path,
    path: &Path,
) -> UseResult<Option<T>> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(store_invalid(
            "A Plugin Host record path escapes its state root.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| store_invalid("A Plugin Host record path is incomplete."))?;
    if !validate_existing_directory_chain(state_root, parent).await? {
        return Ok(None);
    }
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect Plugin Host record", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_HOST_RECORD_BYTES
    {
        return Err(store_invalid(
            "A Plugin Host record is not a bounded regular file.",
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read Plugin Host record", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_HOST_RECORD_BYTES {
        return Err(store_invalid(
            "A Plugin Host record changed outside its size bound.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| store_invalid(format!("A Plugin Host record is invalid JSON: {error}")))
}

async fn write_new<T: Serialize>(state_root: &Path, path: &Path, value: &T) -> UseResult<()> {
    write_record(state_root, path, value, false).await
}

async fn write_replace<T: Serialize>(state_root: &Path, path: &Path, value: &T) -> UseResult<()> {
    write_record(state_root, path, value, true).await
}

async fn write_record<T: Serialize>(
    state_root: &Path,
    path: &Path,
    value: &T,
    replace: bool,
) -> UseResult<()> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(store_invalid(
            "A Plugin Host record path escapes its state root.",
        ));
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        store_invalid(format!("Failed to encode a Plugin Host record: {error}"))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_HOST_RECORD_BYTES {
        return Err(store_invalid(
            "A Plugin Host record exceeds its storage bound.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| store_invalid("A Plugin Host record path is incomplete."))?;
    ensure_owned_directory(state_root, parent).await?;
    let temporary = parent.join(format!(".plugin-host-{}.tmp", unique_suffix()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary Plugin Host record", &temporary, error))?;
    if let Err(error) = async {
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "commit temporary Plugin Host record",
            path,
            error,
        ));
    }
    drop(file);
    let error_target = path.to_path_buf();
    let activation_target = error_target.clone();
    let activation = tokio::task::spawn_blocking(move || {
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        if replace {
            temporary
                .persist(activation_target)
                .map_err(|error| error.error)
        } else {
            temporary
                .persist_noclobber(activation_target)
                .map_err(|error| error.error)
        }
    })
    .await
    .map_err(|error| {
        store_io(format!(
            "Failed to join Plugin Host activation for '{}': {error}",
            error_target.display()
        ))
    })?;
    activation.map_err(|error| path_error("activate Plugin Host record", &error_target, error))?;
    sync_parent(parent).await
}

async fn ensure_owned_directory(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(store_invalid(
            "A Plugin Host directory escapes its state root.",
        ));
    }
    fs::create_dir_all(root)
        .await
        .map_err(|error| path_error("create Plugin Host state root", root, error))?;
    validate_directory(root).await?;
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| store_invalid("A Plugin Host directory has invalid ownership."))?;
    let mut current = root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create Plugin Host state directory",
                    &current,
                    error,
                ))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory_chain(root: &Path, directory: &Path) -> UseResult<bool> {
    if !directory.starts_with(root) {
        return Err(store_invalid(
            "A Plugin Host directory escapes its state root.",
        ));
    }
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| store_invalid("A Plugin Host directory has invalid ownership."))?;
    let mut current = root.to_path_buf();
    match fs::symlink_metadata(&current).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() => {
        }
        Ok(_) => return Err(store_invalid("The Plugin Host state root is invalid.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(path_error(
                "inspect Plugin Host state root",
                &current,
                error,
            ))
        }
    }
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::symlink_metadata(&current).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => return Err(store_invalid("A Plugin Host state directory is invalid.")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    "inspect Plugin Host state directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(true)
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect Plugin Host directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(store_invalid("A Plugin Host directory is invalid."));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_parent(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| path_error("open Plugin Host directory", path, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync Plugin Host directory", path, error))
}

#[cfg(not(unix))]
async fn sync_parent(_path: &Path) -> UseResult<()> {
    Ok(())
}

pub(super) fn digest_value<T: Serialize>(value: &T) -> UseResult<String> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value
        .serialize(&mut serializer)
        .map_err(|error| store_invalid(format!("Failed to canonicalize Host state: {error}")))?;
    Ok(format!("sha256:{}", sha256_hex(&bytes)))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

fn store_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.host_store_invalid", message)
}

fn store_conflict(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.host_store_conflict", message)
}

fn store_io(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.host_store_io", message)
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    store_io(format!("Failed to {action} '{}': {error}", path.display()))
}
