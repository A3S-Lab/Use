use a3s_use_core::{PluginCatalogRecord, UseError, UseResult};
use semver::Version;
use serde::Deserialize;
use tough::{Repository, TargetName};

use super::{
    hex_lower, validate_channel, ResolvedRemotePackage, TrustedRegistry, MAX_REMOTE_ARCHIVE_BYTES,
    REGISTRY_TARGET_SCHEMA_VERSION,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LegacyRegistryTargetMetadata {
    pub(super) schema_version: u32,
    pub(super) package_id: String,
    pub(super) version: String,
    pub(super) channel: String,
    pub(super) target: String,
}

#[derive(Debug, Clone)]
pub(super) enum RegistryTargetMetadata {
    Legacy(LegacyRegistryTargetMetadata),
    Catalog(Box<PluginCatalogRecord>),
}

impl RegistryTargetMetadata {
    pub(super) fn package_id(&self) -> &str {
        match self {
            Self::Legacy(metadata) => &metadata.package_id,
            Self::Catalog(record) => &record.package_id,
        }
    }

    pub(super) fn version(&self) -> &str {
        match self {
            Self::Legacy(metadata) => &metadata.version,
            Self::Catalog(record) => &record.version,
        }
    }

    pub(super) fn channel(&self) -> &str {
        match self {
            Self::Legacy(metadata) => &metadata.channel,
            Self::Catalog(record) => record.channel.as_str(),
        }
    }

    pub(super) fn target(&self) -> &str {
        match self {
            Self::Legacy(metadata) => &metadata.target,
            Self::Catalog(record) => &record.target,
        }
    }

    pub(super) fn catalog_record(&self) -> Option<&PluginCatalogRecord> {
        match self {
            Self::Legacy(_) => None,
            Self::Catalog(record) => Some(record),
        }
    }
}

pub(super) fn decode_registry_target_metadata(
    target_name: &TargetName,
    value: &serde_json::Value,
) -> UseResult<RegistryTargetMetadata> {
    if value.get("schema").is_some() {
        let bytes = serde_json::to_vec(value).map_err(|error| {
            UseError::new(
                "use.extension.registry_target_invalid",
                format!(
                    "Failed to decode A3S catalog metadata for TUF target '{}': {error}",
                    target_name.raw()
                ),
            )
        })?;
        let record = PluginCatalogRecord::from_json(&bytes).map_err(|error| {
            UseError::new(
                "use.extension.registry_target_invalid",
                format!(
                    "TUF target '{}' has an invalid signed plugin catalog record: {}",
                    target_name.raw(),
                    error.message
                ),
            )
        })?;
        return Ok(RegistryTargetMetadata::Catalog(Box::new(record)));
    }

    let metadata =
        serde_json::from_value::<LegacyRegistryTargetMetadata>(value.clone()).map_err(|error| {
            UseError::new(
                "use.extension.registry_target_invalid",
                format!(
                    "TUF target '{}' has invalid legacy A3S metadata: {error}",
                    target_name.raw()
                ),
            )
        })?;
    Ok(RegistryTargetMetadata::Legacy(metadata))
}

pub(super) fn validate_target_metadata(
    target_name: &TargetName,
    target: &tough::schema::Target,
    metadata: &RegistryTargetMetadata,
) -> UseResult<()> {
    if let RegistryTargetMetadata::Legacy(metadata) = metadata {
        if metadata.schema_version != REGISTRY_TARGET_SCHEMA_VERSION {
            return Err(UseError::new(
                "use.extension.registry_target_invalid",
                format!(
                    "TUF target '{}' uses unsupported A3S metadata schema {}.",
                    target_name.raw(),
                    metadata.schema_version
                ),
            ));
        }
    }
    if !super::super::valid_package_id(metadata.package_id()) {
        return Err(UseError::new(
            "use.extension.registry_target_invalid",
            format!(
                "TUF target '{}' has an invalid package ID.",
                target_name.raw()
            ),
        ));
    }
    Version::parse(metadata.version()).map_err(|error| {
        UseError::new(
            "use.extension.registry_target_invalid",
            format!(
                "TUF target '{}' has an invalid package version: {error}",
                target_name.raw()
            ),
        )
    })?;
    validate_channel(metadata.channel())?;
    validate_target_name(target_name, metadata)?;
    if target.length == 0 || target.length > MAX_REMOTE_ARCHIVE_BYTES {
        return Err(UseError::new(
            "use.extension.registry_target_invalid",
            format!(
                "TUF target '{}' exceeds the supported package size.",
                target_name.raw()
            ),
        ));
    }
    let digest = target.hashes.sha256.as_ref();
    if digest.len() != 32 {
        return Err(UseError::new(
            "use.extension.registry_target_invalid",
            format!(
                "TUF target '{}' does not have a valid SHA-256 digest.",
                target_name.raw()
            ),
        ));
    }
    if let Some(record) = metadata.catalog_record() {
        let target_digest = format!("sha256:{}", hex_lower(digest));
        if record.archive.target_name != target_name.raw()
            || record.archive.length != target.length
            || record.archive.sha256 != target_digest
        {
            return Err(UseError::new(
                "use.extension.registry_target_invalid",
                format!(
                    "TUF target '{}' does not match its signed catalog archive evidence.",
                    target_name.raw()
                ),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_target_name(
    target_name: &TargetName,
    metadata: &impl RegistryTargetIdentity,
) -> UseResult<()> {
    let raw = target_name.raw();
    if raw != target_name.resolved()
        || raw.starts_with('/')
        || raw.contains('\\')
        || raw.split('/').any(str::is_empty)
    {
        return Err(UseError::new(
            "use.extension.registry_target_invalid",
            format!("TUF target '{raw}' is not a portable package path."),
        ));
    }
    let archive = raw.rsplit('/').next().unwrap_or_default();
    if !(archive.ends_with(".tar.gz") || archive.ends_with(".tgz") || archive.ends_with(".zip")) {
        return Err(UseError::new(
            "use.extension.registry_target_invalid",
            format!("TUF target '{raw}' is not a supported package archive."),
        ));
    }
    let expected_prefix = format!(
        "extensions/{}/{}/{}/{}/",
        metadata.package_id(),
        metadata.version(),
        metadata.channel(),
        metadata.target()
    );
    if !raw.starts_with(&expected_prefix) {
        return Err(UseError::new(
            "use.extension.registry_target_invalid",
            format!("TUF target '{raw}' must be published below '{expected_prefix}'."),
        ));
    }
    Ok(())
}

pub(super) trait RegistryTargetIdentity {
    fn package_id(&self) -> &str;
    fn version(&self) -> &str;
    fn channel(&self) -> &str;
    fn target(&self) -> &str;
}

impl RegistryTargetIdentity for LegacyRegistryTargetMetadata {
    fn package_id(&self) -> &str {
        &self.package_id
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn channel(&self) -> &str {
        &self.channel
    }

    fn target(&self) -> &str {
        &self.target
    }
}

impl RegistryTargetIdentity for RegistryTargetMetadata {
    fn package_id(&self) -> &str {
        self.package_id()
    }

    fn version(&self) -> &str {
        self.version()
    }

    fn channel(&self) -> &str {
        self.channel()
    }

    fn target(&self) -> &str {
        self.target()
    }
}

pub(super) fn resolved_remote_package(
    registry: &TrustedRegistry,
    repository: &Repository,
    metadata: &RegistryTargetMetadata,
    target_name: &TargetName,
    target: &tough::schema::Target,
) -> ResolvedRemotePackage {
    let archive_name = target_name
        .raw()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    ResolvedRemotePackage {
        registry_name: registry.name.clone(),
        registry_url: registry.base_url.to_string(),
        root_sha256: registry.root_sha256.clone(),
        root_version: repository.root().signed.version.get(),
        timestamp_version: repository.timestamp().signed.version.get(),
        snapshot_version: repository.snapshot().signed.version.get(),
        targets_version: repository.targets().signed.version.get(),
        package_id: metadata.package_id().to_owned(),
        version: metadata.version().to_owned(),
        channel: metadata.channel().to_owned(),
        target: metadata.target().to_owned(),
        target_name: target_name.raw().to_string(),
        archive_name,
        length: target.length,
        sha256: hex_lower(target.hashes.sha256.as_ref()),
    }
}

pub(super) fn target_metadata_from_receipt(
    package_id: String,
    version: String,
    channel: String,
    target: String,
) -> LegacyRegistryTargetMetadata {
    LegacyRegistryTargetMetadata {
        schema_version: REGISTRY_TARGET_SCHEMA_VERSION,
        package_id,
        version,
        channel,
        target,
    }
}
