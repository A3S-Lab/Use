use super::*;

impl RuntimePluginSurfaceLifecycleHost {
    pub(super) async fn provision_tool_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        selected: &SelectedRuntimeSurface,
        idempotency_key: &str,
    ) -> UseResult<RuntimeBindingReceipt> {
        let provisioning = self
            .begin_service_provisioning(selected, "apply-tool", idempotency_key)
            .await?;
        self.advance_tool_provisioning(intent, surface, selected, provisioning)
            .await
    }

    async fn advance_tool_provisioning(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        selected: &SelectedRuntimeSurface,
        mut provisioning: RuntimeServiceProvisioningReceipt,
    ) -> UseResult<RuntimeBindingReceipt> {
        if provisioning.phase != RuntimeServiceProvisioningPhase::GatewayReady {
            let activation = selected
                .client()
                .apply_service(
                    selected.plan(),
                    selected.provider(),
                    provisioning.apply_request_id.clone(),
                    self.deadline_at_ms,
                )
                .await?;
            let observation = activation.observation().clone();
            provisioning.record_runtime_observation(
                selected.plan(),
                selected.provider(),
                observation.clone(),
            )?;
            self.store.put_provisioning(&provisioning).await?;
            let runtime_endpoint = service_endpoint(selected.plan(), &observation)?;
            let endpoint = self
                .readiness
                .bind_tool_service(
                    intent,
                    surface,
                    selected.plan(),
                    &observation,
                    &runtime_endpoint,
                    &provisioning.lifecycle_idempotency_key,
                )
                .await?;
            provisioning
                .record_gateway_readiness(endpoint, RuntimeServiceReadinessEvidence::HttpHealthy)?;
            self.store.put_provisioning(&provisioning).await?;
        }
        self.commit_service_provisioning(intent, selected, provisioning)
            .await
    }

    pub(super) async fn provision_mcp_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        selected: &SelectedRuntimeSurface,
        idempotency_key: &str,
    ) -> UseResult<RuntimeBindingReceipt> {
        let provisioning = self
            .begin_service_provisioning(selected, "apply-mcp", idempotency_key)
            .await?;
        self.advance_mcp_provisioning(intent, surface, selected, provisioning)
            .await
    }

    async fn advance_mcp_provisioning(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        selected: &SelectedRuntimeSurface,
        mut provisioning: RuntimeServiceProvisioningReceipt,
    ) -> UseResult<RuntimeBindingReceipt> {
        if provisioning.phase != RuntimeServiceProvisioningPhase::GatewayReady {
            let activation = selected
                .client()
                .apply_service(
                    selected.plan(),
                    selected.provider(),
                    provisioning.apply_request_id.clone(),
                    self.deadline_at_ms,
                )
                .await?;
            let observation = activation.observation().clone();
            provisioning.record_runtime_observation(
                selected.plan(),
                selected.provider(),
                observation.clone(),
            )?;
            self.store.put_provisioning(&provisioning).await?;
            let runtime_endpoint = service_endpoint(selected.plan(), &observation)?;
            let readiness = self
                .readiness
                .bind_mcp_service(
                    intent,
                    surface,
                    selected.plan(),
                    &observation,
                    &runtime_endpoint,
                    &provisioning.lifecycle_idempotency_key,
                )
                .await?;
            provisioning.record_gateway_readiness(
                readiness.endpoint,
                RuntimeServiceReadinessEvidence::McpInitialized {
                    initialize: readiness.initialize,
                },
            )?;
            self.store.put_provisioning(&provisioning).await?;
        }
        self.commit_service_provisioning(intent, selected, provisioning)
            .await
    }

    pub(super) async fn recover_pending_tool_for_removal(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
    ) -> UseResult<()> {
        let qualified = qualified_surface(intent, PluginSurfaceKind::Tool, &surface.id);
        if self
            .store
            .get_generation(&intent.scope, &qualified, intent.generation)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let Some(provisioning) = self
            .store
            .get_provisioning(&intent.scope, &qualified, intent.generation)
            .await?
        else {
            return Ok(());
        };
        let selected = self.selected(intent, PluginSurfaceKind::Tool, &surface.id)?;
        let apply_request_id = request_id("apply-tool", &provisioning.lifecycle_idempotency_key);
        if !provisioning.matches_plan(
            selected.plan(),
            selected.provider(),
            &provisioning.lifecycle_idempotency_key,
            &apply_request_id,
        )? {
            return Err(runtime_lifecycle_error(
                "use.plugin.runtime_lifecycle_provisioning_mismatch",
                "Candidate rollback found Runtime provisioning evidence for another Tool Service plan.",
            ));
        }
        if provisioning.phase == RuntimeServiceProvisioningPhase::Requested
            && !selected
                .client()
                .provisioning_service_exists(selected.plan(), selected.provider())
                .await?
        {
            self.store.remove_provisioning(&provisioning).await?;
            return Ok(());
        }
        self.advance_tool_provisioning(intent, surface, selected, provisioning)
            .await?;
        Ok(())
    }

    pub(super) async fn recover_pending_mcp_for_removal(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
    ) -> UseResult<()> {
        let qualified = qualified_surface(intent, PluginSurfaceKind::Mcp, &surface.id);
        if self
            .store
            .get_generation(&intent.scope, &qualified, intent.generation)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let Some(provisioning) = self
            .store
            .get_provisioning(&intent.scope, &qualified, intent.generation)
            .await?
        else {
            return Ok(());
        };
        let selected = self.selected(intent, PluginSurfaceKind::Mcp, &surface.id)?;
        let apply_request_id = request_id("apply-mcp", &provisioning.lifecycle_idempotency_key);
        if !provisioning.matches_plan(
            selected.plan(),
            selected.provider(),
            &provisioning.lifecycle_idempotency_key,
            &apply_request_id,
        )? {
            return Err(runtime_lifecycle_error(
                "use.plugin.runtime_lifecycle_provisioning_mismatch",
                "Candidate rollback found Runtime provisioning evidence for another MCP Service plan.",
            ));
        }
        if provisioning.phase == RuntimeServiceProvisioningPhase::Requested
            && !selected
                .client()
                .provisioning_service_exists(selected.plan(), selected.provider())
                .await?
        {
            self.store.remove_provisioning(&provisioning).await?;
            return Ok(());
        }
        self.advance_mcp_provisioning(intent, surface, selected, provisioning)
            .await?;
        Ok(())
    }

    async fn begin_service_provisioning(
        &self,
        selected: &SelectedRuntimeSurface,
        request_label: &str,
        idempotency_key: &str,
    ) -> UseResult<RuntimeServiceProvisioningReceipt> {
        let apply_request_id = request_id(request_label, idempotency_key);
        let expected = RuntimeServiceProvisioningReceipt::from_plan(
            selected.plan(),
            selected.provider(),
            idempotency_key,
            apply_request_id.clone(),
        )?;
        if let Some(current) = self
            .store
            .get_provisioning(&expected.scope, &expected.surface, expected.generation)
            .await?
        {
            if !current.matches_plan(
                selected.plan(),
                selected.provider(),
                idempotency_key,
                &apply_request_id,
            )? {
                return Err(runtime_lifecycle_error(
                    "use.plugin.runtime_lifecycle_provisioning_mismatch",
                    "The retained Runtime Service provisioning evidence does not match the selected lifecycle generation.",
                ));
            }
            return Ok(current);
        }
        self.store.put_provisioning(&expected).await?;
        Ok(expected)
    }

    async fn commit_service_provisioning(
        &self,
        intent: &PluginLifecycleIntent,
        selected: &SelectedRuntimeSurface,
        provisioning: RuntimeServiceProvisioningReceipt,
    ) -> UseResult<RuntimeBindingReceipt> {
        let receipt = RuntimeBindingReceipt::Service(provisioning.binding_receipt()?);
        validate_selected_receipt(intent, selected, &receipt)?;
        self.store
            .commit_provisioning(&provisioning, &receipt)
            .await?;
        Ok(receipt)
    }

    pub(super) async fn reconcile_committed_provisioning(
        &self,
        _intent: &PluginLifecycleIntent,
        selected: &SelectedRuntimeSurface,
        idempotency_key: &str,
        receipt: &RuntimeBindingReceipt,
    ) -> UseResult<()> {
        let Some(provisioning) = self
            .store
            .get_provisioning(receipt.scope(), receipt.surface(), receipt.generation())
            .await?
        else {
            return Ok(());
        };
        let request_label = match selected.plan().contract() {
            RuntimeSurfaceContract::ToolService { .. } => "apply-tool",
            RuntimeSurfaceContract::McpService { .. } => "apply-mcp",
            RuntimeSurfaceContract::ToolTask { .. } => {
                return Err(runtime_lifecycle_error(
                    "use.plugin.runtime_lifecycle_provisioning_mismatch",
                    "A Runtime Task cannot own Service provisioning evidence.",
                ))
            }
        };
        let apply_request_id = request_id(request_label, idempotency_key);
        if !provisioning.matches_plan(
            selected.plan(),
            selected.provider(),
            idempotency_key,
            &apply_request_id,
        )? || provisioning.phase != RuntimeServiceProvisioningPhase::GatewayReady
            || RuntimeBindingReceipt::Service(provisioning.binding_receipt()?) != *receipt
        {
            return Err(runtime_lifecycle_error(
                "use.plugin.runtime_lifecycle_provisioning_mismatch",
                "The final Runtime binding conflicts with retained provisioning evidence.",
            ));
        }
        self.store.remove_provisioning(&provisioning).await?;
        Ok(())
    }
}
