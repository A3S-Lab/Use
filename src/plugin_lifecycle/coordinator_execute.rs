// Plugin lifecycle execute/cutover methods (included into coordinator).

impl PluginLifecycleCoordinator {
    pub(super) async fn execute_and_record(
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
            (PluginLifecycleCheckpointKind::CapabilityPublished, None) => self
                .hosts
                .capability
                .publish_capability_with_cutover(intent, self.expected_capability_generation, key)
                .await
                .map(|publication| publication.evidence),
            (PluginLifecycleCheckpointKind::CapabilityHidden, None) => self
                .hosts
                .capability
                .hide_capability_with_cutover(intent, self.expected_capability_generation, key)
                .await
                .map(|publication| publication.evidence),
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

    pub(crate) async fn complete_single_cutover(
        &self,
        intent: &PluginLifecycleIntent,
    ) -> UseResult<()> {
        for checkpoint in &intent.checkpoints {
            if matches!(
                checkpoint.kind,
                PluginLifecycleCheckpointKind::CapabilityPublished
                    | PluginLifecycleCheckpointKind::CapabilityHidden
            ) {
                self.hosts
                    .capability
                    .complete_capability_cutover(&checkpoint.idempotency_key)
                    .await?;
            }
        }
        Ok(())
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
