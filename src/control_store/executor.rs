use std::path::PathBuf;

use a3s_use_core::{InstallationId, UseError, UseResult};
use tokio::sync::{mpsc, oneshot};

use super::aggregate;
use super::export::{
    self, ControlStoreExport, GeneratedControlStoreExport, VerifiedControlStoreExport,
};
use super::model::{
    corruption_error, ClaimedControlEffect, ControlEffectClaim, ControlEffectObservation,
    ControlEffectRecord, ControlGeneration, ControlOperationRecord,
    ControlPublishedCapabilityCursor, ControlTransition, ReviewedControlOperation,
};
use super::schema::{self, ControlStoreInspection, ControlStoreMetadata};

pub(super) const MAX_QUEUED_CONTROL_STORE_OPERATIONS: usize = 16;

#[derive(Debug, Clone)]
pub(super) struct ControlStoreExecutor {
    sender: mpsc::Sender<ControlStoreRequest>,
}

enum ControlStoreRequest {
    Initialize {
        database_path: PathBuf,
        installation: InstallationId,
        response: oneshot::Sender<UseResult<ControlStoreMetadata>>,
    },
    Inspect {
        database_path: PathBuf,
        installation: InstallationId,
        response: oneshot::Sender<UseResult<ControlStoreInspection>>,
    },
    Export {
        database_path: PathBuf,
        installation: InstallationId,
        response: oneshot::Sender<UseResult<GeneratedControlStoreExport>>,
    },
    VerifyExport {
        bytes: Vec<u8>,
        installation: InstallationId,
        response: oneshot::Sender<UseResult<VerifiedControlStoreExport>>,
    },
    RegisterOperation {
        database_path: PathBuf,
        installation: InstallationId,
        reviewed: Box<ReviewedControlOperation>,
        response: oneshot::Sender<UseResult<ControlOperationRecord>>,
    },
    CancelOperation {
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
        plan_digest: String,
        result_digest: String,
        cancelled_at_ms: u64,
        response: oneshot::Sender<UseResult<ControlOperationRecord>>,
    },
    CommitTransition {
        database_path: PathBuf,
        installation: InstallationId,
        transition: ControlTransition,
        response: oneshot::Sender<UseResult<ControlGeneration>>,
    },
    ProjectTransition {
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
        committed_at_ms: u64,
        response: oneshot::Sender<UseResult<(ReviewedControlOperation, ControlTransition)>>,
    },
    Operation {
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
        response: oneshot::Sender<UseResult<Option<ControlOperationRecord>>>,
    },
    CurrentGeneration {
        database_path: PathBuf,
        installation: InstallationId,
        response: oneshot::Sender<UseResult<Option<ControlGeneration>>>,
    },
    PublishedCapability {
        database_path: PathBuf,
        installation: InstallationId,
        response: oneshot::Sender<UseResult<Option<ControlPublishedCapabilityCursor>>>,
    },
    Effects {
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
        response: oneshot::Sender<UseResult<Vec<ControlEffectRecord>>>,
    },
    ClaimNextEffect {
        database_path: PathBuf,
        installation: InstallationId,
        claim: ControlEffectClaim,
        response: oneshot::Sender<UseResult<Option<ClaimedControlEffect>>>,
    },
    RecordEffectObservation {
        database_path: PathBuf,
        installation: InstallationId,
        observation: ControlEffectObservation,
        response: oneshot::Sender<UseResult<bool>>,
    },
    CompleteOperation {
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
        plan_digest: String,
        result_digest: String,
        completed_at_ms: u64,
        response: oneshot::Sender<UseResult<ControlOperationRecord>>,
    },
    Restore {
        database_path: PathBuf,
        installation: InstallationId,
        export: ControlStoreExport,
        response: oneshot::Sender<UseResult<ControlStoreMetadata>>,
    },
}

impl ControlStoreExecutor {
    pub(super) fn new() -> UseResult<Self> {
        let (sender, receiver) = mpsc::channel(MAX_QUEUED_CONTROL_STORE_OPERATIONS);
        std::thread::Builder::new()
            .name("a3s-use-control-store".to_string())
            .spawn(move || run_worker(receiver))
            .map_err(|error| {
                executor_error(format!(
                    "The Control Store worker could not be started: {error}"
                ))
            })?;
        Ok(Self { sender })
    }

    pub(super) async fn initialize(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
    ) -> UseResult<ControlStoreMetadata> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::Initialize {
            database_path,
            installation,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn inspect(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
    ) -> UseResult<ControlStoreInspection> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::Inspect {
            database_path,
            installation,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn export(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
    ) -> UseResult<GeneratedControlStoreExport> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::Export {
            database_path,
            installation,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn verify_export(
        &self,
        bytes: Vec<u8>,
        installation: InstallationId,
    ) -> UseResult<VerifiedControlStoreExport> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::VerifyExport {
            bytes,
            installation,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn register_operation(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        reviewed: ReviewedControlOperation,
    ) -> UseResult<ControlOperationRecord> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::RegisterOperation {
            database_path,
            installation,
            reviewed: Box::new(reviewed),
            response,
        })
        .await?;
        receive(receiver).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn cancel_operation(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
        plan_digest: String,
        result_digest: String,
        cancelled_at_ms: u64,
    ) -> UseResult<ControlOperationRecord> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::CancelOperation {
            database_path,
            installation,
            operation_id,
            plan_digest,
            result_digest,
            cancelled_at_ms,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn commit_transition(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        transition: ControlTransition,
    ) -> UseResult<ControlGeneration> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::CommitTransition {
            database_path,
            installation,
            transition,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn project_transition(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
        committed_at_ms: u64,
    ) -> UseResult<(ReviewedControlOperation, ControlTransition)> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::ProjectTransition {
            database_path,
            installation,
            operation_id,
            committed_at_ms,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn operation(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
    ) -> UseResult<Option<ControlOperationRecord>> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::Operation {
            database_path,
            installation,
            operation_id,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn current_generation(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
    ) -> UseResult<Option<ControlGeneration>> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::CurrentGeneration {
            database_path,
            installation,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn published_capability(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
    ) -> UseResult<Option<ControlPublishedCapabilityCursor>> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::PublishedCapability {
            database_path,
            installation,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn effects(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
    ) -> UseResult<Vec<ControlEffectRecord>> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::Effects {
            database_path,
            installation,
            operation_id,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn claim_next_effect(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        claim: ControlEffectClaim,
    ) -> UseResult<Option<ClaimedControlEffect>> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::ClaimNextEffect {
            database_path,
            installation,
            claim,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn record_effect_observation(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        observation: ControlEffectObservation,
    ) -> UseResult<bool> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::RecordEffectObservation {
            database_path,
            installation,
            observation,
            response,
        })
        .await?;
        receive(receiver).await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn complete_operation(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        operation_id: String,
        plan_digest: String,
        result_digest: String,
        completed_at_ms: u64,
    ) -> UseResult<ControlOperationRecord> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::CompleteOperation {
            database_path,
            installation,
            operation_id,
            plan_digest,
            result_digest,
            completed_at_ms,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn restore(
        &self,
        database_path: PathBuf,
        installation: InstallationId,
        export: ControlStoreExport,
    ) -> UseResult<ControlStoreMetadata> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::Restore {
            database_path,
            installation,
            export,
            response,
        })
        .await?;
        receive(receiver).await
    }

    async fn send(&self, request: ControlStoreRequest) -> UseResult<()> {
        self.sender.send(request).await.map_err(|_| {
            executor_error("The Control Store worker stopped before accepting the operation.")
        })
    }
}

fn run_worker(mut receiver: mpsc::Receiver<ControlStoreRequest>) {
    while let Some(request) = receiver.blocking_recv() {
        match request {
            ControlStoreRequest::Initialize {
                database_path,
                installation,
                response,
            } => {
                let _ = response.send(schema::initialize(&database_path, &installation));
            }
            ControlStoreRequest::Inspect {
                database_path,
                installation,
                response,
            } => {
                let result =
                    schema::inspect(&database_path, &installation).and_then(|inspection| {
                        let (metadata, authority) =
                            aggregate::export_snapshot(&database_path, &installation)?;
                        if metadata != inspection.metadata {
                            return Err(a3s_use_core::UseError::new(
                                "use.control_store.concurrent_change",
                                "The Control Store changed while its aggregate was inspected.",
                            ));
                        }
                        export::encode(&metadata, authority).map_err(|error| {
                            corruption_error(format!(
                                "The live Control Store aggregate is semantically invalid: {}",
                                error.message
                            ))
                        })?;
                        Ok(inspection)
                    });
                let _ = response.send(result);
            }
            ControlStoreRequest::Export {
                database_path,
                installation,
                response,
            } => {
                let result = aggregate::export_snapshot(&database_path, &installation)
                    .and_then(|(metadata, authority)| export::encode(&metadata, authority));
                let _ = response.send(result);
            }
            ControlStoreRequest::VerifyExport {
                bytes,
                installation,
                response,
            } => {
                let _ = response.send(export::verify(&bytes, &installation));
            }
            ControlStoreRequest::RegisterOperation {
                database_path,
                installation,
                reviewed,
                response,
            } => {
                let _ = response.send(aggregate::register_operation(
                    &database_path,
                    &installation,
                    &reviewed,
                ));
            }
            ControlStoreRequest::CancelOperation {
                database_path,
                installation,
                operation_id,
                plan_digest,
                result_digest,
                cancelled_at_ms,
                response,
            } => {
                let _ = response.send(aggregate::cancel_operation(
                    &database_path,
                    &installation,
                    &operation_id,
                    &plan_digest,
                    &result_digest,
                    cancelled_at_ms,
                ));
            }
            ControlStoreRequest::CommitTransition {
                database_path,
                installation,
                transition,
                response,
            } => {
                let _ = response.send(aggregate::commit_transition(
                    &database_path,
                    &installation,
                    &transition,
                ));
            }
            ControlStoreRequest::ProjectTransition {
                database_path,
                installation,
                operation_id,
                committed_at_ms,
                response,
            } => {
                let _ = response.send(aggregate::project_transition(
                    &database_path,
                    &installation,
                    &operation_id,
                    committed_at_ms,
                ));
            }
            ControlStoreRequest::Operation {
                database_path,
                installation,
                operation_id,
                response,
            } => {
                let _ = response.send(aggregate::operation(
                    &database_path,
                    &installation,
                    &operation_id,
                ));
            }
            ControlStoreRequest::CurrentGeneration {
                database_path,
                installation,
                response,
            } => {
                let _ = response.send(aggregate::current_generation(&database_path, &installation));
            }
            ControlStoreRequest::PublishedCapability {
                database_path,
                installation,
                response,
            } => {
                let _ = response.send(aggregate::published_capability(
                    &database_path,
                    &installation,
                ));
            }
            ControlStoreRequest::Effects {
                database_path,
                installation,
                operation_id,
                response,
            } => {
                let _ = response.send(aggregate::effects(
                    &database_path,
                    &installation,
                    &operation_id,
                ));
            }
            ControlStoreRequest::ClaimNextEffect {
                database_path,
                installation,
                claim,
                response,
            } => {
                let _ = response.send(aggregate::claim_next_effect(
                    &database_path,
                    &installation,
                    &claim,
                ));
            }
            ControlStoreRequest::RecordEffectObservation {
                database_path,
                installation,
                observation,
                response,
            } => {
                let _ = response.send(aggregate::record_effect_observation(
                    &database_path,
                    &installation,
                    &observation,
                ));
            }
            ControlStoreRequest::CompleteOperation {
                database_path,
                installation,
                operation_id,
                plan_digest,
                result_digest,
                completed_at_ms,
                response,
            } => {
                let _ = response.send(aggregate::complete_operation(
                    &database_path,
                    &installation,
                    &operation_id,
                    &plan_digest,
                    &result_digest,
                    completed_at_ms,
                ));
            }
            ControlStoreRequest::Restore {
                database_path,
                installation,
                export,
                response,
            } => {
                let _ = response.send(aggregate::restore_export(
                    &database_path,
                    &installation,
                    &export,
                ));
            }
        }
    }
}

async fn receive<T>(receiver: oneshot::Receiver<UseResult<T>>) -> UseResult<T> {
    receiver.await.map_err(|_| {
        executor_error("The Control Store worker stopped before returning the operation result.")
    })?
}

fn executor_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.executor_unavailable", message)
}
