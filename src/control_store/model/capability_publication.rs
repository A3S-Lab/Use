use a3s_use_core::{InstallationId, PluginPackageId, UseResult, MAX_PLUGIN_PLAN_ITEMS};
use serde::{Deserialize, Serialize};

use super::{input_error, valid_sha256, ControlCapabilityStatus, ControlGeneration};

pub(in crate::control_store) const CONTROL_PUBLISHED_CAPABILITY_CURSOR_SCHEMA: &str =
    "a3s.use.control-published-capability-cursor.v1";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlPublishedCapabilityPackage {
    pub(in crate::control_store) package_id: String,
    pub(in crate::control_store) lifecycle_generation: u64,
    pub(in crate::control_store) package_digest: String,
    pub(in crate::control_store) manifest_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlPublishedCapabilityCursor {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) installation_generation: u64,
    pub(in crate::control_store) capability_generation: u64,
    pub(in crate::control_store) descriptor_digest: String,
    pub(in crate::control_store) receipt_digest: String,
    pub(in crate::control_store) packages: Vec<ControlPublishedCapabilityPackage>,
}

impl ControlPublishedCapabilityCursor {
    pub(in crate::control_store) fn from_generation(
        generation: &ControlGeneration,
        receipt_digest: impl Into<String>,
    ) -> UseResult<Self> {
        let lifecycles = generation
            .package_lifecycles
            .iter()
            .map(|lifecycle| {
                (
                    lifecycle.package_id.as_str(),
                    lifecycle.lifecycle_generation,
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let packages = generation
            .snapshot
            .packages
            .iter()
            .filter(|package| package.enabled)
            .map(|package| {
                let package_id = package.package_id();
                let lifecycle_generation =
                    lifecycles.get(package_id).copied().ok_or_else(|| {
                        input_error(
                            "A published capability package has no immutable lifecycle generation.",
                        )
                    })?;
                let package_digest = package
                    .package
                    .catalog
                    .record
                    .package
                    .sha256
                    .clone()
                    .ok_or_else(|| {
                        input_error("A published capability package has no package digest.")
                    })?;
                let manifest_digest = package
                    .package
                    .catalog
                    .record
                    .package
                    .manifest_sha256
                    .clone()
                    .ok_or_else(|| {
                        input_error("A published capability package has no manifest digest.")
                    })?;
                Ok(ControlPublishedCapabilityPackage {
                    package_id: package_id.to_owned(),
                    lifecycle_generation,
                    package_digest,
                    manifest_digest,
                })
            })
            .collect::<UseResult<Vec<_>>>()?;
        let cursor = Self {
            schema: CONTROL_PUBLISHED_CAPABILITY_CURSOR_SCHEMA.to_owned(),
            installation: generation.snapshot.installation.clone(),
            installation_generation: generation.snapshot.generation,
            capability_generation: generation.capability.generation,
            descriptor_digest: generation.capability.descriptor_digest.clone(),
            receipt_digest: receipt_digest.into(),
            packages,
        };
        cursor.validate()?;
        if generation.capability_status != ControlCapabilityStatus::Published
            || generation.capability_published_at_ms.is_none()
        {
            return Err(input_error(
                "Only one durably published Control capability generation can form a cursor.",
            ));
        }
        Ok(cursor)
    }

    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        if self.schema != CONTROL_PUBLISHED_CAPABILITY_CURSOR_SCHEMA
            || self.installation.validate().is_err()
            || self.installation_generation == 0
            || self.capability_generation == 0
            || !valid_sha256(&self.descriptor_digest)
            || !valid_sha256(&self.receipt_digest)
            || self.packages.len() > MAX_PLUGIN_PLAN_ITEMS
            || self
                .packages
                .windows(2)
                .any(|pair| pair[0].package_id >= pair[1].package_id)
            || self.packages.iter().any(|package| {
                PluginPackageId::parse(package.package_id.clone()).is_err()
                    || package.lifecycle_generation == 0
                    || !valid_sha256(&package.package_digest)
                    || !valid_sha256(&package.manifest_digest)
            })
        {
            return Err(input_error(
                "The published Control capability cursor is invalid or noncanonical.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn contains_incarnation(
        &self,
        package_id: &str,
        lifecycle_generation: u64,
    ) -> bool {
        self.packages
            .binary_search_by(|package| package.package_id.as_str().cmp(package_id))
            .ok()
            .and_then(|index| self.packages.get(index))
            .is_some_and(|package| package.lifecycle_generation == lifecycle_generation)
    }
}
