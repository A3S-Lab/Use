use std::collections::BTreeSet;

use semver::{Version, VersionReq};
use url::Url;

use crate::{UseError, UseResult};

use super::{
    HttpHealthContract, McpServiceContract, ReleaseArtifact, ReleaseCompatibility,
    ReleaseDependency, ReleaseProvenance, ReleaseResolution, SkillContentContract,
};

const MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_LIFECYCLE_MS: u64 = 60 * 60 * 1000;

impl McpServiceContract {
    pub(super) fn validate(&self) -> UseResult<()> {
        if !valid_protocol_date(&self.protocol_version)
            || !valid_segment(&self.port_name)
            || self.port == 0
            || !valid_http_path(&self.endpoint_path)
            || self.startup_timeout_ms == 0
            || self.startup_timeout_ms > MAX_LIFECYCLE_MS
            || self.shutdown_grace_ms == 0
            || self.shutdown_grace_ms > MAX_LIFECYCLE_MS
        {
            return Err(descriptor_error("The MCP service contract is invalid."));
        }
        self.health.validate()
    }
}

impl HttpHealthContract {
    fn validate(&self) -> UseResult<()> {
        if !valid_http_path(&self.path)
            || self.interval_ms == 0
            || self.interval_ms > MAX_LIFECYCLE_MS
            || self.timeout_ms == 0
            || self.timeout_ms > self.interval_ms
            || !(1..=100).contains(&self.success_threshold)
            || !(1..=100).contains(&self.failure_threshold)
        {
            return Err(descriptor_error("The MCP health contract is invalid."));
        }
        Ok(())
    }
}

impl SkillContentContract {
    pub(super) fn validate(&self) -> UseResult<()> {
        if !valid_skill_path(&self.entrypoint)
            || !valid_sha256(&self.entrypoint_digest)
            || !strictly_sorted_unique(&self.required_capabilities)
            || self
                .required_capabilities
                .iter()
                .any(|capability| !valid_capability(capability))
        {
            return Err(descriptor_error("The Skill content contract is invalid."));
        }
        Ok(())
    }
}

pub(super) fn validate_common(
    name: &str,
    version: &str,
    provenance: &ReleaseProvenance,
    artifact: &ReleaseArtifact,
    compatibility: &[ReleaseCompatibility],
    dependencies: &[ReleaseDependency],
) -> UseResult<()> {
    validate_release_name(name)?;
    validate_version(version)?;
    validate_provenance(provenance)?;
    if !valid_sha256(&artifact.digest)
        || artifact.size_bytes == 0
        || artifact.size_bytes > MAX_ARTIFACT_BYTES
    {
        return Err(descriptor_error("The release artifact is invalid."));
    }
    if compatibility.is_empty()
        || compatibility
            .windows(2)
            .any(|pair| pair[0].component >= pair[1].component)
    {
        return Err(descriptor_error(
            "Compatibility requirements must be sorted and unique.",
        ));
    }
    for requirement in compatibility {
        if !valid_component(&requirement.component)
            || VersionReq::parse(&requirement.version_requirement).is_err()
        {
            return Err(descriptor_error(
                "A release compatibility requirement is invalid.",
            ));
        }
    }
    if dependencies
        .windows(2)
        .any(|pair| (pair[0].kind, pair[0].name.as_str()) >= (pair[1].kind, pair[1].name.as_str()))
    {
        return Err(descriptor_error(
            "Release dependencies must be sorted and unique by kind and name.",
        ));
    }
    for dependency in dependencies {
        validate_release_name(&dependency.name)?;
        validate_version(&dependency.version)?;
        if !valid_sha256(&dependency.descriptor_digest) {
            return Err(descriptor_error("A release dependency digest is invalid."));
        }
    }
    Ok(())
}

fn validate_provenance(provenance: &ReleaseProvenance) -> UseResult<()> {
    let repository = Url::parse(&provenance.source_repository)
        .map_err(|_| descriptor_error("The source repository URL is invalid."))?;
    if provenance.source_repository.len() > 2048
        || repository.scheme() != "https"
        || repository.host_str().is_none()
        || !repository.username().is_empty()
        || repository.password().is_some()
        || repository.query().is_some()
        || repository.fragment().is_some()
        || repository.as_str() != provenance.source_repository
        || !valid_commit_sha(&provenance.commit_sha)
        || !valid_sha256(&provenance.manifest_digest)
        || !valid_machine_id(&provenance.builder_id)
        || !valid_machine_id(&provenance.build_operation_id)
    {
        return Err(descriptor_error("The release provenance is invalid."));
    }
    Ok(())
}

pub(super) fn verify_resolution(
    compatibility: &[ReleaseCompatibility],
    dependencies: &[ReleaseDependency],
    resolution: &ReleaseResolution,
) -> UseResult<()> {
    for requirement in compatibility {
        let actual = resolution
            .components
            .get(&requirement.component)
            .ok_or_else(|| {
                UseError::new(
                    "use.release.compatibility_missing",
                    format!(
                        "Required component '{}' is not available.",
                        requirement.component
                    ),
                )
                .with_detail("component", requirement.component.clone())
            })?;
        let version = Version::parse(actual).map_err(|_| {
            UseError::new(
                "use.release.incompatible",
                format!(
                    "Component '{}' reported an invalid version.",
                    requirement.component
                ),
            )
            .with_detail("component", requirement.component.clone())
        })?;
        let expected = VersionReq::parse(&requirement.version_requirement)
            .map_err(|_| descriptor_error("A compatibility requirement is invalid."))?;
        if !expected.matches(&version) {
            return Err(UseError::new(
                "use.release.incompatible",
                format!(
                    "Component '{}' does not satisfy the release requirement.",
                    requirement.component
                ),
            )
            .with_detail("component", requirement.component.clone())
            .with_detail("expected", requirement.version_requirement.clone())
            .with_detail("actual", actual.clone()));
        }
    }

    validate_resolved_dependencies(&resolution.dependencies)?;
    for expected in dependencies {
        let observed = resolution
            .dependencies
            .iter()
            .find(|candidate| candidate.kind == expected.kind && candidate.name == expected.name);
        let Some(observed) = observed else {
            return Err(UseError::new(
                "use.release.dependency_missing",
                format!("Release dependency '{}' is not resolved.", expected.name),
            )
            .with_detail("dependency", expected.name.clone()));
        };
        if observed.version != expected.version
            || observed.descriptor_digest != expected.descriptor_digest
        {
            return Err(UseError::new(
                "use.release.dependency_mismatch",
                format!(
                    "Release dependency '{}' does not match the pinned release.",
                    expected.name
                ),
            )
            .with_detail("dependency", expected.name.clone())
            .with_detail("expectedVersion", expected.version.clone())
            .with_detail("actualVersion", observed.version.clone())
            .with_detail("expectedDigest", expected.descriptor_digest.clone())
            .with_detail("actualDigest", observed.descriptor_digest.clone()));
        }
    }
    Ok(())
}

fn validate_resolved_dependencies(dependencies: &[ReleaseDependency]) -> UseResult<()> {
    let mut identities = BTreeSet::new();
    for dependency in dependencies {
        validate_release_name(&dependency.name)?;
        validate_version(&dependency.version)?;
        if !valid_sha256(&dependency.descriptor_digest)
            || !identities.insert((dependency.kind, dependency.name.as_str()))
        {
            return Err(descriptor_error(
                "The resolved release dependencies are invalid or ambiguous.",
            ));
        }
    }
    Ok(())
}

fn validate_release_name(value: &str) -> UseResult<()> {
    let segments = value.split('/').collect::<Vec<_>>();
    if value.len() > 128 || segments.len() != 2 || !segments.into_iter().all(valid_segment) {
        return Err(descriptor_error(
            "Release names must be lowercase '<publisher>/<name>' identifiers.",
        ));
    }
    Ok(())
}

fn validate_version(value: &str) -> UseResult<()> {
    if value.len() > 128
        || Version::parse(value)
            .map(|version| version.to_string() != value)
            .unwrap_or(true)
    {
        return Err(descriptor_error(
            "Release versions must be canonical semantic versions.",
        ));
    }
    Ok(())
}

fn valid_segment(value: &str) -> bool {
    value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
}

fn valid_capability(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'.' | b':' | b'/' | b'_')
        })
}

fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

fn valid_commit_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn valid_protocol_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return false;
    }

    let Ok(year) = value[0..4].parse::<u16>() else {
        return false;
    };
    let Ok(month) = value[5..7].parse::<u8>() else {
        return false;
    };
    let Ok(day) = value[8..10].parse::<u8>() else {
        return false;
    };
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days_in_month).contains(&day)
}

fn valid_http_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 1024
        && !value.contains("//")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'?' | b'#' | b'\\'))
        && !value
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
}

fn valid_skill_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
        && value.rsplit('/').next() == Some("SKILL.md")
        && value
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
}

fn strictly_sorted_unique(values: &[String]) -> bool {
    !values.windows(2).any(|pair| pair[0] >= pair[1])
}

pub(super) fn descriptor_error(message: impl Into<String>) -> UseError {
    UseError::new("use.release.descriptor_invalid", message)
}
