//! Convert reviewed cognitive-package lifecycle inputs into Control authority.
//!
//! This module owns the seam between package planning/authorization and the
//! inactive Control Store. It deliberately derives store cursors from the
//! immutable reviewed Plan so callers cannot select a different generation.

use a3s_use_core::{PluginOperationPlanEnvelope, UseResult};

use super::model::{input_error, ControlGrantAuthorizationEvidence, ReviewedControlOperation};
use crate::cognitive_package::{
    CognitivePackageAuthorizationEvidence, PlannedWorkspaceGrantOperation,
};

pub(in crate::control_store) fn reviewed_cognitive_package_operation(
    envelope: &PluginOperationPlanEnvelope,
    authorization: &CognitivePackageAuthorizationEvidence,
    grants: Option<&PlannedWorkspaceGrantOperation>,
    reviewed_at_ms: u64,
) -> UseResult<ReviewedControlOperation> {
    let expected_generation = envelope
        .plan
        .state
        .state_revision
        .checked_sub(1)
        .ok_or_else(|| {
            input_error(
                "A reviewed lifecycle Plan cannot derive its prior Control Store generation.",
            )
        })?;
    let grant_transition = grants.map(|planned| ControlGrantAuthorizationEvidence {
        snapshot: planned.snapshot.clone(),
        change_set: planned.change_set.clone(),
    });

    ReviewedControlOperation::new(
        envelope.clone(),
        authorization.operation_confirmation.clone(),
        grant_transition,
        authorization.grant_confirmations.clone(),
        expected_generation,
        envelope.plan.state.capability_generation,
        reviewed_at_ms,
    )
}
