use a3s_use_core::{
    PlanPackageChangeKind, PlanScope, PluginOperationAction, PluginPackageLock, UseResult,
};

use super::package_manager_error;
use super::store::{InstalledPackageGraph, PendingPackageGraphOperation};

pub(super) fn validate_pending_upgrade(
    pending: &PendingPackageGraphOperation,
    candidate_lock: &PluginPackageLock,
    graph: Option<&InstalledPackageGraph>,
    scope: &PlanScope,
) -> UseResult<()> {
    pending.validate()?;
    let prior = pending.prior_package_lock.as_ref().ok_or_else(|| {
        package_manager_error(
            "use.plugin.package_graph_invalid",
            "A pending upgrade omitted its prior dependency lock.",
        )
    })?;
    if pending.envelope.plan.action != PluginOperationAction::Upgrade
        || pending.envelope.package_lock.as_ref() != Some(candidate_lock)
        || pending
            .envelope
            .prior_package_lock
            .as_ref()
            .is_some_and(|bound| bound != prior)
        || pending
            .envelope
            .plan
            .packages
            .iter()
            .any(|transition| transition.change == PlanPackageChangeKind::Remove)
            && pending.envelope.prior_package_lock.as_ref() != Some(prior)
        || &pending.envelope.plan.scope != scope
        || graph.is_none_or(|graph| {
            graph.package_lock != *prior && graph.package_lock != *candidate_lock
        })
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_busy",
            "The pending cognitive-package upgrade no longer matches the resolved or installed graph.",
        ));
    }
    Ok(())
}
