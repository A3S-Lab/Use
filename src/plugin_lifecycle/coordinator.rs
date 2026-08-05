use std::sync::Arc;

use a3s_use_core::{PluginSurfaceKind, UseError, UseResult};
use a3s_use_extension::{
    ExtensionManifest, PluginFlowSurface, PluginMcpSurface, PluginOkfSurface, PluginSkillSurface,
    PluginUiSurface, ToolSurface,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::model::valid_sha256;
use super::{
    PluginLifecycleCheckpoint, PluginLifecycleCheckpointKind, PluginLifecycleCheckpointOutcome,
    PluginLifecycleIntent, PluginLifecycleIntentSpec, PluginLifecycleJournalStore,
    PluginLifecycleOperationRecord, PluginLifecycleOperationStatus,
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
pub trait PluginFlowLifecycleHost: Send + Sync {
    async fn prepare_flow(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn stop_flow(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence>;

    async fn remove_flow(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginFlowSurface,
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
    flow: Arc<dyn PluginFlowLifecycleHost>,
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
        flow: Arc<dyn PluginFlowLifecycleHost>,
        skill: Arc<dyn PluginSkillLifecycleHost>,
        ui: Arc<dyn PluginUiLifecycleHost>,
    ) -> Self {
        Self {
            package,
            capability,
            tool,
            mcp,
            okf,
            flow,
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
            record = self
                .execute_and_record(intent, manifest, &checkpoint, &completed_at_ms)
                .await?;
        }
    }

    pub(super) async fn prepare_for_graph(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        validate_manifest_binding(intent, manifest)?;
        if !matches!(
            intent.action,
            super::PluginLifecycleAction::Install | super::PluginLifecycleAction::Upgrade
        ) {
            return Err(coordinator_error(
                "Only install or upgrade candidates can use dependency-closure staged publication.",
            ));
        }
        if let Some(record) = self.load_exact_record(intent).await? {
            match record.status {
                PluginLifecycleOperationStatus::Completed => return Ok(record),
                PluginLifecycleOperationStatus::RollingBack
                | PluginLifecycleOperationStatus::RolledBack => {
                    return Err(coordinator_error(
                        "A rolled-back candidate requires a fresh reviewed operation.",
                    ))
                }
                PluginLifecycleOperationStatus::Applying => {}
            }
        }
        let mut record = self.journal.begin(intent).await?;
        if record.status == PluginLifecycleOperationStatus::Completed {
            return Ok(record);
        }
        loop {
            let Some(checkpoint) = record.next_checkpoint().cloned() else {
                return self.journal.complete(intent, completed_at_ms()).await;
            };
            if checkpoint.kind == PluginLifecycleCheckpointKind::CapabilityPublished {
                return Ok(record);
            }
            record = self
                .execute_and_record(intent, manifest, &checkpoint, completed_at_ms)
                .await?;
        }
    }

    pub(super) async fn complete_graph_publication(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        evidence: &PluginLifecycleEvidence,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        validate_manifest_binding(intent, manifest)?;
        if let Some(record) = self.load_exact_record(intent).await? {
            if record.status == PluginLifecycleOperationStatus::Completed {
                return Ok(record);
            }
        }
        let mut record = self.journal.begin(intent).await?;
        if record.status == PluginLifecycleOperationStatus::Completed {
            return Ok(record);
        }
        if let Some(checkpoint) = record.next_checkpoint().cloned() {
            if checkpoint.kind != PluginLifecycleCheckpointKind::CapabilityPublished {
                return Err(coordinator_error(
                    "A package graph attempted publication before all surfaces were prepared.",
                ));
            }
            record = self
                .journal
                .record_checkpoint(
                    intent,
                    &checkpoint.idempotency_key,
                    PluginLifecycleCheckpointOutcome::Applied,
                    evidence.digest.clone(),
                    None,
                    completed_at_ms(),
                )
                .await?;
        }
        if record.next_checkpoint().is_some() {
            return Err(coordinator_error(
                "A graph publication did not complete the install checkpoint sequence.",
            ));
        }
        self.journal.complete(intent, completed_at_ms()).await
    }

    pub(super) async fn record_graph_capability_hidden(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        evidence: &PluginLifecycleEvidence,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        validate_manifest_binding(intent, manifest)?;
        if intent.action != super::PluginLifecycleAction::Uninstall {
            return Err(coordinator_error(
                "Only uninstall operations can record an atomic graph hide.",
            ));
        }
        if let Some(record) = self.load_exact_record(intent).await? {
            if record.status == PluginLifecycleOperationStatus::Completed {
                return Ok(record);
            }
        }
        let record = self.journal.begin(intent).await?;
        let Some(checkpoint) = record.next_checkpoint().cloned() else {
            return Ok(record);
        };
        if checkpoint.kind != PluginLifecycleCheckpointKind::CapabilityHidden {
            return Ok(record);
        }
        self.journal
            .record_checkpoint(
                intent,
                &checkpoint.idempotency_key,
                PluginLifecycleCheckpointOutcome::Applied,
                evidence.digest.clone(),
                None,
                completed_at_ms(),
            )
            .await
    }

    /// Advance an uninstall operation through hide and accepted-call drain,
    /// but stop before any surface or package cleanup. The graph coordinator
    /// uses this boundary so prior authorization remains valid for calls that
    /// acquired the old capability generation before atomic cutover.
    pub(super) async fn drain_graph_retirement(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        validate_manifest_binding(intent, manifest)?;
        if intent.action != super::PluginLifecycleAction::Uninstall {
            return Err(coordinator_error(
                "Only uninstall operations can drain a graph retirement.",
            ));
        }
        if let Some(record) = self.load_exact_record(intent).await? {
            if record.status == PluginLifecycleOperationStatus::Completed {
                return Ok(record);
            }
        }
        let mut record = self.journal.begin(intent).await?;
        loop {
            let Some(checkpoint) = record.next_checkpoint().cloned() else {
                return Ok(record);
            };
            if !matches!(
                checkpoint.kind,
                PluginLifecycleCheckpointKind::CapabilityHidden
                    | PluginLifecycleCheckpointKind::CallsDrained
            ) {
                return Ok(record);
            }
            record = self
                .execute_and_record(intent, manifest, &checkpoint, completed_at_ms)
                .await?;
        }
    }

    /// Remove every candidate surface that may have been prepared before a
    /// dependency-closure cutover failed. The next unrecorded surface is also
    /// cleaned because a host side effect may have succeeded before its
    /// checkpoint receipt could be persisted.
    pub(crate) async fn graph_candidate_status(
        &self,
        intent: &PluginLifecycleIntent,
    ) -> UseResult<Option<PluginLifecycleOperationStatus>> {
        if let Some(record) = self.load_exact_record(intent).await? {
            return Ok(Some(record.status));
        }
        match self
            .journal
            .load_active(&intent.scope_id, &intent.package_id)
            .await?
        {
            Some(record)
                if matches!(
                    record.status,
                    PluginLifecycleOperationStatus::Applying
                        | PluginLifecycleOperationStatus::RollingBack
                ) =>
            {
                Err(coordinator_error(
                    "Another lifecycle operation owns the candidate package.",
                ))
            }
            _ => Ok(None),
        }
    }

    pub(super) async fn start_graph_rollback(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
    ) -> UseResult<()> {
        validate_manifest_binding(intent, manifest)?;
        self.journal.start_rollback(intent).await?;
        Ok(())
    }

    pub(super) async fn rollback_graph_candidate_surfaces(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_manifest_binding(intent, manifest)?;
        if !matches!(
            intent.action,
            super::PluginLifecycleAction::Install | super::PluginLifecycleAction::Upgrade
        ) {
            return Err(coordinator_error(
                "Only an unpublished install or upgrade candidate can roll back.",
            ));
        }
        let record = self.load_exact_record(intent).await?.ok_or_else(|| {
            coordinator_error("A candidate rollback has no durable lifecycle operation evidence.")
        })?;
        if record.intent != *intent {
            return Err(coordinator_error(
                "A candidate rollback does not match the active lifecycle operation.",
            ));
        }
        if record.status == PluginLifecycleOperationStatus::Completed {
            return Err(coordinator_error(
                "A published lifecycle operation cannot use pre-cutover rollback.",
            ));
        }
        if record.status == PluginLifecycleOperationStatus::RolledBack {
            return PluginLifecycleEvidence::new(record.rollback_evidence_digest.ok_or_else(
                || coordinator_error("A rolled-back lifecycle operation omitted its evidence."),
            )?);
        }

        let mut attempted = record
            .receipts
            .iter()
            .filter_map(|receipt| {
                intent
                    .checkpoints
                    .get(receipt.sequence.saturating_sub(1) as usize)
            })
            .filter(|checkpoint| checkpoint.kind == PluginLifecycleCheckpointKind::SurfacePrepared)
            .cloned()
            .collect::<Vec<_>>();
        if let Some(checkpoint) = record.next_checkpoint() {
            if checkpoint.kind == PluginLifecycleCheckpointKind::SurfacePrepared {
                attempted.push(checkpoint.clone());
            }
        }

        let mut evidence = Vec::with_capacity(attempted.len());
        for checkpoint in attempted.into_iter().rev() {
            let surface = checkpoint.surface.as_ref().ok_or_else(|| {
                coordinator_error("A candidate surface checkpoint omitted its identity.")
            })?;
            let key = rollback_checkpoint_key(intent, &checkpoint);
            evidence.push(
                self.execute_surface(
                    intent,
                    manifest,
                    PluginLifecycleCheckpointKind::SurfaceRemoved,
                    surface.kind,
                    &surface.id,
                    &key,
                )
                .await?
                .digest,
            );
        }
        let mut identity = format!(
            "{}\ncandidate-surface-rollback",
            intent.descriptor_digest()?
        );
        for digest in evidence {
            identity.push('\n');
            identity.push_str(&digest);
        }
        PluginLifecycleEvidence::new(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
    }

    async fn load_exact_record(
        &self,
        intent: &PluginLifecycleIntent,
    ) -> UseResult<Option<PluginLifecycleOperationRecord>> {
        let active = self
            .journal
            .load_active(&intent.scope_id, &intent.package_id)
            .await?;
        if active
            .as_ref()
            .is_some_and(|record| record.intent == *intent)
        {
            return Ok(active);
        }
        let last = self
            .journal
            .load_last(&intent.scope_id, &intent.package_id)
            .await?;
        Ok(last.filter(|record| record.intent == *intent))
    }

    pub(super) async fn complete_graph_rollback(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        surface_evidence: &PluginLifecycleEvidence,
        package_evidence: &PluginLifecycleEvidence,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        validate_manifest_binding(intent, manifest)?;
        let identity = format!(
            "{}\n{}\n{}\ncandidate-rollback-complete",
            intent.descriptor_digest()?,
            surface_evidence.digest(),
            package_evidence.digest()
        );
        self.journal
            .roll_back(
                intent,
                format!("sha256:{:x}", Sha256::digest(identity.as_bytes())),
                completed_at_ms(),
            )
            .await
    }

    async fn execute_and_record(
        &self,
        intent: &PluginLifecycleIntent,
        manifest: &ExtensionManifest,
        checkpoint: &PluginLifecycleCheckpoint,
        completed_at_ms: &impl Fn() -> u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        match self.execute_checkpoint(intent, manifest, checkpoint).await {
            Ok(evidence) => {
                self.journal
                    .record_checkpoint(
                        intent,
                        &checkpoint.idempotency_key,
                        PluginLifecycleCheckpointOutcome::Applied,
                        evidence.digest,
                        None,
                        completed_at_ms(),
                    )
                    .await
            }
            Err(error)
                if !checkpoint.required
                    && checkpoint.kind == PluginLifecycleCheckpointKind::SurfacePrepared =>
            {
                let evidence_digest = failure_evidence_digest(checkpoint, &error.code);
                self.journal
                    .record_checkpoint(
                        intent,
                        &checkpoint.idempotency_key,
                        PluginLifecycleCheckpointOutcome::OptionalFailed,
                        evidence_digest,
                        Some(error.code.to_string()),
                        completed_at_ms(),
                    )
                    .await
            }
            Err(error) => {
                let evidence_digest = failure_evidence_digest(checkpoint, &error.code);
                self.journal
                    .record_failure(
                        intent,
                        &checkpoint.idempotency_key,
                        error.code.clone(),
                        evidence_digest,
                        completed_at_ms(),
                    )
                    .await?;
                Err(error)
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
            PluginSurfaceKind::Flow => {
                let surface = manifest
                    .flows
                    .iter()
                    .find(|surface| surface.id == surface_id)
                    .ok_or_else(surface_missing)?;
                match kind {
                    PluginLifecycleCheckpointKind::SurfacePrepared => {
                        self.hosts.flow.prepare_flow(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceStopped => {
                        self.hosts.flow.stop_flow(intent, surface, key).await
                    }
                    PluginLifecycleCheckpointKind::SurfaceRemoved => {
                        self.hosts.flow.remove_flow(intent, surface, key).await
                    }
                    _ => Err(surface_missing()),
                }
            }
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

fn rollback_checkpoint_key(
    intent: &PluginLifecycleIntent,
    checkpoint: &PluginLifecycleCheckpoint,
) -> String {
    let identity = format!(
        "{}\n{}\ncandidate-surface-rollback",
        intent.operation_id, checkpoint.idempotency_key
    );
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
