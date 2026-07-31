use a3s_use_core::UseResult;
use a3s_use_extension::{WorkspaceGrantLifecyclePhase, WorkspaceGrantOperationJournal};

use crate::plugin_runtime::{RuntimeBindingOperationJournal, RuntimeBindingOperationPhase};

use super::binding::PluginLifecycleOperationBinding;
use super::cutover::PluginLifecycleCutoverEvidence;
use super::validation::lifecycle_error;

impl PluginLifecycleOperationBinding {
    /// Verify that every scope-specific child journal has reached the exact
    /// pre-publication gate.
    ///
    /// A host reopening durable parent state must first call
    /// `validate_children` with the reviewed plan and these journals' intents.
    pub fn verify_ready_for_cutover(
        &self,
        grant_journals: &[WorkspaceGrantOperationJournal],
        runtime_journals: &[RuntimeBindingOperationJournal],
    ) -> UseResult<()> {
        validate_journal_children(self, grant_journals, runtime_journals)?;
        if grant_journals
            .iter()
            .any(|journal| journal.phase != WorkspaceGrantLifecyclePhase::Prepared)
            || runtime_journals
                .iter()
                .any(|journal| journal.phase != RuntimeBindingOperationPhase::BindingsPublished)
        {
            return Err(lifecycle_error(
                "Capability publication requires every grant prepared and every Runtime binding published.",
            ));
        }
        Ok(())
    }

    /// Verify exact child completion after replaying one parent cutover.
    pub fn verify_completed(
        &self,
        cutover: &PluginLifecycleCutoverEvidence,
        grant_journals: &[WorkspaceGrantOperationJournal],
        runtime_journals: &[RuntimeBindingOperationJournal],
        now_ms: u64,
    ) -> UseResult<()> {
        cutover.validate_against(self, now_ms)?;
        validate_journal_children(self, grant_journals, runtime_journals)?;
        for journal in grant_journals {
            if journal.phase != WorkspaceGrantLifecyclePhase::Completed
                || journal.cutover.as_ref()
                    != Some(&cutover.grant_cutover(self, &journal.intent, now_ms)?)
            {
                return Err(lifecycle_error(
                    "A workspace-grant child journal is not completed under the parent cutover.",
                ));
            }
        }
        for journal in runtime_journals {
            if journal.phase != RuntimeBindingOperationPhase::Completed
                || journal.cutover.as_ref()
                    != Some(&cutover.runtime_cutover(self, &journal.intent, now_ms)?)
            {
                return Err(lifecycle_error(
                    "A Runtime child journal is not completed under the parent cutover.",
                ));
            }
        }
        Ok(())
    }
}

fn validate_journal_children(
    binding: &PluginLifecycleOperationBinding,
    grant_journals: &[WorkspaceGrantOperationJournal],
    runtime_journals: &[RuntimeBindingOperationJournal],
) -> UseResult<()> {
    binding.validate()?;
    for journal in grant_journals {
        journal.validate()?;
    }
    for journal in runtime_journals {
        journal.validate()?;
    }
    binding.validate_child_identity(
        &grant_journals
            .iter()
            .map(|journal| journal.intent.clone())
            .collect::<Vec<_>>(),
        &runtime_journals
            .iter()
            .map(|journal| journal.intent.clone())
            .collect::<Vec<_>>(),
    )
}
