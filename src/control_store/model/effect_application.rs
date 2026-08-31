use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::*;

pub(in crate::control_store) const CONTROL_APPLIED_EFFECT_SCHEMA: &str =
    "a3s.use.control-applied-effect.v1";
pub(in crate::control_store) const MAX_CONTROL_APPLIED_EFFECT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::control_store) enum ControlSurfaceObservationState {
    Prepared,
    Stopped,
    Removed,
}

impl ControlSurfaceObservationState {
    fn matches(self, kind: ControlEffectKind) -> bool {
        matches!(
            (self, kind),
            (Self::Prepared, ControlEffectKind::SurfacePrepare)
                | (Self::Stopped, ControlEffectKind::SurfaceStop)
                | (Self::Removed, ControlEffectKind::SurfaceRemove)
        )
    }
}

/// Portable Runtime binding evidence retained by the Control aggregate.
///
/// Task bindings need no endpoint. Service bindings retain only the opaque,
/// non-secret Gateway reference and a readiness digest; URLs and local paths
/// are deliberately unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "bindingKind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlRuntimeBindingObservation {
    Task,
    Service {
        endpoint_ref: String,
        readiness_digest: String,
    },
}

impl ControlRuntimeBindingObservation {
    fn validate(&self) -> bool {
        match self {
            Self::Task => true,
            Self::Service {
                endpoint_ref,
                readiness_digest,
            } => valid_gateway_endpoint_ref(endpoint_ref) && valid_sha256(readiness_digest),
        }
    }
}

/// Typed, owner-specific proof returned after one external effect is applied.
///
/// Every variant carries a provider receipt digest. Preparation additionally
/// retains only the portable materialization evidence needed by capability
/// publication; retirement variants cannot smuggle a new binding into state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "owner",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlAppliedEffectEvidence {
    CapabilityIndex {
        capability_generation: u64,
        descriptor_digest: String,
        receipt_digest: String,
    },
    InvocationLeases {
        package_id: String,
        lifecycle_generation: u64,
        receipt_digest: String,
    },
    RuntimeProvider {
        state: ControlSurfaceObservationState,
        provider_id: String,
        selection_digest: String,
        receipt_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binding: Option<ControlRuntimeBindingObservation>,
    },
    FlowHost {
        state: ControlSurfaceObservationState,
        receipt_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact_digest: Option<String>,
    },
    KnowledgeHost {
        state: ControlSurfaceObservationState,
        receipt_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projection_digest: Option<String>,
    },
    SkillHost {
        state: ControlSurfaceObservationState,
        receipt_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_digest: Option<String>,
    },
    UiHost {
        state: ControlSurfaceObservationState,
        receipt_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_digest: Option<String>,
    },
}

impl ControlAppliedEffectEvidence {
    fn matches(&self, intent: &ControlEffectIntent) -> bool {
        match (self, &intent.owner, &intent.subject, intent.kind) {
            (
                Self::CapabilityIndex {
                    capability_generation,
                    descriptor_digest,
                    receipt_digest,
                },
                ControlEffectOwner::CapabilityIndex,
                ControlEffectSubject::Installation {
                    capability_generation: expected_generation,
                    descriptor_digest: expected_descriptor,
                    ..
                },
                ControlEffectKind::CapabilityCutover,
            ) => {
                capability_generation == expected_generation
                    && descriptor_digest == expected_descriptor
                    && valid_sha256(receipt_digest)
            }
            (
                Self::InvocationLeases {
                    package_id,
                    lifecycle_generation,
                    receipt_digest,
                },
                ControlEffectOwner::InvocationLeases,
                ControlEffectSubject::Package {
                    package_id: expected_package,
                    lifecycle_generation: expected_generation,
                    ..
                },
                ControlEffectKind::CallsDrain,
            ) => {
                package_id == expected_package
                    && lifecycle_generation == expected_generation
                    && valid_sha256(receipt_digest)
            }
            (
                Self::RuntimeProvider {
                    state,
                    provider_id,
                    selection_digest,
                    receipt_digest,
                    binding,
                },
                ControlEffectOwner::RuntimeProvider {
                    provider_id: expected_provider,
                    selection_digest: expected_selection,
                },
                ControlEffectSubject::Surface { .. },
                kind,
            ) => {
                state.matches(kind)
                    && provider_id == expected_provider
                    && selection_digest == expected_selection
                    && valid_sha256(receipt_digest)
                    && match kind {
                        ControlEffectKind::SurfacePrepare => {
                            binding.as_ref().is_some_and(|binding| binding.validate())
                        }
                        ControlEffectKind::SurfaceStop | ControlEffectKind::SurfaceRemove => {
                            binding.is_none()
                        }
                        ControlEffectKind::CapabilityCutover | ControlEffectKind::CallsDrain => {
                            false
                        }
                    }
            }
            (
                Self::FlowHost {
                    state,
                    receipt_digest,
                    artifact_digest,
                },
                ControlEffectOwner::FlowHost,
                ControlEffectSubject::Surface { .. },
                kind,
            ) => {
                state.matches(kind)
                    && valid_sha256(receipt_digest)
                    && preparation_digest_matches(kind, artifact_digest.as_deref())
            }
            (
                Self::KnowledgeHost {
                    state,
                    receipt_digest,
                    projection_digest,
                },
                ControlEffectOwner::KnowledgeHost,
                ControlEffectSubject::Surface { .. },
                kind,
            ) => {
                state.matches(kind)
                    && valid_sha256(receipt_digest)
                    && preparation_digest_matches(kind, projection_digest.as_deref())
            }
            (
                Self::SkillHost {
                    state,
                    receipt_digest,
                    content_digest,
                },
                ControlEffectOwner::SkillHost,
                ControlEffectSubject::Surface { .. },
                kind,
            )
            | (
                Self::UiHost {
                    state,
                    receipt_digest,
                    content_digest,
                },
                ControlEffectOwner::UiHost,
                ControlEffectSubject::Surface { .. },
                kind,
            ) => {
                state.matches(kind)
                    && valid_sha256(receipt_digest)
                    && preparation_digest_matches(kind, content_digest.as_deref())
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlAppliedEffect {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) idempotency_key: String,
    pub(in crate::control_store) evidence: ControlAppliedEffectEvidence,
}

impl ControlAppliedEffect {
    pub(in crate::control_store) fn new(
        intent: &ControlEffectIntent,
        evidence: ControlAppliedEffectEvidence,
    ) -> UseResult<Self> {
        let applied = Self {
            schema: CONTROL_APPLIED_EFFECT_SCHEMA.to_string(),
            idempotency_key: intent.idempotency_key.clone(),
            evidence,
        };
        applied.validate_for(intent)?;
        Ok(applied)
    }

    pub(in crate::control_store) fn validate_for(
        &self,
        intent: &ControlEffectIntent,
    ) -> UseResult<()> {
        if self.schema != CONTROL_APPLIED_EFFECT_SCHEMA
            || self.idempotency_key != intent.idempotency_key
            || !valid_sha256(&self.idempotency_key)
            || !self.evidence.matches(intent)
            || self.canonical_bytes().is_err()
        {
            return Err(input_error(
                "The applied Control Store effect evidence does not bind its exact intent.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            input_error(format!(
                "Failed to encode canonical applied Control Store effect evidence: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_CONTROL_APPLIED_EFFECT_BYTES {
            return Err(input_error(
                "The applied Control Store effect evidence exceeds its size bound.",
            ));
        }
        Ok(bytes)
    }

    pub(in crate::control_store) fn descriptor_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }
}

fn preparation_digest_matches(kind: ControlEffectKind, digest: Option<&str>) -> bool {
    match kind {
        ControlEffectKind::SurfacePrepare => digest.is_some_and(valid_sha256),
        ControlEffectKind::SurfaceStop | ControlEffectKind::SurfaceRemove => digest.is_none(),
        ControlEffectKind::CapabilityCutover | ControlEffectKind::CallsDrain => false,
    }
}

fn valid_gateway_endpoint_ref(value: &str) -> bool {
    let Some(binding_id) = value.strip_prefix("gateway:") else {
        return false;
    };
    !binding_id.is_empty()
        && binding_id.len() <= 256
        && binding_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
        && !binding_id.contains("//")
        && !binding_id
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
}
