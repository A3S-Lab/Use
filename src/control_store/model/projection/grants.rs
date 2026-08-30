use std::collections::BTreeMap;

use a3s_use_core::{
    InstallationSnapshot, PluginWorkspaceGrantSnapshot, UseResult, WorkspaceGrantEvidence,
    PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};

use super::super::{
    validate_grant_selections, ControlGeneration, ControlGrantSelection, ReviewedControlOperation,
};
use super::projection_error;

pub(super) fn project_grants(
    operation: &ReviewedControlOperation,
    prior: Option<&ControlGeneration>,
    target: &InstallationSnapshot,
) -> UseResult<Vec<ControlGrantSelection>> {
    let prior_grants = prior
        .map(|generation| {
            validate_grant_selections(&generation.grants, &generation.snapshot)?;
            Ok(generation.grants.clone())
        })
        .transpose()?
        .unwrap_or_default();
    let required = operation
        .envelope
        .plan
        .workspace_grant_changes_required()
        .map_err(|_| {
            projection_error("The reviewed Plan has invalid Workspace Grant impact evidence.")
        })?;
    let Some(evidence) = operation.authorization.grant_transition.as_ref() else {
        if required {
            return Err(projection_error(
                "A permission-bearing Control Store operation omitted reviewed Grant evidence.",
            ));
        }
        validate_grant_selections(&prior_grants, target)?;
        return Ok(prior_grants);
    };
    if !required {
        return Err(projection_error(
            "A permission-free Control Store operation supplied Grant evidence.",
        ));
    }

    let expected_snapshot = PluginWorkspaceGrantSnapshot {
        schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_string(),
        scope_id: operation.envelope.plan.scope.id.clone(),
        state_revision: operation.envelope.plan.state.state_revision,
        grants: prior_grants
            .iter()
            .map(|selection| WorkspaceGrantEvidence {
                package_id: selection.package_id().to_string(),
                package_digest: selection.grant.package_digest.clone(),
                receipt_revision: selection.receipt_revision,
                grant_digest: selection.grant_digest.clone(),
            })
            .collect(),
    };
    expected_snapshot.validate().map_err(|_| {
        projection_error("The prior Control Store Grants cannot form canonical snapshot evidence.")
    })?;
    if evidence.snapshot != expected_snapshot {
        return Err(projection_error(
            "The reviewed Grant snapshot differs from the exact prior Control Store generation.",
        ));
    }

    let resolved = evidence
        .change_set
        .finalize_against_plan(
            &operation.envelope.plan,
            Some(&evidence.snapshot),
            operation.authorization.operation_confirmation.as_ref(),
            &operation.authorization.grant_confirmations,
            operation.reviewed_at_ms,
        )
        .map_err(|_| {
            projection_error("The reviewed Workspace Grant change set cannot be re-derived.")
        })?;
    let mut projected = prior_grants
        .into_iter()
        .map(|selection| (selection.package_id().to_string(), selection))
        .collect::<BTreeMap<_, _>>();
    for revocation in &resolved.revocations {
        let exact = projected
            .get(&revocation.package_id)
            .is_some_and(|selection| {
                selection.grant.package_digest == revocation.package_digest
                    && selection.receipt_revision == revocation.receipt_revision
                    && selection.grant_digest == revocation.grant_digest
            });
        if !exact {
            return Err(projection_error(
                "A reviewed Grant revocation differs from the exact active Grant revision.",
            ));
        }
        projected.remove(&revocation.package_id);
    }
    for candidate in resolved.grants {
        let package_id = candidate.grant.package_id.clone();
        let grant_digest = candidate
            .grant
            .descriptor_digest()
            .map_err(|_| projection_error("A resolved Control Store Grant is not canonical."))?;
        let selection = ControlGrantSelection {
            grant: candidate.grant,
            grant_digest,
            receipt_revision: resolved.revision,
        };
        if projected.insert(package_id, selection).is_some() {
            return Err(projection_error(
                "A resolved Control Store Grant would replace an unrevoked active Grant.",
            ));
        }
    }
    let projected = projected.into_values().collect::<Vec<_>>();
    validate_grant_selections(&projected, target)?;
    Ok(projected)
}
