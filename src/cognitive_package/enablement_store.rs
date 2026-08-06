use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    PlanPackageChangeKind, PlanScope, PlanScopeKind, PluginOperationAction,
    PluginOperationPlanEnvelope, PluginPackageId, UseError, UseResult,
};
use fs2::FileExt;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::plugin_lifecycle::{PluginLifecycleAction, PluginLifecycleIntent};

use super::grant::PackageGraphAuthorization;
use super::{CognitivePackageEnablementRequest, CognitivePackageEnablementResult};

const ENABLEMENT_STATE_SCHEMA: &str = "a3s.use.cognitive-package-enablement-state.v1";
pub(super) const ENABLEMENT_STATE_SCHEMA_V2: &str = "a3s.use.cognitive-package-enablement-state.v2";
const ENABLEMENT_OPERATION_SCHEMA: &str = "a3s.use.cognitive-package-enablement-operation.v1";
const ENABLEMENT_OPERATION_SCHEMA_V2: &str = "a3s.use.cognitive-package-enablement-operation.v2";
const MAX_ENABLEMENT_RECORD_BYTES: u64 = 2 * 1024 * 1024;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CognitivePackageArtifactState {
    pub version: String,
    pub generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
}

impl CognitivePackageArtifactState {
    pub fn validate(&self) -> UseResult<()> {
        if self.version.is_empty()
            || self.version.len() > 256
            || self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.manifest_digest)
        {
            return Err(store_invalid(
                "A cognitive-package enablement artifact identity is invalid.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PendingCognitivePackageEnablement {
    pub request_digest: String,
    pub request: CognitivePackageEnablementRequest,
    pub intent: PluginLifecycleIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<PluginOperationPlanEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<PackageGraphAuthorization>,
    pub state_generation_after: u64,
    pub started_at_ms: u64,
}

impl PendingCognitivePackageEnablement {
    fn validate_against(&self, state: &StoredCognitivePackageEnablement) -> UseResult<()> {
        self.request.validate()?;
        self.intent.validate()?;
        let artifact = state.artifact.as_ref().ok_or_else(|| {
            store_invalid("An absent cognitive package cannot retain an enablement operation.")
        })?;
        let expected_action = if self.request.enabled {
            PluginLifecycleAction::Enable
        } else {
            PluginLifecycleAction::Disable
        };
        if !valid_sha256(&self.request_digest)
            || self.request.descriptor_digest()? != self.request_digest
            || self.request.package_id.as_str() != state.package_id
            || self.request.expected_package_generation != state.state_generation
            || self.request.enabled == state.enabled
            || self.state_generation_after <= state.state_generation
            || self.started_at_ms == 0
            || self.intent.operation_id != self.request.operation_id
            || self.intent.scope_id != state.scope.id
            || self.intent.package_id != state.package_id
            || self.intent.package_digest != artifact.package_digest
            || self.intent.manifest_digest != artifact.manifest_digest
            || self.intent.generation != artifact.generation
            || self.intent.action != expected_action
        {
            return Err(store_invalid(
                "A pending cognitive-package enablement operation is invalid.",
            ));
        }
        match (&self.envelope, &self.authorization) {
            (None, None) if self.intent.plan_digest == self.request_digest => Ok(()),
            (Some(envelope), Some(authorization)) => {
                envelope.validate()?;
                authorization.validate_against(envelope, self.started_at_ms)?;
                let expected_plan_action = if self.request.enabled {
                    PluginOperationAction::Enable
                } else {
                    PluginOperationAction::Disable
                };
                let transition = envelope.plan.packages.as_slice();
                let receipt = envelope.plan.state.receipt_digest.as_deref();
                let planned_state = transition
                    .first()
                    .and_then(|package| package.after.as_ref());
                if envelope.plan.operation_id != self.request.operation_id
                    || envelope.plan.action != expected_plan_action
                    || envelope.plan.package_id != state.package_id
                    || envelope.plan.scope != state.scope
                    || envelope.plan.state.state_revision == 0
                    || transition.len() != 1
                    || transition[0].package_id != state.package_id
                    || transition[0].change != PlanPackageChangeKind::Retain
                    || transition[0].before != transition[0].after
                    || planned_state.is_none_or(|planned| {
                        planned.release.version != artifact.version
                            || planned.release.package_sha256 != artifact.package_digest
                            || planned.release.manifest_sha256 != artifact.manifest_digest
                    })
                    || receipt.is_none()
                    || self.intent.plan_digest != envelope.plan_digest
                {
                    return Err(store_invalid(
                        "A pending enablement plan drifted from its exact installed artifact or request.",
                    ));
                }
                Ok(())
            }
            _ => Err(store_invalid(
                "A pending enablement operation contains incomplete plan authorization evidence.",
            )),
        }
    }

    pub fn requires_authority_revalidation(&self) -> bool {
        self.envelope.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredCognitivePackageEnablement {
    pub schema: String,
    pub scope: PlanScope,
    pub package_id: String,
    pub state_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<CognitivePackageArtifactState>,
    pub enabled: bool,
    pub updated_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<PendingCognitivePackageEnablement>,
}

impl StoredCognitivePackageEnablement {
    pub fn new(
        scope: PlanScope,
        package_id: impl Into<String>,
        state_generation: u64,
        artifact: Option<CognitivePackageArtifactState>,
        enabled: bool,
        updated_at_ms: u64,
    ) -> UseResult<Self> {
        let state = Self {
            schema: ENABLEMENT_STATE_SCHEMA_V2.to_string(),
            scope,
            package_id: package_id.into(),
            state_generation,
            artifact,
            enabled,
            updated_at_ms,
            active: None,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn validate(&self) -> UseResult<()> {
        validate_scope(&self.scope)?;
        PluginPackageId::parse(self.package_id.clone()).map_err(|_| {
            store_invalid("A cognitive-package enablement package identity is invalid.")
        })?;
        if !matches!(
            self.schema.as_str(),
            ENABLEMENT_STATE_SCHEMA | ENABLEMENT_STATE_SCHEMA_V2
        ) || self.state_generation == 0
            || self.updated_at_ms == 0
            || (self.artifact.is_none() && self.enabled)
        {
            return Err(store_invalid(
                "A cognitive-package enablement state record is invalid.",
            ));
        }
        if let Some(artifact) = &self.artifact {
            artifact.validate()?;
        }
        if let Some(active) = &self.active {
            active.validate_against(self)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredCognitivePackageEnablementOperation {
    pub schema: String,
    pub scope: PlanScope,
    pub request_digest: String,
    pub request: CognitivePackageEnablementRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<PluginOperationPlanEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization: Option<PackageGraphAuthorization>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admitted_at_ms: Option<u64>,
    pub result: CognitivePackageEnablementResult,
    pub state_after: StoredCognitivePackageEnablement,
}

impl StoredCognitivePackageEnablementOperation {
    pub fn new(
        scope: PlanScope,
        request: CognitivePackageEnablementRequest,
        envelope: Option<PluginOperationPlanEnvelope>,
        authorization: Option<PackageGraphAuthorization>,
        admitted_at_ms: Option<u64>,
        result: CognitivePackageEnablementResult,
        state_after: StoredCognitivePackageEnablement,
    ) -> UseResult<Self> {
        let operation = Self {
            schema: if envelope.is_some() {
                ENABLEMENT_OPERATION_SCHEMA_V2.to_string()
            } else {
                ENABLEMENT_OPERATION_SCHEMA.to_string()
            },
            request_digest: request.descriptor_digest()?,
            scope,
            request,
            envelope,
            authorization,
            admitted_at_ms,
            result,
            state_after,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn validate(&self) -> UseResult<()> {
        validate_scope(&self.scope)?;
        self.request.validate()?;
        self.result.validate_for(&self.request)?;
        self.state_after.validate()?;
        match (&self.envelope, &self.authorization, self.admitted_at_ms) {
            (None, None, None) if self.schema == ENABLEMENT_OPERATION_SCHEMA => {}
            (Some(envelope), Some(authorization), Some(admitted_at_ms))
                if self.schema == ENABLEMENT_OPERATION_SCHEMA_V2 && admitted_at_ms > 0 =>
            {
                envelope.validate()?;
                authorization.validate_against(envelope, admitted_at_ms)?;
                let expected_action = if self.request.enabled {
                    PluginOperationAction::Enable
                } else {
                    PluginOperationAction::Disable
                };
                if envelope.plan.operation_id != self.request.operation_id
                    || envelope.plan.package_id != self.request.package_id.as_str()
                    || envelope.plan.scope != self.scope
                    || envelope.plan.action != expected_action
                {
                    return Err(store_invalid(
                        "A completed enablement plan does not bind its exact request and scope.",
                    ));
                }
            }
            _ => {
                return Err(store_invalid(
                    "A completed enablement operation contains incomplete authorization evidence.",
                ))
            }
        }
        let package_generation = self.result.state.package_generation.ok_or_else(|| {
            store_invalid("A stored enablement result omitted its state generation.")
        })?;
        let artifact = self.state_after.artifact.as_ref().ok_or_else(|| {
            store_invalid("A completed enablement operation omitted its artifact identity.")
        })?;
        if !valid_sha256(&self.request_digest)
            || self.request.descriptor_digest()? != self.request_digest
            || self.result.replayed
            || self.state_after.scope != self.scope
            || self.state_after.package_id != self.request.package_id.as_str()
            || self.state_after.state_generation != package_generation
            || self.state_after.enabled != self.request.enabled
            || self.state_after.active.is_some()
            || self.state_after.updated_at_ms != self.result.completed_at_ms
            || self.result.state.version.as_deref() != Some(artifact.version.as_str())
            || self.result.state.package_digest.as_deref() != Some(artifact.package_digest.as_str())
            || self.result.state.manifest_digest.as_deref()
                != Some(artifact.manifest_digest.as_str())
        {
            return Err(store_invalid(
                "A completed cognitive-package enablement operation is invalid.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(super) struct CognitivePackageEnablementStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl CognitivePackageEnablementStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("package-enablement"),
            state_root,
        }
    }

    pub async fn lock_package(
        &self,
        scope: &PlanScope,
        package_id: &PluginPackageId,
    ) -> UseResult<StdFile> {
        validate_scope(scope)?;
        let directory = self.package_directory(scope, package_id)?;
        ensure_owned_directory(&self.state_root, &directory).await?;
        let lock_path = directory.join(".state.lock");
        acquire_lock(lock_path).await
    }

    pub async fn lock_operation(
        &self,
        scope: &PlanScope,
        operation_id: &str,
    ) -> UseResult<StdFile> {
        validate_scope(scope)?;
        CognitivePackageEnablementRequest::validate_operation_id(operation_id)?;
        let digest = format!("{:x}", Sha256::digest(operation_id.as_bytes()));
        let directory = self.root.join("operation-locks").join(scope_digest(scope)?);
        ensure_owned_directory(&self.state_root, &directory).await?;
        acquire_lock(directory.join(format!("{digest}.lock"))).await
    }

    pub async fn get_state(
        &self,
        scope: &PlanScope,
        package_id: &PluginPackageId,
    ) -> UseResult<Option<StoredCognitivePackageEnablement>> {
        let path = self.state_path(scope, package_id)?;
        let Some(state) =
            read_optional::<StoredCognitivePackageEnablement>(&self.state_root, &path).await?
        else {
            return Ok(None);
        };
        state.validate()?;
        if state.scope != *scope || state.package_id != package_id.as_str() {
            return Err(path_invalid());
        }
        Ok(Some(state))
    }

    pub async fn put_state(&self, state: &StoredCognitivePackageEnablement) -> UseResult<()> {
        state.validate()?;
        let package_id = PluginPackageId::parse(state.package_id.clone())?;
        let path = self.state_path(&state.scope, &package_id)?;
        write_replace(&self.state_root, &path, state).await
    }

    pub async fn get_operation(
        &self,
        scope: &PlanScope,
        operation_id: &str,
    ) -> UseResult<Option<StoredCognitivePackageEnablementOperation>> {
        let path = self.operation_path(scope, operation_id)?;
        let Some(operation) =
            read_optional::<StoredCognitivePackageEnablementOperation>(&self.state_root, &path)
                .await?
        else {
            return Ok(None);
        };
        operation.validate()?;
        if operation.scope != *scope || operation.request.operation_id != operation_id {
            return Err(path_invalid());
        }
        Ok(Some(operation))
    }

    pub async fn put_operation(
        &self,
        operation: &StoredCognitivePackageEnablementOperation,
    ) -> UseResult<bool> {
        operation.validate()?;
        let path = self.operation_path(&operation.scope, &operation.request.operation_id)?;
        if let Some(current) =
            read_optional::<StoredCognitivePackageEnablementOperation>(&self.state_root, &path)
                .await?
        {
            current.validate()?;
            if current == *operation {
                return Ok(false);
            }
            return Err(operation_conflict());
        }
        write_new(&self.state_root, &path, operation).await?;
        Ok(true)
    }

    fn package_directory(
        &self,
        scope: &PlanScope,
        package_id: &PluginPackageId,
    ) -> UseResult<PathBuf> {
        let (publisher, package) = package_id
            .as_str()
            .split_once('/')
            .ok_or_else(path_invalid)?;
        Ok(self
            .root
            .join("scopes")
            .join(scope_digest(scope)?)
            .join(publisher)
            .join(package))
    }

    fn state_path(&self, scope: &PlanScope, package_id: &PluginPackageId) -> UseResult<PathBuf> {
        Ok(self
            .package_directory(scope, package_id)?
            .join("state.json"))
    }

    fn operation_path(&self, scope: &PlanScope, operation_id: &str) -> UseResult<PathBuf> {
        CognitivePackageEnablementRequest::validate_operation_id(operation_id)?;
        let digest = format!("{:x}", Sha256::digest(operation_id.as_bytes()));
        Ok(self
            .root
            .join("operations")
            .join(scope_digest(scope)?)
            .join(format!("{digest}.json")))
    }
}

async fn acquire_lock(lock_path: PathBuf) -> UseResult<StdFile> {
    match fs::symlink_metadata(&lock_path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(path_invalid())
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect enablement lock", &lock_path, error)),
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
            "Failed to join the enablement lock task '{}': {error}",
            error_path.display()
        ))
    })?
    .map_err(|error| path_error("acquire enablement lock", &error_path, error))
}

fn scope_digest(scope: &PlanScope) -> UseResult<String> {
    validate_scope(scope)?;
    let kind = match scope.kind {
        PlanScopeKind::User => "user",
        PlanScopeKind::Workspace => "workspace",
    };
    Ok(format!(
        "{:x}",
        Sha256::digest(format!("{kind}\n{}", scope.id).as_bytes())
    ))
}

fn validate_scope(scope: &PlanScope) -> UseResult<()> {
    if scope.id.is_empty()
        || scope.id.len() > 256
        || !scope
            .id
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !scope.id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
    {
        return Err(store_invalid(
            "A cognitive-package enablement scope identity is invalid.",
        ));
    }
    Ok(())
}

async fn read_optional<T: DeserializeOwned>(
    state_root: &Path,
    path: &Path,
) -> UseResult<Option<T>> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(path_invalid());
    }
    let parent = path.parent().ok_or_else(path_invalid)?;
    if !validate_existing_directory_chain(state_root, parent).await? {
        return Ok(None);
    }
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect enablement record", path, error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_ENABLEMENT_RECORD_BYTES
    {
        return Err(store_invalid(
            "A cognitive-package enablement record is not a bounded regular file.",
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read enablement record", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_ENABLEMENT_RECORD_BYTES {
        return Err(store_invalid(
            "A cognitive-package enablement record changed outside its size bound.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| store_invalid(format!("An enablement record is invalid JSON: {error}")))
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
        return Err(path_invalid());
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        store_invalid(format!(
            "Failed to encode cognitive-package enablement state: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_ENABLEMENT_RECORD_BYTES {
        return Err(store_invalid(
            "A cognitive-package enablement record exceeds its size bound.",
        ));
    }
    let parent = path.parent().ok_or_else(path_invalid)?;
    ensure_owned_directory(state_root, parent).await?;
    let temporary = parent.join(format!(".enablement-{}.tmp", unique_suffix()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary enablement record", &temporary, error))?;
    if let Err(error) = async {
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "commit temporary enablement record",
            path,
            error,
        ));
    }
    drop(file);
    if replace {
        if let Err(error) = activate_replace(temporary.clone(), path.to_path_buf()).await {
            let _ = fs::remove_file(&temporary).await;
            return Err(error);
        }
    } else if let Err(error) = activate_new(temporary.clone(), path.to_path_buf()).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent(parent).await
}

async fn activate_replace(temporary: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        temporary.persist(target).map_err(|error| error.error)
    })
    .await
    .map_err(|error| {
        store_io(format!(
            "Failed to join enablement activation for '{}': {error}",
            error_target.display()
        ))
    })?
    .map_err(|error| path_error("activate enablement record", &error_target, error))
}

async fn activate_new(temporary: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        temporary
            .persist_noclobber(target)
            .map_err(|error| error.error)
    })
    .await
    .map_err(|error| {
        store_io(format!(
            "Failed to join immutable enablement activation for '{}': {error}",
            error_target.display()
        ))
    })?
    .map_err(|error| path_error("activate immutable enablement record", &error_target, error))
}

async fn ensure_owned_directory(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(path_invalid());
    }
    fs::create_dir_all(root)
        .await
        .map_err(|error| path_error("create enablement state root", root, error))?;
    validate_directory(root).await?;
    let relative = directory.strip_prefix(root).map_err(|_| path_invalid())?;
    let mut current = root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create enablement state directory",
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
        return Err(path_invalid());
    }
    let relative = directory.strip_prefix(root).map_err(|_| path_invalid())?;
    let mut current = root.to_path_buf();
    for segment in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(segment) = segment {
            current.push(segment.as_os_str());
        }
        match fs::symlink_metadata(&current).await {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
            Ok(_) => return Err(path_invalid()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    "inspect enablement state directory",
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
        .map_err(|error| path_error("inspect enablement state directory", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(path_invalid());
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_parent(parent: &Path) -> UseResult<()> {
    fs::File::open(parent)
        .await
        .map_err(|error| path_error("open enablement state directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync enablement state directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: &Path) -> UseResult<()> {
    Ok(())
}

fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(super) fn operation_conflict() -> UseError {
    UseError::new(
        "use.plugin.package_enablement_operation_conflict",
        "The enablement operation ID is already bound to a different durable request or result.",
    )
}

fn path_invalid() -> UseError {
    UseError::new(
        "use.plugin.package_enablement_path_invalid",
        "A cognitive-package enablement record escaped or traversed its owned state path.",
    )
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    store_io(format!("Failed to {action} '{}': {error}", path.display()))
}

fn store_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.package_enablement_store_invalid", message)
}

fn store_io(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.package_enablement_io", message)
}

#[cfg(test)]
#[path = "enablement_store_tests.rs"]
mod tests;
