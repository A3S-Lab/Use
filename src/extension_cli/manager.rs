use std::sync::Arc;

use a3s_use_core::{
    PlanPackageChangeKind, PlanPolicyDecision, PlanScopeKind, PluginHostApplyResult,
    PluginHostPlanResult, PluginManagedScope, PluginManagerApplyPlanInput,
    PluginManagerInstallPlanInput, PluginManagerPackageScopeInput, PluginManagerUpgradePlanInput,
    PluginOperationAction, PluginPackageId, PluginPackageLock, PluginReleaseChannel, UseError,
    UseResult, PLUGIN_MANAGED_SCOPE_SCHEMA_V2,
};
use a3s_use_extension::{ExtensionRegistry, RegistrySourceStore};

use crate::cognitive_package::{
    verify_expected_lock, CognitivePackageHostManager, CognitiveRegistryAccess,
    StandaloneCognitivePackageAuthorizationProvider, StandaloneCognitivePackageLifecycleFactory,
};
use crate::plugin_manager::PluginManagerService;
use crate::COGNITIVE_PACKAGE_DEFAULT_SCOPE;

use super::PluginManagerMutationView;

const STANDALONE_ASSIGNMENT_GENERATION: u64 = 1;
const STANDALONE_FENCE_DIGEST: &str =
    "sha256:ca77efcea9662c63ece2b19809d29816580a764bb9f1ec88331b350108888d0a";

pub(super) struct AppliedGraphMutation {
    pub manager: PluginManagerMutationView,
    pub package_graph: serde_json::Value,
    pub registry_access: &'static str,
    pub registry_source_revision: String,
}

pub(super) async fn install(
    package_id: &str,
    registry_name: Option<&str>,
    version_requirement: Option<&str>,
    channel: Option<&str>,
    expected_package_lock_digest: Option<&str>,
    offline: bool,
) -> UseResult<AppliedGraphMutation> {
    let source_store = RegistrySourceStore::from_env()?;
    let source_revision = source_store.snapshot().await?.revision;
    let service = standalone_service()?;
    let plan = service
        .plan_install_checked(
            PluginManagerInstallPlanInput {
                package_id: PluginPackageId::parse(package_id)?,
                registry_name: registry_name.map(str::to_owned),
                version_requirement: version_requirement.map(str::to_owned),
                channel: channel.map(parse_channel).transpose()?,
                surfaces: None,
                scope_kind: PlanScopeKind::User,
                scope_id: COGNITIVE_PACKAGE_DEFAULT_SCOPE.to_owned(),
            },
            registry_access(offline),
            expected_package_lock_digest,
        )
        .await?;
    verify_plan_lock(&plan, expected_package_lock_digest)?;
    verify_source_revision(&source_store, &source_revision).await?;
    let result = apply(&service, &plan).await?;
    let package_graph = install_graph(&plan, &result).await?;
    Ok(AppliedGraphMutation {
        manager: PluginManagerMutationView::new(plan, result)?,
        package_graph,
        registry_access: access_name(offline),
        registry_source_revision: source_revision,
    })
}

pub(super) async fn upgrade(
    package_id: &str,
    expected_registry_name: Option<&str>,
    version_requirement: Option<&str>,
    channel: Option<&str>,
    expected_package_lock_digest: Option<&str>,
    offline: bool,
) -> UseResult<AppliedGraphMutation> {
    let source_store = RegistrySourceStore::from_env()?;
    let source_revision = source_store.snapshot().await?.revision;
    let service = standalone_service()?;
    let plan = service
        .plan_upgrade_checked(
            PluginManagerUpgradePlanInput {
                package_id: PluginPackageId::parse(package_id)?,
                version_requirement: version_requirement.map(str::to_owned),
                channel: channel.map(parse_channel).transpose()?,
                surfaces: None,
                scope_kind: PlanScopeKind::User,
                scope_id: COGNITIVE_PACKAGE_DEFAULT_SCOPE.to_owned(),
            },
            registry_access(offline),
            expected_package_lock_digest,
        )
        .await?;
    verify_upgrade_registry(&plan, expected_registry_name)?;
    verify_plan_lock(&plan, expected_package_lock_digest)?;
    verify_source_revision(&source_store, &source_revision).await?;
    let result = apply(&service, &plan).await?;
    let package_graph = upgrade_graph(&plan, &result).await?;
    Ok(AppliedGraphMutation {
        manager: PluginManagerMutationView::new(plan, result)?,
        package_graph,
        registry_access: access_name(offline),
        registry_source_revision: source_revision,
    })
}

pub(super) async fn uninstall(package_id: &str) -> UseResult<AppliedGraphMutation> {
    let service = standalone_service()?;
    let plan = service
        .plan_uninstall(PluginManagerPackageScopeInput {
            package_id: PluginPackageId::parse(package_id)?,
            scope_kind: PlanScopeKind::User,
            scope_id: COGNITIVE_PACKAGE_DEFAULT_SCOPE.to_owned(),
        })
        .await?;
    let result = apply(&service, &plan).await?;
    let package_graph = uninstall_graph(&plan, &result)?;
    Ok(AppliedGraphMutation {
        manager: PluginManagerMutationView::new(plan, result)?,
        package_graph,
        registry_access: "installed",
        registry_source_revision: String::new(),
    })
}

fn standalone_service() -> UseResult<PluginManagerService> {
    let host = CognitivePackageHostManager::new(
        standalone_scope(),
        format!("standalone:{}", env!("CARGO_PKG_VERSION")),
        ExtensionRegistry::from_env()?,
        Arc::new(StandaloneCognitivePackageLifecycleFactory::from_env()?),
        Arc::new(StandaloneCognitivePackageAuthorizationProvider),
    )?;
    PluginManagerService::new(host, STANDALONE_ASSIGNMENT_GENERATION)
}

fn standalone_scope() -> PluginManagedScope {
    PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:a3s-use-standalone".to_owned(),
        scope_kind: PlanScopeKind::User,
        scope_id: COGNITIVE_PACKAGE_DEFAULT_SCOPE.to_owned(),
        authority_id: "user:current".to_owned(),
        fence_generation: STANDALONE_ASSIGNMENT_GENERATION,
        fence_digest: STANDALONE_FENCE_DIGEST.to_owned(),
    }
}

async fn apply(
    service: &PluginManagerService,
    plan: &PluginHostPlanResult,
) -> UseResult<PluginHostApplyResult> {
    match service
        .apply_plan(
            PluginManagerApplyPlanInput {
                operation_id: plan.plan.plan.operation_id.clone(),
                plan_digest: plan.plan.plan_digest.clone(),
            },
            None,
        )
        .await
    {
        Err(error)
            if error.code == "use.plugin.plan_confirmation_mismatch"
                && plan.plan.plan.authority.decision == PlanPolicyDecision::Ask =>
        {
            Err(UseError::new(
                "use.plugin.package_confirmation_required",
                "The cognitive-package plan requests ambient authority and requires exact user confirmation.",
            )
            .with_detail("operationId", plan.plan.plan.operation_id.clone())
            .with_detail("planDigest", plan.plan.plan_digest.clone())
            .with_detail(
                "plan",
                serde_json::to_value(&plan.plan).unwrap_or_default(),
            )
            .with_suggestion(
                "Review the immutable plan through a trusted host and apply it with an injected authorization provider.",
            ))
        }
        result => result,
    }
}

fn verify_plan_lock(
    plan: &PluginHostPlanResult,
    expected_package_lock_digest: Option<&str>,
) -> UseResult<()> {
    let actual = plan
        .plan
        .plan
        .package_lock_digest
        .as_deref()
        .ok_or_else(|| {
            manager_error("The reviewed Plugin Manager plan omitted its package lock.")
        })?;
    verify_expected_lock(actual, expected_package_lock_digest)
}

fn verify_upgrade_registry(
    plan: &PluginHostPlanResult,
    expected_registry_name: Option<&str>,
) -> UseResult<()> {
    let Some(expected) = expected_registry_name else {
        return Ok(());
    };
    let selected = root_lock(plan)?
        .package(plan.package_id.as_str())
        .ok_or_else(|| manager_error("The reviewed upgrade lock omitted its root package."))?;
    if selected.catalog.provenance.registry_name != expected {
        return Err(UseError::new(
            "use.plugin.manager_registry_mismatch",
            "The requested upgrade Registry differs from the installed package provenance.",
        )
        .with_detail("expected", expected)
        .with_detail("actual", selected.catalog.provenance.registry_name.as_str()));
    }
    Ok(())
}

async fn verify_source_revision(
    source_store: &RegistrySourceStore,
    expected: &str,
) -> UseResult<()> {
    let actual = source_store.snapshot().await?.revision;
    if actual != expected {
        return Err(UseError::new(
            "use.plugin.manager_registry_revision_changed",
            "Registry source configuration changed while the CLI reviewed the plugin plan.",
        )
        .with_detail("expected", expected)
        .with_detail("actual", actual));
    }
    Ok(())
}

async fn install_graph(
    plan: &PluginHostPlanResult,
    result: &PluginHostApplyResult,
) -> UseResult<serde_json::Value> {
    require_action(plan, PluginOperationAction::Install)?;
    let lock = root_lock(plan)?;
    let root = ExtensionRegistry::from_env()?
        .get(plan.package_id.as_str())
        .await?
        .ok_or_else(|| {
            manager_error("The installed root is missing after Plugin Manager apply.")
        })?;
    let (installed_packages, retained_packages, legacy_plan) = if result.replayed {
        (
            Vec::new(),
            install_order_for(lock, &plan.plan.plan.packages, None)?,
            None,
        )
    } else {
        (
            install_order_for(
                lock,
                &plan.plan.plan.packages,
                Some(PlanPackageChangeKind::Add),
            )?,
            install_order_for(
                lock,
                &plan.plan.plan.packages,
                Some(PlanPackageChangeKind::Retain),
            )?,
            Some(&plan.plan),
        )
    };
    Ok(serde_json::json!({
        "changed": !result.replayed,
        "root": root,
        "packageLock": lock,
        "packageLockDigest": plan.plan.plan.package_lock_digest,
        "plan": legacy_plan,
        "installedPackages": installed_packages,
        "retainedPackages": retained_packages
    }))
}

async fn upgrade_graph(
    plan: &PluginHostPlanResult,
    result: &PluginHostApplyResult,
) -> UseResult<serde_json::Value> {
    require_action(plan, PluginOperationAction::Upgrade)?;
    let lock = root_lock(plan)?;
    let prior = plan.plan.prior_package_lock.as_ref().ok_or_else(|| {
        manager_error("The reviewed upgrade plan omitted its prior package lock.")
    })?;
    let root = ExtensionRegistry::from_env()?
        .get(plan.package_id.as_str())
        .await?
        .ok_or_else(|| manager_error("The upgraded root is missing after Plugin Manager apply."))?;
    let (added, replaced, removed, retained, legacy_plan) = if result.replayed {
        (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            install_order_for(lock, &plan.plan.plan.packages, None)?,
            None,
        )
    } else {
        (
            install_order_for(
                lock,
                &plan.plan.plan.packages,
                Some(PlanPackageChangeKind::Add),
            )?,
            install_order_for(
                lock,
                &plan.plan.plan.packages,
                Some(PlanPackageChangeKind::Replace),
            )?,
            removal_order_for(
                prior,
                &plan.plan.plan.packages,
                Some(PlanPackageChangeKind::Remove),
            )?,
            sorted_plan_packages(
                &plan.plan.plan.packages,
                Some(PlanPackageChangeKind::Retain),
            ),
            Some(&plan.plan),
        )
    };
    Ok(serde_json::json!({
        "changed": !result.replayed,
        "root": root,
        "priorPackageLock": prior,
        "packageLock": lock,
        "packageLockDigest": plan.plan.plan.package_lock_digest,
        "plan": legacy_plan,
        "addedPackages": added,
        "replacedPackages": replaced,
        "removedPackages": removed,
        "retainedPackages": retained
    }))
}

fn uninstall_graph(
    plan: &PluginHostPlanResult,
    result: &PluginHostApplyResult,
) -> UseResult<serde_json::Value> {
    require_action(plan, PluginOperationAction::Uninstall)?;
    let lock = root_lock(plan)?;
    let removed = if result.replayed {
        Vec::new()
    } else {
        removal_order_for(
            lock,
            &plan.plan.plan.packages,
            Some(PlanPackageChangeKind::Remove),
        )?
    };
    let retained = install_order_for(
        lock,
        &plan.plan.plan.packages,
        Some(PlanPackageChangeKind::Retain),
    )?;
    Ok(serde_json::json!({
        "changed": !result.replayed,
        "rootPackageId": plan.package_id,
        "packageLock": lock,
        "packageLockDigest": plan.plan.plan.package_lock_digest,
        "plan": (!result.replayed).then_some(&plan.plan),
        "removedPackages": removed,
        "retainedPackages": retained
    }))
}

fn root_lock(plan: &PluginHostPlanResult) -> UseResult<&PluginPackageLock> {
    plan.plan
        .package_lock
        .as_ref()
        .ok_or_else(|| manager_error("The reviewed Plugin Manager plan omitted its package lock."))
}

fn install_order_for(
    lock: &PluginPackageLock,
    transitions: &[a3s_use_core::PlannedPackageTransition],
    change: Option<PlanPackageChangeKind>,
) -> UseResult<Vec<String>> {
    Ok(lock
        .install_order()?
        .into_iter()
        .filter(|package| transition_matches(transitions, package.package_id(), change))
        .map(|package| package.package_id().to_owned())
        .collect())
}

fn removal_order_for(
    lock: &PluginPackageLock,
    transitions: &[a3s_use_core::PlannedPackageTransition],
    change: Option<PlanPackageChangeKind>,
) -> UseResult<Vec<String>> {
    Ok(lock
        .removal_order()?
        .into_iter()
        .filter(|package| transition_matches(transitions, package.package_id(), change))
        .map(|package| package.package_id().to_owned())
        .collect())
}

fn sorted_plan_packages(
    transitions: &[a3s_use_core::PlannedPackageTransition],
    change: Option<PlanPackageChangeKind>,
) -> Vec<String> {
    transitions
        .iter()
        .filter(|transition| change.is_none_or(|change| transition.change == change))
        .map(|transition| transition.package_id.clone())
        .collect()
}

fn transition_matches(
    transitions: &[a3s_use_core::PlannedPackageTransition],
    package_id: &str,
    change: Option<PlanPackageChangeKind>,
) -> bool {
    transitions.iter().any(|transition| {
        transition.package_id == package_id
            && change.is_none_or(|change| transition.change == change)
    })
}

fn require_action(plan: &PluginHostPlanResult, expected: PluginOperationAction) -> UseResult<()> {
    if plan.plan.plan.action != expected {
        return Err(manager_error(
            "The Plugin Manager returned a different action than the CLI requested.",
        ));
    }
    Ok(())
}

fn parse_channel(channel: &str) -> UseResult<PluginReleaseChannel> {
    match channel {
        "stable" => Ok(PluginReleaseChannel::Stable),
        "beta" => Ok(PluginReleaseChannel::Beta),
        "nightly" => Ok(PluginReleaseChannel::Nightly),
        _ => Err(UseError::new(
            "use.extension.channel_invalid",
            "The cognitive-package Registry channel is invalid.",
        )),
    }
}

const fn registry_access(offline: bool) -> CognitiveRegistryAccess {
    if offline {
        CognitiveRegistryAccess::Cached
    } else {
        CognitiveRegistryAccess::Refreshed
    }
}

const fn access_name(offline: bool) -> &'static str {
    if offline {
        "cached"
    } else {
        "refreshed"
    }
}

fn manager_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.manager_cli_invalid", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_scope_preserves_the_existing_cli_scope() {
        let scope = standalone_scope();
        scope.validate().unwrap();
        assert_eq!(scope.scope_kind, PlanScopeKind::User);
        assert_eq!(scope.scope_id, COGNITIVE_PACKAGE_DEFAULT_SCOPE);
        assert_eq!(scope.plan_scope().id, "user/current");
    }
}
