use std::path::PathBuf;

use a3s_use_core::{InstallationId, UseError, UseResult};
use tokio::sync::{mpsc, oneshot};

use super::export::{self, VerifiedControlStoreExport};
use super::schema::{self, ControlStoreInspection, ControlStoreMetadata};

pub(super) const MAX_QUEUED_CONTROL_STORE_OPERATIONS: usize = 16;

#[derive(Debug, Clone)]
pub(super) struct ControlStoreExecutor {
    sender: mpsc::Sender<ControlStoreRequest>,
}

enum ControlStoreRequest {
    Initialize {
        installation: InstallationId,
        response: oneshot::Sender<UseResult<ControlStoreMetadata>>,
    },
    Inspect {
        installation: InstallationId,
        response: oneshot::Sender<UseResult<ControlStoreInspection>>,
    },
    Export {
        installation: InstallationId,
        response: oneshot::Sender<UseResult<Vec<u8>>>,
    },
    VerifyExport {
        bytes: Vec<u8>,
        installation: InstallationId,
        response: oneshot::Sender<UseResult<VerifiedControlStoreExport>>,
    },
}

impl ControlStoreExecutor {
    pub(super) fn new(database_path: PathBuf) -> UseResult<Self> {
        let (sender, receiver) = mpsc::channel(MAX_QUEUED_CONTROL_STORE_OPERATIONS);
        std::thread::Builder::new()
            .name("a3s-use-control-store".to_string())
            .spawn(move || run_worker(database_path, receiver))
            .map_err(|error| {
                executor_error(format!(
                    "The Control Store worker could not be started: {error}"
                ))
            })?;
        Ok(Self { sender })
    }

    pub(super) async fn initialize(
        &self,
        installation: InstallationId,
    ) -> UseResult<ControlStoreMetadata> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::Initialize {
            installation,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn inspect(
        &self,
        installation: InstallationId,
    ) -> UseResult<ControlStoreInspection> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::Inspect {
            installation,
            response,
        })
        .await?;
        receive(receiver).await
    }

    pub(super) async fn export(&self, installation: InstallationId) -> UseResult<Vec<u8>> {
        let (response, receiver) = oneshot::channel();
        self.send(ControlStoreRequest::Export {
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

    async fn send(&self, request: ControlStoreRequest) -> UseResult<()> {
        self.sender.send(request).await.map_err(|_| {
            executor_error("The Control Store worker stopped before accepting the operation.")
        })
    }
}

fn run_worker(database_path: PathBuf, mut receiver: mpsc::Receiver<ControlStoreRequest>) {
    while let Some(request) = receiver.blocking_recv() {
        match request {
            ControlStoreRequest::Initialize {
                installation,
                response,
            } => {
                let _ = response.send(schema::initialize(&database_path, &installation));
            }
            ControlStoreRequest::Inspect {
                installation,
                response,
            } => {
                let _ = response.send(schema::inspect(&database_path, &installation));
            }
            ControlStoreRequest::Export {
                installation,
                response,
            } => {
                let result = schema::inspect(&database_path, &installation)
                    .and_then(|inspection| export::encode(&inspection.metadata));
                let _ = response.send(result);
            }
            ControlStoreRequest::VerifyExport {
                bytes,
                installation,
                response,
            } => {
                let _ = response.send(export::verify(&bytes, &installation));
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
