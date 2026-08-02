use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{UseError, UseResult};

use super::contract_error;
use super::validation::valid_segment;

const PACKAGE_ID_ERROR: &str = "use.plugin.package_id_invalid";

/// Canonical A3S Use package identity in `<publisher>/<name>` form.
///
/// Host and control-plane contracts use this value object instead of
/// restating package-ID validation at every boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct PluginPackageId(String);

impl PluginPackageId {
    pub fn parse(value: impl Into<String>) -> UseResult<Self> {
        let value = value.into();
        if !Self::is_valid(&value) {
            return Err(package_id_error(
                "Plugin package IDs must use canonical '<publisher>/<name>' form.",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn component_id(&self) -> String {
        format!("use/{}", self.0)
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub(crate) fn is_valid(value: &str) -> bool {
        let Some((publisher, name)) = value.split_once('/') else {
            return false;
        };
        !name.contains('/') && valid_segment(publisher) && valid_segment(name)
    }
}

impl<'de> Deserialize<'de> for PluginPackageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(D::Error::custom)
    }
}

impl fmt::Display for PluginPackageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for PluginPackageId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for PluginPackageId {
    type Err = UseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<String> for PluginPackageId {
    type Error = UseError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

fn package_id_error(message: impl Into<String>) -> UseError {
    contract_error(PACKAGE_ID_ERROR, message)
}
