use std::sync::Arc;

use a3s_use_core::{PluginSurfaceKind, UseError, UseResult};
use a3s_use_extension::{
    ExtensionManifest, PluginMcpSurface, PluginOkfSurface, PluginSkillSurface, PluginUiSurface,
    ToolSurface,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::model::valid_sha256;
use super::{
    PluginLifecycleCheckpoint, PluginLifecycleCheckpointKind, PluginLifecycleCheckpointOutcome,
    PluginLifecycleIntent, PluginLifecycleIntentSpec, PluginLifecycleJournalStore,
    PluginLifecycleOperationRecord,
};

/// Digest of host-validated, non-secret evidence for one lifecycle checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLifecycleEvidence {
    digest: String,
}

impl PluginLifecycleEvidence {
    pub fn new(digest: impl Into<String>) -> UseResult<Self> {
        let digest = digest.into();
        if !valid_sha256(&digest) {
            return Err(coordinator_error(
                "Lifecycle host evidence must be a canonical SHA-256 digest.",
            ));
        }
        Ok(Self { digest })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[async_trait]
pub trait PluginPackageLifecycleHost: Send + Sync {
    /// Commit the exact immutable generation as installed-disabled state.
    /// No capability may be visible when this checkpoint returns.
    async fn commit_package(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn remove_package(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;
}

#[async_trait]
pub trait PluginCapabilityLifecycleHost: Send + Sync {
    /// Atomically publish the complete required contribution generation.
    async fn publish_capability(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn hide_capability(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn drain_calls(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;
}

#[async_trait]
pub trait PluginToolLifecycleHost: Send + Sync {
    async fn prepare_tool(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn stop_tool(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn remove_tool(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;
}

#[async_trait]
pub trait PluginMcpLifecycleHost: Send + Sync {
    async fn prepare_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn stop_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn remove_mcp(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;
}

#[async_trait]
pub trait PluginOkfLifecycleHost: Send + Sync {
    async fn prepare_okf(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn stop_okf(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn remove_okf(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginOkfSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;
}

#[async_trait]
pub trait PluginSkillLifecycleHost: Send + Sync {
    async fn prepare_skill(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginSkillSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn stop_skill(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginSkillSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn remove_skill(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginSkillSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;
}

#[async_trait]
pub trait PluginUiLifecycleHost: Send + Sync {
    async fn prepare_ui(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginUiSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn stop_ui(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginUiSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn remove_ui(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginUiSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;
}

#[derive(Clone)]
pub struct PluginLifecycleHosts {
    package: Arc<dyn PluginPackageLifecycleHost>,
    capability: Arc<dyn PluginCapabilityLifecycleHost>,
    tool: Arc<dyn PluginToolLifecycleHost>,
    mcp: Arc<dyn PluginMcpLifecycleHost>,
    okf: Arc<dyn PluginOkfLifecycleHost>,
    skill: Arc<dyn PluginSkillLifecycleHost>,
    ui: Arc<dyn PluginUiLifecycleHost>,
}

impl PluginLifecycleHosts {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        package: Arc<dyn PluginPackageLifecycleHost>,
        capability: Arc<dyn PluginCapabilityLifecycleHost>,
        tool: Arc<dyn PluginToolLifecycleHost>,
        mcp: Arc<dyn PluginMcpLifecycleHost>,
        okf: Arc<dyn PluginOkfLifecycleHost>,
        skill: Arc<dyn PluginSkillLifecycleHost>,
        ui: Arc<dyn PluginUiLifecycleHost>,
    ) -> Self {
        Self {
            package,
            capability,
            tool,
            mcp,
            okf,
            skill,
            ui,
        }
    }
}

#[derive(Clone)]
pub struct PluginLifecycleCoordinator {
    journal: PluginLifecycleJournalStore,
    hosts: PluginLifecycleHosts,
}

impl PluginLifecycleCoordinator {
    pub fn new(journal: PluginLifecycleJournalStore, hosts: PluginLifecycleHosts) -> Self {
        Self { journal, hosts }
    }

    pub async fn apply(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        completed_at_ms: impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        validate_manifest_binding(intent, manifest)?;
        let mut record = self.journal.begin(intent).await?;
        loop {
            let Some(checkpoint) = record.next_checkpoint().cloned() else {
                return self.journal.complete(intent, completed_at_ms()).await;
            };
            match self.execute_checkpoint(intent, manifest, &checkpoint).await {
                Ok(evidence) => {
                    record = self
                        .journal
                        .record_checkpoint(
                            intent,
                            &checkpoint.idempotency_key,
                            PluginLifecycleCheckpointOutcome::Applied,
                            evidence.digest,
                            None,
                            completed_at_ms(),
                        )
                        .await?;
                }
                Err(error)
                    if !checkpoint.required
                        && checkpoint.kind == PluginLifecycleCheckpointKind::SurfacePrepared =>
                {
                    let evidence_digest = failure_evidence_digest(&checkpoint, &error.code);
                    record = self
                        .journal
                        .record_checkpoint(
                            intent,
                            &checkpoint.idempotency_key,
                            PluginLifecycleCheckpointOutcome::OptionalFailed,
                            evidence_digest,
                            Some(error.code.to_string()),
                            completed_at_ms(),
                        )
                        .await?;
                }
                Err(error) => {
                    let evidence_digest = failure_evidence_digest(&checkpoint, &error.code);
                    self.journal
                        .record_failure(
                            intent,
                            &checkpoint.idempotency_key,
                            error.code.clone(),
                            evidence_digest,
                            completed_at_ms(),
                        )
                        .await?;
                    return Err(error);
                }
            }
        }
    }

    async fn execute_checkpoint(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        checkpoint: &PluginLifecycleCheckpoint,
    ) -> UseResult<PluginLifecycleEvidence> {
        let key = checkpoint.idempotency_key.as_str();
        match (checkpoint.kind, checkpoint.surface.as_ref()) {
            (PluginLifecycleCheckpointKind::PackageCommitted, None) => {
                self.hosts.package.commit_package(intent, key).await
            }
            (PluginLifecycleCheckpointKind::PackageRemoved, None) => {
                self.hosts.package.remove_package(intent, key).await
            }
            (PluginLifecycleCheckpointKind::CapabilityPublished, None) => {
                self.hosts.capability.publish_capability(intent, key).await
            }
            (PluginLifecycleCheckpointKind::CapabilityHidden, None) => {
                self.hosts.capability.hide_capability(intent, key).await
            }
            (PluginLifecycleCheckpointKind::CallsDrained, None) => {
                self.hosts.capability.drain_calls(intent, key).await
            }
            (
                PluginLifecycleCheckpointKind::SurfacePrepared
                | PluginLifecycleCheckpointKind::SurfaceStopped
                | PluginLifecycleCheckpointKind::SurfaceRemoved,
                Some(surface),
            ) => {
                self.execute_surface(
                    intent,
                    manifest,
                    checkpoint.kind,
                    surface.kind,
                    &surface.id,
                    key,
                )
                .await
            }
            _ => Err(coordinator_error(
                "The lifecycle checkpoint kind and surface identity disagree.",
            )),
        }
    }

    async fn execute_surface(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        kind: PluginLifecycleCheckpointKind,
        surface_kind: PluginSurfaceKind,
        surface_id: &str,
        key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        match surface_kind {
            PluginSurfaceKind::Tool => {
                let surface = manifest
                    .tools
                    .iter()
                    .find(|surface| surface.id == surface_id)
                    .ok_or_else(surface_missing)?;
                match kind {
                    PluginLifecycleCheckpointKind::SurfacePrepared => {
                        self.hosts.tool.prepare_tool(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceStopped => {
                        self.hosts.tool.stop_tool(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceRemoved => {
                        self.hosts.tool.remove_tool(intent, surface, key).await
                    }
                    _ => Err(surface_missing()),
                }
            }
            PluginSurfaceKind::Mcp => {
                let surface = manifest
                    .mcp_servers
                    .iter()
                    .find(|surface| surface.id == surface_id)
                    .ok_or_else(surface_missing)?;
                match kind {
                    PluginLifecycleCheckpointKind::SurfacePrepared => {
                        self.hosts.mcp.prepare_mcp(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceStopped => {
                        self.hosts.mcp.stop_mcp(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceRemoved => {
                        self.hosts.mcp.remove_mcp(intent, surface, key).await
                    }
                    _ => Err(surface_missing()),
                }
            }
            PluginSurfaceKind::Okf => {
                let surface = manifest
                    .okf
                    .iter()
                    .find(|surface| surface.id == surface_id)
                    .ok_or_else(surface_missing)?;
                match kind {
                    PluginLifecycleCheckpointKind::SurfacePrepared => {
                        self.hosts.okf.prepare_okf(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceStopped => {
                        self.hosts.okf.stop_okf(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceRemoved => {
                        self.hosts.okf.remove_okf(intent, surface, key).await
                    }
                    _ => Err(surface_missing()),
                }
            }
            PluginSurfaceKind::Skill => {
                let surface = manifest
                    .skills
                    .iter()
                    .find(|surface| surface.id == surface_id)
                    .ok_or_else(surface_missing)?;
                match kind {
                    PluginLifecycleCheckpointKind::SurfacePrepared => {
                        self.hosts.skill.prepare_skill(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceStopped => {
                        self.hosts.skill.stop_skill(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceRemoved => {
                        self.hosts.skill.remove_skill(intent, surface, key).await
                    }
                    _ => Err(surface_missing()),
                }
            }
            PluginSurfaceKind::Ui => {
                let surface = manifest
                    .ui
                    .iter()
                    .find(|surface| surface.id == surface_id)
                    .ok_or_else(surface_missing)?;
                match kind {
                    PluginLifecycleCheckpointKind::SurfacePrepared => {
                        self.hosts.ui.prepare_ui(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceStopped => {
                        self.hosts.ui.stop_ui(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceRemoved => {
                        self.hosts.ui.remove_ui(intent, surface, key).await
                    }
                    _ => Err(surface_missing()),
                }
            }
        }
    }
}

fn validate_manifest_binding(
    intent: &PluginLifecycleIntent,
    manifest: &ExtensionManifest,
) -> UseResult<()> {
    let expected = PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: intent.operation_id.clone(),
            plan_digest: intent.plan_digest.clone(),
            scope_id: intent.scope_id.clone(),
            package_id: intent.package_id.clone(),
            package_digest: intent.package_digest.clone(),
            manifest_digest: intent.manifest_digest.clone(),
            generation: intent.generation,
            action: intent.action,
        },
        manifest,
    )?;
    if expected != *intent {
        return Err(coordinator_error(
            "The lifecycle intent no longer matches the admitted package surface graph.",
        ));
    }
    Ok(())
}

fn failure_evidence_digest(checkpoint: &PluginLifecycleCheckpoint, error_code: &str) -> String {
    let identity = format!("{}\n{error_code}", checkpoint.idempotency_key);
    format!("sha256:{:x}", Sha256::digest(identity.as_bytes()))
}

fn surface_missing() -> UseError {
    coordinator_error("A lifecycle checkpoint references a missing manifest surface.")
}

fn coordinator_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.lifecycle_coordinator_invalid", message)
}

#[cfg(test)]
mod tests;
