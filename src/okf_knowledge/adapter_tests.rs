use std::sync::Arc;

use a3s_use_core::{
    OkfKnowledgeObservation, OkfKnowledgeObservedState, OkfProjectionReceipt, UseResult,
};
use async_trait::async_trait;

use super::test_support::{binding, files, observation, receipt, stage_spec};
use super::*;

struct FakeKnowledgeAdapter {
    staged: OkfKnowledgeBinding,
    promoted: OkfKnowledgeObservation,
    observed: OkfKnowledgeObservation,
    removed: OkfKnowledgeObservation,
}

#[async_trait]
impl OkfKnowledgeAdapter for FakeKnowledgeAdapter {
    async fn stage(&self, _request: &OkfKnowledgeStageRequest) -> UseResult<OkfKnowledgeBinding> {
        Ok(self.staged.clone())
    }

    async fn promote(&self, _receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        Ok(self.promoted.clone())
    }

    async fn observe(&self, _receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        Ok(self.observed.clone())
    }

    async fn remove(&self, _receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        Ok(self.removed.clone())
    }
}

fn client_with_receipt(receipt: &OkfProjectionReceipt) -> OkfKnowledgeClient {
    OkfKnowledgeClient::new(Arc::new(FakeKnowledgeAdapter {
        staged: binding(
            receipt,
            OkfKnowledgeObservedState::Staged,
            None,
            receipt.staged_at_ms + 1,
        ),
        promoted: observation(
            receipt,
            OkfKnowledgeObservedState::Promoted,
            Some(receipt),
            receipt.staged_at_ms + 2,
        ),
        observed: observation(
            receipt,
            OkfKnowledgeObservedState::Promoted,
            Some(receipt),
            receipt.staged_at_ms + 3,
        ),
        removed: observation(
            receipt,
            OkfKnowledgeObservedState::Removed,
            None,
            receipt.staged_at_ms + 4,
        ),
    }))
}

#[tokio::test]
async fn client_checks_stage_promote_observe_and_remove_evidence() {
    let receipt = receipt(1);
    let client = client_with_receipt(&receipt);
    let staged = client
        .stage(OkfKnowledgeStageRequest::new(stage_spec(1), files()).unwrap())
        .await
        .unwrap();
    assert_eq!(staged.observation.state, OkfKnowledgeObservedState::Staged);

    let promoted = client.promote(&receipt).await.unwrap();
    assert_eq!(
        promoted.observation.state,
        OkfKnowledgeObservedState::Promoted
    );
    assert_eq!(
        client.observe(&receipt).await.unwrap().observation.state,
        OkfKnowledgeObservedState::Promoted
    );
    assert_eq!(
        client.remove(&receipt).await.unwrap().observation.state,
        OkfKnowledgeObservedState::Removed
    );
}

#[test]
fn stage_request_revalidates_the_exact_borrowed_file_snapshot() {
    let mut changed = files();
    changed[0].content.extend_from_slice(b"changed");

    let error = OkfKnowledgeStageRequest::new(stage_spec(1), changed).unwrap_err();
    assert_eq!(error.code, "use.okf.contract_mismatch");
}

#[tokio::test]
async fn client_rejects_a_valid_binding_for_a_different_reviewed_candidate() {
    let wrong_receipt = receipt(2);
    let client = client_with_receipt(&wrong_receipt);

    let error = client
        .stage(OkfKnowledgeStageRequest::new(stage_spec(1), files()).unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_adapter_evidence_mismatch");
}

#[test]
fn adapter_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OkfKnowledgeClient>();
    assert_send_sync::<OkfKnowledgeStageSpec>();
    assert_send_sync::<OkfKnowledgeStageRequest>();
    assert_send_sync::<OkfKnowledgeBinding>();
}
