use super::*;

pub(super) struct StaticRuntimeFactory {
    pub(super) provider_id: ProviderId,
    pub(super) client: Arc<dyn RuntimeClient>,
}

#[async_trait]
impl RuntimeProviderFactory for StaticRuntimeFactory {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn create(&self) -> RuntimeResult<Arc<dyn RuntimeClient>> {
        Ok(self.client.clone())
    }
}

pub(super) struct FakeRuntime {
    capabilities: RuntimeCapabilities,
    pub(super) observation: Mutex<Option<RuntimeObservation>>,
    apply_receipts: Mutex<BTreeMap<String, (String, RuntimeObservation)>>,
    pub(super) apply_count: AtomicUsize,
    pub(super) stop_count: AtomicUsize,
    pub(super) remove_count: AtomicUsize,
}

impl FakeRuntime {
    pub(super) fn new(capabilities: RuntimeCapabilities) -> Self {
        Self {
            capabilities,
            observation: Mutex::new(None),
            apply_receipts: Mutex::new(BTreeMap::new()),
            apply_count: AtomicUsize::new(0),
            stop_count: AtomicUsize::new(0),
            remove_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl RuntimeClient for FakeRuntime {
    async fn capabilities(&self) -> RuntimeResult<RuntimeCapabilities> {
        Ok(self.capabilities.clone())
    }

    async fn apply(&self, request: &RuntimeApplyRequest) -> RuntimeResult<RuntimeObservation> {
        let spec_digest = request.spec.digest().map_err(RuntimeError::Protocol)?;
        if let Some((retained_digest, observation)) = self
            .apply_receipts
            .lock()
            .unwrap()
            .get(&request.request_id)
            .cloned()
        {
            if retained_digest != spec_digest {
                return Err(RuntimeError::Protocol(
                    "test Runtime apply request identity was reused for another spec".to_string(),
                ));
            }
            return Ok(observation);
        }
        let apply_number = self.apply_count.fetch_add(1, Ordering::SeqCst) + 1;
        let port = request.spec.network.ports.first().ok_or_else(|| {
            RuntimeError::Protocol("test Runtime Service omitted its declared port".to_string())
        })?;
        let mut claims = BTreeMap::new();
        RuntimeServiceEndpoint::node_local_tcp(&port.name, 31_337 + apply_number as u16)
            .map_err(RuntimeError::Protocol)?
            .insert_claim(&mut claims)
            .map_err(RuntimeError::Protocol)?;
        let observed_at_ms = 1_000 * apply_number as u64;
        let observation = RuntimeObservation {
            schema: RuntimeObservation::SCHEMA.to_string(),
            unit_id: request.spec.unit_id.clone(),
            generation: request.spec.generation,
            spec_digest: spec_digest.clone(),
            class: request.spec.class,
            state: RuntimeUnitState::Running,
            provider_resource_id: Some(format!("resource-{apply_number:02}")),
            provider_build: Some(self.capabilities.provider_build.clone()),
            observed_at_ms,
            started_at_ms: Some(observed_at_ms - 100),
            finished_at_ms: None,
            health: Some(RuntimeHealthObservation {
                state: RuntimeHealthState::Healthy,
                checked_at_ms: observed_at_ms,
                message: None,
            }),
            outputs: Vec::new(),
            usage: None,
            evidence: Some(RuntimeEvidence {
                provider_build: self.capabilities.provider_build.clone(),
                spec_digest: spec_digest.clone(),
                semantics_profile_digest: request.spec.semantics_profile_digest.clone(),
                claims,
            }),
            provider_attestation: None,
            failure: None,
        };
        *self.observation.lock().unwrap() = Some(observation.clone());
        self.apply_receipts.lock().unwrap().insert(
            request.request_id.clone(),
            (spec_digest, observation.clone()),
        );
        Ok(observation)
    }

    async fn inspect(&self, unit_id: &str) -> RuntimeResult<RuntimeInspection> {
        Ok(match self.observation.lock().unwrap().clone() {
            Some(observation) if observation.unit_id == unit_id => RuntimeInspection::Found {
                schema: RuntimeInspection::SCHEMA.to_string(),
                observation: Box::new(observation),
            },
            _ => RuntimeInspection::NotFound {
                schema: RuntimeInspection::SCHEMA.to_string(),
                unit_id: unit_id.to_string(),
                last_generation: None,
            },
        })
    }

    async fn stop(&self, request: &RuntimeActionRequest) -> RuntimeResult<RuntimeInspection> {
        self.stop_count.fetch_add(1, Ordering::SeqCst);
        let mut current = self.observation.lock().unwrap();
        let Some(observation) = current.as_mut() else {
            return Ok(RuntimeInspection::NotFound {
                schema: RuntimeInspection::SCHEMA.to_string(),
                unit_id: request.unit_id.clone(),
                last_generation: None,
            });
        };
        let stopped_at_ms = observation.observed_at_ms + 100;
        observation.state = RuntimeUnitState::Stopped;
        observation.observed_at_ms = stopped_at_ms;
        observation.finished_at_ms = Some(stopped_at_ms);
        observation.clear_service_endpoints();
        Ok(RuntimeInspection::Found {
            schema: RuntimeInspection::SCHEMA.to_string(),
            observation: Box::new(observation.clone()),
        })
    }

    async fn remove(&self, request: &RuntimeActionRequest) -> RuntimeResult<RuntimeRemoval> {
        self.remove_count.fetch_add(1, Ordering::SeqCst);
        let already_absent = self.observation.lock().unwrap().take().is_none();
        Ok(RuntimeRemoval {
            schema: RuntimeRemoval::SCHEMA.to_string(),
            request_id: request.request_id.clone(),
            unit_id: request.unit_id.clone(),
            generation: request.generation,
            removed_at_ms: 1_200,
            already_absent,
        })
    }

    async fn logs(&self, _query: &RuntimeLogQuery) -> RuntimeResult<Vec<RuntimeLogChunk>> {
        Ok(Vec::new())
    }

    async fn exec(&self, _request: &RuntimeExecRequest) -> RuntimeResult<RuntimeExecResult> {
        Err(RuntimeError::Protocol("unexpected exec".to_string()))
    }
}

pub(super) async fn selection(
    plans: Vec<RuntimeSurfacePlan>,
    tool: Arc<FakeRuntime>,
    mcp: Arc<FakeRuntime>,
) -> (RuntimeProviderSelection, Arc<RuntimeClientRegistry>) {
    let mut registry = RuntimeClientRegistry::new();
    let providers: [(&str, Arc<dyn RuntimeClient>); 2] =
        [("tool-runtime", tool), ("mcp-runtime", mcp)];
    for (provider, client) in providers {
        registry
            .register(Arc::new(StaticRuntimeFactory {
                provider_id: ProviderId::parse(provider).unwrap(),
                client,
            }))
            .unwrap();
    }
    let assignments = plans
        .iter()
        .map(|plan| {
            let provider = match plan.context().surface().kind {
                PluginSurfaceKind::Tool => "tool-runtime",
                PluginSurfaceKind::Mcp => "mcp-runtime",
                _ => unreachable!(),
            };
            RuntimeProviderAssignment::new(plan.surface(), provider).unwrap()
        })
        .collect();
    let registry = Arc::new(registry);
    let selection = RuntimeProviderSelector::new(&registry)
        .select(plans, assignments)
        .await
        .unwrap();
    (selection, registry)
}
