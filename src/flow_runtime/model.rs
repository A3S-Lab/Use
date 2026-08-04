use std::path::{Path, PathBuf};

use a3s_use_core::{
    PlanQualifiedSurfaceRef, PluginPackageId, PluginSurfaceKind, UseError, UseResult,
};
use a3s_use_extension::{
    inspect_flow_surface_file, PluginFlowEngine, PluginFlowRuntime, PluginFlowSurface,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncReadExt;

pub const FLOW_RUNTIME_BINDING_SCHEMA: &str = "a3s.use.flow-runtime-binding.v1";
const MAX_FLOW_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

/// Durable proof that the sole A3S Flow engine compiled one immutable package
/// source for an exact lifecycle generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRuntimeBinding {
    schema: String,
    scope_id: String,
    surface: PlanQualifiedSurfaceRef,
    generation: u64,
    package_digest: String,
    manifest_digest: String,
    engine: PluginFlowEngine,
    runtime: PluginFlowRuntime,
    source_digest: String,
    export_name: String,
    entrypoint: PathBuf,
    artifact: PathBuf,
    artifact_sha256: String,
    source_hash: String,
}

pub(crate) struct FlowRuntimeBindingSpec {
    pub scope_id: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
    pub engine: PluginFlowEngine,
    pub runtime: PluginFlowRuntime,
    pub source_digest: String,
    pub export_name: String,
    pub entrypoint: PathBuf,
    pub artifact: PathBuf,
    pub artifact_sha256: String,
    pub source_hash: String,
}

impl FlowRuntimeBinding {
    pub(crate) fn new(spec: FlowRuntimeBindingSpec) -> UseResult<Self> {
        let binding = Self {
            schema: FLOW_RUNTIME_BINDING_SCHEMA.to_string(),
            scope_id: spec.scope_id,
            surface: spec.surface,
            generation: spec.generation,
            package_digest: spec.package_digest,
            manifest_digest: spec.manifest_digest,
            engine: spec.engine,
            runtime: spec.runtime,
            source_digest: spec.source_digest,
            export_name: spec.export_name,
            entrypoint: spec.entrypoint,
            artifact: spec.artifact,
            artifact_sha256: spec.artifact_sha256,
            source_hash: spec.source_hash,
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != FLOW_RUNTIME_BINDING_SCHEMA
            || !valid_machine_id(&self.scope_id)
            || PluginPackageId::parse(self.surface.package_id.clone()).is_err()
            || self.surface.surface.kind != PluginSurfaceKind::Flow
            || !valid_segment(&self.surface.surface.id)
            || self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.manifest_digest)
            || !valid_sha256(&self.source_digest)
            || !valid_sha256(&self.artifact_sha256)
            || !valid_hex_digest(&self.source_hash)
            || self.export_name.is_empty()
            || self.export_name.len() > 128
            || !self.entrypoint.is_absolute()
            || !self.artifact.is_absolute()
            || self.entrypoint == self.artifact
        {
            return Err(flow_error(
                "use.plugin.flow_binding_invalid",
                "The retained A3S Flow binding identity or evidence is invalid.",
            ));
        }
        Ok(())
    }

    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub fn surface(&self) -> &PlanQualifiedSurfaceRef {
        &self.surface
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }

    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub fn artifact(&self) -> &Path {
        &self.artifact
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| {
            flow_error(
                "use.plugin.flow_binding_invalid",
                format!("Failed to encode A3S Flow binding evidence: {error}"),
            )
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    /// Revalidate both immutable source evidence and the exact compiled
    /// artifact. A retained receipt alone never establishes runtime readiness.
    pub async fn inspect(&self, surface: &PluginFlowSurface, package_root: &Path) -> UseResult<()> {
        self.validate()?;
        let expected_entrypoint = package_root.join(&surface.source);
        if self.engine != surface.engine
            || self.runtime != surface.runtime
            || self.export_name != surface.export_name
            || self.entrypoint != expected_entrypoint
        {
            return Err(flow_error(
                "use.plugin.flow_binding_mismatch",
                "The retained A3S Flow binding no longer matches its admitted manifest surface.",
            ));
        }
        let source = inspect_flow_surface_file(surface, package_root).await?;
        if source.digest() != self.source_digest {
            return Err(flow_error(
                "use.plugin.flow_source_changed",
                "The A3S Flow source changed after runtime preflight.",
            ));
        }
        let artifact_sha256 = digest_artifact(&self.artifact).await?;
        if artifact_sha256 != self.artifact_sha256 {
            return Err(flow_error(
                "use.plugin.flow_artifact_changed",
                "The compiled A3S Flow artifact changed after runtime preflight.",
            ));
        }
        Ok(())
    }
}

pub(crate) async fn digest_artifact(path: &Path) -> UseResult<String> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| artifact_io_error("inspect", path, error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_FLOW_ARTIFACT_BYTES
    {
        return Err(flow_error(
            "use.plugin.flow_artifact_invalid",
            format!(
                "Compiled A3S Flow artifact '{}' is not a bounded regular file.",
                path.display()
            ),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(flow_error(
                "use.plugin.flow_artifact_invalid",
                format!(
                    "Compiled A3S Flow artifact '{}' is not executable.",
                    path.display()
                ),
            ));
        }
    }
    let mut file = fs::File::open(path)
        .await
        .map_err(|error| artifact_io_error("open", path, error))?;
    let mut hasher = Sha256::new();
    let mut read_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .await
            .map_err(|error| artifact_io_error("read", path, error))?;
        if count == 0 {
            break;
        }
        read_bytes = read_bytes
            .checked_add(u64::try_from(count).map_err(|_| {
                flow_error(
                    "use.plugin.flow_artifact_invalid",
                    "The compiled A3S Flow artifact exceeds numeric bounds.",
                )
            })?)
            .ok_or_else(|| {
                flow_error(
                    "use.plugin.flow_artifact_invalid",
                    "The compiled A3S Flow artifact exceeds numeric bounds.",
                )
            })?;
        if read_bytes > MAX_FLOW_ARTIFACT_BYTES {
            return Err(flow_error(
                "use.plugin.flow_artifact_invalid",
                "The compiled A3S Flow artifact exceeds its size bound.",
            ));
        }
        hasher.update(&buffer[..count]);
    }
    if read_bytes != metadata.len() {
        return Err(flow_error(
            "use.plugin.flow_artifact_changed",
            "The compiled A3S Flow artifact changed while it was inspected.",
        ));
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

pub(crate) fn valid_machine_id(value: &str) -> bool {
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

pub(crate) fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(valid_hex_digest)
}

fn valid_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn artifact_io_error(action: &str, path: &Path, error: std::io::Error) -> UseError {
    flow_error(
        "use.plugin.flow_artifact_io",
        format!(
            "Failed to {action} compiled A3S Flow artifact '{}': {error}",
            path.display()
        ),
    )
}

pub(crate) fn flow_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
