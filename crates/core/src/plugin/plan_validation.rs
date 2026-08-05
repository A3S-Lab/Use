use std::collections::BTreeSet;

use crate::UseResult;

use super::plan::{
    plan_error, PlanActor, PlanEnforcementProfile, PlanPackageChangeKind, PlanPackageRole,
    PlanPolicyDecision, PlanQualifiedSurfaceRef, PlanScopeKind, PlannedOkfSurfaceChange,
    PlannedPackageState, PlannedProviderEvidence, PlannedSecretChange, PlannedSecretChangeKind,
    PluginOperationAction, PluginOperationPlan, PluginPlanSource, SurfaceChangeKind,
    MAX_PLAN_LIFETIME_MS,
};
use super::validation::{
    strictly_sorted_unique, valid_machine_id, valid_package_id, valid_permission_name, valid_sha256,
};
use super::{
    PluginSurfaceKind, MAX_PLUGIN_PLAN_ITEMS, PLUGIN_OPERATION_PLAN_SCHEMA,
    PLUGIN_OPERATION_PLAN_SCHEMA_V2, PLUGIN_OPERATION_PLAN_SCHEMA_V3,
};

impl PluginOperationPlan {
    pub fn validate(&self) -> UseResult<()> {
        if !matches!(
            self.schema.as_str(),
            PLUGIN_OPERATION_PLAN_SCHEMA
                | PLUGIN_OPERATION_PLAN_SCHEMA_V2
                | PLUGIN_OPERATION_PLAN_SCHEMA_V3
        ) || Self::validate_operation_id(&self.operation_id).is_err()
            || !valid_package_id(&self.package_id)
            || !valid_machine_id(&self.component_id)
            || self
                .package_lock_digest
                .as_deref()
                .is_some_and(|value| !valid_sha256(value))
            || self
                .prior_package_lock_digest
                .as_deref()
                .is_some_and(|value| !valid_sha256(value))
            || self.prior_package_lock_digest.is_some()
                != (self.schema == PLUGIN_OPERATION_PLAN_SCHEMA_V3)
            || self.prior_package_lock_digest.is_some()
                && (self.action != PluginOperationAction::Upgrade
                    || self.package_lock_digest.is_none())
            || self.created_at_ms == 0
            || self.expires_at_ms <= self.created_at_ms
            || self.expires_at_ms - self.created_at_ms > MAX_PLAN_LIFETIME_MS
            || self.packages.is_empty()
            || self.packages.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.secret_changes.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.providers.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.workspace_impacts.len() > MAX_PLUGIN_PLAN_ITEMS
        {
            return Err(plan_error(
                "The plugin operation plan identity, lifetime, or item count is invalid.",
            ));
        }
        self.validate_scope()?;
        self.validate_authority_and_state()?;
        self.validate_packages()?;
        self.validate_secrets()?;
        self.validate_providers()?;
        self.validate_workspace_impacts()?;
        self.validate_impact()
    }

    fn validate_scope(&self) -> UseResult<()> {
        if !valid_machine_id(&self.scope.id) {
            return Err(plan_error("The plugin operation scope is invalid."));
        }
        Ok(())
    }

    fn validate_authority_and_state(&self) -> UseResult<()> {
        if !valid_sha256(&self.authority.policy_digest)
            || self.authority.confirmation_required
                != (self.authority.decision == PlanPolicyDecision::Ask)
            || self.state.state_revision == 0
            || self
                .state
                .receipt_digest
                .as_deref()
                .is_some_and(|value| !valid_sha256(value))
        {
            return Err(plan_error(
                "The plugin plan authority or durable state evidence is invalid.",
            ));
        }
        let receipt_matches_action = match self.action {
            PluginOperationAction::Install => self.state.receipt_digest.is_none(),
            PluginOperationAction::Upgrade | PluginOperationAction::Uninstall => {
                self.state.receipt_digest.is_some()
            }
        };
        if !receipt_matches_action {
            return Err(plan_error(
                "The plugin plan receipt evidence does not match its operation.",
            ));
        }
        Ok(())
    }

    fn validate_packages(&self) -> UseResult<()> {
        if self
            .packages
            .windows(2)
            .any(|pair| pair[0].package_id >= pair[1].package_id)
        {
            return Err(plan_error(
                "Planned packages must be sorted uniquely by package ID.",
            ));
        }
        let mut roots = 0;
        for package in &self.packages {
            package.validate()?;
            if package.role == PlanPackageRole::Root {
                roots += 1;
                if package.package_id != self.package_id
                    || package.change != self.expected_root_change()
                {
                    return Err(plan_error(
                        "The root package transition does not match the requested operation.",
                    ));
                }
            } else if package.package_id == self.package_id {
                return Err(plan_error(
                    "The requested package must be the root plan transition.",
                ));
            }
        }
        if roots != 1 {
            return Err(plan_error(
                "A plugin operation plan must contain exactly one root package.",
            ));
        }
        let has_local_source = self
            .packages
            .iter()
            .any(|package| matches!(package.source, Some(PluginPlanSource::LocalReviewed { .. })));
        if has_local_source
            && (self.authority.actor != PlanActor::User
                || self.authority.decision != PlanPolicyDecision::Ask)
        {
            return Err(plan_error(
                "An unsigned reviewed local source requires an interactive user decision.",
            ));
        }
        Ok(())
    }

    fn expected_root_change(&self) -> PlanPackageChangeKind {
        match self.action {
            PluginOperationAction::Install => PlanPackageChangeKind::Add,
            PluginOperationAction::Upgrade => PlanPackageChangeKind::Replace,
            PluginOperationAction::Uninstall => PlanPackageChangeKind::Remove,
        }
    }

    fn validate_secrets(&self) -> UseResult<()> {
        if !strictly_sorted_unique(&self.secret_changes) {
            return Err(plan_error(
                "Planned secret changes must be sorted and unique.",
            ));
        }
        let expected = planned_secret_changes(&self.packages);
        if self.secret_changes != expected {
            return Err(plan_error(
                "Planned secret changes do not equal the resolved permission delta.",
            ));
        }
        Ok(())
    }

    fn validate_providers(&self) -> UseResult<()> {
        if self.action == PluginOperationAction::Uninstall {
            if !self.providers.is_empty() {
                return Err(plan_error(
                    "Uninstall plans must not select new runtime providers.",
                ));
            }
            return Ok(());
        }

        let mut required = Vec::new();
        for package in &self.packages {
            if let Some(state) = &package.after {
                for surface in &state.release.surfaces {
                    if matches!(
                        surface.kind,
                        PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
                    ) {
                        required.push(PlanQualifiedSurfaceRef {
                            package_id: package.package_id.clone(),
                            surface: surface.reference(),
                        });
                    }
                }
            }
        }
        required.sort();
        if self.providers.len() != required.len() {
            return Err(plan_error(
                "The runtime provider set does not cover the resolved executable surfaces.",
            ));
        }
        for (provider, required_surface) in self.providers.iter().zip(required) {
            provider.validate(self, &required_surface)?;
        }
        Ok(())
    }

    fn validate_workspace_impacts(&self) -> UseResult<()> {
        if self
            .workspace_impacts
            .windows(2)
            .any(|pair| pair[0].scope_id >= pair[1].scope_id)
        {
            return Err(plan_error(
                "Workspace impacts must be sorted uniquely by scope ID.",
            ));
        }
        for impact in &self.workspace_impacts {
            if !valid_machine_id(&impact.scope_id)
                || impact
                    .grant_before_digest
                    .as_deref()
                    .is_some_and(|value| !valid_sha256(value))
                || impact
                    .grant_after_digest
                    .as_deref()
                    .is_some_and(|value| !valid_sha256(value))
                || (impact.grant_before_digest == impact.grant_after_digest
                    && impact.enabled_before == impact.enabled_after)
            {
                return Err(plan_error("A planned workspace impact is invalid."));
            }
        }
        if self.scope.kind == PlanScopeKind::Workspace
            && (self.workspace_impacts.len() != 1
                || self.workspace_impacts[0].scope_id != self.scope.id)
        {
            return Err(plan_error(
                "A workspace-scoped plan must bind exactly its requested workspace.",
            ));
        }
        Ok(())
    }

    fn validate_impact(&self) -> UseResult<()> {
        let valid = match self.action {
            PluginOperationAction::Install => {
                self.impact.download_bytes > 0
                    && self.impact.installed_bytes_after > 0
                    && self.impact.reclaimed_bytes == 0
                    && !self.impact.retained_data
            }
            PluginOperationAction::Upgrade => {
                self.impact.download_bytes > 0
                    && self.impact.installed_bytes_after > 0
                    && !self.impact.retained_data
            }
            PluginOperationAction::Uninstall => {
                self.impact.download_bytes == 0
                    && self.impact.installed_bytes_after == 0
                    && self.impact.retained_data
            }
        };
        let drain_required = self.packages.iter().any(|package| {
            matches!(
                package.change,
                PlanPackageChangeKind::Remove | PlanPackageChangeKind::Replace
            ) && package.before.as_ref().is_some_and(has_private_service)
        });
        let okf_changes = planned_okf_changes(&self.packages)?;
        let schema_matches = if self.prior_package_lock_digest.is_some() {
            self.schema == PLUGIN_OPERATION_PLAN_SCHEMA_V3
        } else if okf_changes.is_empty() {
            self.schema == PLUGIN_OPERATION_PLAN_SCHEMA
        } else {
            self.schema == PLUGIN_OPERATION_PLAN_SCHEMA_V2
        };
        if !valid
            || self.impact.drain_required != drain_required
            || self.impact.okf_changes != okf_changes
            || !schema_matches
        {
            return Err(plan_error(
                "The aggregate plugin operation impact is inconsistent with the package delta.",
            ));
        }
        Ok(())
    }
}

pub(super) fn planned_okf_changes(
    packages: &[super::PlannedPackageTransition],
) -> UseResult<Vec<PlannedOkfSurfaceChange>> {
    let mut before = std::collections::BTreeMap::new();
    let mut after = std::collections::BTreeMap::new();
    for package in packages {
        collect_okf_bundles(&package.package_id, package.before.as_ref(), &mut before)?;
        collect_okf_bundles(&package.package_id, package.after.as_ref(), &mut after)?;
    }
    let references = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::with_capacity(references.len());
    for surface in references {
        let before = before.get(&surface).cloned();
        let after = after.get(&surface).cloned();
        let change = match (before.is_some(), after.is_some()) {
            (false, true) => SurfaceChangeKind::Add,
            (true, false) => SurfaceChangeKind::Remove,
            (true, true) => SurfaceChangeKind::Replace,
            (false, false) => {
                return Err(plan_error(
                    "A planned OKF impact has no before or after bundle contract.",
                ))
            }
        };
        changes.push(PlannedOkfSurfaceChange {
            surface,
            change,
            before,
            after,
        });
    }
    Ok(changes)
}

fn collect_okf_bundles(
    package_id: &str,
    state: Option<&PlannedPackageState>,
    output: &mut std::collections::BTreeMap<PlanQualifiedSurfaceRef, crate::OkfBundleContract>,
) -> UseResult<()> {
    let Some(state) = state else {
        return Ok(());
    };
    for surface in &state.release.surfaces {
        if surface.kind != PluginSurfaceKind::Okf {
            continue;
        }
        let bundle = surface.okf_bundle.clone().ok_or_else(|| {
            plan_error("A planned OKF surface omitted its exact bundle contract.")
        })?;
        bundle
            .validate()
            .map_err(|_| plan_error("A planned OKF bundle contract is invalid."))?;
        output.insert(
            PlanQualifiedSurfaceRef {
                package_id: package_id.to_owned(),
                surface: surface.reference(),
            },
            bundle,
        );
    }
    Ok(())
}

pub(super) fn planned_secret_changes(
    packages: &[super::PlannedPackageTransition],
) -> Vec<PlannedSecretChange> {
    let mut before = BTreeSet::new();
    let mut after = BTreeSet::new();
    for package in packages {
        collect_secrets(&package.package_id, package.before.as_ref(), &mut before);
        collect_secrets(&package.package_id, package.after.as_ref(), &mut after);
    }
    let mut changes = Vec::new();
    for (surface, secret_name) in before.difference(&after) {
        changes.push(PlannedSecretChange {
            surface: surface.clone(),
            secret_name: secret_name.clone(),
            change: PlannedSecretChangeKind::Revoke,
        });
    }
    for (surface, secret_name) in after.difference(&before) {
        changes.push(PlannedSecretChange {
            surface: surface.clone(),
            secret_name: secret_name.clone(),
            change: PlannedSecretChangeKind::Grant,
        });
    }
    changes.sort();
    changes
}

impl PlannedProviderEvidence {
    fn validate(
        &self,
        plan: &PluginOperationPlan,
        required: &PlanQualifiedSurfaceRef,
    ) -> UseResult<()> {
        if &self.surface != required
            || !valid_machine_id(&self.provider_id)
            || !valid_machine_id(&self.provider_build_id)
            || !valid_sha256(&self.capability_digest)
            || !valid_sha256(&self.semantics_profile_digest)
        {
            return Err(plan_error("A selected runtime provider proof is invalid."));
        }
        let permission = permission_for(plan, required).ok_or_else(|| {
            plan_error("A selected runtime provider has no resolved permission ceiling.")
        })?;
        let enforcement_matches = if permission.native_execution {
            matches!(
                self.enforcement,
                PlanEnforcementProfile::Sandbox | PlanEnforcementProfile::NativeUnconfined
            )
        } else {
            self.enforcement == PlanEnforcementProfile::Container
        };
        if !enforcement_matches
            || (plan.authority.actor == PlanActor::Agent
                && plan.authority.decision == PlanPolicyDecision::Allow
                && self.enforcement == PlanEnforcementProfile::NativeUnconfined)
        {
            return Err(plan_error(
                "A runtime provider cannot enforce the resolved execution authority.",
            ));
        }
        Ok(())
    }
}

fn collect_secrets(
    package_id: &str,
    state: Option<&PlannedPackageState>,
    output: &mut BTreeSet<(PlanQualifiedSurfaceRef, String)>,
) {
    if let Some(state) = state {
        for permission in &state.permissions.surfaces {
            for secret_name in &permission.secrets {
                if valid_permission_name(secret_name) {
                    output.insert((
                        PlanQualifiedSurfaceRef {
                            package_id: package_id.to_owned(),
                            surface: permission.surface.clone(),
                        },
                        secret_name.clone(),
                    ));
                }
            }
        }
    }
}

fn permission_for<'a>(
    plan: &'a PluginOperationPlan,
    surface: &PlanQualifiedSurfaceRef,
) -> Option<&'a super::SurfacePermissionCeiling> {
    plan.packages
        .iter()
        .find(|package| package.package_id == surface.package_id)
        .and_then(|package| package.after.as_ref())
        .and_then(|state| {
            state
                .permissions
                .surfaces
                .iter()
                .find(|permission| permission.surface == surface.surface)
        })
}

fn has_private_service(state: &PlannedPackageState) -> bool {
    state
        .permissions
        .surfaces
        .iter()
        .any(|permission| permission.private_service)
}
