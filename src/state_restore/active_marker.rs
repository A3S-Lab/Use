use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};

use super::valid_sha256;

pub(crate) const CONTROL_INSTALLATION_RESTORE_ACTIVE_SCHEMA: &str =
    "a3s.use.control-installation-restore-active.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ControlInstallationRestoreActiveMarker {
    schema: String,
    plan_digest: String,
    operation_digest: String,
}

impl ControlInstallationRestoreActiveMarker {
    pub(crate) fn new(plan_digest: &str, operation_digest: &str) -> UseResult<Self> {
        let marker = Self {
            schema: CONTROL_INSTALLATION_RESTORE_ACTIVE_SCHEMA.to_owned(),
            plan_digest: plan_digest.to_owned(),
            operation_digest: operation_digest.to_owned(),
        };
        marker.validate()?;
        Ok(marker)
    }

    pub(crate) fn validate(&self) -> UseResult<()> {
        if self.schema != CONTROL_INSTALLATION_RESTORE_ACTIVE_SCHEMA
            || !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.operation_digest)
        {
            return Err(marker_invalid(
                "The active complete restore marker identity is invalid.",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_exact(
        &self,
        plan_digest: &str,
        operation_digest: &str,
    ) -> UseResult<()> {
        self.validate()?;
        if self.plan_digest != plan_digest || self.operation_digest != operation_digest {
            return Err(marker_invalid(
                "The active complete restore marker was rebound.",
            ));
        }
        Ok(())
    }

    pub(crate) fn plan_digest(&self) -> &str {
        &self.plan_digest
    }

    pub(crate) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| {
            marker_invalid(format!(
                "Failed to encode the active complete restore marker: {error}"
            ))
        })
    }
}

fn marker_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.state.restore_active_marker_invalid", message)
}
