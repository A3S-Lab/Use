use a3s_use_core::{ResolvedWorkspaceGrantChangeSet, UseResult, WorkspaceGrantEvidence};

use super::workspace_grant::{
    StoredWorkspaceGrant, WorkspaceGrantReceipt, WorkspaceGrantRevocation, WorkspaceGrantStore,
};
use super::workspace_grant_io::{
    acquire_lock, ensure_owned_directory, sync_parent_directory, validate_existing_directory_chain,
    write_record,
};
use super::workspace_grant_operation::{
    operation_state_error, validate_resolved, WorkspaceGrantCandidateCeiling,
    WorkspaceGrantCutoverEvidence, WorkspaceGrantLifecyclePhase, WorkspaceGrantOperationIntent,
    WorkspaceGrantOperationJournal, WorkspaceGrantPreparedCandidate, WorkspaceGrantRetirement,
    WorkspaceGrantRollbackEvidence, WORKSPACE_GRANT_ROLLBACK_SCHEMA,
};
use super::workspace_grant_operation_io::{read_optional_operation, write_operation};

impl WorkspaceGrantStore {
    /// Records a durable immutable intent before any candidate grant is written.
    pub async fn begin_change_set(
        &self,
        resolved: &ResolvedWorkspaceGrantChangeSet,
        ceilings: &[WorkspaceGrantCandidateCeiling],
    ) -> UseResult<WorkspaceGrantOperationJournal> {
        validate_resolved(resolved)?;
        let mut candidates = build_candidates(resolved, ceilings)?;
        let _lock = acquire_lock(self.state_root(), self.root()).await?;
        let path = self.operation_path(&resolved.operation_id)?;
        ensure_owned_directory(self.root(), path.parent()).await?;

        if let Some(existing) = read_optional_operation(&path).await? {
            verify_operation_ownership(&existing, &resolved.operation_id)?;
            if !intent_matches_resolved(&existing.intent, resolved, &candidates) {
                return Err(operation_conflict());
            }
            return Ok(existing);
        }

        let before_snapshot = self
            .snapshot_scope_locked(&resolved.scope_id, resolved.state_revision_before)
            .await?;
        let observed_before_snapshot_digest = before_snapshot.descriptor_digest()?;
        if let Some(expected) = &resolved.before_snapshot_digest {
            if observed_before_snapshot_digest != *expected {
                return Err(operation_state_error(
                    "use.plugin.grant_operation.snapshot_changed",
                    "The durable workspace grant snapshot changed after operation planning.",
                ));
            }
        }

        let mut retirements = Vec::with_capacity(resolved.revocations.len());
        for evidence in &resolved.revocations {
            let Some(StoredWorkspaceGrant::Granted(prior_receipt)) = self
                .observe_record(
                    &resolved.scope_id,
                    &evidence.package_id,
                    &evidence.package_digest,
                )
                .await?
            else {
                return Err(retirement_ownership_changed());
            };
            if !receipt_matches_evidence(&prior_receipt, evidence) {
                return Err(retirement_ownership_changed());
            }
            retirements.push(WorkspaceGrantRetirement {
                evidence: evidence.clone(),
                prior_receipt,
            });
        }

        for candidate in &mut candidates {
            candidate.prior_record = self
                .observe_record(
                    &resolved.scope_id,
                    &candidate.receipt.grant.package_id,
                    &candidate.receipt.grant.package_digest,
                )
                .await?;
        }

        let intent = WorkspaceGrantOperationIntent {
            operation_id: resolved.operation_id.clone(),
            plan_digest: resolved.plan_digest.clone(),
            change_set_digest: resolved.change_set_digest.clone(),
            scope_id: resolved.scope_id.clone(),
            state_revision_before: resolved.state_revision_before,
            revision: resolved.revision,
            capability_generation_before: resolved.capability_generation_before,
            capability_generation_after: resolved.capability_generation_after,
            before_snapshot_digest: resolved.before_snapshot_digest.clone(),
            observed_before_snapshot_digest,
            transitioned_at_ms: resolved.transitioned_at_ms,
            revocation_authority: resolved.revocation_authority.clone(),
            candidates,
            retirements,
        };
        let journal = WorkspaceGrantOperationJournal::new(intent)?;
        write_operation(&path, &journal).await?;
        Ok(journal)
    }

    /// Idempotently writes every candidate grant and checkpoints preparation.
    pub async fn prepare_change_set(
        &self,
        operation_id: &str,
        now_ms: u64,
    ) -> UseResult<WorkspaceGrantOperationJournal> {
        let _lock = acquire_lock(self.state_root(), self.root()).await?;
        let path = self.operation_path(operation_id)?;
        let mut journal = self.load_operation(&path, operation_id).await?;
        match journal.phase {
            WorkspaceGrantLifecyclePhase::IntentRecorded => {
                journal.phase = WorkspaceGrantLifecyclePhase::Preparing;
                write_operation(&path, &journal).await?;
            }
            WorkspaceGrantLifecyclePhase::Preparing | WorkspaceGrantLifecyclePhase::Prepared => {}
            WorkspaceGrantLifecyclePhase::CutoverCommitted
            | WorkspaceGrantLifecyclePhase::Retiring
            | WorkspaceGrantLifecyclePhase::Completed => return Ok(journal),
            WorkspaceGrantLifecyclePhase::RollingBack
            | WorkspaceGrantLifecyclePhase::RolledBack => return Err(operation_rolled_back()),
        }

        for candidate in &journal.intent.candidates {
            self.put_locked(&candidate.receipt, &candidate.ceiling, now_ms)
                .await?;
        }
        journal.phase = WorkspaceGrantLifecyclePhase::Prepared;
        write_operation(&path, &journal).await?;
        Ok(journal)
    }

    /// Persists proof that capability publication selected the prepared generation.
    pub async fn commit_change_set_cutover(
        &self,
        operation_id: &str,
        cutover: WorkspaceGrantCutoverEvidence,
        now_ms: u64,
    ) -> UseResult<WorkspaceGrantOperationJournal> {
        let _lock = acquire_lock(self.state_root(), self.root()).await?;
        let path = self.operation_path(operation_id)?;
        let mut journal = self.load_operation(&path, operation_id).await?;
        cutover.validate_against(&journal.intent)?;
        if cutover.committed_at_ms > now_ms {
            return Err(operation_state_error(
                "use.plugin.grant_operation.cutover_in_future",
                "Capability cutover evidence cannot be committed from the future.",
            ));
        }

        match journal.phase {
            WorkspaceGrantLifecyclePhase::Prepared => {
                self.verify_candidates(&journal.intent, Some(cutover.committed_at_ms))
                    .await?;
                journal.cutover = Some(cutover);
                journal.phase = WorkspaceGrantLifecyclePhase::CutoverCommitted;
                write_operation(&path, &journal).await?;
                Ok(journal)
            }
            WorkspaceGrantLifecyclePhase::CutoverCommitted
            | WorkspaceGrantLifecyclePhase::Retiring
            | WorkspaceGrantLifecyclePhase::Completed => {
                if journal.cutover.as_ref() != Some(&cutover) {
                    return Err(operation_conflict());
                }
                Ok(journal)
            }
            WorkspaceGrantLifecyclePhase::IntentRecorded
            | WorkspaceGrantLifecyclePhase::Preparing => Err(operation_state_error(
                "use.plugin.grant_operation.not_prepared",
                "Capability cutover cannot be committed before every candidate grant is prepared.",
            )),
            WorkspaceGrantLifecyclePhase::RollingBack
            | WorkspaceGrantLifecyclePhase::RolledBack => Err(operation_rolled_back()),
        }
    }

    /// Idempotently revokes exact prior generations after durable cutover proof.
    pub async fn retire_change_set(
        &self,
        operation_id: &str,
    ) -> UseResult<WorkspaceGrantOperationJournal> {
        let _lock = acquire_lock(self.state_root(), self.root()).await?;
        let path = self.operation_path(operation_id)?;
        let mut journal = self.load_operation(&path, operation_id).await?;
        match journal.phase {
            WorkspaceGrantLifecyclePhase::CutoverCommitted => {
                journal.phase = WorkspaceGrantLifecyclePhase::Retiring;
                write_operation(&path, &journal).await?;
            }
            WorkspaceGrantLifecyclePhase::Retiring => {}
            WorkspaceGrantLifecyclePhase::Completed => return Ok(journal),
            WorkspaceGrantLifecyclePhase::IntentRecorded
            | WorkspaceGrantLifecyclePhase::Preparing
            | WorkspaceGrantLifecyclePhase::Prepared => {
                return Err(operation_state_error(
                    "use.plugin.grant_operation.cutover_required",
                    "Prior grants cannot be retired before durable capability cutover evidence.",
                ));
            }
            WorkspaceGrantLifecyclePhase::RollingBack
            | WorkspaceGrantLifecyclePhase::RolledBack => return Err(operation_rolled_back()),
        }

        self.verify_candidates(&journal.intent, None).await?;
        let revoked_at_ms = journal
            .cutover
            .as_ref()
            .ok_or_else(operation_corrupt)?
            .committed_at_ms;
        for retirement in &journal.intent.retirements {
            if let Some(candidate) = matching_candidate(&journal.intent, &retirement.evidence) {
                let current = self
                    .observe_record(
                        &journal.intent.scope_id,
                        candidate.receipt.grant.package_id.as_str(),
                        candidate.receipt.grant.package_digest.as_str(),
                    )
                    .await?;
                if current != Some(StoredWorkspaceGrant::Granted(candidate.receipt.clone())) {
                    return Err(retirement_ownership_changed());
                }
                continue;
            }
            let revocation = WorkspaceGrantRevocation::new(
                journal.intent.revision,
                &retirement.prior_receipt,
                journal.intent.revocation_authority.clone(),
                revoked_at_ms,
            )?;
            self.revoke_locked(&retirement.prior_receipt, &revocation)
                .await?;
        }
        journal.phase = WorkspaceGrantLifecyclePhase::Completed;
        write_operation(&path, &journal).await?;
        Ok(journal)
    }

    /// Restores every exact candidate path to its state before preparation.
    ///
    /// This is valid only before capability cutover. The rollback phase and
    /// evidence are persisted first, so a crash while restoring candidate
    /// records can resume without deleting or overwriting unrelated grants.
    pub async fn rollback_change_set(
        &self,
        operation_id: &str,
        evidence_digest: impl Into<String>,
        rolled_back_at_ms: u64,
        now_ms: u64,
    ) -> UseResult<WorkspaceGrantOperationJournal> {
        let _lock = acquire_lock(self.state_root(), self.root()).await?;
        let path = self.operation_path(operation_id)?;
        let mut journal = self.load_operation(&path, operation_id).await?;
        let rollback = WorkspaceGrantRollbackEvidence {
            schema: WORKSPACE_GRANT_ROLLBACK_SCHEMA.to_string(),
            evidence_digest: evidence_digest.into(),
            rolled_back_at_ms,
        };
        rollback.validate_against(&journal.intent)?;
        if rolled_back_at_ms > now_ms {
            return Err(operation_state_error(
                "use.plugin.grant_operation.rollback_in_future",
                "Candidate rollback evidence cannot be committed from the future.",
            ));
        }

        match journal.phase {
            WorkspaceGrantLifecyclePhase::IntentRecorded
            | WorkspaceGrantLifecyclePhase::Preparing
            | WorkspaceGrantLifecyclePhase::Prepared => {
                journal.phase = WorkspaceGrantLifecyclePhase::RollingBack;
                journal.rollback = Some(rollback.clone());
                write_operation(&path, &journal).await?;
            }
            WorkspaceGrantLifecyclePhase::RollingBack => {
                if journal.rollback.as_ref() != Some(&rollback) {
                    return Err(operation_conflict());
                }
            }
            WorkspaceGrantLifecyclePhase::RolledBack => {
                if journal.rollback.as_ref() != Some(&rollback) {
                    return Err(operation_conflict());
                }
                return Ok(journal);
            }
            WorkspaceGrantLifecyclePhase::CutoverCommitted
            | WorkspaceGrantLifecyclePhase::Retiring
            | WorkspaceGrantLifecyclePhase::Completed => {
                return Err(operation_state_error(
                    "use.plugin.grant_operation.cutover_committed",
                    "A cutover-committed workspace grant operation cannot roll back candidates.",
                ));
            }
        }

        for candidate in journal.intent.candidates.iter().rev() {
            self.restore_candidate_record(candidate).await?;
        }
        journal.phase = WorkspaceGrantLifecyclePhase::RolledBack;
        write_operation(&path, &journal).await?;
        Ok(journal)
    }

    pub async fn observe_change_set(
        &self,
        operation_id: &str,
    ) -> UseResult<Option<WorkspaceGrantOperationJournal>> {
        let _lock = acquire_lock(self.state_root(), self.root()).await?;
        let path = self.operation_path(operation_id)?;
        if !validate_existing_directory_chain(self.state_root(), path.parent()).await? {
            return Ok(None);
        }
        let journal = read_optional_operation(&path).await?;
        if let Some(journal) = &journal {
            verify_operation_ownership(journal, operation_id)?;
        }
        Ok(journal)
    }

    async fn load_operation(
        &self,
        path: &std::path::Path,
        operation_id: &str,
    ) -> UseResult<WorkspaceGrantOperationJournal> {
        if !validate_existing_directory_chain(self.state_root(), path.parent()).await? {
            return Err(operation_not_found());
        }
        let journal = read_optional_operation(path)
            .await?
            .ok_or_else(operation_not_found)?;
        verify_operation_ownership(&journal, operation_id)?;
        Ok(journal)
    }

    async fn verify_candidates(
        &self,
        intent: &WorkspaceGrantOperationIntent,
        active_at_ms: Option<u64>,
    ) -> UseResult<()> {
        for candidate in &intent.candidates {
            if let Some(active_at_ms) = active_at_ms {
                candidate
                    .receipt
                    .grant
                    .validate_active_against(&candidate.ceiling, active_at_ms)
                    .map_err(|_| {
                        operation_state_error(
                            "use.plugin.grant_operation.candidate_inactive",
                            "A prepared candidate grant is inactive at capability cutover.",
                        )
                    })?;
            }
            let current = self
                .observe_record(
                    &intent.scope_id,
                    &candidate.receipt.grant.package_id,
                    &candidate.receipt.grant.package_digest,
                )
                .await?;
            if current != Some(StoredWorkspaceGrant::Granted(candidate.receipt.clone())) {
                return Err(operation_state_error(
                    "use.plugin.grant_operation.candidate_changed",
                    "A prepared candidate grant changed before capability cutover or retirement.",
                ));
            }
        }
        Ok(())
    }

    async fn restore_candidate_record(
        &self,
        candidate: &WorkspaceGrantPreparedCandidate,
    ) -> UseResult<()> {
        let current = self
            .observe_record(
                &candidate.receipt.grant.scope_id,
                &candidate.receipt.grant.package_id,
                &candidate.receipt.grant.package_digest,
            )
            .await?;
        let prepared = Some(StoredWorkspaceGrant::Granted(candidate.receipt.clone()));
        if current == candidate.prior_record {
            return Ok(());
        }
        if current != prepared {
            return Err(operation_state_error(
                "use.plugin.grant_operation.candidate_changed",
                "A prepared candidate grant changed before pre-cutover rollback.",
            ));
        }

        let path = self.record_path(
            &candidate.receipt.grant.scope_id,
            &candidate.receipt.grant.package_id,
            &candidate.receipt.grant.package_digest,
        )?;
        if let Some(prior) = &candidate.prior_record {
            write_record(&path, prior).await?;
        } else {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(operation_state_error(
                        "use.plugin.grant_operation.io",
                        format!(
                            "Failed to remove rolled-back workspace grant candidate '{}': {error}",
                            path.display()
                        ),
                    ));
                }
            }
            sync_parent_directory(path.parent(), "rolled-back candidate grant").await?;
        }
        Ok(())
    }
}

fn build_candidates(
    resolved: &ResolvedWorkspaceGrantChangeSet,
    ceilings: &[WorkspaceGrantCandidateCeiling],
) -> UseResult<Vec<WorkspaceGrantPreparedCandidate>> {
    if ceilings.len() != resolved.grants.len()
        || ceilings
            .windows(2)
            .any(|pair| pair[0].package_id >= pair[1].package_id)
    {
        return Err(candidate_ceiling_mismatch());
    }
    let revision = resolved.revision;
    let transitioned_at_ms = resolved.transitioned_at_ms;
    resolved
        .grants
        .iter()
        .zip(ceilings)
        .map(|(candidate, ceiling)| {
            ceiling.validate()?;
            if ceiling.package_id != candidate.grant.package_id
                || ceiling.package_digest != candidate.grant.package_digest
            {
                return Err(candidate_ceiling_mismatch());
            }
            let receipt = WorkspaceGrantReceipt::new(revision, candidate.grant.clone())?;
            receipt
                .grant
                .validate_active_against(&ceiling.ceiling, transitioned_at_ms)
                .map_err(|_| candidate_ceiling_mismatch())?;
            Ok(WorkspaceGrantPreparedCandidate {
                proposal_digest: candidate.proposal_digest.clone(),
                receipt,
                ceiling: ceiling.ceiling.clone(),
                prior_record: None,
            })
        })
        .collect()
}

fn intent_matches_resolved(
    intent: &WorkspaceGrantOperationIntent,
    resolved: &ResolvedWorkspaceGrantChangeSet,
    candidates: &[WorkspaceGrantPreparedCandidate],
) -> bool {
    intent.operation_id == resolved.operation_id
        && intent.plan_digest == resolved.plan_digest
        && intent.change_set_digest == resolved.change_set_digest
        && intent.scope_id == resolved.scope_id
        && intent.state_revision_before == resolved.state_revision_before
        && intent.revision == resolved.revision
        && intent.capability_generation_before == resolved.capability_generation_before
        && intent.capability_generation_after == resolved.capability_generation_after
        && intent.before_snapshot_digest == resolved.before_snapshot_digest
        && intent.transitioned_at_ms == resolved.transitioned_at_ms
        && intent.revocation_authority == resolved.revocation_authority
        && intent.candidates.len() == candidates.len()
        && intent
            .candidates
            .iter()
            .zip(candidates)
            .all(|(recorded, candidate)| {
                recorded.proposal_digest == candidate.proposal_digest
                    && recorded.receipt == candidate.receipt
                    && recorded.ceiling == candidate.ceiling
            })
        && intent
            .retirements
            .iter()
            .map(|retirement| &retirement.evidence)
            .eq(resolved.revocations.iter())
}

fn receipt_matches_evidence(
    receipt: &WorkspaceGrantReceipt,
    evidence: &WorkspaceGrantEvidence,
) -> bool {
    receipt.grant.package_id == evidence.package_id
        && receipt.grant.package_digest == evidence.package_digest
        && receipt.revision == evidence.receipt_revision
        && receipt.grant_digest == evidence.grant_digest
}

fn matching_candidate<'a>(
    intent: &'a WorkspaceGrantOperationIntent,
    evidence: &WorkspaceGrantEvidence,
) -> Option<&'a WorkspaceGrantPreparedCandidate> {
    intent.candidates.iter().find(|candidate| {
        candidate.receipt.grant.package_id == evidence.package_id
            && candidate.receipt.grant.package_digest == evidence.package_digest
    })
}

fn verify_operation_ownership(
    journal: &WorkspaceGrantOperationJournal,
    operation_id: &str,
) -> UseResult<()> {
    if journal.intent.operation_id != operation_id {
        return Err(operation_state_error(
            "use.plugin.grant_operation.ownership_mismatch",
            "A workspace grant operation journal does not match its operation path.",
        ));
    }
    Ok(())
}

fn candidate_ceiling_mismatch() -> a3s_use_core::UseError {
    operation_state_error(
        "use.plugin.grant_operation.ceiling_mismatch",
        "Candidate ceilings do not exactly match the resolved workspace grant generations.",
    )
}

fn retirement_ownership_changed() -> a3s_use_core::UseError {
    operation_state_error(
        "use.plugin.grant_operation.ownership_changed",
        "A prior workspace grant changed before the durable operation intent was recorded.",
    )
}

fn operation_conflict() -> a3s_use_core::UseError {
    operation_state_error(
        "use.plugin.grant_operation.conflict",
        "The operation ID already owns different immutable workspace grant intent.",
    )
}

fn operation_not_found() -> a3s_use_core::UseError {
    operation_state_error(
        "use.plugin.grant_operation.not_found",
        "The workspace grant operation intent does not exist.",
    )
}

fn operation_corrupt() -> a3s_use_core::UseError {
    operation_state_error(
        "use.plugin.grant_operation.invalid",
        "The workspace grant operation journal is internally inconsistent.",
    )
}

fn operation_rolled_back() -> a3s_use_core::UseError {
    operation_state_error(
        "use.plugin.grant_operation.rolled_back",
        "The workspace grant operation was rolled back and requires a fresh reviewed plan.",
    )
}
