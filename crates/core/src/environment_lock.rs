//! Immutable analysis-environment locks owned by A3S Use.
//!
//! A lock names one interpreter and one package closure. Its installable
//! identity is an OCI image digest. Conda URLs, host `python`/`R`, and
//! compiler toolchains are not leases.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{UseError, UseResult};

pub const ENVIRONMENT_LOCK_SCHEMA_V1: &str = "a3s.use.environment-lock.v1";
pub const ENVIRONMENT_LEASE_SCHEMA_V1: &str = "a3s.use.environment-lease.v1";
pub const SCIENCE_CONNECTOR_SCHEMA_V1: &str = "a3s.use.science-connector.v1";
pub const SCIENCE_PYTHON_ANALYSIS_LOCK_ID: &str = "science-python-3.11";
pub const SCIENCE_R_ANALYSIS_LOCK_ID: &str = "science-r-4.5";
pub const SCIENCE_OPENALEX_CONNECTOR_ID: &str = "science-connector-openalex";
pub const SCIENCE_SCIVERSE_CONNECTOR_ID: &str = "science-connector-sciverse";

const MAX_PACKAGES: usize = 64;
const FORBIDDEN_PACKAGES: &[&str] = &[
    "clang",
    "cmake",
    "compilers",
    "gcc",
    "gfortran",
    "git",
    "llvm",
    "nodejs",
    "python",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnvironmentLanguageV1 {
    Python,
    R,
}

impl EnvironmentLanguageV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::R => "r",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentPreflightStatusV1 {
    Leased,
    Installable,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentPackagePinV1 {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentImageBindingV1 {
    pub architecture: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentLockV1 {
    pub schema: String,
    pub lock_id: String,
    pub language: EnvironmentLanguageV1,
    pub runtime_version: String,
    pub guest_platform: String,
    pub packages: Vec<EnvironmentPackagePinV1>,
    pub images: Vec<EnvironmentImageBindingV1>,
    pub lock_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentPreflightV1 {
    pub schema: String,
    pub lock_id: String,
    pub lock_digest: String,
    pub status: EnvironmentPreflightStatusV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<String>,
    pub language: EnvironmentLanguageV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvironmentLeaseV1 {
    pub schema: String,
    pub lock_id: String,
    pub lock_digest: String,
    pub image_digest: String,
    pub architecture: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScienceConnectorV1 {
    pub schema: String,
    pub connector_id: String,
    pub display_name: String,
    /// Signed package digest. Absent means the connector is not installable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_digest: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvironmentInstallDecisionV1 {
    /// Pull this digest-qualified reference, then write the lease.
    Pull {
        reference: String,
        lease: EnvironmentLeaseV1,
    },
    /// Image is already present. Write or keep the lease. Do not pull again.
    RecordLease {
        lease: EnvironmentLeaseV1,
    },
    /// No published digest. Box builds the frozen recipe. Do not write a lease
    /// until the build returns an image digest.
    Build {
        lock_id: String,
        lock_digest: String,
        architecture: String,
    },
    Refuse {
        preflight: EnvironmentPreflightV1,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvironmentExecutionPinV1 {
    pub lock_id: String,
    pub lock_digest: String,
    pub image_digest: String,
    pub generation: u64,
}

pub fn scientist_python_analysis_lock() -> EnvironmentLockV1 {
    lock_from_pins(
        SCIENCE_PYTHON_ANALYSIS_LOCK_ID,
        EnvironmentLanguageV1::Python,
        "3.11",
        python_analysis_packages(),
    )
}

pub fn scientist_r_analysis_lock() -> EnvironmentLockV1 {
    lock_from_pins(
        SCIENCE_R_ANALYSIS_LOCK_ID,
        EnvironmentLanguageV1::R,
        "4.5",
        vec![
            pin("pandoc", "3.6.4"),
            pin("r-base", "4.5.3"),
            pin("r-tidyverse", "2.0.0"),
            pin("fonts-dejavu", "2.37"),
        ],
    )
}

/// Locks offered after a Scientist Bot switch. R is not in this set.
pub fn scientist_switch_prompt_locks() -> Vec<EnvironmentLockV1> {
    vec![scientist_python_analysis_lock()]
}

pub fn scientist_openalex_connector() -> ScienceConnectorV1 {
    ScienceConnectorV1 {
        schema: SCIENCE_CONNECTOR_SCHEMA_V1.to_owned(),
        connector_id: SCIENCE_OPENALEX_CONNECTOR_ID.to_owned(),
        display_name: "OpenAlex".to_owned(),
        package_digest: None,
    }
}

pub fn scientist_sciverse_connector() -> ScienceConnectorV1 {
    ScienceConnectorV1 {
        schema: SCIENCE_CONNECTOR_SCHEMA_V1.to_owned(),
        connector_id: SCIENCE_SCIVERSE_CONNECTOR_ID.to_owned(),
        display_name: "SciVerse".to_owned(),
        package_digest: None,
    }
}

pub fn classify_environment_preflight(
    lock: &EnvironmentLockV1,
    architecture: &str,
    image_present: bool,
    leased: bool,
) -> UseResult<EnvironmentPreflightV1> {
    lock.validate()?;
    let image_digest = image_digest_for(lock, architecture);
    let status = match (&image_digest, image_present, leased) {
        (_, true, true) => EnvironmentPreflightStatusV1::Leased,
        (Some(_), _, _) => EnvironmentPreflightStatusV1::Installable,
        (None, _, _) => EnvironmentPreflightStatusV1::Installable,
    };
    let reason = match (&image_digest, image_present, leased, status) {
        (None, false, false, _) => Some(
            "Install builds the frozen recipe in a3s-box and records the image digest. Host Python is not used."
                .to_owned(),
        ),
        (_, false, true, _) => Some(
            "The previous lease is missing its image. Install again to restore this lock."
                .to_owned(),
        ),
        _ => None,
    };
    Ok(EnvironmentPreflightV1 {
        schema: ENVIRONMENT_LOCK_SCHEMA_V1.to_owned(),
        lock_id: lock.lock_id.clone(),
        lock_digest: lock.lock_digest.clone(),
        status,
        reason,
        image_digest,
        language: lock.language,
    })
}

/// Box could not be read. This is not a lease and not an install offer.
pub fn failed_environment_probe(
    lock: &EnvironmentLockV1,
    architecture: &str,
) -> UseResult<EnvironmentPreflightV1> {
    lock.validate()?;
    Ok(EnvironmentPreflightV1 {
        schema: ENVIRONMENT_LOCK_SCHEMA_V1.to_owned(),
        lock_id: lock.lock_id.clone(),
        lock_digest: lock.lock_digest.clone(),
        status: EnvironmentPreflightStatusV1::Failed,
        reason: Some(
            "a3s-box could not be read. No lease was written. Host Python is not used.".to_owned(),
        ),
        image_digest: image_digest_for(lock, architecture),
        language: lock.language,
    })
}

pub fn decide_environment_install(
    lock: &EnvironmentLockV1,
    architecture: &str,
    image_present: bool,
    current: Option<&EnvironmentLeaseV1>,
) -> UseResult<EnvironmentInstallDecisionV1> {
    let preflight = classify_environment_preflight(
        lock,
        architecture,
        image_present,
        current.is_some_and(|lease| {
            lease_matches(lock, architecture, lease)
                || lease_matches_recipe(lock, architecture, lease)
        }),
    )?;
    if preflight.status == EnvironmentPreflightStatusV1::Failed {
        return Ok(EnvironmentInstallDecisionV1::Refuse { preflight });
    }
    let Some(image_digest) = preflight.image_digest.clone() else {
        if current.is_some_and(|lease| lease_matches_recipe(lock, architecture, lease))
            && image_present
        {
            let lease = current.expect("lease checked").clone();
            return Ok(EnvironmentInstallDecisionV1::RecordLease { lease });
        }
        return Ok(EnvironmentInstallDecisionV1::Build {
            lock_id: lock.lock_id.clone(),
            lock_digest: lock.lock_digest.clone(),
            architecture: architecture.to_owned(),
        });
    };
    let generation = next_generation(current, &image_digest);
    let lease = EnvironmentLeaseV1 {
        schema: ENVIRONMENT_LEASE_SCHEMA_V1.to_owned(),
        lock_id: lock.lock_id.clone(),
        lock_digest: lock.lock_digest.clone(),
        image_digest: image_digest.clone(),
        architecture: architecture.to_owned(),
        generation,
    };
    if image_present {
        return Ok(EnvironmentInstallDecisionV1::RecordLease { lease });
    }
    Ok(EnvironmentInstallDecisionV1::Pull {
        reference: format!("{}@{image_digest}", lock.lock_id),
        lease,
    })
}

/// Pin the lease an in-flight session already holds. A newer generation does
/// not replace it. Missing leases fail the run; host interpreters are not a
/// fallback.
pub fn pin_environment_execution(
    current: Option<&EnvironmentLeaseV1>,
    pinned_generation: Option<u64>,
    retained: &[EnvironmentLeaseV1],
) -> UseResult<EnvironmentExecutionPinV1> {
    let lease = if let Some(generation) = pinned_generation {
        retained
            .iter()
            .find(|lease| lease.generation == generation)
            .or_else(|| current.filter(|lease| lease.generation == generation))
            .ok_or_else(|| {
                lock_error(
                    "environment.lease.missing",
                    "The analysis environment generation is not leased.",
                )
            })?
    } else {
        current.ok_or_else(|| {
            lock_error(
                "environment.lease.missing",
                "The analysis environment is not leased.",
            )
        })?
    };
    lease.validate()?;
    Ok(EnvironmentExecutionPinV1 {
        lock_id: lease.lock_id.clone(),
        lock_digest: lease.lock_digest.clone(),
        image_digest: lease.image_digest.clone(),
        generation: lease.generation,
    })
}

pub const SCIENCE_CONNECTOR_LEASE_SCHEMA_V1: &str = "a3s.use.science-connector-lease.v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScienceConnectorLeaseV1 {
    pub schema: String,
    pub connector_id: String,
    pub package_digest: String,
    pub generation: u64,
}

pub fn scientist_openalex_package_digest() -> UseResult<String> {
    let connector = ScienceConnectorV1 {
        schema: SCIENCE_CONNECTOR_SCHEMA_V1.to_owned(),
        connector_id: SCIENCE_OPENALEX_CONNECTOR_ID.to_owned(),
        display_name: "OpenAlex".to_owned(),
        package_digest: None,
    };
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    connector
        .serialize(&mut serializer)
        .map_err(|error| lock_error("science.connector.encode", error.to_string()))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

pub fn scientist_sciverse_package_digest() -> UseResult<String> {
    connector_package_digest(&scientist_sciverse_connector())
}

fn connector_package_digest(connector: &ScienceConnectorV1) -> UseResult<String> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    connector
        .serialize(&mut serializer)
        .map_err(|error| lock_error("science.connector.encode", error.to_string()))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

pub fn lease_literature_connector(
    connector: &ScienceConnectorV1,
    current: Option<&ScienceConnectorLeaseV1>,
) -> UseResult<ScienceConnectorLeaseV1> {
    if connector.connector_id != SCIENCE_SCIVERSE_CONNECTOR_ID {
        return Err(lock_error(
            "science.connector.unknown",
            "Only the SciVerse literature connector can be leased.",
        ));
    }
    let package_digest = scientist_sciverse_package_digest()?;
    let generation = match current {
        Some(lease) if lease.package_digest == package_digest => lease.generation,
        Some(lease) => lease.generation.saturating_add(1).max(1),
        None => 1,
    };
    Ok(ScienceConnectorLeaseV1 {
        schema: SCIENCE_CONNECTOR_LEASE_SCHEMA_V1.to_owned(),
        connector_id: connector.connector_id.clone(),
        package_digest,
        generation,
    })
}

pub fn authorize_literature_search(
    connector: &ScienceConnectorV1,
    lease: Option<&ScienceConnectorLeaseV1>,
) -> UseResult<()> {
    let Some(lease) = lease else {
        return Err(literature_search_refusal(connector));
    };
    let expected = literature_package_digest(connector)?;
    if lease.connector_id != connector.connector_id || lease.package_digest != expected {
        return Err(literature_search_refusal(connector));
    }
    Ok(())
}

fn literature_package_digest(connector: &ScienceConnectorV1) -> UseResult<String> {
    if connector.connector_id == SCIENCE_SCIVERSE_CONNECTOR_ID {
        return scientist_sciverse_package_digest();
    }
    if connector.connector_id == SCIENCE_OPENALEX_CONNECTOR_ID {
        return scientist_openalex_package_digest();
    }
    Err(literature_search_refusal(connector))
}

pub fn literature_search_refusal(connector: &ScienceConnectorV1) -> UseError {
    if connector.package_digest.is_none() {
        return lock_error(
            "science.connector.not_searched",
            "The literature connector is not leased, so no database was queried.",
        );
    }
    lock_error(
        "science.connector.not_searched",
        "The literature connector is not leased, so no database was queried.",
    )
}

impl EnvironmentLockV1 {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != ENVIRONMENT_LOCK_SCHEMA_V1 {
            return Err(lock_error(
                "environment.lock.schema",
                "Unsupported environment lock schema.",
            ));
        }
        if self.lock_id.is_empty() || self.runtime_version.is_empty() {
            return Err(lock_error(
                "environment.lock.identity",
                "Environment lock identity is incomplete.",
            ));
        }
        if self.guest_platform != "linux" {
            return Err(lock_error(
                "environment.lock.platform",
                "Analysis locks run in a Linux guest, not on the host PATH.",
            ));
        }
        if self.packages.is_empty() || self.packages.len() > MAX_PACKAGES {
            return Err(lock_error(
                "environment.lock.packages",
                "Environment lock package closure is empty or too large.",
            ));
        }
        let mut names = Vec::new();
        for package in &self.packages {
            validate_pin(package)?;
            if names.contains(&package.name) {
                return Err(lock_error(
                    "environment.lock.duplicate",
                    format!("Package '{}' is pinned more than once.", package.name),
                ));
            }
            names.push(package.name.clone());
        }
        if self.language == EnvironmentLanguageV1::Python {
            reject_python_lock_drift(self)?;
        }
        if self.lock_digest != expected_lock_digest(self)? {
            return Err(lock_error(
                "environment.lock.digest",
                "Environment lock digest does not match its canonical contents.",
            ));
        }
        Ok(())
    }

    pub fn image_digest(&self, architecture: &str) -> Option<&str> {
        self.images
            .iter()
            .find(|image| image.architecture == architecture)
            .and_then(|image| image.image_digest.as_deref())
    }
}

impl EnvironmentLeaseV1 {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != ENVIRONMENT_LEASE_SCHEMA_V1 || self.generation == 0 {
            return Err(lock_error(
                "environment.lease.invalid",
                "Environment lease is incomplete.",
            ));
        }
        if !is_sha256_digest(&self.image_digest) || !is_sha256_digest(&self.lock_digest) {
            return Err(lock_error(
                "environment.lease.digest",
                "Environment lease digests must be sha256.",
            ));
        }
        Ok(())
    }
}

pub const SCIENCE_CAPABILITY_SCHEMA_V1: &str = "a3s.use.science-capability-binding.v1";

/// Published capability generation for one leased image digest. A lease file
/// alone is not a published generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScienceCapabilityGenerationV1 {
    pub schema: String,
    pub lock_id: String,
    pub lock_digest: String,
    pub image_digest: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScienceArtifactReceiptV1 {
    pub path: String,
    pub digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScienceSessionReceiptV1 {
    pub lock_id: String,
    pub lock_digest: String,
    pub image_digest: String,
    pub generation: u64,
    pub script_digest: String,
    pub output_digest: String,
    pub artifacts: Vec<ScienceArtifactReceiptV1>,
}

pub fn write_environment_lease(directory: &Path, lease: &EnvironmentLeaseV1) -> UseResult<PathBuf> {
    lease.validate()?;
    fs::create_dir_all(directory).map_err(|error| io_error(error.to_string()))?;
    let current = directory.join(format!("{}.json", lease.lock_id));
    let retained = directory.join(format!("{}-{}.json", lease.lock_id, lease.generation));
    let bytes = serde_json::to_vec_pretty(lease)
        .map_err(|error| lock_error("environment.lease.encode", error.to_string()))?;
    atomic_write(&retained, &bytes)?;
    atomic_write(&current, &bytes)?;
    Ok(current)
}

pub fn read_environment_lease(
    directory: &Path,
    lock_id: &str,
) -> UseResult<Option<EnvironmentLeaseV1>> {
    let path = directory.join(format!("{lock_id}.json"));
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|error| io_error(error.to_string()))?;
    let lease: EnvironmentLeaseV1 = serde_json::from_slice(&bytes)
        .map_err(|error| lock_error("environment.lease.decode", error.to_string()))?;
    lease.validate()?;
    Ok(Some(lease))
}

pub fn read_retained_environment_leases(
    directory: &Path,
    lock_id: &str,
) -> UseResult<Vec<EnvironmentLeaseV1>> {
    let mut leases = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(leases),
        Err(error) => return Err(io_error(error.to_string())),
    };
    let prefix = format!("{lock_id}-");
    for entry in entries {
        let entry = entry.map_err(|error| io_error(error.to_string()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        let Some(generation) = stem.strip_prefix(&prefix) else {
            continue;
        };
        // `{lock}-capability-{n}.json` shares the lock prefix. Only a numeric
        // generation is a retained lease.
        if generation.is_empty() || !generation.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let bytes = fs::read(entry.path()).map_err(|error| io_error(error.to_string()))?;
        let lease: EnvironmentLeaseV1 = serde_json::from_slice(&bytes)
            .map_err(|error| lock_error("environment.lease.decode", error.to_string()))?;
        lease.validate()?;
        leases.push(lease);
    }
    leases.sort_by_key(|lease| lease.generation);
    Ok(leases)
}

fn capability_from_lease(lease: &EnvironmentLeaseV1) -> UseResult<ScienceCapabilityGenerationV1> {
    lease.validate()?;
    Ok(ScienceCapabilityGenerationV1 {
        schema: SCIENCE_CAPABILITY_SCHEMA_V1.to_owned(),
        lock_id: lease.lock_id.clone(),
        lock_digest: lease.lock_digest.clone(),
        image_digest: lease.image_digest.clone(),
        generation: lease.generation,
    })
}

/// Publish the capability generation only after a digest lease exists.
/// The capability file is written first. A crash before the lease write
/// leaves no lease. A failed lease write removes the capability file.
pub fn publish_environment_capability(
    directory: &Path,
    lease: &EnvironmentLeaseV1,
) -> UseResult<ScienceCapabilityGenerationV1> {
    let capability = capability_from_lease(lease)?;
    let capability_path = write_capability_generation(directory, &capability)?;
    if let Err(error) = write_environment_lease(directory, lease) {
        let _ = fs::remove_file(&capability_path);
        let retained = directory.join(format!(
            "{}-capability-{}.json",
            capability.lock_id, capability.generation
        ));
        let _ = fs::remove_file(retained);
        return Err(error);
    }
    Ok(capability)
}

pub fn read_published_capability(
    directory: &Path,
    lock_id: &str,
) -> UseResult<Option<ScienceCapabilityGenerationV1>> {
    let path = directory.join(format!("{lock_id}.capability.json"));
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|error| io_error(error.to_string()))?;
    let capability: ScienceCapabilityGenerationV1 = serde_json::from_slice(&bytes)
        .map_err(|error| lock_error("environment.capability.decode", error.to_string()))?;
    if capability.schema != SCIENCE_CAPABILITY_SCHEMA_V1
        || capability.generation == 0
        || !is_sha256_digest(&capability.image_digest)
        || !is_sha256_digest(&capability.lock_digest)
    {
        return Err(lock_error(
            "environment.capability.invalid",
            "Published capability generation is incomplete.",
        ));
    }
    Ok(Some(capability))
}

/// An in-flight run stays on the generation it already holds. A lease that
/// was never published cannot start a session.
pub fn pin_published_capability(
    current: Option<&EnvironmentLeaseV1>,
    published: Option<&ScienceCapabilityGenerationV1>,
    pinned_generation: Option<u64>,
    retained_leases: &[EnvironmentLeaseV1],
    retained_capabilities: &[ScienceCapabilityGenerationV1],
) -> UseResult<EnvironmentExecutionPinV1> {
    let pin = pin_environment_execution(current, pinned_generation, retained_leases)?;
    let capability = if let Some(generation) = pinned_generation {
        retained_capabilities
            .iter()
            .find(|item| item.generation == generation)
            .or_else(|| published.filter(|item| item.generation == generation))
    } else {
        published
    };
    let Some(capability) = capability else {
        return Err(lock_error(
            "environment.capability.unpublished",
            "The analysis environment generation is not published.",
        ));
    };
    if capability.generation != pin.generation
        || capability.image_digest != pin.image_digest
        || capability.lock_digest != pin.lock_digest
        || capability.lock_id != pin.lock_id
    {
        return Err(lock_error(
            "environment.capability.unpublished",
            "The analysis environment generation is not published.",
        ));
    }
    Ok(pin)
}

/// A cancelled session is not a success. Host interpreters cannot mint this
/// receipt; the caller must already hold a published image pin.
pub fn analysis_session_receipt(
    cancelled: bool,
    pin: &EnvironmentExecutionPinV1,
    script_digest: &str,
    output_digest: &str,
    artifacts: &[ScienceArtifactReceiptV1],
) -> UseResult<ScienceSessionReceiptV1> {
    if cancelled {
        return Err(lock_error(
            "science.session.cancelled",
            "Analysis was cancelled. No success receipt was written.",
        ));
    }
    if !is_sha256_digest(&pin.image_digest)
        || !is_sha256_digest(script_digest)
        || !is_sha256_digest(output_digest)
        || artifacts
            .iter()
            .any(|artifact| artifact.path.is_empty() || !is_sha256_digest(&artifact.digest))
    {
        return Err(lock_error(
            "science.session.receipt",
            "An analysis receipt requires sha256 digests for the image, script, output, and artifacts.",
        ));
    }
    Ok(ScienceSessionReceiptV1 {
        lock_id: pin.lock_id.clone(),
        lock_digest: pin.lock_digest.clone(),
        image_digest: pin.image_digest.clone(),
        generation: pin.generation,
        script_digest: script_digest.to_owned(),
        output_digest: output_digest.to_owned(),
        artifacts: artifacts.to_vec(),
    })
}

pub fn read_retained_capabilities(
    directory: &Path,
    lock_id: &str,
) -> UseResult<Vec<ScienceCapabilityGenerationV1>> {
    let mut capabilities = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(capabilities),
        Err(error) => return Err(io_error(error.to_string())),
    };
    let prefix = format!("{lock_id}-capability-");
    for entry in entries {
        let entry = entry.map_err(|error| io_error(error.to_string()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".json") {
            continue;
        }
        let bytes = fs::read(entry.path()).map_err(|error| io_error(error.to_string()))?;
        let capability: ScienceCapabilityGenerationV1 = serde_json::from_slice(&bytes)
            .map_err(|error| lock_error("environment.capability.decode", error.to_string()))?;
        capabilities.push(capability);
    }
    capabilities.sort_by_key(|capability| capability.generation);
    Ok(capabilities)
}

fn write_capability_generation(
    directory: &Path,
    capability: &ScienceCapabilityGenerationV1,
) -> UseResult<PathBuf> {
    fs::create_dir_all(directory).map_err(|error| io_error(error.to_string()))?;
    let current = directory.join(format!("{}.capability.json", capability.lock_id));
    let retained = directory.join(format!(
        "{}-capability-{}.json",
        capability.lock_id, capability.generation
    ));
    let bytes = serde_json::to_vec_pretty(capability)
        .map_err(|error| lock_error("environment.capability.encode", error.to_string()))?;
    atomic_write(&retained, &bytes)?;
    atomic_write(&current, &bytes)?;
    Ok(current)
}

/// Runtime image recipe derived from the lock. Compilers stay out of the
/// runtime stage. This is a build input, not an install action.
pub fn render_runtime_image_recipe(lock: &EnvironmentLockV1) -> UseResult<String> {
    lock.validate()?;
    match lock.language {
        EnvironmentLanguageV1::Python => {
            let packages = lock
                .packages
                .iter()
                .map(|package| (package.name.clone(), package.version.clone()))
                .collect::<Vec<_>>();
            crate::environment_recipe::render_python_runtime_recipe(&packages)
        }
        EnvironmentLanguageV1::R => render_r_lock_recipe(lock),
    }
}

fn render_r_lock_recipe(lock: &EnvironmentLockV1) -> UseResult<String> {
    const PINS: [&str; 4] = ["fonts-dejavu", "pandoc", "r-base", "r-tidyverse"];
    if lock.packages.len() != PINS.len()
        || lock
            .packages
            .iter()
            .any(|package| !PINS.contains(&package.name.as_str()))
    {
        return Err(lock_error(
            "environment.lock.recipe",
            "The R analysis recipe only materializes fonts-dejavu, pandoc, r-base, and r-tidyverse.",
        ));
    }
    crate::environment_recipe::render_r_runtime_recipe(
        pin_version(lock, "r-base")?,
        pin_version(lock, "r-tidyverse")?,
        pin_version(lock, "pandoc")?,
        pin_version(lock, "fonts-dejavu")?,
    )
}

fn pin_version<'a>(lock: &'a EnvironmentLockV1, name: &str) -> UseResult<&'a str> {
    lock.packages
        .iter()
        .find(|package| package.name == name)
        .map(|package| package.version.as_str())
        .ok_or_else(|| {
            lock_error(
                "environment.lock.recipe",
                format!("R analysis lock is missing pin '{name}'."),
            )
        })
}

fn python_analysis_packages() -> Vec<EnvironmentPackagePinV1> {
    vec![
        pin("matplotlib", "3.10.1"),
        pin("numpy", "2.2.6"),
        pin("pandas", "2.2.3"),
        pin("pillow", "11.1.0"),
        pin("scikit-learn", "1.6.1"),
        pin("scipy", "1.15.2"),
        pin("seaborn", "0.13.2"),
        pin("statsmodels", "0.14.4"),
    ]
}

fn pin(name: &str, version: &str) -> EnvironmentPackagePinV1 {
    EnvironmentPackagePinV1 {
        name: name.to_owned(),
        version: version.to_owned(),
    }
}

fn lock_from_pins(
    lock_id: &str,
    language: EnvironmentLanguageV1,
    runtime_version: &str,
    mut packages: Vec<EnvironmentPackagePinV1>,
) -> EnvironmentLockV1 {
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    let mut lock = EnvironmentLockV1 {
        schema: ENVIRONMENT_LOCK_SCHEMA_V1.to_owned(),
        lock_id: lock_id.to_owned(),
        language,
        runtime_version: runtime_version.to_owned(),
        guest_platform: "linux".to_owned(),
        packages,
        images: vec![
            EnvironmentImageBindingV1 {
                architecture: "aarch64".to_owned(),
                image_digest: None,
            },
            EnvironmentImageBindingV1 {
                architecture: "x86_64".to_owned(),
                image_digest: None,
            },
        ],
        lock_digest: String::new(),
    };
    lock.lock_digest = expected_lock_digest(&lock).expect("baseline lock must encode");
    lock
}

fn validate_pin(package: &EnvironmentPackagePinV1) -> UseResult<()> {
    if package.name.is_empty()
        || package.version.is_empty()
        || package.version.contains(['*', '>', '<', '|', ' '])
        || package
            .name
            .chars()
            .any(|character| character.is_uppercase())
    {
        return Err(lock_error(
            "environment.lock.pin",
            format!("Package '{}' must be an exact lowercase pin.", package.name),
        ));
    }
    if FORBIDDEN_PACKAGES.contains(&package.name.as_str())
        || package.name.starts_with("clang")
        || package.name.starts_with("gcc")
        || package.name.starts_with("gfortran")
    {
        return Err(lock_error(
            "environment.lock.forbidden",
            format!(
                "Package '{}' is not part of an analysis runtime.",
                package.name
            ),
        ));
    }
    Ok(())
}

fn reject_python_lock_drift(lock: &EnvironmentLockV1) -> UseResult<()> {
    if lock.runtime_version != "3.11" {
        return Err(lock_error(
            "environment.lock.python",
            "The Python analysis lock is Python 3.11 only.",
        ));
    }
    let pandas: Vec<_> = lock
        .packages
        .iter()
        .filter(|package| package.name == "pandas")
        .collect();
    if pandas.len() != 1 || pandas[0].version.starts_with('3') {
        return Err(lock_error(
            "environment.lock.pandas",
            "The Python analysis lock pins one pandas 2.x.",
        ));
    }
    Ok(())
}

fn image_digest_for(lock: &EnvironmentLockV1, architecture: &str) -> Option<String> {
    lock.images
        .iter()
        .find(|image| image.architecture == architecture)
        .and_then(|image| image.image_digest.clone())
}

/// Record a lease only after Box returns an image digest. A crash before
/// this write leaves no half lease.
pub fn lease_built_image(
    lock: &EnvironmentLockV1,
    architecture: &str,
    image_digest: &str,
    current: Option<&EnvironmentLeaseV1>,
) -> UseResult<EnvironmentLeaseV1> {
    lock.validate()?;
    if !is_sha256_digest(image_digest) {
        return Err(lock_error(
            "environment.lease.digest",
            "Box did not return an image digest, so no lease was written.",
        ));
    }
    if architecture != "aarch64" && architecture != "x86_64" {
        return Err(lock_error(
            "environment.lock.architecture",
            "Analysis images are built for aarch64 or x86_64 Linux guests.",
        ));
    }
    Ok(EnvironmentLeaseV1 {
        schema: ENVIRONMENT_LEASE_SCHEMA_V1.to_owned(),
        lock_id: lock.lock_id.clone(),
        lock_digest: lock.lock_digest.clone(),
        image_digest: image_digest.to_owned(),
        architecture: architecture.to_owned(),
        generation: next_generation(current, image_digest),
    })
}

fn lease_matches_recipe(
    lock: &EnvironmentLockV1,
    architecture: &str,
    lease: &EnvironmentLeaseV1,
) -> bool {
    lease.lock_id == lock.lock_id
        && lease.lock_digest == lock.lock_digest
        && lease.architecture == architecture
        && is_sha256_digest(&lease.image_digest)
}

fn lease_matches(lock: &EnvironmentLockV1, architecture: &str, lease: &EnvironmentLeaseV1) -> bool {
    lease.lock_id == lock.lock_id
        && lease.lock_digest == lock.lock_digest
        && lease.architecture == architecture
        && image_digest_for(lock, architecture).as_deref() == Some(lease.image_digest.as_str())
}

fn next_generation(current: Option<&EnvironmentLeaseV1>, image_digest: &str) -> u64 {
    match current {
        Some(lease) if lease.image_digest == image_digest => lease.generation,
        Some(lease) => lease.generation.saturating_add(1).max(1),
        None => 1,
    }
}

fn expected_lock_digest(lock: &EnvironmentLockV1) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DigestBody<'a> {
        schema: &'a str,
        lock_id: &'a str,
        language: EnvironmentLanguageV1,
        runtime_version: &'a str,
        guest_platform: &'a str,
        packages: &'a [EnvironmentPackagePinV1],
        images: &'a [EnvironmentImageBindingV1],
    }
    let body = DigestBody {
        schema: &lock.schema,
        lock_id: &lock.lock_id,
        language: lock.language,
        runtime_version: &lock.runtime_version,
        guest_platform: &lock.guest_platform,
        packages: &lock.packages,
        images: &lock.images,
    };
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    body.serialize(&mut serializer)
        .map_err(|error| lock_error("environment.lock.encode", error.to_string()))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn is_sha256_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64 && hex.chars().all(|character| character.is_ascii_hexdigit())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> UseResult<()> {
    let temporary = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&temporary).map_err(|error| io_error(error.to_string()))?;
        file.write_all(bytes)
            .map_err(|error| io_error(error.to_string()))?;
        file.sync_all()
            .map_err(|error| io_error(error.to_string()))?;
    }
    fs::rename(&temporary, path).map_err(|error| io_error(error.to_string()))?;
    Ok(())
}

fn lock_error(code: &str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

fn io_error(message: String) -> UseError {
    UseError::new("environment.lease.io", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_lock_is_stable_and_excludes_the_second_stack() {
        let lock = scientist_python_analysis_lock();
        lock.validate().expect("python lock");
        assert_eq!(
            scientist_python_analysis_lock().lock_digest,
            lock.lock_digest
        );
        assert_eq!(lock.language, EnvironmentLanguageV1::Python);
        assert_eq!(lock.runtime_version, "3.11");
        assert_eq!(
            lock.packages
                .iter()
                .filter(|package| package.name == "pandas")
                .count(),
            1
        );
        assert!(lock
            .packages
            .iter()
            .all(|package| !package.version.starts_with('3') || package.name != "pandas"));
        assert!(lock.image_digest("aarch64").is_none());
        assert!(lock.image_digest("x86_64").is_none());
        assert_eq!(
            lock.packages
                .iter()
                .map(|package| (package.name.as_str(), package.version.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("matplotlib", "3.10.1"),
                ("numpy", "2.2.6"),
                ("pandas", "2.2.3"),
                ("pillow", "11.1.0"),
                ("scikit-learn", "1.6.1"),
                ("scipy", "1.15.2"),
                ("seaborn", "0.13.2"),
                ("statsmodels", "0.14.4"),
            ]
        );
        let recipe = render_runtime_image_recipe(&lock).expect("recipe");
        assert!(recipe.contains("numpy==2.2.6"));
        assert!(!recipe.contains("clang"));
        assert!(!recipe.contains("gfortran"));
        assert!(!recipe.contains("nodejs"));
    }

    #[test]
    fn unpublished_lock_builds_in_box_and_does_not_lease_without_a_digest() {
        let lock = scientist_python_analysis_lock();
        let preflight =
            classify_environment_preflight(&lock, "aarch64", false, false).expect("preflight");
        assert_eq!(preflight.status, EnvironmentPreflightStatusV1::Installable);
        let failed = failed_environment_probe(&lock, "aarch64").expect("probe");
        assert_eq!(failed.status, EnvironmentPreflightStatusV1::Failed);
        assert!(failed
            .reason
            .unwrap_or_default()
            .contains("No lease was written"));
        let decision = decide_environment_install(&lock, "aarch64", false, None).expect("install");
        assert!(matches!(
            decision,
            EnvironmentInstallDecisionV1::Build { .. }
        ));
        let error = lease_built_image(&lock, "aarch64", "not-a-digest", None).expect_err("digest");
        assert_eq!(error.code, "environment.lease.digest");
        let error = pin_environment_execution(None, None, &[]).expect_err("unleased");
        assert_eq!(error.code, "environment.lease.missing");
        assert!(!error.message.to_ascii_lowercase().contains("path"));
    }

    #[test]
    fn switch_prompt_is_python_only() {
        let locks = scientist_switch_prompt_locks();
        assert_eq!(locks.len(), 1);
        assert_eq!(locks[0].lock_id, SCIENCE_PYTHON_ANALYSIS_LOCK_ID);
        assert_ne!(scientist_r_analysis_lock().lock_id, locks[0].lock_id);
        scientist_r_analysis_lock().validate().expect("r lock");
    }

    #[test]
    fn same_digest_install_does_not_bump_generation() {
        let mut lock = scientist_python_analysis_lock();
        let digest = format!("sha256:{}", "ab".repeat(32));
        lock.images[0].image_digest = Some(digest.clone());
        lock.lock_digest = expected_lock_digest(&lock).expect("digest");
        let first = match decide_environment_install(&lock, "aarch64", true, None).expect("first") {
            EnvironmentInstallDecisionV1::RecordLease { lease } => lease,
            other => panic!("expected record, got {other:?}"),
        };
        assert_eq!(first.generation, 1);
        let second = match decide_environment_install(&lock, "aarch64", true, Some(&first))
            .expect("second")
        {
            EnvironmentInstallDecisionV1::RecordLease { lease } => lease,
            other => panic!("expected record, got {other:?}"),
        };
        assert_eq!(second.generation, 1);
        let pinned =
            pin_environment_execution(Some(&second), Some(1), &[first.clone()]).expect("pin");
        assert_eq!(pinned.image_digest, digest);
    }

    #[test]
    fn literature_connector_does_not_report_a_search() {
        let error = literature_search_refusal(&scientist_openalex_connector());
        assert_eq!(error.code, "science.connector.not_searched");
        assert!(error.message.contains("no database was queried"));
        let digest = scientist_openalex_package_digest().expect("digest");
        assert_eq!(
            digest,
            "sha256:c7cfc6938806f608eda54f8f7933e10bbfd56cf8346b445af0cbca0fea9a0b8c"
        );
        let error = lease_literature_connector(&scientist_openalex_connector(), None)
            .expect_err("openalex is not the literature connector");
        assert_eq!(error.code, "science.connector.unknown");
        let refused = authorize_literature_search(&scientist_sciverse_connector(), None)
            .expect_err("unleased");
        assert!(refused.message.contains("no database was queried"));
    }

    #[test]
    fn publish_requires_a_digest_and_cancel_writes_no_success_receipt() {
        let directory = tempfile::tempdir().expect("temp");
        let directory = directory.path();
        let lock = scientist_python_analysis_lock();
        let digest = format!("sha256:{}", "11".repeat(32));
        let lease = lease_built_image(&lock, "aarch64", &digest, None).expect("lease");
        let published = publish_environment_capability(directory, &lease).expect("publish");
        assert_eq!(published.generation, 1);
        assert_eq!(published.image_digest, digest);
        assert!(
            read_environment_lease(directory, SCIENCE_PYTHON_ANALYSIS_LOCK_ID)
                .expect("read")
                .is_some()
        );
        let pin =
            pin_published_capability(Some(&lease), Some(&published), None, &[], &[]).expect("pin");
        let unpublished = pin_published_capability(Some(&lease), None, None, &[], &[]);
        assert_eq!(
            unpublished.expect_err("unpublished").code,
            "environment.capability.unpublished"
        );
        let cancelled = analysis_session_receipt(
            true,
            &pin,
            &format!("sha256:{}", "22".repeat(32)),
            &format!("sha256:{}", "33".repeat(32)),
            &[],
        );
        assert_eq!(
            cancelled.expect_err("cancelled").code,
            "science.session.cancelled"
        );
        let receipt = analysis_session_receipt(
            false,
            &pin,
            &format!("sha256:{}", "22".repeat(32)),
            &format!("sha256:{}", "33".repeat(32)),
            &[ScienceArtifactReceiptV1 {
                path: "figure.png".to_owned(),
                digest: format!("sha256:{}", "44".repeat(32)),
            }],
        )
        .expect("receipt");
        assert_eq!(receipt.artifacts.len(), 1);
        assert_eq!(receipt.image_digest, digest);
    }

    #[test]
    fn lease_write_is_replaced_atomically() {
        let directory = tempfile::tempdir().expect("temp");
        let directory = directory.path();
        let lease = EnvironmentLeaseV1 {
            schema: ENVIRONMENT_LEASE_SCHEMA_V1.to_owned(),
            lock_id: SCIENCE_PYTHON_ANALYSIS_LOCK_ID.to_owned(),
            lock_digest: format!("sha256:{}", "cd".repeat(32)),
            image_digest: format!("sha256:{}", "ef".repeat(32)),
            architecture: "aarch64".to_owned(),
            generation: 1,
        };
        write_environment_lease(&directory, &lease).expect("write");
        let read = read_environment_lease(&directory, SCIENCE_PYTHON_ANALYSIS_LOCK_ID)
            .expect("read")
            .expect("present");
        assert_eq!(read, lease);
        assert!(directory
            .join(format!("{SCIENCE_PYTHON_ANALYSIS_LOCK_ID}-1.json"))
            .is_file());
    }

    #[test]
    fn committed_runtime_recipes_match_the_locks() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../environments");
        let python =
            fs::read_to_string(root.join("science-python-3.11.Dockerfile")).expect("python recipe");
        let r = fs::read_to_string(root.join("science-r-4.5.Dockerfile")).expect("r recipe");
        assert_eq!(
            python,
            render_runtime_image_recipe(&scientist_python_analysis_lock()).expect("python")
        );
        assert_eq!(
            r,
            render_runtime_image_recipe(&scientist_r_analysis_lock()).expect("r")
        );
        assert!(!python
            .lines()
            .any(|line| line.starts_with("RUN") && line.contains("gcc")));
        assert!(r.contains("Do not install"));
        assert!(!r
            .lines()
            .any(|line| line.starts_with("RUN") && line.contains("gfortran")));
        assert_r_recipe_materializes_pins(&r);
    }

    #[test]
    fn r_lock_builds_without_a_digest_and_does_not_lease() {
        let lock = scientist_r_analysis_lock();
        assert!(lock.image_digest("aarch64").is_none());
        assert!(lock.image_digest("x86_64").is_none());
        let decision = decide_environment_install(&lock, "aarch64", false, None).expect("install");
        assert!(matches!(
            decision,
            EnvironmentInstallDecisionV1::Build { .. }
        ));
        let error = lease_built_image(&lock, "aarch64", "not-a-digest", None).expect_err("digest");
        assert_eq!(error.code, "environment.lease.digest");
        let recipe = render_runtime_image_recipe(&lock).expect("recipe");
        assert_r_recipe_materializes_pins(&recipe);
    }

    fn recipe_code_contains(recipe: &str, fragment: &str) -> bool {
        recipe.lines().any(|line| {
            let code = line.split('#').next().unwrap_or("");
            code.contains(fragment)
        })
    }

    fn assert_r_recipe_materializes_pins(recipe: &str) {
        let lock = scientist_r_analysis_lock();
        assert_eq!(
            lock.packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>(),
            vec!["fonts-dejavu", "pandoc", "r-base", "r-tidyverse"]
        );
        let runtime = recipe
            .rsplit_once("FROM r-base:4.5.3\n")
            .expect("runtime stage")
            .1;
        assert!(!runtime.contains("AS packages"));
        assert!(runtime.contains("COPY --from=packages"));
        assert!(runtime.contains("refusing to install a toolchain"));
        assert!(!runtime.contains("conda"));
        assert!(!runtime.contains("osx-arm64"));
        assert!(recipe_code_contains(recipe, "FROM r-base:4.5.3"));
        assert!(recipe_code_contains(recipe, "r-cran-tidyverse"));
        assert!(recipe_code_contains(
            recipe,
            "packageVersion(\"tidyverse\")) == \"2.0.0\""
        ));
        assert!(recipe_code_contains(recipe, "pandoc-3.6.4"));
        assert!(recipe_code_contains(recipe, "fonts-dejavu"));
        assert!(recipe_code_contains(recipe, "= \"2.37\""));
        assert!(!runtime.lines().any(|line| {
            let code = line.split('#').next().unwrap_or("");
            code.contains("apt-get install")
                && (code.contains("gcc")
                    || code.contains("gfortran")
                    || code.contains("clang")
                    || code.contains(" git")
                    || code.contains("nodejs"))
        }));
    }

    #[test]
    fn retained_leases_skip_capability_bindings() {
        let digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let directory =
            std::env::temp_dir().join(format!("a3s-retained-lease-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let lease = EnvironmentLeaseV1 {
            schema: ENVIRONMENT_LEASE_SCHEMA_V1.to_owned(),
            lock_id: SCIENCE_PYTHON_ANALYSIS_LOCK_ID.to_owned(),
            lock_digest: digest.to_owned(),
            image_digest: digest.to_owned(),
            architecture: "aarch64".to_owned(),
            generation: 1,
        };
        publish_environment_capability(&directory, &lease).expect("publish");
        let retained =
            read_retained_environment_leases(&directory, SCIENCE_PYTHON_ANALYSIS_LOCK_ID)
                .expect("retained leases");
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].generation, 1);
        let _ = fs::remove_dir_all(&directory);
    }
}
