use std::cmp::Ordering;

use a3s_use_core::{
    PlanActor, PlanScope, PluginHostApplyRequest, PluginHostApplyResult, PluginHostCancelRequest,
    PluginHostCancelResult, PluginHostEnablementPlanRequest, PluginHostEnablementPlanResult,
    PluginHostManager, PluginHostObservationRequest, PluginHostObservationResult,
    PluginHostObservationStatus, PluginHostOperationObservationRequest,
    PluginHostOperationObservationResult, PluginHostOperationWatchRequest, PluginHostPlanRequest,
    PluginHostPlanResult, PluginManagerApplyPlanInput, PluginManagerInspectInput,
    PluginManagerInstallPlanInput, PluginManagerListInstalledInput, PluginManagerOperationInput,
    PluginManagerOperationWatchInput, PluginManagerPackageScopeInput, PluginManagerSearchInput,
    PluginManagerToolset, PluginManagerUpgradePlanInput, PluginOperationAction,
    PluginOperationConfirmation, PluginPackageId, PluginPackageLock, PluginReleaseChannel,
    PluginSurfaceRef, UseError, UseResult, VerifiedPluginCatalogRecord,
    PLUGIN_HOST_APPLY_REQUEST_SCHEMA, PLUGIN_HOST_CANCEL_REQUEST_SCHEMA,
    PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA, PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA,
    PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA, PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA,
    PLUGIN_HOST_PLAN_REQUEST_SCHEMA,
};
use a3s_use_extension::PluginCatalogSearch;
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::cognitive_package::{CognitivePackageHostManager, CognitiveRegistryAccess};

use super::model::{
    PluginManagerInstalledPackage, PluginManagerInstalledPage, PluginManagerSearchResult,
};

const MANAGER_SERVICE_ERROR: &str = "use.plugin.manager_service_invalid";
const MAX_STABLE_LIST_ATTEMPTS: usize = 3;

#[derive(Clone)]
pub struct PluginManagerService {
    host: CognitivePackageHostManager,
    assignment_generation: u64,
    capabilities_digest: String,
    scope_digest: String,
}

impl std::fmt::Debug for PluginManagerService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginManagerService")
            .field("host", &self.host)
            .field("assignment_generation", &self.assignment_generation)
            .finish_non_exhaustive()
    }
}

impl PluginManagerService {
    pub fn new(host: CognitivePackageHostManager, assignment_generation: u64) -> UseResult<Self> {
        if assignment_generation == 0 {
            return Err(service_error(
                "The Plugin Manager assignment generation must be positive.",
            ));
        }
        host.managed_scope().validate()?;
        host.host_capabilities().validate()?;
        let capabilities_digest = host.host_capabilities().descriptor_digest()?;
        let scope_digest = host.managed_scope().descriptor_digest()?;
        Ok(Self {
            host,
            assignment_generation,
            capabilities_digest,
            scope_digest,
        })
    }

    pub fn toolset(&self) -> PluginManagerToolset {
        PluginManagerToolset::v5()
    }

    pub fn managed_scope(&self) -> &a3s_use_core::PluginManagedScope {
        self.host.managed_scope()
    }

    pub fn assignment_generation(&self) -> u64 {
        self.assignment_generation
    }

    pub async fn search(
        &self,
        input: PluginManagerSearchInput,
        access: CognitiveRegistryAccess,
    ) -> UseResult<PluginManagerSearchResult> {
        input.validate()?;
        let page_limit = input.page_limit();
        let (selected_registry, catalog_cursor) = input
            .cursor
            .as_deref()
            .map(decode_catalog_cursor)
            .transpose()?
            .map_or((None, None), |(registry, cursor)| {
                (Some(registry), Some(cursor))
            });
        let search = PluginCatalogSearch {
            query: input.query,
            kind: input.kind,
            channel: input.channel,
            publisher: None,
            category: None,
            availability: None,
            cursor: catalog_cursor,
            limit: page_limit,
        };
        let result = self
            .host
            .search_cognitive_packages(access, selected_registry.as_deref(), &search)
            .await?;
        let next_cursors = result
            .next_cursors
            .into_iter()
            .map(|cursor| encode_catalog_cursor(&cursor.registry_name, &cursor.cursor))
            .collect::<UseResult<Vec<_>>>()?;
        Ok(PluginManagerSearchResult {
            source_revision: result.source_revision,
            snapshots: result.snapshots,
            plugins: result.plugins,
            total_matches: result.total_matches,
            next_cursors,
        })
    }

    pub async fn inspect(
        &self,
        input: PluginManagerInspectInput,
        access: CognitiveRegistryAccess,
    ) -> UseResult<a3s_use_extension::PluginCatalogInspection> {
        input.validate()?;
        let search = PluginCatalogSearch {
            query: input.package_id.to_string(),
            kind: None,
            channel: input.channel,
            publisher: None,
            category: None,
            availability: None,
            cursor: None,
            limit: a3s_use_extension::MAX_PLUGIN_CATALOG_PAGE_SIZE,
        };
        let result = self
            .host
            .search_cognitive_packages(access, None, &search)
            .await?;
        let candidate = select_inspection_candidate(
            result.plugins,
            input.package_id.as_str(),
            input.version.as_deref(),
            input.channel,
        )?;
        self.host
            .inspect_cognitive_package(access, &candidate)
            .await
    }

    pub async fn list_installed(
        &self,
        input: PluginManagerListInstalledInput,
    ) -> UseResult<PluginManagerInstalledPage> {
        input.validate()?;
        self.verify_scope(&input.scope())?;
        let packages = self.stable_installed_packages().await?;
        let snapshot_digest = digest_value("a3s.use.plugin-manager-installed-page.v1", &packages)?;
        let start = input
            .cursor
            .as_deref()
            .map(|cursor| decode_list_cursor(cursor, &snapshot_digest, packages.len()))
            .transpose()?
            .unwrap_or(0);
        let end = start.saturating_add(input.page_limit()).min(packages.len());
        let next_cursor = (end < packages.len()).then(|| encode_list_cursor(&snapshot_digest, end));
        Ok(PluginManagerInstalledPage {
            scope: input.scope(),
            snapshot_digest,
            packages: packages[start..end].to_vec(),
            next_cursor,
        })
    }

    pub async fn status(
        &self,
        input: PluginManagerPackageScopeInput,
    ) -> UseResult<PluginHostObservationResult> {
        input.validate()?;
        self.verify_scope(&input.scope())?;
        self.observe(input.package_id).await
    }

    pub async fn plan_install(
        &self,
        input: PluginManagerInstallPlanInput,
        access: CognitiveRegistryAccess,
    ) -> UseResult<PluginHostPlanResult> {
        self.plan_install_checked(input, access, None).await
    }

    pub(crate) async fn plan_install_checked(
        &self,
        input: PluginManagerInstallPlanInput,
        access: CognitiveRegistryAccess,
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<PluginHostPlanResult> {
        input.validate()?;
        self.verify_scope(&input.scope())?;
        let selected_surfaces = input.canonical_surfaces();
        let channel = input.channel.unwrap_or(PluginReleaseChannel::Stable);
        let requirement = input.canonical_version_requirement();
        let lock = self
            .host
            .resolve_cognitive_package_requirement(
                PluginOperationAction::Install,
                access,
                input.registry_name.as_deref(),
                input.package_id.as_str(),
                requirement.as_deref(),
                channel,
                expected_package_lock_digest,
            )
            .await?;
        self.plan_graph(
            PluginOperationAction::Install,
            input.package_id,
            lock,
            selected_surfaces,
            access,
        )
        .await
    }

    pub async fn plan_upgrade(
        &self,
        input: PluginManagerUpgradePlanInput,
        access: CognitiveRegistryAccess,
    ) -> UseResult<PluginHostPlanResult> {
        self.plan_upgrade_checked(input, access, None).await
    }

    pub(crate) async fn plan_upgrade_checked(
        &self,
        input: PluginManagerUpgradePlanInput,
        access: CognitiveRegistryAccess,
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<PluginHostPlanResult> {
        input.validate()?;
        self.verify_scope(&input.scope())?;
        let selected_surfaces = input.canonical_surfaces();
        let installed = self
            .require_installed_lock(input.package_id.as_str())
            .await?;
        let root = require_root_candidate(&installed, input.package_id.as_str())?;
        let channel = input.channel.unwrap_or(root.record.channel);
        let requirement = input.canonical_version_requirement();
        let lock = self
            .host
            .resolve_cognitive_package_requirement(
                PluginOperationAction::Upgrade,
                access,
                Some(&root.provenance.registry_name),
                input.package_id.as_str(),
                requirement.as_deref(),
                channel,
                expected_package_lock_digest,
            )
            .await?;
        self.plan_graph(
            PluginOperationAction::Upgrade,
            input.package_id,
            lock,
            selected_surfaces,
            access,
        )
        .await
    }

    pub async fn plan_uninstall(
        &self,
        input: PluginManagerPackageScopeInput,
    ) -> UseResult<PluginHostPlanResult> {
        input.validate()?;
        self.verify_scope(&input.scope())?;
        let lock = self
            .host
            .cognitive_package_uninstall_plan_lock(input.package_id.as_str())
            .await?
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.package_graph_missing",
                    format!(
                        "Cognitive package '{}' has no installed dependency-lock ownership record.",
                        input.package_id
                    ),
                )
            })?;
        let request = self.graph_plan_request(
            PluginOperationAction::Uninstall,
            input.package_id,
            None,
            lock,
            Vec::new(),
            None,
        )?;
        self.submit_graph_plan(request, CognitiveRegistryAccess::Cached)
            .await
    }

    pub async fn plan_enable(
        &self,
        input: PluginManagerPackageScopeInput,
    ) -> UseResult<PluginHostEnablementPlanResult> {
        self.plan_enablement(input, true).await
    }

    pub async fn plan_disable(
        &self,
        input: PluginManagerPackageScopeInput,
    ) -> UseResult<PluginHostEnablementPlanResult> {
        self.plan_enablement(input, false).await
    }

    pub async fn reviewed_plan(
        &self,
        input: &PluginManagerApplyPlanInput,
    ) -> UseResult<PluginHostPlanResult> {
        input.validate()?;
        let result = self
            .host
            .reviewed_cognitive_package_plan(
                self.host.managed_scope(),
                &input.operation_id,
                &input.plan_digest,
            )
            .await?;
        if result.assignment_generation != self.assignment_generation
            || result.capabilities_digest != self.capabilities_digest
            || result.scope != *self.host.managed_scope()
        {
            return Err(service_error(
                "The reviewed operation does not match the current manager assignment.",
            ));
        }
        Ok(result)
    }

    pub async fn apply_plan(
        &self,
        input: PluginManagerApplyPlanInput,
        confirmation: Option<PluginOperationConfirmation>,
    ) -> UseResult<PluginHostApplyResult> {
        let plan = self.reviewed_plan(&input).await?;
        let request_id = self.request_id(
            "apply",
            &(
                input.operation_id.as_str(),
                input.plan_digest.as_str(),
                confirmation.as_ref(),
            ),
        )?;
        let request = PluginHostApplyRequest {
            schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_string(),
            request_id,
            assignment_generation: self.assignment_generation,
            capabilities_digest: self.capabilities_digest.clone(),
            scope: self.host.managed_scope().clone(),
            package_id: plan.package_id.clone(),
            operation_id: input.operation_id,
            plan_digest: input.plan_digest,
            confirmation,
        };
        request.validate_for_plan(&plan, self.host.host_capabilities())?;
        self.host.apply(request).await
    }

    /// Observe one exact reviewed operation through the typed Host Manager
    /// boundary. The complete identity is required so an operation ID can
    /// never be confused across packages, scopes, or plan generations.
    pub async fn observe_operation(
        &self,
        input: PluginManagerOperationInput,
    ) -> UseResult<PluginHostOperationObservationResult> {
        input.validate()?;
        self.verify_scope(&input.scope())?;
        let request = self.operation_observation_request("observe-operation", &input)?;
        request.validate_for_capabilities(self.host.host_capabilities())?;
        let result = self.host.observe_operation(request.clone()).await?;
        result.validate_for(&request, self.host.host_capabilities())?;
        Ok(result)
    }

    /// Long-poll one exact reviewed operation for a new durable status
    /// revision. A zero timeout is a bounded immediate read.
    pub async fn watch_operation(
        &self,
        input: PluginManagerOperationWatchInput,
    ) -> UseResult<PluginHostOperationObservationResult> {
        input.validate()?;
        self.verify_scope(&input.scope())?;
        let observation = self.operation_observation_request("watch-operation", &input)?;
        let request = PluginHostOperationWatchRequest {
            schema: PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA.to_owned(),
            observation,
            after_revision: input.after_revision,
            timeout_ms: input.timeout_ms,
        };
        request.validate_for_capabilities(self.host.host_capabilities())?;
        let result = self.host.watch_operation(request.clone()).await?;
        result.validate_for(&request.observation, self.host.host_capabilities())?;
        Ok(result)
    }

    /// Request explicit-user cancellation at the Host Manager's durable safe
    /// point. Cancellation is intentionally not inferred from observation or
    /// from an MCP transport disconnect.
    pub async fn cancel_operation(
        &self,
        input: PluginManagerOperationInput,
        confirmation: Option<PluginOperationConfirmation>,
    ) -> UseResult<PluginHostCancelResult> {
        input.validate()?;
        self.verify_scope(&input.scope())?;
        let confirmation = confirmation.ok_or_else(|| {
            UseError::new(
                "use.plugin.plan_confirmation_required",
                "Explicit trusted user confirmation is required before cancelling a plugin operation.",
            )
        })?;
        confirmation.validate()?;
        if confirmation.operation_id != input.operation_id
            || confirmation.plan_digest != input.plan_digest
            || confirmation.confirmed_by != PlanActor::User
        {
            return Err(UseError::new(
                "use.plugin.plan_confirmation_mismatch",
                "User cancellation confirmation does not bind the exact plugin operation.",
            ));
        }
        let request_id = self.request_id("cancel-operation", &input)?;
        let request = PluginHostCancelRequest {
            schema: PLUGIN_HOST_CANCEL_REQUEST_SCHEMA.to_owned(),
            request_id,
            assignment_generation: self.assignment_generation,
            capabilities_digest: self.capabilities_digest.clone(),
            scope: self.host.managed_scope().clone(),
            package_id: input.package_id,
            operation_id: input.operation_id,
            plan_digest: input.plan_digest,
            requested_by: confirmation.confirmed_by,
        };
        request.validate_for_capabilities(self.host.host_capabilities())?;
        let result = self.host.cancel(request.clone()).await?;
        result.validate_for(&request, self.host.host_capabilities())?;
        Ok(result)
    }

    fn operation_observation_request<T: Serialize + OperationIdentity>(
        &self,
        domain: &str,
        input: &T,
    ) -> UseResult<PluginHostOperationObservationRequest> {
        Ok(PluginHostOperationObservationRequest {
            schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
            request_id: self.request_id(domain, input)?,
            assignment_generation: self.assignment_generation,
            capabilities_digest: self.capabilities_digest.clone(),
            scope: self.host.managed_scope().clone(),
            package_id: input.package_id().clone(),
            operation_id: input.operation_id().to_owned(),
            plan_digest: input.plan_digest().to_owned(),
        })
    }

    async fn plan_graph(
        &self,
        action: PluginOperationAction,
        package_id: PluginPackageId,
        lock: PluginPackageLock,
        selected_surfaces: Vec<PluginSurfaceRef>,
        access: CognitiveRegistryAccess,
    ) -> UseResult<PluginHostPlanResult> {
        let candidate = require_root_candidate(&lock, package_id.as_str())?.clone();
        self.host
            .inspect_cognitive_package(access, &candidate)
            .await?;
        let request = self.graph_plan_request(
            action,
            package_id,
            Some(candidate),
            lock,
            selected_surfaces,
            None,
        )?;
        self.submit_graph_plan(request, access).await
    }

    async fn submit_graph_plan(
        &self,
        request: PluginHostPlanRequest,
        access: CognitiveRegistryAccess,
    ) -> UseResult<PluginHostPlanResult> {
        match self
            .host
            .plan_cognitive_package(request.clone(), access)
            .await
        {
            Ok(result) => Ok(result),
            Err(error) if error.code == "use.plugin.host_outcome_stale" => {
                let revision = self.host.current_package_graph_revision().await?;
                let lock = request.package_lock.clone().ok_or_else(|| {
                    service_error("A graph plan retry omitted its exact package lock.")
                })?;
                let retry = self.graph_plan_request(
                    request.action,
                    request.package_id,
                    request.candidate,
                    lock,
                    request.selected_surfaces,
                    Some(&revision),
                )?;
                self.host.plan_cognitive_package(retry, access).await
            }
            Err(error) => Err(error),
        }
    }

    fn graph_plan_request(
        &self,
        action: PluginOperationAction,
        package_id: PluginPackageId,
        candidate: Option<VerifiedPluginCatalogRecord>,
        package_lock: PluginPackageLock,
        selected_surfaces: Vec<PluginSurfaceRef>,
        replay_revision: Option<&(u64, String)>,
    ) -> UseResult<PluginHostPlanRequest> {
        let package_lock_digest = package_lock.descriptor_digest()?;
        let candidate_digest = candidate
            .as_ref()
            .map(|candidate| candidate.provenance.catalog_record_digest.as_str());
        let request_id = match replay_revision {
            Some(revision) => self.request_id(
                "plan-after-stale-outcome",
                &(
                    action,
                    package_id.as_str(),
                    candidate_digest,
                    package_lock_digest.as_str(),
                    &selected_surfaces,
                    revision,
                ),
            )?,
            None => self.request_id(
                "plan",
                &(
                    action,
                    package_id.as_str(),
                    candidate_digest,
                    package_lock_digest.as_str(),
                    &selected_surfaces,
                ),
            )?,
        };
        let request = PluginHostPlanRequest {
            schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_string(),
            request_id,
            assignment_generation: self.assignment_generation,
            capabilities_digest: self.capabilities_digest.clone(),
            scope: self.host.managed_scope().clone(),
            action,
            package_id,
            candidate,
            package_lock: Some(package_lock),
            selected_surfaces,
        };
        request.validate_for_capabilities(self.host.host_capabilities())?;
        Ok(request)
    }

    async fn plan_enablement(
        &self,
        input: PluginManagerPackageScopeInput,
        enabled: bool,
    ) -> UseResult<PluginHostEnablementPlanResult> {
        input.validate()?;
        self.verify_scope(&input.scope())?;
        let observation = self.observe(input.package_id.clone()).await?;
        let state =
            match observation.status {
                PluginHostObservationStatus::Available { state } => state,
                PluginHostObservationStatus::Unavailable { .. } => return Err(service_error(
                    "The Plugin Manager cannot plan enablement while package state is unavailable.",
                )),
            };
        let expected_package_generation = state.package_generation.ok_or_else(|| {
            service_error("The Plugin Manager cannot plan enablement for an absent package.")
        })?;
        let request_id = self.request_id(
            "enablement",
            &(
                input.package_id.as_str(),
                expected_package_generation,
                enabled,
                state.receipt_digest.as_deref(),
                state.capability_generation,
                state.capability_revision.as_str(),
            ),
        )?;
        let request = PluginHostEnablementPlanRequest {
            schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_string(),
            request_id,
            assignment_generation: self.assignment_generation,
            capabilities_digest: self.capabilities_digest.clone(),
            scope: self.host.managed_scope().clone(),
            package_id: input.package_id,
            expected_package_generation,
            enabled,
        };
        request.validate_for_capabilities(self.host.host_capabilities())?;
        self.host.plan_enablement(request).await
    }

    async fn observe(&self, package_id: PluginPackageId) -> UseResult<PluginHostObservationResult> {
        let request_id = self.request_id("observe", &package_id)?;
        let request = PluginHostObservationRequest {
            schema: PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_string(),
            request_id,
            assignment_generation: self.assignment_generation,
            capabilities_digest: self.capabilities_digest.clone(),
            scope: self.host.managed_scope().clone(),
            package_id,
        };
        request.validate_for_capabilities(self.host.host_capabilities())?;
        self.host.observe(request).await
    }

    async fn stable_installed_packages(&self) -> UseResult<Vec<PluginManagerInstalledPackage>> {
        for _ in 0..MAX_STABLE_LIST_ATTEMPTS {
            let package_ids = self.host.installed_cognitive_package_ids().await?;
            let mut packages = Vec::with_capacity(package_ids.len());
            let mut unstable = false;
            for package_id in &package_ids {
                let observation = self
                    .observe(PluginPackageId::parse(package_id.clone())?)
                    .await?;
                let state = match observation.status {
                    PluginHostObservationStatus::Available { state } => state,
                    PluginHostObservationStatus::Unavailable { .. } => {
                        unstable = true;
                        break;
                    }
                };
                packages.push(PluginManagerInstalledPackage {
                    package_id: package_id.clone(),
                    state,
                });
            }
            if unstable {
                continue;
            }
            let stable_ids = self.host.installed_cognitive_package_ids().await?;
            let same_revision = packages.first().is_none_or(|first| {
                packages.iter().all(|package| {
                    package.state.capability_generation == first.state.capability_generation
                        && package.state.capability_revision == first.state.capability_revision
                })
            });
            if package_ids == stable_ids && same_revision {
                return Ok(packages);
            }
        }
        Err(service_error(
            "Installed package state changed during every bounded list attempt.",
        ))
    }

    async fn require_installed_lock(&self, package_id: &str) -> UseResult<PluginPackageLock> {
        self.host
            .installed_cognitive_package_lock(package_id)
            .await?
            .ok_or_else(|| {
                UseError::new(
                    "use.extension.not_installed",
                    format!("Cognitive package '{package_id}' is not installed."),
                )
            })
    }

    pub(super) fn verify_scope(&self, scope: &PlanScope) -> UseResult<()> {
        if scope != &self.host.managed_scope().plan_scope() {
            return Err(UseError::new(
                "use.plugin.manager_scope_mismatch",
                "The Plugin Manager request belongs to a different managed scope.",
            ));
        }
        Ok(())
    }

    pub(super) fn request_id<T: Serialize>(&self, domain: &str, payload: &T) -> UseResult<String> {
        let encoded = serde_json::to_vec(&(
            "a3s.use.plugin-manager-request.v1",
            domain,
            self.assignment_generation,
            self.capabilities_digest.as_str(),
            self.scope_digest.as_str(),
            payload,
        ))
        .map_err(|error| {
            service_error(format!(
                "Failed to encode Plugin Manager request identity: {error}"
            ))
        })?;
        Ok(format!("manager:{:x}", Sha256::digest(encoded)))
    }
}

fn require_root_candidate<'a>(
    lock: &'a PluginPackageLock,
    package_id: &str,
) -> UseResult<&'a VerifiedPluginCatalogRecord> {
    lock.package(package_id)
        .map(|package| &package.catalog)
        .ok_or_else(|| {
            service_error("The resolved package lock omitted its selected root catalog record.")
        })
}

trait OperationIdentity {
    fn package_id(&self) -> &PluginPackageId;
    fn operation_id(&self) -> &str;
    fn plan_digest(&self) -> &str;
}

impl OperationIdentity for PluginManagerOperationInput {
    fn package_id(&self) -> &PluginPackageId {
        &self.package_id
    }

    fn operation_id(&self) -> &str {
        &self.operation_id
    }

    fn plan_digest(&self) -> &str {
        &self.plan_digest
    }
}

impl OperationIdentity for PluginManagerOperationWatchInput {
    fn package_id(&self) -> &PluginPackageId {
        &self.package_id
    }

    fn operation_id(&self) -> &str {
        &self.operation_id
    }

    fn plan_digest(&self) -> &str {
        &self.plan_digest
    }
}

fn select_inspection_candidate(
    candidates: Vec<VerifiedPluginCatalogRecord>,
    package_id: &str,
    version: Option<&str>,
    channel: Option<PluginReleaseChannel>,
) -> UseResult<VerifiedPluginCatalogRecord> {
    let mut candidates = candidates
        .into_iter()
        .filter(|candidate| {
            candidate.record.package_id == package_id
                && version.is_none_or(|version| candidate.record.version == version)
                && channel.is_none_or(|channel| candidate.record.channel == channel)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(compare_inspection_candidate);
    let selected = candidates.first().cloned().ok_or_else(|| {
        UseError::new(
            "use.extension.catalog_package_missing",
            format!("No enabled Registry has a matching '{package_id}' release."),
        )
    })?;
    if candidates.get(1).is_some_and(|candidate| {
        candidate.record.version == selected.record.version
            && candidate.record.channel == selected.record.channel
            && candidate.provenance.catalog_record_digest
                != selected.provenance.catalog_record_digest
    }) {
        return Err(UseError::new(
            "use.plugin.manager_catalog_ambiguous",
            "More than one enabled Registry supplies a conflicting selected plugin release.",
        ));
    }
    Ok(selected)
}

fn compare_inspection_candidate(
    left: &VerifiedPluginCatalogRecord,
    right: &VerifiedPluginCatalogRecord,
) -> Ordering {
    Version::parse(&right.record.version)
        .ok()
        .cmp(&Version::parse(&left.record.version).ok())
        .then_with(|| channel_rank(left.record.channel).cmp(&channel_rank(right.record.channel)))
        .then_with(|| {
            left.provenance
                .catalog_record_digest
                .cmp(&right.provenance.catalog_record_digest)
        })
}

const fn channel_rank(channel: PluginReleaseChannel) -> u8 {
    match channel {
        PluginReleaseChannel::Stable => 0,
        PluginReleaseChannel::Beta => 1,
        PluginReleaseChannel::Nightly => 2,
    }
}

fn encode_catalog_cursor(registry_name: &str, cursor: &str) -> UseResult<String> {
    if !valid_registry_name(registry_name)
        || cursor.is_empty()
        || cursor.chars().any(char::is_control)
    {
        return Err(service_error(
            "The Registry catalog returned an invalid page cursor.",
        ));
    }
    let encoded = format!("v1.{registry_name}.{cursor}");
    if encoded.len() > 512 {
        return Err(service_error(
            "The Registry catalog cursor exceeds the manager contract bound.",
        ));
    }
    Ok(encoded)
}

fn decode_catalog_cursor(cursor: &str) -> UseResult<(String, String)> {
    let mut parts = cursor.splitn(3, '.');
    let version = parts.next();
    let registry_name = parts.next();
    let catalog_cursor = parts.next();
    if version != Some("v1")
        || registry_name.is_none_or(|name| !valid_registry_name(name))
        || catalog_cursor.is_none_or(str::is_empty)
    {
        return Err(UseError::new(
            "use.plugin.manager_cursor_invalid",
            "The Plugin Manager catalog cursor is malformed.",
        ));
    }
    Ok((
        registry_name.unwrap_or_default().to_string(),
        catalog_cursor.unwrap_or_default().to_string(),
    ))
}

pub(super) fn encode_list_cursor(snapshot_digest: &str, offset: usize) -> String {
    format!(
        "v1.{}.{}",
        snapshot_digest
            .strip_prefix("sha256:")
            .unwrap_or(snapshot_digest),
        offset
    )
}

pub(super) fn decode_list_cursor(
    cursor: &str,
    snapshot_digest: &str,
    count: usize,
) -> UseResult<usize> {
    let mut parts = cursor.split('.');
    let version = parts.next();
    let digest = parts.next();
    let offset = parts.next();
    if version != Some("v1") || parts.next().is_some() {
        return Err(list_cursor_invalid());
    }
    let expected = snapshot_digest
        .strip_prefix("sha256:")
        .unwrap_or(snapshot_digest);
    if digest != Some(expected) {
        return Err(UseError::new(
            "use.plugin.manager_cursor_stale",
            "Installed package state changed; restart pagination.",
        ));
    }
    offset
        .and_then(|offset| offset.parse::<usize>().ok())
        .filter(|offset| *offset <= count)
        .ok_or_else(list_cursor_invalid)
}

fn digest_value<T: Serialize>(domain: &str, value: &T) -> UseResult<String> {
    let encoded = serde_json::to_vec(&(domain, value)).map_err(|error| {
        service_error(format!("Failed to encode Plugin Manager state: {error}"))
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(encoded)))
}

fn valid_registry_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn list_cursor_invalid() -> UseError {
    UseError::new(
        "use.plugin.manager_cursor_invalid",
        "The installed package cursor is malformed.",
    )
}

fn service_error(message: impl Into<String>) -> UseError {
    UseError::new(MANAGER_SERVICE_ERROR, message)
}
