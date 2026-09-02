use std::fs as std_fs;
#[cfg(unix)]
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use a3s_flow::{NativeTsRuntime, NativeTsRuntimeConfig, WorkflowSpec};
use a3s_use_core::{PluginOperationAction, PluginSurfaceKind, UseError, UseResult};
use a3s_use_extension::{ArtifactStore, VerifiedFlowSurfacePayload};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::fs;

use super::super::effect_port::{
    ControlEffectFailure, ControlEffectPortOutcome, ControlFlowEffectPort,
    ControlSurfaceApplication, ControlSurfaceEffectAction, ControlSurfaceEffectRequest,
};
use super::super::model::{valid_error_code, ControlEffectOwner};
use crate::flow_runtime::digest_artifact;

const FLOW_RECEIPT_DOMAIN: &[u8] = b"a3s.use.control-flow-receipt.v1\0";
const FLOW_FAILURE_DOMAIN: &[u8] = b"a3s.use.control-flow-failure.v1\0";
const FLOW_SOURCE_DOMAIN: &[u8] = b"a3s.use.control-flow-source.v1\0";
const FLOW_AUTHORITY_ERROR: &str = "use.control_store.flow_authority_invalid";
const FLOW_OWNER_ERROR: &str = "use.control_store.flow_owner_failed";
const FLOW_MATERIALIZATION_ERROR: &str = "use.control_store.flow_materialization_failed";
const FLOW_PREFLIGHT_ERROR: &str = "use.control_store.flow_preflight_failed";
const FLOW_PREFLIGHT_MISMATCH: &str = "use.control_store.flow_preflight_mismatch";
const FLOW_SOURCE_CONFLICT: &str = "use.control_store.flow_source_conflict";

/// Inactive Control owner for A3S Flow Native TypeScript preparation.
///
/// Control remains the only desired-state authority. The Artifact Store lease
/// supplies one verified, path-free source snapshot; this owner materializes
/// that snapshot into its own immutable content-addressed workspace before
/// invoking `a3s-flow`. No Artifact Store package path or legacy Flow binding
/// is exposed to the compiler boundary.
#[derive(Debug, Clone)]
pub(in crate::control_store) struct ControlA3sFlowEffectPort {
    artifact_store: ArtifactStore,
    cache_dir: PathBuf,
    source_root: PathBuf,
    runtime: NativeTsRuntime,
}

impl ControlA3sFlowEffectPort {
    pub(in crate::control_store) fn new(
        artifact_store: ArtifactStore,
        compiler_binary: impl Into<PathBuf>,
        cache_dir: impl Into<PathBuf>,
    ) -> UseResult<Self> {
        let compiler_binary = compiler_binary.into();
        let cache_dir = cache_dir.into();
        if !stable_absolute_path(&compiler_binary) || !stable_absolute_path(&cache_dir) {
            return Err(flow_error(
                FLOW_AUTHORITY_ERROR,
                "The Flow compiler and owner cache must use absolute paths.",
            ));
        }
        let source_root = cache_dir.join("control-sources");
        let runtime = NativeTsRuntime::new(NativeTsRuntimeConfig::new(
            compiler_binary.clone(),
            cache_dir.clone(),
            cache_dir.clone(),
        ));
        Ok(Self {
            artifact_store,
            cache_dir,
            source_root,
            runtime,
        })
    }

    async fn apply(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        if request
            .validate_for_owner(PluginSurfaceKind::Flow, ControlEffectOwner::FlowHost)
            .is_err()
        {
            return rejected(request, FLOW_AUTHORITY_ERROR);
        }
        match request.action {
            ControlSurfaceEffectAction::Prepare => self.prepare(request).await,
            ControlSurfaceEffectAction::Stop => self.checkpoint(request, "stopped"),
            ControlSurfaceEffectAction::Remove => self.checkpoint(request, "removed"),
        }
    }

    async fn prepare(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        let package = match self
            .artifact_store
            .acquire_verified_package(&request.authority.package.package.catalog)
            .await
        {
            Ok(package) => package,
            Err(error) => return before_effect_failure(request, "artifact-acquire", error),
        };
        let payload = match package.read_flow_surface(&request.surface.id).await {
            Ok(payload) => payload,
            Err(error) => return before_effect_failure(request, "artifact-read", error),
        };
        let source_identity = match source_identity(request, &payload) {
            Ok(identity) => identity,
            Err(error) => return rejected(request, error.code),
        };
        let source_path = match self.materialize_source(&source_identity, &payload).await {
            Ok(path) => path,
            Err(error) if error.code == FLOW_SOURCE_CONFLICT => {
                return rejected(request, error.code)
            }
            Err(error) => return unknown(request, "source-materialization", error),
        };
        // The lease is intentionally released before compiler I/O. The source
        // bytes have already been verified and durably copied to the owner's
        // private workspace.
        drop(package);

        let entrypoint = match compiler_entrypoint(&source_path) {
            Ok(path) => path,
            Err(error) => return rejected(request, error.code),
        };
        let spec = WorkflowSpec::native_ts(
            workflow_name(request),
            format!("generation-{}", request.lifecycle_generation),
            entrypoint.to_string_lossy(),
            payload.surface().export_name.clone(),
        );
        let preflight = match self.runtime.preflight(&spec).await {
            Ok(preflight) => preflight,
            Err(_) => return rejected(request, FLOW_PREFLIGHT_ERROR),
        };
        if preflight.entrypoint != entrypoint {
            return rejected(request, FLOW_PREFLIGHT_MISMATCH);
        }
        let artifact_digest = match digest_artifact(&preflight.artifact).await {
            Ok(digest) => digest,
            Err(_) => return rejected(request, FLOW_PREFLIGHT_ERROR),
        };
        let receipt_digest =
            match receipt_digest(request, &payload, &preflight.source_hash, &artifact_digest) {
                Ok(digest) => digest,
                Err(error) => return rejected(request, error.code),
            };
        match ControlSurfaceApplication::new(request, receipt_digest, Some(artifact_digest)) {
            Ok(application) => ControlEffectPortOutcome::applied(application),
            Err(error) => rejected(request, error.code),
        }
    }

    fn checkpoint(
        &self,
        request: &ControlSurfaceEffectRequest,
        state: &str,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        let receipt = checkpoint_digest(request, state);
        match ControlSurfaceApplication::new(request, receipt, None) {
            Ok(application) => ControlEffectPortOutcome::applied(application),
            Err(error) => rejected(request, error.code),
        }
    }

    async fn materialize_source(
        &self,
        identity: &str,
        payload: &VerifiedFlowSurfacePayload,
    ) -> UseResult<PathBuf> {
        ensure_directory(&self.cache_dir).await?;
        ensure_directory(&self.source_root).await?;
        let digest = identity.strip_prefix("sha256:").ok_or_else(|| {
            flow_error(FLOW_MATERIALIZATION_ERROR, "Invalid Flow source identity.")
        })?;
        let digest_directory = self.source_root.join("sha256");
        ensure_directory(&digest_directory).await?;
        let target = digest_directory.join(format!("{digest}.ts"));
        let target = absolute_path(&target)?;
        let parent = target
            .parent()
            .ok_or_else(|| flow_error(FLOW_MATERIALIZATION_ERROR, "Flow source has no parent."))?
            .to_path_buf();
        let bytes = payload.source().to_vec();
        let target_for_worker = target.clone();
        let parent_for_worker = parent.clone();
        let published = tokio::task::spawn_blocking(move || {
            publish_source_blocking(&parent_for_worker, &target_for_worker, &bytes)
        })
        .await
        .map_err(|error| {
            flow_error(
                FLOW_MATERIALIZATION_ERROR,
                format!("Flow source materialization worker failed: {error}"),
            )
        })?
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                flow_error(
                    FLOW_SOURCE_CONFLICT,
                    "The immutable Flow source materialization conflicts with existing bytes.",
                )
            } else {
                flow_error(
                    FLOW_MATERIALIZATION_ERROR,
                    format!(
                        "Failed to publish the immutable Flow source '{}' in '{}': {error}",
                        target.display(),
                        parent.display()
                    ),
                )
            }
        });
        published?;
        let metadata = fs::symlink_metadata(&target).await.map_err(|error| {
            flow_error(
                FLOW_MATERIALIZATION_ERROR,
                format!("Failed to inspect the published Flow source: {error}"),
            )
        })?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
            return Err(flow_error(
                FLOW_SOURCE_CONFLICT,
                "The published Flow source is not a regular file.",
            ));
        }
        let observed = fs::read(&target).await.map_err(|error| {
            flow_error(
                FLOW_MATERIALIZATION_ERROR,
                format!("Failed to verify the published Flow source: {error}"),
            )
        })?;
        if observed != payload.source() {
            return Err(flow_error(
                FLOW_SOURCE_CONFLICT,
                "The published Flow source changed before compiler admission.",
            ));
        }
        Ok(target)
    }
}

#[async_trait]
impl ControlFlowEffectPort for ControlA3sFlowEffectPort {
    async fn apply_surface(
        &self,
        request: &ControlSurfaceEffectRequest,
    ) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
        self.apply(request).await
    }
}

fn source_identity(
    request: &ControlSurfaceEffectRequest,
    payload: &VerifiedFlowSurfacePayload,
) -> UseResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(FLOW_SOURCE_DOMAIN);
    hash_field(&mut hasher, &request.package_digest);
    hash_field(&mut hasher, &request.manifest_digest);
    hash_field(&mut hasher, &payload.surface().id);
    hash_field(&mut hasher, payload.evidence().digest());
    hash_field(&mut hasher, &payload.surface().export_name);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn compiler_entrypoint(path: &Path) -> UseResult<PathBuf> {
    let absolute = absolute_path(path)?;
    #[cfg(windows)]
    {
        a3s_use_core::windows_extended_length_path(&absolute).map_err(|error| {
            flow_error(
                FLOW_MATERIALIZATION_ERROR,
                format!("Failed to prepare the Flow compiler source path: {error}"),
            )
        })
    }
    #[cfg(not(windows))]
    {
        Ok(absolute)
    }
}

fn absolute_path(path: &Path) -> UseResult<PathBuf> {
    std::path::absolute(path).map_err(|error| {
        flow_error(
            FLOW_MATERIALIZATION_ERROR,
            format!("Failed to resolve the Flow owner path: {error}"),
        )
    })
}

fn stable_absolute_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

async fn ensure_directory(path: &Path) -> UseResult<()> {
    fs::create_dir_all(path).await.map_err(|error| {
        flow_error(
            FLOW_MATERIALIZATION_ERROR,
            format!(
                "Failed to create the Flow owner directory '{}': {error}",
                path.display()
            ),
        )
    })?;
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        flow_error(
            FLOW_MATERIALIZATION_ERROR,
            format!(
                "Failed to inspect the Flow owner directory '{}': {error}",
                path.display()
            ),
        )
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(flow_error(
            FLOW_MATERIALIZATION_ERROR,
            format!(
                "Flow owner directory '{}' is not a real directory.",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn publish_source_blocking(parent: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let target_metadata = match std_fs::symlink_metadata(target) {
        Ok(metadata) => {
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Flow source target is not a regular file",
                ));
            }
            Some(metadata)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if target_metadata.is_some() {
        let existing = std_fs::read(target)?;
        if existing == bytes {
            return Ok(());
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Flow source target contains different bytes",
        ));
    }

    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    match a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, target) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = std_fs::symlink_metadata(target)?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Flow source target is not a regular file",
                ));
            }
            if std_fs::read(target)? != bytes {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Flow source target contains different bytes",
                ));
            }
        }
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    {
        let directory = OpenOptions::new().read(true).open(parent)?;
        directory.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Ok(())
    }
}

fn receipt_digest(
    request: &ControlSurfaceEffectRequest,
    payload: &VerifiedFlowSurfacePayload,
    source_hash: &str,
    artifact_digest: &str,
) -> UseResult<String> {
    let catalog_digest = request
        .authority
        .package
        .package
        .catalog
        .descriptor_digest()
        .map_err(|_| flow_error(FLOW_AUTHORITY_ERROR, "Flow catalog authority is invalid."))?;
    let mut hasher = Sha256::new();
    hasher.update(FLOW_RECEIPT_DOMAIN);
    hash_request_identity(&mut hasher, request);
    hash_field(&mut hasher, &catalog_digest);
    hash_field(&mut hasher, payload.surface().id.as_str());
    hash_field(
        &mut hasher,
        payload.surface().source.to_string_lossy().as_ref(),
    );
    hash_field(&mut hasher, payload.surface().export_name.as_str());
    hash_field(&mut hasher, payload.evidence().digest());
    hash_field(&mut hasher, source_hash);
    hash_field(&mut hasher, artifact_digest);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn checkpoint_digest(request: &ControlSurfaceEffectRequest, state: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(FLOW_RECEIPT_DOMAIN);
    hash_request_identity(&mut hasher, request);
    hash_field(&mut hasher, state);
    format!("sha256:{:x}", hasher.finalize())
}

fn hash_request_identity(hasher: &mut Sha256, request: &ControlSurfaceEffectRequest) {
    hash_field(hasher, &request.identity.operation_id);
    hash_field(hasher, request.identity.installation.kind.as_str());
    hash_field(hasher, &request.identity.installation.id);
    hash_field(hasher, &request.identity.plan_digest);
    hash_field(hasher, operation_action(request.identity.operation_action));
    hash_u64(hasher, request.identity.installation_generation);
    hash_u64(hasher, u64::from(request.identity.sequence));
    hash_field(hasher, &request.identity.idempotency_key);
    hash_field(hasher, &request.authority.generation_operation_id);
    hash_field(hasher, &request.authority.snapshot_digest);
    hash_u64(hasher, request.authority.committed_at_ms);
    hash_field(hasher, &request.authority.host.target);
    hash_field(hasher, &request.authority.host.use_version);
    hash_field(hasher, &request.package_id);
    hash_u64(hasher, request.lifecycle_generation);
    hash_field(hasher, &request.package_digest);
    hash_field(hasher, &request.manifest_digest);
    hash_field(hasher, &request.surface.id);
    hash_field(hasher, surface_action(request.action));
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn hash_u64(hasher: &mut Sha256, value: u64) {
    hasher.update(value.to_be_bytes());
}

fn workflow_name(request: &ControlSurfaceEffectRequest) -> String {
    format!("{}:{}", request.package_id, request.surface.id)
}

fn before_effect_failure(
    request: &ControlSurfaceEffectRequest,
    phase: &str,
    error: UseError,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    let code = normalized_error_code(error);
    let failure = failure(request, phase, &code);
    if matches!(
        code.as_str(),
        "use.artifact_store.busy" | "use.artifact_store.io" | "use.extension.io"
    ) {
        ControlEffectPortOutcome::deferred(failure)
    } else {
        ControlEffectPortOutcome::rejected(failure)
    }
}

fn rejected(
    request: &ControlSurfaceEffectRequest,
    error_code: impl Into<String>,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    let error_code = error_code.into();
    let error_code = if valid_error_code(&error_code) {
        error_code
    } else {
        FLOW_OWNER_ERROR.to_string()
    };
    ControlEffectPortOutcome::rejected(failure(request, "rejected", &error_code))
}

fn unknown(
    request: &ControlSurfaceEffectRequest,
    phase: &str,
    error: UseError,
) -> ControlEffectPortOutcome<ControlSurfaceApplication> {
    let code = normalized_error_code(error);
    ControlEffectPortOutcome::unknown(failure(request, phase, &code))
}

fn normalized_error_code(error: UseError) -> String {
    if valid_error_code(&error.code) {
        error.code
    } else {
        FLOW_OWNER_ERROR.to_string()
    }
}

fn failure(
    request: &ControlSurfaceEffectRequest,
    phase: &str,
    error_code: &str,
) -> ControlEffectFailure {
    let mut hasher = Sha256::new();
    hasher.update(FLOW_FAILURE_DOMAIN);
    hash_field(&mut hasher, &request.identity.idempotency_key);
    hash_field(&mut hasher, phase);
    hash_field(&mut hasher, error_code);
    ControlEffectFailure {
        evidence_digest: format!("sha256:{:x}", hasher.finalize()),
        error_code: error_code.to_string(),
    }
}

const fn operation_action(action: PluginOperationAction) -> &'static str {
    match action {
        PluginOperationAction::Install => "install",
        PluginOperationAction::Uninstall => "uninstall",
        PluginOperationAction::Upgrade => "upgrade",
        PluginOperationAction::Enable => "enable",
        PluginOperationAction::Disable => "disable",
    }
}

const fn surface_action(action: ControlSurfaceEffectAction) -> &'static str {
    match action {
        ControlSurfaceEffectAction::Prepare => "prepare",
        ControlSurfaceEffectAction::Stop => "stop",
        ControlSurfaceEffectAction::Remove => "remove",
    }
}

fn flow_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
