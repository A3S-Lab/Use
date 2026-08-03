use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::validation::valid_package_id;

const PACKAGE_DEPENDENCY_ERROR: &str = "use.plugin.package_dependency_invalid";
const MAX_VERSION_REQUIREMENT_BYTES: usize = 256;
pub const MAX_PLUGIN_PACKAGE_DEPENDENCIES: usize = 128;

/// A versioned dependency on another cognitive package.
///
/// Package dependencies name only a canonical package ID and SemVer range.
/// Registry selection remains host-owned, so manifests cannot smuggle a
/// mutable download endpoint into the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPackageDependency {
    pub package_id: String,
    pub version_requirement: String,
}

impl PluginPackageDependency {
    pub fn new(
        package_id: impl Into<String>,
        version_requirement: impl Into<String>,
    ) -> UseResult<Self> {
        let dependency = Self {
            package_id: package_id.into(),
            version_requirement: version_requirement.into(),
        };
        dependency.validate()?;
        Ok(dependency)
    }

    pub fn validate(&self) -> UseResult<()> {
        if !valid_package_id(&self.package_id)
            || self.version_requirement.is_empty()
            || self.version_requirement.len() > MAX_VERSION_REQUIREMENT_BYTES
        {
            return Err(dependency_error(
                "A cognitive-package dependency ID or version requirement is invalid.",
            ));
        }
        let requirement = VersionReq::parse(&self.version_requirement).map_err(|_| {
            dependency_error(
                "A cognitive-package dependency must use a canonical semantic-version requirement.",
            )
        })?;
        if requirement.to_string() != self.version_requirement {
            return Err(dependency_error(
                "A cognitive-package dependency must use canonical semantic-version syntax.",
            ));
        }
        Ok(())
    }

    pub fn validate_set(owner_package_id: &str, dependencies: &[Self]) -> UseResult<()> {
        if !valid_package_id(owner_package_id)
            || dependencies.len() > MAX_PLUGIN_PACKAGE_DEPENDENCIES
        {
            return Err(dependency_error(
                "The cognitive-package dependency set exceeds its identity or item bound.",
            ));
        }
        let mut previous = None;
        for dependency in dependencies {
            dependency.validate()?;
            if dependency.package_id == owner_package_id
                || previous
                    .as_ref()
                    .is_some_and(|package_id| package_id >= &dependency.package_id)
            {
                return Err(dependency_error(
                    "Cognitive-package dependencies must exclude self and be sorted uniquely by package ID.",
                ));
            }
            previous = Some(dependency.package_id.clone());
        }
        Ok(())
    }

    pub fn matches(&self, version: &str) -> UseResult<bool> {
        self.validate()?;
        let parsed_version = Version::parse(version).map_err(|_| {
            dependency_error("A resolved cognitive-package version is not valid SemVer.")
        })?;
        if parsed_version.to_string() != version {
            return Err(dependency_error(
                "A resolved cognitive-package version is not canonical SemVer.",
            ));
        }
        let requirement = VersionReq::parse(&self.version_requirement).map_err(|_| {
            dependency_error("A cognitive-package dependency requirement is invalid.")
        })?;
        Ok(requirement.matches(&parsed_version))
    }

    pub(crate) fn parsed_requirement(&self) -> UseResult<VersionReq> {
        self.validate()?;
        VersionReq::parse(&self.version_requirement)
            .map_err(|_| dependency_error("A cognitive-package dependency requirement is invalid."))
    }
}

fn dependency_error(message: impl Into<String>) -> UseError {
    UseError::new(PACKAGE_DEPENDENCY_ERROR, message)
}
