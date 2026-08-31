use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[cfg(test)]
use std::path::Path;

use a3s_use_core::{
    InstallationId, PluginHostPackageState, PluginManagedScope, PluginOperationPlanEnvelope,
    UseError, UseResult,
};

use super::host_store::{
    StoredPluginHostCancellation, StoredPluginHostPlan, StoredPluginHostRequest,
};

mod filesystem;
#[cfg(test)]
mod fixtures;

pub(crate) use filesystem::scan_host_projection_snapshot;
pub(crate) const HOST_PROJECTION_SNAPSHOT_MAX_RECORD_BYTES: u64 =
    super::host_store::MAX_HOST_RECORD_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HostProjectionSnapshotRecordKind {
    Request,
    Cancellation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostProjectionSnapshotOutcome {
    pub(crate) completed_at_ms: u64,
    pub(crate) operation_result_digest: String,
    pub(crate) state: PluginHostPackageState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostProjectionSnapshotRequest {
    pub(crate) scope: PluginManagedScope,
    pub(crate) request_id: String,
    pub(crate) request_digest: String,
    pub(crate) package_id: String,
    pub(crate) envelope: Option<PluginOperationPlanEnvelope>,
    pub(crate) reviewed_state: Option<PluginHostPackageState>,
    pub(crate) expected_package_generation: Option<u64>,
    pub(crate) outcome: Option<HostProjectionSnapshotOutcome>,
}

impl HostProjectionSnapshotRequest {
    fn from_stored(stored: &StoredPluginHostRequest) -> Self {
        let (package_id, reviewed_state, expected_package_generation) = match &stored.plan {
            StoredPluginHostPlan::Graph { request, .. } => {
                (request.package_id.to_string(), None, None)
            }
            StoredPluginHostPlan::Enablement {
                request, result, ..
            } => (
                request.package_id.to_string(),
                Some(result.state.clone()),
                Some(request.expected_package_generation),
            ),
        };
        Self {
            scope: stored.plan.scope().clone(),
            request_id: stored.plan.request_id().to_owned(),
            request_digest: stored.request_digest.clone(),
            package_id,
            envelope: stored.plan.envelope().cloned(),
            reviewed_state,
            expected_package_generation,
            outcome: stored
                .outcome
                .as_ref()
                .map(|outcome| HostProjectionSnapshotOutcome {
                    completed_at_ms: outcome.completed_at_ms,
                    operation_result_digest: outcome.operation_result_digest.clone(),
                    state: outcome.state.clone(),
                }),
        }
    }

    pub(crate) fn operation_binding(&self) -> Option<(&str, &str)> {
        self.envelope.as_ref().map(|envelope| {
            (
                envelope.plan.operation_id.as_str(),
                envelope.plan_digest.as_str(),
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostProjectionSnapshotCancellation {
    pub(crate) scope_digest: String,
    pub(crate) request_id: String,
    pub(crate) operation_id: String,
    pub(crate) plan_digest: String,
    pub(crate) cancelled_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostProjectionSnapshotRecord {
    Request(Box<HostProjectionSnapshotRequest>),
    Cancellation(HostProjectionSnapshotCancellation),
}

impl HostProjectionSnapshotRecord {
    pub(crate) const fn kind(&self) -> HostProjectionSnapshotRecordKind {
        match self {
            Self::Request(_) => HostProjectionSnapshotRecordKind::Request,
            Self::Cancellation(_) => HostProjectionSnapshotRecordKind::Cancellation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostProjectionSnapshotSource {
    pub(crate) source: PathBuf,
    pub(crate) logical_path: String,
    pub(crate) kind: HostProjectionSnapshotRecordKind,
    pub(crate) length: u64,
    pub(crate) sha256: String,
    pub(crate) record: HostProjectionSnapshotRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostProjectionSnapshotInventory {
    pub(crate) sources: Vec<HostProjectionSnapshotSource>,
    pub(crate) validated_index_records: u64,
}

pub(crate) fn validate_host_projection_snapshot_record(
    logical_path: &str,
    bytes: &[u8],
    installation: &InstallationId,
) -> UseResult<HostProjectionSnapshotRecord> {
    installation
        .validate()
        .map_err(|_| host_snapshot_invalid("The Host projection installation is invalid."))?;
    let segments = logical_path.split('/').collect::<Vec<_>>();
    let [scope_digest, family, file_name] = segments.as_slice() else {
        return Err(host_snapshot_invalid(
            "A Host projection record has a non-canonical logical path.",
        ));
    };
    if !valid_hex_digest(scope_digest) || bytes.is_empty() {
        return Err(host_snapshot_invalid(
            "A Host projection record path or payload is invalid.",
        ));
    }

    let record = match *family {
        "requests" => {
            let stored: StoredPluginHostRequest = serde_json::from_slice(bytes).map_err(|_| {
                host_snapshot_invalid("A Host request record is not valid owner-schema JSON.")
            })?;
            stored.validate().map_err(|_| {
                host_snapshot_invalid("A Host request record failed owner-native validation.")
            })?;
            require_scope(stored.plan.scope(), scope_digest, installation)?;
            if *file_name
                != format!(
                    "{}.json",
                    super::host_store::sha256_hex(stored.plan.request_id().as_bytes())
                )
            {
                return Err(host_snapshot_invalid(
                    "A Host request record moved across its owned path.",
                ));
            }
            HostProjectionSnapshotRecord::Request(Box::new(
                HostProjectionSnapshotRequest::from_stored(&stored),
            ))
        }
        "cancellations" => {
            let stored: StoredPluginHostCancellation =
                serde_json::from_slice(bytes).map_err(|_| {
                    host_snapshot_invalid("A Host cancellation is not valid owner-schema JSON.")
                })?;
            stored.validate().map_err(|_| {
                host_snapshot_invalid("A Host cancellation failed owner-native validation.")
            })?;
            let expected_file = format!(
                "{}.json",
                super::host_store::operation_binding_digest(
                    &stored.operation_id,
                    &stored.plan_digest,
                )
            );
            if *file_name != expected_file {
                return Err(host_snapshot_invalid(
                    "A Host cancellation moved across its canonical binding path.",
                ));
            }
            HostProjectionSnapshotRecord::Cancellation(HostProjectionSnapshotCancellation {
                scope_digest: (*scope_digest).to_owned(),
                request_id: stored.request_id,
                operation_id: stored.operation_id,
                plan_digest: stored.plan_digest,
                cancelled_at_ms: stored.cancelled_at_ms,
            })
        }
        _ => {
            return Err(host_snapshot_invalid(
                "A Host projection record is outside its semantic inventory.",
            ))
        }
    };
    Ok(record)
}

/// Validate cross-record relations after every record has passed the owning
/// Host decoder. Cancellation scope is resolved from its immutable request so
/// the cancellation schema does not acquire a second scope authority.
pub(crate) fn validate_host_projection_snapshot_set(
    records: &[HostProjectionSnapshotRecord],
    installation: &InstallationId,
) -> UseResult<()> {
    installation
        .validate()
        .map_err(|_| host_snapshot_invalid("The Host projection installation is invalid."))?;
    let mut requests = BTreeMap::new();
    let mut bindings = BTreeMap::new();
    for record in records.iter() {
        let HostProjectionSnapshotRecord::Request(request) = record else {
            continue;
        };
        require_installation(&request.scope, installation)?;
        let scope_digest = scope_digest(&request.scope)?;
        if requests
            .insert((scope_digest.clone(), request.request_id.clone()), request)
            .is_some()
        {
            return Err(host_snapshot_invalid(
                "The Host projection contains duplicate request identities.",
            ));
        }
        if let Some((operation_id, plan_digest)) = request.operation_binding() {
            if bindings
                .insert(
                    (
                        scope_digest,
                        operation_id.to_owned(),
                        plan_digest.to_owned(),
                    ),
                    request,
                )
                .is_some()
            {
                return Err(host_snapshot_invalid(
                    "The Host projection contains duplicate operation bindings.",
                ));
            }
        } else if request.outcome.is_some() {
            return Err(host_snapshot_invalid(
                "A no-change Host request carries an operation outcome.",
            ));
        }
    }

    let mut cancellations = BTreeSet::new();
    for record in records {
        let HostProjectionSnapshotRecord::Cancellation(cancellation) = record else {
            continue;
        };
        let candidates = bindings
            .iter()
            .filter(|((_, operation_id, plan_digest), request)| {
                operation_id == &cancellation.operation_id
                    && plan_digest == &cancellation.plan_digest
                    && request.request_id == cancellation.request_id
            })
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Err(host_snapshot_invalid(
                "A Host cancellation has no unique immutable request binding.",
            ));
        }
        let ((scope_digest, _, _), request) = candidates[0];
        if scope_digest != &cancellation.scope_digest || request.outcome.is_some() {
            return Err(host_snapshot_invalid(
                "A Host cancellation conflicts with its request scope or outcome.",
            ));
        }
        if !cancellations.insert((
            scope_digest.clone(),
            cancellation.operation_id.clone(),
            cancellation.plan_digest.clone(),
        )) {
            return Err(host_snapshot_invalid(
                "The Host projection contains duplicate cancellations.",
            ));
        }
    }
    Ok(())
}

pub(super) fn require_scope(
    scope: &PluginManagedScope,
    expected_digest: &str,
    installation: &InstallationId,
) -> UseResult<()> {
    require_installation(scope, installation)?;
    if scope_digest(scope)? != expected_digest {
        return Err(host_snapshot_invalid(
            "A Host projection scope moved across its owned path.",
        ));
    }
    Ok(())
}

pub(super) fn require_installation(
    scope: &PluginManagedScope,
    installation: &InstallationId,
) -> UseResult<()> {
    scope
        .validate()
        .map_err(|_| host_snapshot_invalid("A Host managed scope is invalid."))?;
    installation
        .ensure_same(&scope.plan_scope())
        .map_err(|_| host_snapshot_invalid("A Host managed scope belongs to another installation."))
}

pub(super) fn scope_digest(scope: &PluginManagedScope) -> UseResult<String> {
    scope
        .descriptor_digest()
        .map_err(|_| host_snapshot_invalid("A Host managed scope digest is invalid."))?
        .strip_prefix("sha256:")
        .map(str::to_owned)
        .ok_or_else(|| host_snapshot_invalid("A Host managed scope digest is malformed."))
}

pub(super) fn valid_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn host_snapshot_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.host_projection_snapshot_invalid",
        message,
    )
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostProjectionSnapshotFixtureOutcome {
    Completed {
        completed_at_ms: u64,
        result_digest: String,
    },
    Cancelled {
        cancelled_at_ms: u64,
    },
}

#[cfg(test)]
pub(crate) async fn write_host_projection_snapshot_fixture(
    state_root: &Path,
    installation: &InstallationId,
    envelope: PluginOperationPlanEnvelope,
    state: PluginHostPackageState,
    outcome: HostProjectionSnapshotFixtureOutcome,
) -> UseResult<()> {
    fixtures::write_fixture(state_root, installation, envelope, state, outcome).await
}

#[cfg(test)]
pub(crate) async fn write_host_projection_no_change_fixture(
    state_root: &Path,
    installation: &InstallationId,
    package_id: a3s_use_core::PluginPackageId,
    state: PluginHostPackageState,
) -> UseResult<()> {
    fixtures::write_no_change_fixture(state_root, installation, package_id, state).await
}

#[cfg(test)]
pub(crate) async fn host_projection_snapshot_fixture_sources(
    state_root: &Path,
    installation: &InstallationId,
) -> UseResult<Vec<(String, Vec<u8>)>> {
    fixtures::fixture_sources(state_root, installation).await
}
