//! Inactive A2 Control Store kernel.
//!
//! ADR-003 permits this backend to be qualified before the coordinated
//! authority cutover. Nothing in the production package lifecycle constructs
//! it yet, so the current JSON stores remain the only authority and no dual
//! write path exists.

use std::path::PathBuf;

use a3s_use_core::{InstallationId, UseResult};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceLock};

mod executor;
mod export;
mod filesystem;
mod schema;

use executor::ControlStoreExecutor;
use export::VerifiedControlStoreExport;
use filesystem::CONTROL_STORE_DATABASE_FILE;
use schema::{ControlStoreInspection, ControlStoreMetadata};

#[cfg(test)]
use executor::MAX_QUEUED_CONTROL_STORE_OPERATIONS;
#[cfg(test)]
use schema::{CONTROL_STORE_SCHEMA_VERSION, SQLITE_SYNCHRONOUS_FULL};

/// Installation-bound SQLite kernel kept inactive until the ADR-003 cutover.
///
/// Construction is side-effect free except for starting one bounded worker.
/// `initialize` is the only operation that may create a database, and it
/// rejects every existing legacy authority before doing so.
#[derive(Debug, Clone)]
struct ControlStore {
    installation: InstallationId,
    state_root: PathBuf,
    database_path: PathBuf,
    executor: ControlStoreExecutor,
}

impl ControlStore {
    fn new(state_root: impl Into<PathBuf>, installation: InstallationId) -> UseResult<Self> {
        installation.validate()?;
        let state_root = state_root.into();
        let database_path = state_root.join(CONTROL_STORE_DATABASE_FILE);
        let executor = ControlStoreExecutor::new(database_path.clone())?;
        Ok(Self {
            installation,
            state_root,
            database_path,
            executor,
        })
    }

    fn from_extension_paths(paths: &ExtensionPaths) -> UseResult<Self> {
        Self::new(
            paths.installation_state_root(),
            paths.installation().clone(),
        )
    }

    async fn initialize(&self) -> UseResult<ControlStoreMetadata> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_exclusive()
            .await?;
        filesystem::prepare_initialization(&self.state_root, &self.database_path).await?;
        let metadata = self.executor.initialize(self.installation.clone()).await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(metadata)
    }

    async fn inspect(&self) -> UseResult<ControlStoreInspection> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let inspection = self.executor.inspect(self.installation.clone()).await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(inspection)
    }

    async fn export(&self) -> UseResult<Vec<u8>> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let export = self.executor.export(self.installation.clone()).await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(export)
    }

    async fn verify_export(&self, bytes: Vec<u8>) -> UseResult<VerifiedControlStoreExport> {
        self.executor
            .verify_export(bytes, self.installation.clone())
            .await
    }

    #[cfg(test)]
    fn database_path(&self) -> &std::path::Path {
        &self.database_path
    }
}

#[cfg(test)]
mod tests;
