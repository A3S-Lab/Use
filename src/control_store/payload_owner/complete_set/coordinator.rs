use std::path::PathBuf;

use a3s_use_core::UseResult;

use super::super::host_projection::VerifiedControlHostProjectionSnapshot;
use super::super::knowledge::VerifiedControlKnowledgePayloadSnapshot;
use super::super::observations::VerifiedControlObservationPayloadSnapshot;
use super::super::restore_coordinator::VerifiedControlRestoreCoordinatorSnapshot;
use super::super::{ControlPayloadOwnerRegistry, ControlPayloadSnapshotSession};
use super::{
    archive, nested_snapshot_invalid, snapshot_invalid, snapshot_io, snapshot_path_invalid,
    CapturedOwnerSnapshots, ControlInstallationSnapshotManifest,
};
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

#[derive(Debug)]
pub(in crate::control_store) struct VerifiedControlInstallationSnapshot {
    pub(super) manifest: ControlInstallationSnapshotManifest,
    pub(super) control_export: Vec<u8>,
    _host_projection: VerifiedControlHostProjectionSnapshot,
    _knowledge: VerifiedControlKnowledgePayloadSnapshot,
    _observations: VerifiedControlObservationPayloadSnapshot,
    _restore_coordinator: VerifiedControlRestoreCoordinatorSnapshot,
    _temporary: tempfile::TempDir,
}

impl VerifiedControlInstallationSnapshot {
    pub(in crate::control_store) async fn verify_offline(
        registry: ControlPayloadOwnerRegistry,
        archive_path: impl Into<PathBuf>,
    ) -> UseResult<Self> {
        registry
            .validate()
            .map_err(|error| nested_snapshot_invalid("owner registry", error))?;
        let archive_path = archive_path.into();
        let extraction_registry = registry.clone();
        let extracted = tokio::task::spawn_blocking(move || {
            archive::extract(&extraction_registry, &archive_path)
        })
        .await
        .map_err(|error| {
            snapshot_invalid(format!(
                "The complete snapshot verification worker did not complete: {error}"
            ))
        })??;
        let binding = &extracted.manifest.snapshot_set.binding;
        binding
            .verify_control_export(&registry, &extracted.control_export)
            .map_err(|error| nested_snapshot_invalid("Control export", error))?;
        let host_projection = extracted
            .manifest
            .host_projection
            .verify_offline(
                &registry,
                binding,
                &extracted.control_export,
                extracted.host_projection.clone(),
            )
            .await
            .map_err(|error| nested_snapshot_invalid("Host projection", error))?;
        let knowledge = extracted
            .manifest
            .knowledge
            .verify_offline(
                &registry,
                binding,
                &extracted.control_export,
                extracted.knowledge.clone(),
            )
            .await
            .map_err(|error| nested_snapshot_invalid("Knowledge payload", error))?;
        let observations = extracted
            .manifest
            .observations
            .verify_offline(
                &registry,
                binding,
                &extracted.control_export,
                extracted.observations.clone(),
            )
            .await
            .map_err(|error| nested_snapshot_invalid("observation payload", error))?;
        let restore_coordinator = extracted
            .manifest
            .restore_coordinator
            .verify_offline(
                &registry,
                binding,
                &extracted.control_export,
                extracted.restore_coordinator.clone(),
            )
            .await
            .map_err(|error| nested_snapshot_invalid("Restore Coordinator", error))?;
        Ok(Self {
            manifest: extracted.manifest,
            control_export: extracted.control_export,
            _host_projection: host_projection,
            _knowledge: knowledge,
            _observations: observations,
            _restore_coordinator: restore_coordinator,
            _temporary: extracted.temporary,
        })
    }

    #[cfg(test)]
    pub(in crate::control_store) fn manifest(&self) -> &ControlInstallationSnapshotManifest {
        &self.manifest
    }

    #[cfg(test)]
    pub(in crate::control_store) fn control_export(&self) -> &[u8] {
        &self.control_export
    }
}

impl ControlPayloadSnapshotSession {
    pub(in crate::control_store) async fn snapshot_complete_set(
        &self,
        destination: impl Into<PathBuf>,
        knowledge_policy: OkfKnowledgeStoragePolicy,
        created_at_ms: u64,
    ) -> UseResult<ControlInstallationSnapshotManifest> {
        let destination = archive::resolve_destination(destination.into(), self.owned_roots())?;
        let parent = destination
            .parent()
            .ok_or_else(|| {
                snapshot_path_invalid("The complete snapshot destination has no parent directory.")
            })?
            .to_path_buf();
        let staging = tempfile::Builder::new()
            .prefix(".a3s-use-control-snapshot-payloads-")
            .tempdir_in(&parent)
            .map_err(|error| snapshot_io(format!("create payload staging directory: {error}")))?;
        let host_path = staging.path().join("host-projection.payload");
        let knowledge_path = staging.path().join("knowledge.sqlite3");
        let observations_path = staging.path().join("observations.payload");
        let restore_path = staging.path().join("restore-coordinator.payload");

        let host_projection = self
            .snapshot_host_projection(host_path.clone(), created_at_ms)
            .await?;
        let knowledge = self
            .snapshot_knowledge(knowledge_policy, knowledge_path.clone(), created_at_ms)
            .await?;
        let observations = self
            .snapshot_planning_and_diagnostics(observations_path.clone(), created_at_ms)
            .await?;
        let restore_coordinator = self
            .snapshot_restore_coordinator(restore_path.clone(), created_at_ms)
            .await?;
        let snapshot_set = self.complete(vec![
            host_projection.receipt.clone(),
            knowledge.receipt.clone(),
            observations.receipt.clone(),
            restore_coordinator.receipt.clone(),
        ])?;
        let control_export_bytes = u64::try_from(self.control_export().len())
            .map_err(|_| snapshot_invalid("The Control export byte count overflowed."))?;
        let manifest = ControlInstallationSnapshotManifest::new(
            self.registry(),
            created_at_ms,
            control_export_bytes,
            snapshot_set,
            CapturedOwnerSnapshots {
                host_projection,
                knowledge,
                observations,
                restore_coordinator,
            },
        )?;
        let sources = archive::ArchiveSources {
            control_export: self.control_export().to_vec(),
            host_projection: host_path,
            knowledge: knowledge_path,
            observations: observations_path,
            restore_coordinator: restore_path,
        };
        let writing_manifest = manifest.clone();
        let temporary = tokio::task::spawn_blocking(move || {
            archive::write_temporary(&parent, &writing_manifest, &sources)
        })
        .await
        .map_err(|error| {
            snapshot_io(format!(
                "The complete snapshot writer did not complete: {error}"
            ))
        })??;

        let verified = VerifiedControlInstallationSnapshot::verify_offline(
            self.registry().clone(),
            temporary.path().to_path_buf(),
        )
        .await?;
        if verified.manifest != manifest || verified.control_export != self.control_export() {
            return Err(snapshot_invalid(
                "The staged complete snapshot differs from its captured owner set.",
            ));
        }
        archive::publish(temporary, &destination)?;
        Ok(manifest)
    }
}
