use std::path::Path;

use a3s_use_core::{UseError, UseResult, VerifiedPluginCatalogRecord};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::fs;
use tough::{Repository, TargetName};

use super::catalog::{load_verified_cached_repository, record_catalog_refresh};
use super::download::download_and_cache_target;
use super::target_cache::stage_cached_target;
use super::{
    hex_lower, load_repository, verified_registry_metadata, RemoteRegistryAccess, TrustedRegistry,
};

pub const COGNITIVE_PACKAGE_PRESENTATION_INDEX_SCHEMA: &str =
    "a3s.use.cognitive-package-presentation-index.v1";
pub const COGNITIVE_PACKAGE_PRESENTATION_SCHEMA: &str = "a3s.use.cognitive-package-presentation.v1";
pub const MAX_COGNITIVE_PACKAGE_PRESENTATION_MEDIA: usize = 8;
pub const MAX_COGNITIVE_PACKAGE_PRESENTATION_BYTES: u64 = 256 * 1024;
pub const MAX_COGNITIVE_PACKAGE_MEDIA_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PRESENTATION_INDEX_BYTES: u64 = 2 * 1024 * 1024;
const PRESENTATION_INDEX_TARGET: &str = "cognitive/presentation-index-v1.json";
const PRESENTATION_INDEX_CUSTOM_KEY: &str = "a3sCognitivePresentationIndex";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CognitivePackageFormFactor {
    Mobile,
    Desktop,
    Spatial,
    Document,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CognitivePackageMediaKind {
    Image,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitivePackagePresentationIndexV1 {
    pub schema: String,
    pub entries: Vec<CognitivePackagePresentationRecordV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitivePackagePresentationRecordV1 {
    pub package_id: String,
    pub version: String,
    pub channel: String,
    pub host_target: String,
    pub catalog_record_digest: String,
    pub descriptor_target_name: String,
    pub descriptor_sha256: String,
    pub descriptor_byte_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitivePackagePresentationV1 {
    pub schema: String,
    pub package_id: String,
    pub locale: String,
    pub short_title: String,
    pub short_summary: String,
    pub form_factors: Vec<CognitivePackageFormFactor>,
    pub media: Vec<CognitivePackagePresentationMediaV1>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitivePackagePresentationMediaV1 {
    pub kind: CognitivePackageMediaKind,
    pub target_name: String,
    pub sha256: String,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub byte_length: u64,
    pub alt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerifiedCognitivePackagePresentation {
    pub registry_name: String,
    pub registry_url: String,
    pub root_sha256: String,
    pub snapshot_version: u64,
    pub targets_version: u64,
    pub index_target_name: String,
    pub index_sha256: String,
    pub index_byte_length: u64,
    pub record: CognitivePackagePresentationRecordV1,
    pub descriptor: CognitivePackagePresentationV1,
}

#[derive(Debug)]
pub struct VerifiedCognitivePackageMedia {
    path: std::path::PathBuf,
    media: CognitivePackagePresentationMediaV1,
    _temporary: TempDir,
}

impl VerifiedCognitivePackageMedia {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn media(&self) -> &CognitivePackagePresentationMediaV1 {
        &self.media
    }
}

pub async fn inspect_cognitive_package_presentation(
    registry: &TrustedRegistry,
    candidate: &VerifiedPluginCatalogRecord,
) -> UseResult<Option<VerifiedCognitivePackagePresentation>> {
    let repository = load_repository(registry).await?;
    let metadata = verified_registry_metadata(registry, &repository)?;
    record_catalog_refresh(registry, &repository, &metadata).await?;
    inspect_presentation(
        registry,
        &repository,
        candidate,
        RemoteRegistryAccess::Refreshed,
    )
    .await
}

pub async fn inspect_cached_cognitive_package_presentation(
    registry: &TrustedRegistry,
    candidate: &VerifiedPluginCatalogRecord,
) -> UseResult<Option<VerifiedCognitivePackagePresentation>> {
    let repository = load_verified_cached_repository(registry).await?;
    inspect_presentation(
        registry,
        &repository,
        candidate,
        RemoteRegistryAccess::Cached,
    )
    .await
}

pub async fn fetch_cognitive_package_media(
    registry: &TrustedRegistry,
    presentation: &VerifiedCognitivePackagePresentation,
    target_name: &str,
) -> UseResult<VerifiedCognitivePackageMedia> {
    let repository = load_repository(registry).await?;
    fetch_media(
        registry,
        &repository,
        presentation,
        target_name,
        RemoteRegistryAccess::Refreshed,
    )
    .await
}

pub async fn fetch_cached_cognitive_package_media(
    registry: &TrustedRegistry,
    presentation: &VerifiedCognitivePackagePresentation,
    target_name: &str,
) -> UseResult<VerifiedCognitivePackageMedia> {
    let repository = load_verified_cached_repository(registry).await?;
    fetch_media(
        registry,
        &repository,
        presentation,
        target_name,
        RemoteRegistryAccess::Cached,
    )
    .await
}

async fn inspect_presentation(
    registry: &TrustedRegistry,
    repository: &Repository,
    candidate: &VerifiedPluginCatalogRecord,
    access: RemoteRegistryAccess,
) -> UseResult<Option<VerifiedCognitivePackagePresentation>> {
    candidate.validate()?;
    if !registry.matches_provenance(&candidate.provenance) {
        return Err(presentation_error(
            "use.extension.presentation_provenance_mismatch",
            "The presentation Registry does not match the selected catalog provenance.",
        ));
    }
    let index_name = TargetName::new(PRESENTATION_INDEX_TARGET)
        .map_err(|_| presentation_invalid("The presentation index target name is invalid."))?;
    let Some(index_target) = find_target(repository, &index_name) else {
        return Ok(None);
    };
    let marker = index_target.custom.get(PRESENTATION_INDEX_CUSTOM_KEY);
    if marker != Some(&serde_json::json!({"schema": COGNITIVE_PACKAGE_PRESENTATION_INDEX_SCHEMA})) {
        return Err(presentation_invalid(
            "The presentation index target has invalid signed role metadata.",
        ));
    }
    let index_sha256 = signed_target_sha256(index_target)?;
    let index_bytes = load_target_bytes(
        registry,
        repository,
        &index_name,
        index_target.length,
        &index_sha256,
        MAX_PRESENTATION_INDEX_BYTES,
        access,
    )
    .await?;
    let index: CognitivePackagePresentationIndexV1 = decode_bounded(
        &index_bytes,
        MAX_PRESENTATION_INDEX_BYTES,
        "presentation index",
    )?;
    validate_index(&index)?;
    let digest = candidate.provenance.catalog_record_digest.as_str();
    let matching = index
        .entries
        .iter()
        .filter(|record| {
            record.package_id == candidate.record.package_id
                && record.version == candidate.record.version
                && record.channel == candidate.record.channel.as_str()
                && (record.host_target == candidate.record.target || record.host_target == "any")
                && record.catalog_record_digest == digest
        })
        .collect::<Vec<_>>();
    let [record] = matching.as_slice() else {
        if matching.is_empty() {
            return Ok(None);
        }
        return Err(presentation_invalid(
            "The signed presentation index has duplicate records for one catalog release.",
        ));
    };
    let descriptor_name = portable_target_name(&record.descriptor_target_name)?;
    let descriptor_target = find_target(repository, &descriptor_name).ok_or_else(|| {
        presentation_invalid("The presentation descriptor target is absent from TUF metadata.")
    })?;
    verify_target_evidence(
        descriptor_target,
        record.descriptor_byte_length,
        &record.descriptor_sha256,
        MAX_COGNITIVE_PACKAGE_PRESENTATION_BYTES,
    )?;
    let bytes = load_target_bytes(
        registry,
        repository,
        &descriptor_name,
        record.descriptor_byte_length,
        &record.descriptor_sha256,
        MAX_COGNITIVE_PACKAGE_PRESENTATION_BYTES,
        access,
    )
    .await?;
    let descriptor: CognitivePackagePresentationV1 = decode_bounded(
        &bytes,
        MAX_COGNITIVE_PACKAGE_PRESENTATION_BYTES,
        "presentation descriptor",
    )?;
    validate_descriptor(&descriptor, &candidate.record.package_id, repository)?;
    Ok(Some(VerifiedCognitivePackagePresentation {
        registry_name: candidate.provenance.registry_name.clone(),
        registry_url: candidate.provenance.registry_url.clone(),
        root_sha256: candidate.provenance.root_sha256.clone(),
        snapshot_version: repository.snapshot().signed.version.get(),
        targets_version: repository.targets().signed.version.get(),
        index_target_name: index_name.raw().to_owned(),
        index_sha256,
        index_byte_length: index_target.length,
        record: (*record).clone(),
        descriptor,
    }))
}

async fn fetch_media(
    registry: &TrustedRegistry,
    repository: &Repository,
    presentation: &VerifiedCognitivePackagePresentation,
    target_name: &str,
    access: RemoteRegistryAccess,
) -> UseResult<VerifiedCognitivePackageMedia> {
    if registry.name() != presentation.registry_name
        || registry.base_url().as_str() != presentation.registry_url
        || normalize_digest(registry.root_sha256()) != normalize_digest(&presentation.root_sha256)
        || repository.snapshot().signed.version.get() != presentation.snapshot_version
        || repository.targets().signed.version.get() != presentation.targets_version
    {
        return Err(presentation_error(
            "use.extension.presentation_snapshot_drift",
            "The Registry snapshot changed after the presentation was reviewed.",
        ));
    }
    let media = presentation
        .descriptor
        .media
        .iter()
        .find(|media| media.target_name == target_name)
        .cloned()
        .ok_or_else(|| presentation_invalid("The requested media is not in the descriptor."))?;
    let name = portable_target_name(target_name)?;
    let target = find_target(repository, &name)
        .ok_or_else(|| presentation_invalid("The signed media target is unavailable."))?;
    verify_target_evidence(
        target,
        media.byte_length,
        &media.sha256,
        MAX_COGNITIVE_PACKAGE_MEDIA_BYTES,
    )?;
    let digest = normalize_digest(&media.sha256);
    let file_name = name
        .resolved()
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| presentation_invalid("The media target file name is invalid."))?;
    let (temporary, path) = match access {
        RemoteRegistryAccess::Refreshed => {
            download_and_cache_target(
                repository,
                &name,
                registry.datastore(),
                &registry.targets_url()?,
                registry.network_policy(),
                repository.root().signed.consistent_snapshot,
                media.byte_length,
                digest,
                registry.target_cache_policy(),
                false,
            )
            .await?
        }
        RemoteRegistryAccess::Cached => {
            stage_cached_target(
                registry.datastore(),
                file_name,
                media.byte_length,
                digest,
                registry.target_cache_policy(),
            )
            .await?
        }
    };
    Ok(VerifiedCognitivePackageMedia {
        path,
        media,
        _temporary: temporary,
    })
}

async fn load_target_bytes(
    registry: &TrustedRegistry,
    repository: &Repository,
    target_name: &TargetName,
    length: u64,
    sha256: &str,
    bound: u64,
    access: RemoteRegistryAccess,
) -> UseResult<Vec<u8>> {
    if length == 0 || length > bound {
        return Err(presentation_invalid(
            "A presentation target exceeds its type-specific byte bound.",
        ));
    }
    let digest = normalize_digest(sha256);
    let file_name = target_name
        .resolved()
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| presentation_invalid("The presentation target file name is invalid."))?;
    let (temporary, path) = match access {
        RemoteRegistryAccess::Refreshed => {
            download_and_cache_target(
                repository,
                target_name,
                registry.datastore(),
                &registry.targets_url()?,
                registry.network_policy(),
                repository.root().signed.consistent_snapshot,
                length,
                digest,
                registry.target_cache_policy(),
                false,
            )
            .await?
        }
        RemoteRegistryAccess::Cached => {
            stage_cached_target(
                registry.datastore(),
                file_name,
                length,
                digest,
                registry.target_cache_policy(),
            )
            .await?
        }
    };
    let bytes = fs::read(&path).await.map_err(|error| {
        presentation_error(
            "use.extension.presentation_io",
            format!("Failed to read verified presentation target: {error}"),
        )
    })?;
    drop(temporary);
    Ok(bytes)
}

fn validate_index(index: &CognitivePackagePresentationIndexV1) -> UseResult<()> {
    if index.schema != COGNITIVE_PACKAGE_PRESENTATION_INDEX_SCHEMA || index.entries.len() > 10_000 {
        return Err(presentation_invalid(
            "The presentation index schema or entry bound is invalid.",
        ));
    }
    for record in &index.entries {
        if !super::super::valid_package_id(&record.package_id)
            || !matches!(record.channel.as_str(), "stable" | "beta" | "nightly")
            || !valid_digest(&record.catalog_record_digest)
            || !valid_digest(&record.descriptor_sha256)
            || record.descriptor_byte_length == 0
            || record.descriptor_byte_length > MAX_COGNITIVE_PACKAGE_PRESENTATION_BYTES
        {
            return Err(presentation_invalid(
                "A presentation index record is invalid.",
            ));
        }
        portable_target_name(&record.descriptor_target_name)?;
    }
    if index.entries.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(presentation_invalid(
            "Presentation index records must be sorted and unique.",
        ));
    }
    Ok(())
}

fn validate_descriptor(
    descriptor: &CognitivePackagePresentationV1,
    package_id: &str,
    repository: &Repository,
) -> UseResult<()> {
    if descriptor.schema != COGNITIVE_PACKAGE_PRESENTATION_SCHEMA
        || descriptor.package_id != package_id
        || !valid_locale(&descriptor.locale)
        || !valid_text(&descriptor.short_title, 80)
        || !valid_text(&descriptor.short_summary, 240)
        || descriptor.form_factors.is_empty()
        || descriptor.form_factors.len() > 4
        || descriptor
            .form_factors
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || descriptor.media.is_empty()
        || descriptor.media.len() > MAX_COGNITIVE_PACKAGE_PRESENTATION_MEDIA
        || descriptor
            .accent
            .as_deref()
            .is_some_and(|color| !valid_color(color))
    {
        return Err(presentation_invalid(
            "The cognitive-package presentation descriptor is invalid.",
        ));
    }
    let mut prior = None;
    for media in &descriptor.media {
        if prior.is_some_and(|name: &str| name >= media.target_name.as_str())
            || !valid_media_type(media.kind, &media.media_type)
            || media.width == 0
            || media.height == 0
            || media.width > 7680
            || media.height > 4320
            || media.byte_length == 0
            || media.byte_length > MAX_COGNITIVE_PACKAGE_MEDIA_BYTES
            || !valid_digest(&media.sha256)
            || !valid_text(&media.alt, 240)
        {
            return Err(presentation_invalid(
                "A presentation media record is invalid.",
            ));
        }
        let target_name = portable_target_name(&media.target_name)?;
        let target = find_target(repository, &target_name)
            .ok_or_else(|| presentation_invalid("A presentation media target is missing."))?;
        verify_target_evidence(
            target,
            media.byte_length,
            &media.sha256,
            MAX_COGNITIVE_PACKAGE_MEDIA_BYTES,
        )?;
        prior = Some(media.target_name.as_str());
    }
    Ok(())
}

fn verify_target_evidence(
    target: &tough::schema::Target,
    expected_length: u64,
    expected_sha256: &str,
    bound: u64,
) -> UseResult<()> {
    let digest = signed_target_sha256(target)?;
    if target.length == 0
        || target.length > bound
        || target.length != expected_length
        || digest != expected_sha256
    {
        return Err(presentation_invalid(
            "The signed TUF target does not match presentation evidence.",
        ));
    }
    Ok(())
}

fn signed_target_sha256(target: &tough::schema::Target) -> UseResult<String> {
    let bytes = target.hashes.sha256.as_ref();
    if bytes.len() != 32 {
        return Err(presentation_invalid(
            "A presentation target has no valid SHA-256 digest.",
        ));
    }
    Ok(format!("sha256:{}", hex_lower(bytes)))
}

fn find_target<'a>(
    repository: &'a Repository,
    target_name: &TargetName,
) -> Option<&'a tough::schema::Target> {
    repository
        .all_targets()
        .find(|(name, _)| *name == target_name)
        .map(|(_, target)| target)
}

fn portable_target_name(value: &str) -> UseResult<TargetName> {
    let name = TargetName::new(value.to_owned())
        .map_err(|_| presentation_invalid("A presentation target name is invalid."))?;
    if value != name.resolved()
        || value.starts_with('/')
        || value.contains('\\')
        || value.split('/').any(|segment| segment.is_empty())
    {
        return Err(presentation_invalid(
            "A presentation target name is not a portable path.",
        ));
    }
    Ok(name)
}

fn decode_bounded<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
    bound: u64,
    label: &str,
) -> UseResult<T> {
    if bytes.is_empty() || bytes.len() as u64 > bound {
        return Err(presentation_invalid(format!(
            "The {label} exceeds its input bound."
        )));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| presentation_invalid(format!("Failed to decode the {label}: {error}")))
}

fn valid_locale(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 35
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_media_type(kind: CognitivePackageMediaKind, value: &str) -> bool {
    matches!(
        (kind, value),
        (CognitivePackageMediaKind::Image, "image/avif")
            | (CognitivePackageMediaKind::Image, "image/jpeg")
            | (CognitivePackageMediaKind::Image, "image/png")
            | (CognitivePackageMediaKind::Image, "image/webp")
            | (CognitivePackageMediaKind::Video, "video/mp4")
            | (CognitivePackageMediaKind::Video, "video/webm")
    )
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn normalize_digest(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
}

fn presentation_invalid(message: impl Into<String>) -> UseError {
    presentation_error("use.extension.presentation_invalid", message)
}

fn presentation_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
