use a3s_use_core::{UseError, UseResult};

use super::binding_operation::{
    candidate_for, operation_state_error, RuntimeBindingCutoverEvidence,
    RuntimeBindingOperationIntent, RuntimeBindingOperationJournal, RuntimeBindingOperationPhase,
    RuntimeBindingRetirementEvidence,
};
use super::binding_operation_io::{operation_path, read_optional_operation, write_operation};
use super::store::{validate_replacement, RuntimeBindingStore};
use super::RuntimeBindingReceipt;

impl RuntimeBindingStore {
    /// Persist immutable Runtime binding intent before candidate workloads or
    /// launcher bindings are prepared.
    pub async fn begin_binding_change(
        &self,
        intent: &RuntimeBindingOperationIntent,
    ) -> UseResult<RuntimeBindingOperationJournal> {
        intent.validate()?;
        let _lock = self.acquire_lock().await?;
        let path = operation_path(self, &intent.scope_id, &intent.operation_id)?;
        if let Some(existing) = read_optional_operation(self, &path).await? {
            verify_operation_ownership(&existing, &intent.scope_id, &intent.operation_id)?;
            if existing.intent != *intent {
                return Err(operation_conflict());
            }
            return Ok(existing);
        }

        for retirement in &intent.retirements {
            let current = self
                .get_locked(&intent.scope_id, retirement.surface())
                .await?;
            if current.as_ref() != Some(retirement) {
                return Err(before_state_changed());
            }
        }
        for candidate in &intent.candidates {
            if retirement_for(intent, &candidate.surface).is_none()
                && self
                    .get_locked(&intent.scope_id, &candidate.surface)
                    .await?
                    .is_some()
            {
                return Err(before_state_changed());
            }
        }

        let journal = RuntimeBindingOperationJournal::new(intent.clone())?;
        write_operation(self, &path, &journal).await?;
        Ok(journal)
    }

    /// Checkpoint one exact candidate after Task preparation or successful
    /// Service health/Gateway/MCP readiness.
    pub async fn record_prepared_binding(
        &self,
        scope_id: &str,
        operation_id: &str,
        receipt: &RuntimeBindingReceipt,
    ) -> UseResult<RuntimeBindingOperationJournal> {
        receipt.validate()?;
        let _lock = self.acquire_lock().await?;
        let path = operation_path(self, scope_id, operation_id)?;
        let mut journal = self
            .load_binding_operation(&path, scope_id, operation_id)
            .await?;
        let candidate =
            candidate_for(&journal.intent, receipt.surface()).ok_or_else(candidate_mismatch)?;
        if !candidate.matches_receipt(receipt)? {
            return Err(candidate_mismatch());
        }
        if matches!(
            journal.phase,
            RuntimeBindingOperationPhase::Publishing
                | RuntimeBindingOperationPhase::BindingsPublished
                | RuntimeBindingOperationPhase::CutoverCommitted
                | RuntimeBindingOperationPhase::Retiring
                | RuntimeBindingOperationPhase::Completed
        ) {
            return prepared_replay(&journal, receipt);
        }

        match journal
            .prepared
            .binary_search_by(|prepared| prepared.surface().cmp(receipt.surface()))
        {
            Ok(index) if journal.prepared[index] == *receipt => return Ok(journal),
            Ok(index) => {
                validate_replacement(&journal.prepared[index], receipt)
                    .map_err(|_| candidate_mismatch())?;
                journal.prepared[index] = receipt.clone();
            }
            Err(index) => journal.prepared.insert(index, receipt.clone()),
        }
        journal.phase = if journal.prepared.len() == journal.intent.candidates.len() {
            RuntimeBindingOperationPhase::Prepared
        } else {
            RuntimeBindingOperationPhase::Preparing
        };
        write_operation(self, &path, &journal).await?;
        Ok(journal)
    }

    /// Publish every prepared candidate receipt under the active binding
    /// paths. A durable `publishing` phase makes partial multi-surface writes
    /// replayable.
    pub async fn publish_prepared_bindings(
        &self,
        scope_id: &str,
        operation_id: &str,
    ) -> UseResult<RuntimeBindingOperationJournal> {
        let _lock = self.acquire_lock().await?;
        let path = operation_path(self, scope_id, operation_id)?;
        let mut journal = self
            .load_binding_operation(&path, scope_id, operation_id)
            .await?;
        match journal.phase {
            RuntimeBindingOperationPhase::IntentRecorded
                if journal.intent.candidates.is_empty() =>
            {
                journal.phase = RuntimeBindingOperationPhase::Prepared;
                write_operation(self, &path, &journal).await?;
            }
            RuntimeBindingOperationPhase::Prepared | RuntimeBindingOperationPhase::Publishing => {}
            RuntimeBindingOperationPhase::BindingsPublished => {
                self.verify_published_state(&journal).await?;
                return Ok(journal);
            }
            RuntimeBindingOperationPhase::CutoverCommitted
            | RuntimeBindingOperationPhase::Retiring
            | RuntimeBindingOperationPhase::Completed => return Ok(journal),
            RuntimeBindingOperationPhase::IntentRecorded
            | RuntimeBindingOperationPhase::Preparing => {
                return Err(operation_state_error(
                    "use.plugin.runtime.binding_operation_not_prepared",
                    "Runtime bindings cannot be published before every candidate is prepared.",
                ))
            }
        }

        journal.phase = RuntimeBindingOperationPhase::Publishing;
        write_operation(self, &path, &journal).await?;
        for receipt in &journal.prepared {
            let current = self
                .get_locked(&journal.intent.scope_id, receipt.surface())
                .await?;
            if current.as_ref() == Some(receipt) {
                continue;
            }
            let expected_prior = retirement_for(&journal.intent, receipt.surface());
            if current.as_ref() != expected_prior {
                return Err(candidate_changed());
            }
            self.put_locked(receipt)
                .await
                .map_err(|_| candidate_changed())?;
        }
        self.verify_published_state(&journal).await?;
        journal.phase = RuntimeBindingOperationPhase::BindingsPublished;
        write_operation(self, &path, &journal).await?;
        Ok(journal)
    }

    /// Commit exact capability snapshot evidence after candidate bindings are
    /// published. Prior bindings remain retirement-owned by the journal.
    pub async fn commit_binding_cutover(
        &self,
        scope_id: &str,
        operation_id: &str,
        cutover: RuntimeBindingCutoverEvidence,
        now_ms: u64,
    ) -> UseResult<RuntimeBindingOperationJournal> {
        let _lock = self.acquire_lock().await?;
        let path = operation_path(self, scope_id, operation_id)?;
        let mut journal = self
            .load_binding_operation(&path, scope_id, operation_id)
            .await?;
        cutover.validate_against(&journal.intent)?;
        if cutover.committed_at_ms > now_ms {
            return Err(operation_state_error(
                "use.plugin.runtime.binding_operation_cutover_in_future",
                "Runtime binding cutover evidence cannot be committed from the future.",
            ));
        }
        match journal.phase {
            RuntimeBindingOperationPhase::BindingsPublished => {
                self.verify_published_state(&journal).await?;
                journal.cutover = Some(cutover);
                journal.phase = if journal.intent.retirements.is_empty() {
                    RuntimeBindingOperationPhase::Completed
                } else {
                    RuntimeBindingOperationPhase::CutoverCommitted
                };
                write_operation(self, &path, &journal).await?;
                Ok(journal)
            }
            RuntimeBindingOperationPhase::CutoverCommitted
            | RuntimeBindingOperationPhase::Retiring
            | RuntimeBindingOperationPhase::Completed => {
                if journal.cutover.as_ref() != Some(&cutover) {
                    return Err(operation_conflict());
                }
                Ok(journal)
            }
            _ => Err(operation_state_error(
                "use.plugin.runtime.binding_operation_not_published",
                "Capability cutover requires every candidate Runtime binding to be published.",
            )),
        }
    }

    /// Checkpoint exact prior binding retirement after the caller has removed
    /// the old Runtime Service generation. Task launcher retirement requires
    /// only a trusted timestamp; Service retirement requires Runtime removal
    /// evidence.
    pub async fn record_retired_binding(
        &self,
        scope_id: &str,
        operation_id: &str,
        evidence: &RuntimeBindingRetirementEvidence,
        now_ms: u64,
    ) -> UseResult<RuntimeBindingOperationJournal> {
        let _lock = self.acquire_lock().await?;
        let path = operation_path(self, scope_id, operation_id)?;
        let mut journal = self
            .load_binding_operation(&path, scope_id, operation_id)
            .await?;
        let cutover = journal.cutover.as_ref().ok_or_else(cutover_required)?;
        evidence.validate_against(cutover, now_ms)?;
        let retirement =
            retirement_for(&journal.intent, evidence.receipt().surface()).ok_or_else(|| {
                operation_state_error(
                    "use.plugin.runtime.binding_operation_retirement_mismatch",
                    "Runtime binding retirement evidence is absent from operation intent.",
                )
            })?;
        if retirement != evidence.receipt() {
            return Err(operation_state_error(
                "use.plugin.runtime.binding_operation_retirement_mismatch",
                "Runtime binding retirement evidence changed exact prior ownership.",
            ));
        }
        match journal.retired.binary_search_by(|retired| {
            retired
                .receipt()
                .surface()
                .cmp(evidence.receipt().surface())
        }) {
            Ok(index) if journal.retired[index] == *evidence => return Ok(journal),
            Ok(_) => return Err(operation_conflict()),
            Err(_) if journal.phase == RuntimeBindingOperationPhase::Completed => {
                return Err(operation_conflict())
            }
            Err(_) => {}
        }
        if !matches!(
            journal.phase,
            RuntimeBindingOperationPhase::CutoverCommitted | RuntimeBindingOperationPhase::Retiring
        ) {
            return Err(cutover_required());
        }

        journal.phase = RuntimeBindingOperationPhase::Retiring;
        write_operation(self, &path, &journal).await?;
        if let Some(candidate) = prepared_for(&journal, evidence.receipt().surface()) {
            let current = self
                .get_locked(&journal.intent.scope_id, candidate.surface())
                .await?;
            if current.as_ref() != Some(candidate) {
                return Err(candidate_changed());
            }
        } else {
            match self
                .get_locked(&journal.intent.scope_id, evidence.receipt().surface())
                .await?
            {
                Some(current) if current == *evidence.receipt() => {
                    self.remove_locked(evidence.receipt()).await?;
                }
                None => {}
                Some(_) => return Err(retirement_ownership_changed()),
            }
        }

        let index = journal
            .retired
            .binary_search_by(|retired| {
                retired
                    .receipt()
                    .surface()
                    .cmp(evidence.receipt().surface())
            })
            .expect_err("retirement duplication was handled before cleanup");
        journal.retired.insert(index, evidence.clone());
        journal.phase = if journal.retired.len() == journal.intent.retirements.len() {
            RuntimeBindingOperationPhase::Completed
        } else {
            RuntimeBindingOperationPhase::Retiring
        };
        write_operation(self, &path, &journal).await?;
        Ok(journal)
    }

    pub async fn observe_binding_change(
        &self,
        scope_id: &str,
        operation_id: &str,
    ) -> UseResult<Option<RuntimeBindingOperationJournal>> {
        let _lock = self.acquire_lock().await?;
        let path = operation_path(self, scope_id, operation_id)?;
        let journal = read_optional_operation(self, &path).await?;
        if let Some(journal) = &journal {
            verify_operation_ownership(journal, scope_id, operation_id)?;
        }
        Ok(journal)
    }

    async fn load_binding_operation(
        &self,
        path: &std::path::Path,
        scope_id: &str,
        operation_id: &str,
    ) -> UseResult<RuntimeBindingOperationJournal> {
        let journal = read_optional_operation(self, path)
            .await?
            .ok_or_else(operation_not_found)?;
        verify_operation_ownership(&journal, scope_id, operation_id)?;
        Ok(journal)
    }

    async fn verify_published_state(
        &self,
        journal: &RuntimeBindingOperationJournal,
    ) -> UseResult<()> {
        for receipt in &journal.prepared {
            let current = self
                .get_locked(&journal.intent.scope_id, receipt.surface())
                .await?;
            if current.as_ref() != Some(receipt) {
                return Err(candidate_changed());
            }
        }
        for retirement in &journal.intent.retirements {
            if candidate_for(&journal.intent, retirement.surface()).is_none() {
                let current = self
                    .get_locked(&journal.intent.scope_id, retirement.surface())
                    .await?;
                if current.as_ref() != Some(retirement) {
                    return Err(retirement_ownership_changed());
                }
            }
        }
        Ok(())
    }
}

fn prepared_replay(
    journal: &RuntimeBindingOperationJournal,
    receipt: &RuntimeBindingReceipt,
) -> UseResult<RuntimeBindingOperationJournal> {
    match journal
        .prepared
        .binary_search_by(|prepared| prepared.surface().cmp(receipt.surface()))
    {
        Ok(index) if journal.prepared[index] == *receipt => Ok(journal.clone()),
        _ => Err(candidate_mismatch()),
    }
}

fn retirement_for<'a>(
    intent: &'a RuntimeBindingOperationIntent,
    surface: &a3s_use_core::PlanQualifiedSurfaceRef,
) -> Option<&'a RuntimeBindingReceipt> {
    intent
        .retirements
        .binary_search_by(|receipt| receipt.surface().cmp(surface))
        .ok()
        .and_then(|index| intent.retirements.get(index))
}

fn prepared_for<'a>(
    journal: &'a RuntimeBindingOperationJournal,
    surface: &a3s_use_core::PlanQualifiedSurfaceRef,
) -> Option<&'a RuntimeBindingReceipt> {
    journal
        .prepared
        .binary_search_by(|receipt| receipt.surface().cmp(surface))
        .ok()
        .and_then(|index| journal.prepared.get(index))
}

fn verify_operation_ownership(
    journal: &RuntimeBindingOperationJournal,
    scope_id: &str,
    operation_id: &str,
) -> UseResult<()> {
    if journal.intent.scope_id != scope_id || journal.intent.operation_id != operation_id {
        return Err(operation_state_error(
            "use.plugin.runtime.binding_operation_ownership_mismatch",
            "A Runtime binding operation journal does not match its operation path.",
        ));
    }
    Ok(())
}

fn candidate_mismatch() -> UseError {
    operation_state_error(
        "use.plugin.runtime.binding_operation_candidate_mismatch",
        "A prepared Runtime binding does not match immutable candidate intent.",
    )
}

fn candidate_changed() -> UseError {
    operation_state_error(
        "use.plugin.runtime.binding_operation_candidate_changed",
        "A published Runtime binding candidate changed before capability cutover or retirement.",
    )
}

fn before_state_changed() -> UseError {
    operation_state_error(
        "use.plugin.runtime.binding_operation_before_changed",
        "The active Runtime binding state changed before durable operation intent.",
    )
}

fn retirement_ownership_changed() -> UseError {
    operation_state_error(
        "use.plugin.runtime.binding_operation_retirement_changed",
        "A prior Runtime binding changed before exact retirement cleanup.",
    )
}

fn cutover_required() -> UseError {
    operation_state_error(
        "use.plugin.runtime.binding_operation_cutover_required",
        "Prior Runtime bindings cannot retire before durable capability cutover evidence.",
    )
}

fn operation_conflict() -> UseError {
    operation_state_error(
        "use.plugin.runtime.binding_operation_conflict",
        "The operation ID already owns different immutable Runtime binding state.",
    )
}

fn operation_not_found() -> UseError {
    operation_state_error(
        "use.plugin.runtime.binding_operation_not_found",
        "The Runtime binding operation journal does not exist.",
    )
}
