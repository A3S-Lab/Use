//! Verified Agent descriptor projection for the inactive Control capability
//! plane.
//!
//! The Capability Index must not treat a host supplied descriptor as proof of
//! an executable route.  This module joins the host's already-verified signed
//! description with the terminal owner evidence retained by Control.  It is a
//! pure, deterministic projection: it performs no filesystem or provider I/O
//! and therefore can safely be retried before immutable catalog publication.

use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    capability_schema_digest, ArtifactRef, CapabilityDescriptionProof, CapabilityDescriptor,
    CapabilityDescriptorKind, CapabilityGatewayCatalog, EndpointRef, InstallationId, InvocationRef,
    PluginPackageId, PluginSurfaceKind, PluginSurfaceRef, ResourceRef, ToolWorkloadClass, UseError,
    UseResult,
};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::super::effect_port::{
    ControlCapabilityCatalogProjectionPort, ControlEffectFailure, ControlEffectPortOutcome,
};
use super::super::super::model::{
    valid_sha256, ControlAppliedEffectEvidence, ControlCapabilityEffectAuthority,
    ControlCapabilitySurfaceState, ControlEffectOwner, ControlEffectSubject,
    ControlRuntimeSchemaAttestation,
};
use super::descriptor_snapshot::{
    ControlCapabilityDescriptorSnapshotKey, ControlCapabilityDescriptorSnapshotStore,
    SNAPSHOT_MISSING, SNAPSHOT_RETRYABLE_BUSY, SNAPSHOT_RETRYABLE_IO,
};

/// Fixed error exposed by the owner port.  Detailed proof and package data
/// never crosses the generic effect-observation boundary.
const DESCRIPTOR_PROJECTION_ERROR: &str =
    "use.control_store.capability_descriptor_projection_invalid";
const DESCRIPTOR_FAILURE_DOMAIN: &[u8] = b"a3s.use.control-capability-descriptor-failure.v1\0";
const ROUTE_BINDING_SCHEMA: &str = "a3s.use.control-capability-route-binding.v1";
const ROUTE_BINDING_DOMAIN: &[u8] = b"a3s.use.control-capability-route-binding.v1\0";
const MAX_DESCRIPTION_PROOFS: usize = 1_024;
const MAX_TRUSTED_SIGNERS: usize = 4_096;

/// Explicit package-to-signer admission policy.
///
/// `CapabilityDescriptionProof` records that a host has completed signature
/// verification, but the core proof envelope intentionally does not choose a
/// key store or a trust root.  The Control projector therefore requires an
/// immutable, package-scoped allowlist at construction time.  A missing entry
/// is a rejection, never an implicit allow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilitySignerPolicy {
    package_signers: BTreeMap<String, BTreeSet<String>>,
}

impl ControlCapabilitySignerPolicy {
    pub(in crate::control_store) fn new(
        package_signers: BTreeMap<String, BTreeSet<String>>,
    ) -> UseResult<Self> {
        let signer_count = package_signers.values().map(BTreeSet::len).sum::<usize>();
        if package_signers.len() > MAX_TRUSTED_SIGNERS || signer_count > MAX_TRUSTED_SIGNERS {
            return Err(projection_error(
                "The capability signer policy exceeds its bounded entry limit.",
            ));
        }
        for (package_id, signers) in &package_signers {
            PluginPackageId::parse(package_id.clone()).map_err(|_| {
                projection_error("The capability signer policy contains an invalid package.")
            })?;
            if signers.is_empty()
                || signers.len() > MAX_TRUSTED_SIGNERS
                || signers.iter().any(|signer| !valid_signer_id(signer))
            {
                return Err(projection_error(
                    "The capability signer policy contains an invalid signer set.",
                ));
            }
        }
        Ok(Self { package_signers })
    }

    fn permits(&self, package_id: &str, signer_id: &str) -> bool {
        self.package_signers
            .get(package_id)
            .is_some_and(|signers| signers.contains(signer_id))
    }

    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        Self::new(self.package_signers.clone()).map(|_| ())
    }

    pub(in crate::control_store) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
            projection_error(format!(
                "Failed to encode the capability signer policy: {error}"
            ))
        })?;
        Ok(bytes)
    }
}

/// Host-owned, deterministic projection from verified descriptions to one
/// Control-bound Agent catalog.
///
/// The proof list is immutable for the lifetime of the projector.  Hosts must
/// construct a new projector when their signed-description snapshot or signer
/// policy changes.  A subset of prepared surfaces is allowed: omitting a
/// surface is how a host implements consumer/profile filtering without
/// pretending that an unreviewed descriptor is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityDescriptorProjection {
    source: DescriptorProjectionSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DescriptorProjectionSource {
    InMemory {
        proofs: Vec<CapabilityDescriptionProof>,
        signer_policy: ControlCapabilitySignerPolicy,
    },
    Durable(ControlCapabilityDescriptorSnapshotStore),
}

impl ControlCapabilityDescriptorProjection {
    pub(in crate::control_store) fn new(
        mut proofs: Vec<CapabilityDescriptionProof>,
        signer_policy: ControlCapabilitySignerPolicy,
    ) -> UseResult<Self> {
        signer_policy.validate()?;
        if proofs.len() > MAX_DESCRIPTION_PROOFS {
            return Err(projection_error(
                "The capability description proof set exceeds its bound.",
            ));
        }
        let mut identities = BTreeSet::new();
        for proof in &proofs {
            proof
                .validate()
                .map_err(|_| projection_error("A capability description proof is invalid."))?;
            let descriptor = proof.descriptor();
            let identity = (
                descriptor.package_id.to_string(),
                descriptor.surface.clone(),
                proof.descriptor_digest.clone(),
            );
            if !identities.insert(identity) {
                return Err(projection_error(
                    "The capability description proof set contains a duplicate identity.",
                ));
            }
        }
        // The catalog constructor sorts descriptors, but sorting the immutable
        // proof set as well makes policy/debug output deterministic before the
        // projection starts.
        proofs.sort_by(|left, right| {
            left.descriptor
                .package_id
                .to_string()
                .cmp(&right.descriptor.package_id.to_string())
                .then_with(|| left.descriptor.surface.cmp(&right.descriptor.surface))
                .then_with(|| left.descriptor_digest.cmp(&right.descriptor_digest))
        });
        Ok(Self {
            source: DescriptorProjectionSource::InMemory {
                proofs,
                signer_policy,
            },
        })
    }

    pub(in crate::control_store) fn from_snapshot_store(
        store: ControlCapabilityDescriptorSnapshotStore,
    ) -> UseResult<Self> {
        store.validate_configuration()?;
        Ok(Self {
            source: DescriptorProjectionSource::Durable(store),
        })
    }

    pub(in crate::control_store) fn into_parts(
        self,
    ) -> UseResult<(
        Vec<CapabilityDescriptionProof>,
        ControlCapabilitySignerPolicy,
    )> {
        match self.source {
            DescriptorProjectionSource::InMemory {
                proofs,
                signer_policy,
            } => Ok((proofs, signer_policy)),
            DescriptorProjectionSource::Durable(_) => Err(projection_error(
                "A durable descriptor projector has no in-memory proof set.",
            )),
        }
    }

    /// Project and validate all supplied proofs against one committed target
    /// generation.  This method is useful to synchronous composition tests;
    /// the effect-port implementation below maps failures to a safe rejected
    /// outcome without exposing diagnostic text.
    pub(in crate::control_store) fn project_catalog(
        &self,
        authority: &ControlCapabilityEffectAuthority,
    ) -> UseResult<CapabilityGatewayCatalog> {
        let DescriptorProjectionSource::InMemory {
            proofs,
            signer_policy,
        } = &self.source
        else {
            return Err(projection_error(
                "A durable descriptor projector must be awaited before projection.",
            ));
        };
        project_catalog_from_parts(authority, proofs, signer_policy)
    }

    async fn project_catalog_async(
        &self,
        authority: &ControlCapabilityEffectAuthority,
    ) -> UseResult<CapabilityGatewayCatalog> {
        match &self.source {
            DescriptorProjectionSource::InMemory {
                proofs,
                signer_policy,
            } => project_catalog_from_parts(authority, proofs, signer_policy),
            DescriptorProjectionSource::Durable(store) => {
                let key = ControlCapabilityDescriptorSnapshotKey::from_authority(authority)?;
                let snapshot = store.get(&key).await?.ok_or_else(|| {
                    UseError::new(
                        SNAPSHOT_MISSING,
                        "The committed capability descriptor proof snapshot is not present.",
                    )
                })?;
                project_catalog_from_parts(authority, snapshot.proofs(), snapshot.signer_policy())
            }
        }
    }

    /// Return the deterministic route identities that a host must place in a
    /// signed description.  Keeping this derivation beside validation avoids
    /// a second, subtly different implementation in a production host.
    pub(in crate::control_store) fn route_binding(
        authority: &ControlCapabilityEffectAuthority,
        package_id: &PluginPackageId,
        surface: &PluginSurfaceRef,
    ) -> UseResult<ControlCapabilityRouteBinding> {
        route_binding(authority, package_id.as_str(), surface)
    }
}

fn project_catalog_from_parts(
    authority: &ControlCapabilityEffectAuthority,
    proofs: &[CapabilityDescriptionProof],
    signer_policy: &ControlCapabilitySignerPolicy,
) -> UseResult<CapabilityGatewayCatalog> {
    let mut descriptors = Vec::with_capacity(proofs.len());
    for proof in proofs {
        proof
            .validate()
            .map_err(|_| projection_error("A capability description proof is invalid."))?;
        let descriptor = proof.descriptor();
        if !signer_policy.permits(descriptor.package_id.as_str(), &proof.signer_id) {
            return Err(projection_error(
                "A capability description signer is not authorized for its package.",
            ));
        }
        validate_descriptor_binding(authority, descriptor)?;
        descriptors.push(descriptor.clone());
    }
    CapabilityGatewayCatalog::new(
        authority.generation.snapshot.installation.clone(),
        authority.generation.capability.generation,
        descriptors,
    )
    .map_err(|_| projection_error("The projected capability catalog is invalid."))
}

#[async_trait::async_trait]
impl ControlCapabilityCatalogProjectionPort for ControlCapabilityDescriptorProjection {
    async fn project(
        &self,
        authority: &ControlCapabilityEffectAuthority,
    ) -> ControlEffectPortOutcome<CapabilityGatewayCatalog> {
        match self.project_catalog_async(authority).await {
            Ok(catalog) => ControlEffectPortOutcome::applied(catalog),
            Err(error) if is_retryable_snapshot_error(&error.code) => {
                ControlEffectPortOutcome::deferred(projection_failure(&error.code))
            }
            Err(error) => ControlEffectPortOutcome::rejected(projection_failure(&error.code)),
        }
    }
}

fn is_retryable_snapshot_error(code: &str) -> bool {
    matches!(
        code,
        SNAPSHOT_MISSING | SNAPSHOT_RETRYABLE_IO | SNAPSHOT_RETRYABLE_BUSY
    )
}

/// Opaque references derived from one exact prepared owner receipt.
///
/// The values are intended for host composition: the host can generate a
/// description from these identities without copying a local path, endpoint
/// URL, or provider credential into the descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityRouteBinding {
    pub(in crate::control_store) evidence_digest: String,
    pub(in crate::control_store) invocation_ref: InvocationRef,
    pub(in crate::control_store) endpoint_ref: Option<EndpointRef>,
    pub(in crate::control_store) artifact_ref: Option<ArtifactRef>,
    pub(in crate::control_store) resource_ref: ResourceRef,
    pub(in crate::control_store) runtime_schema_attestation:
        Option<ControlRuntimeSchemaAttestation>,
}

fn validate_descriptor_binding(
    authority: &ControlCapabilityEffectAuthority,
    descriptor: &CapabilityDescriptor,
) -> UseResult<()> {
    descriptor
        .validate()
        .map_err(|_| projection_error("A capability descriptor is invalid."))?;
    let generation = &authority.generation;
    let package_id = descriptor.package_id.as_str();
    let package = generation
        .snapshot
        .package_selection(package_id)
        .ok_or_else(|| projection_error("A descriptor package is outside Control authority."))?;
    let lifecycle_generation = generation
        .package_lifecycles
        .iter()
        .find(|lifecycle| lifecycle.package_id == *package_id)
        .map(|lifecycle| lifecycle.lifecycle_generation)
        .ok_or_else(|| projection_error("A descriptor package has no lifecycle identity."))?;
    let catalog_package = &package.package.catalog.record.package;
    let catalog_record_digest = &package.package.catalog.provenance.catalog_record_digest;
    if !package.enabled
        || descriptor.generation != lifecycle_generation
        || catalog_package.sha256.as_deref() != Some(descriptor.package_digest.as_str())
        || catalog_package.manifest_sha256.as_deref() != Some(descriptor.manifest_digest.as_str())
        || descriptor.publication.catalog_record_digest != *catalog_record_digest
        || !package.selected_surfaces.contains(&descriptor.surface)
    {
        return Err(projection_error(
            "A descriptor does not match its enabled package incarnation.",
        ));
    }

    let catalog_surface = package
        .package
        .catalog
        .record
        .surfaces
        .iter()
        .find(|surface| surface.reference() == descriptor.surface)
        .ok_or_else(|| {
            projection_error("A descriptor surface is absent from its package catalog.")
        })?;
    if descriptor.dependencies != catalog_surface.requires {
        return Err(projection_error(
            "A descriptor dependency set differs from the signed surface graph.",
        ));
    }

    let route = route_binding(
        authority,
        descriptor.package_id.as_str(),
        &descriptor.surface,
    )?;
    if descriptor.invocation_ref != route.invocation_ref
        || descriptor.endpoint_ref != route.endpoint_ref
        || descriptor.artifact_ref != route.artifact_ref
    {
        return Err(projection_error(
            "A descriptor opaque route is not bound to its prepared owner receipt.",
        ));
    }

    if let CapabilityDescriptorKind::Resource { uri, .. } = &descriptor.capability {
        if uri != &route.resource_ref {
            return Err(projection_error(
                "A Resource URI is not bound to its prepared owner receipt.",
            ));
        }
    }

    validate_descriptor_kind(authority, descriptor, catalog_surface, &route)
}

fn validate_descriptor_kind(
    authority: &ControlCapabilityEffectAuthority,
    descriptor: &CapabilityDescriptor,
    catalog_surface: &a3s_use_core::CatalogSurface,
    route: &ControlCapabilityRouteBinding,
) -> UseResult<()> {
    let owner = prepared_owner(
        authority,
        descriptor.package_id.as_str(),
        &descriptor.surface,
    )?;
    match &descriptor.capability {
        CapabilityDescriptorKind::Tool {
            input_schema,
            output_schema,
            runtime_descriptor_digest,
            ..
        } => {
            if descriptor.surface.kind != PluginSurfaceKind::Tool
                || !matches!(owner, ControlEffectOwner::RuntimeProvider { .. })
            {
                return Err(projection_error(
                    "A Tool descriptor must bind a prepared Runtime Tool surface.",
                ));
            }
            match catalog_surface.workload {
                Some(ToolWorkloadClass::Task) if route.endpoint_ref.is_none() => {}
                Some(ToolWorkloadClass::Service) if route.endpoint_ref.is_some() => {}
                _ => {
                    return Err(projection_error(
                        "A Tool descriptor does not match its reviewed Runtime workload.",
                    ))
                }
            }
            let attestation = route.runtime_schema_attestation.as_ref().ok_or_else(|| {
                projection_error(
                    "An agent-visible Tool requires Runtime schema attestation evidence.",
                )
            })?;
            let runtime_descriptor_digest =
                runtime_descriptor_digest.as_ref().ok_or_else(|| {
                    projection_error(
                        "An agent-visible Tool must bind its signed Runtime release descriptor.",
                    )
                })?;
            let (expected_input, expected_output) = (
                capability_schema_digest(input_schema)?,
                capability_schema_digest(output_schema)?,
            );
            if attestation.descriptor_digest != *runtime_descriptor_digest
                || attestation.input_schema_digest != expected_input
                || attestation.output_schema_digest != expected_output
            {
                return Err(projection_error(
                    "A Tool descriptor schema contract differs from Runtime attestation evidence.",
                ));
            }
        }
        CapabilityDescriptorKind::McpServer { .. } => {
            if route.runtime_schema_attestation.is_some() {
                return Err(projection_error(
                    "MCP descriptors cannot carry Tool schema attestation evidence.",
                ));
            }
            if descriptor.surface.kind != PluginSurfaceKind::Mcp
                || !matches!(
                    catalog_surface.mcp_transport,
                    Some(a3s_use_core::CatalogMcpTransport::StreamableHttp)
                )
                || !matches!(owner, ControlEffectOwner::RuntimeProvider { .. })
                || route.endpoint_ref.is_none()
            {
                return Err(projection_error(
                    "An MCP descriptor requires a prepared Streamable HTTP Runtime surface.",
                ));
            }
        }
        CapabilityDescriptorKind::Resource { .. } | CapabilityDescriptorKind::Prompt { .. } => {
            if route.runtime_schema_attestation.is_some() {
                return Err(projection_error(
                    "Non-Tool descriptors cannot carry Runtime schema attestation evidence.",
                ));
            }
            // Resources and prompts may be projected by a static A3S surface
            // or by a Runtime surface.  The route/evidence checks above still
            // require a prepared owner and exact opaque references.
        }
    }
    Ok(())
}

fn prepared_owner(
    authority: &ControlCapabilityEffectAuthority,
    expected_package_id: &str,
    surface: &PluginSurfaceRef,
) -> UseResult<ControlEffectOwner> {
    let mut found = None;
    for materialization in &authority.materializations {
        let ControlEffectSubject::Surface {
            package_id,
            lifecycle_generation,
            surface: candidate,
            ..
        } = &materialization.intent.subject
        else {
            continue;
        };
        if package_id != expected_package_id || candidate != surface {
            continue;
        }
        if found.is_some() {
            return Err(projection_error(
                "A capability surface has multiple terminal owner receipts.",
            ));
        }
        let _ = lifecycle_generation;
        if !matches!(
            materialization.state,
            ControlCapabilitySurfaceState::Prepared { .. }
        ) {
            return Err(projection_error(
                "A descriptor surface is not in a prepared terminal state.",
            ));
        }
        found = Some(materialization.intent.owner.clone());
    }
    found.ok_or_else(|| projection_error("A descriptor surface has no prepared owner receipt."))
}

fn route_binding(
    authority: &ControlCapabilityEffectAuthority,
    expected_package_id: &str,
    surface: &PluginSurfaceRef,
) -> UseResult<ControlCapabilityRouteBinding> {
    let generation = &authority.generation;
    let mut materializations = authority.materializations.iter().filter(|materialization| {
        matches!(
            &materialization.intent.subject,
            ControlEffectSubject::Surface {
                package_id,
                lifecycle_generation: _,
                surface: candidate,
                ..
            } if package_id == expected_package_id && candidate == surface
        )
    });
    let materialization = materializations
        .next()
        .ok_or_else(|| projection_error("A capability surface has no terminal owner receipt."))?;
    if materializations.next().is_some() {
        return Err(projection_error(
            "A capability surface has multiple terminal owner receipts.",
        ));
    }
    let ControlCapabilitySurfaceState::Prepared {
        application,
        observed_at_ms,
    } = &materialization.state
    else {
        return Err(projection_error(
            "A capability route cannot be derived from degraded evidence.",
        ));
    };
    if *observed_at_ms == 0 {
        return Err(projection_error(
            "A prepared capability route has no observation timestamp.",
        ));
    }
    application
        .validate_for(&materialization.intent)
        .map_err(|_| projection_error("A capability owner receipt is invalid."))?;
    let (package_id, subject_package_digest, subject_manifest_digest) =
        match &materialization.intent.subject {
            ControlEffectSubject::Surface {
                package_id,
                lifecycle_generation,
                package_digest,
                manifest_digest,
                ..
            } => {
                if *lifecycle_generation
                    != generation
                        .package_lifecycles
                        .iter()
                        .find(|lifecycle| lifecycle.package_id == *package_id)
                        .map(|lifecycle| lifecycle.lifecycle_generation)
                        .unwrap_or_default()
                {
                    return Err(projection_error(
                        "A capability owner receipt has a stale lifecycle identity.",
                    ));
                }
                (package_id, package_digest, manifest_digest)
            }
            _ => {
                return Err(projection_error(
                    "A capability owner receipt has no surface identity.",
                ))
            }
        };
    let package = generation
        .snapshot
        .package_selection(package_id)
        .ok_or_else(|| projection_error("A capability owner receipt package is absent."))?;
    if !package.enabled
        || !package.selected_surfaces.contains(surface)
        || package.package.catalog.record.package.sha256.as_deref()
            != Some(subject_package_digest.as_str())
        || package
            .package
            .catalog
            .record
            .package
            .manifest_sha256
            .as_deref()
            != Some(subject_manifest_digest.as_str())
        || materialization.intent.installation != generation.snapshot.installation
        || !materialization.intent.owner.matches_generation(
            &materialization.intent.subject,
            materialization.intent.kind,
            &generation.provider_selections,
        )
    {
        return Err(projection_error(
            "A capability owner receipt is outside selected package authority.",
        ));
    }
    let lifecycle_generation = generation
        .package_lifecycles
        .iter()
        .find(|lifecycle| lifecycle.package_id == *package_id)
        .map(|lifecycle| lifecycle.lifecycle_generation)
        .ok_or_else(|| projection_error("A capability package has no lifecycle identity."))?;
    let application_digest = application.descriptor_digest()?;
    let (has_endpoint, artifact_source, runtime_schema_attestation) = match &application.evidence {
        ControlAppliedEffectEvidence::RuntimeProvider {
            binding,
            schema_attestation,
            ..
        } => match binding {
            Some(super::super::super::model::ControlRuntimeBindingObservation::Service {
                ..
            }) => (true, None, schema_attestation.clone()),
            Some(super::super::super::model::ControlRuntimeBindingObservation::Task) => {
                (false, None, schema_attestation.clone())
            }
            None => {
                return Err(projection_error(
                    "A prepared Runtime surface has no binding evidence.",
                ))
            }
        },
        ControlAppliedEffectEvidence::FlowHost {
            artifact_digest, ..
        } => (false, artifact_digest.as_deref(), None),
        ControlAppliedEffectEvidence::KnowledgeHost {
            projection_digest, ..
        } => (false, projection_digest.as_deref(), None),
        ControlAppliedEffectEvidence::SkillHost { content_digest, .. }
        | ControlAppliedEffectEvidence::UiHost { content_digest, .. } => {
            (false, content_digest.as_deref(), None)
        }
        ControlAppliedEffectEvidence::CapabilityIndex { .. }
        | ControlAppliedEffectEvidence::InvocationLeases { .. } => {
            return Err(projection_error(
                "A capability descriptor references an incompatible owner receipt.",
            ))
        }
    };
    let grant = generation
        .grants
        .iter()
        .find(|grant| grant.package_id() == package_id);
    let selected = package
        .package
        .catalog
        .selected_state(&package.selected_surfaces)
        .map_err(|_| projection_error("A capability package permission projection is invalid."))?;
    let grant_digest = if selected.permissions.surfaces.is_empty() {
        if grant.is_some() {
            return Err(projection_error(
                "A capability package carries an unexpected Workspace Grant.",
            ));
        }
        None
    } else {
        let grant = grant.ok_or_else(|| {
            projection_error("A permission-bearing capability has no committed Grant.")
        })?;
        grant
            .grant
            .validate_active_against(
                &package.package.catalog.record.permission_ceiling,
                generation.committed_at_ms,
            )
            .map_err(|_| projection_error("A capability Grant is not active at publication."))?;
        Some((grant.grant_digest.as_str(), grant.receipt_revision))
    };
    let evidence_digest = route_evidence_digest(RouteEvidenceMaterial {
        schema: ROUTE_BINDING_SCHEMA,
        installation: &generation.snapshot.installation,
        installation_generation: generation.snapshot.generation,
        capability_generation: generation.capability.generation,
        capability_descriptor_digest: &generation.capability.descriptor_digest,
        snapshot_digest: &generation.snapshot_digest,
        package_id,
        lifecycle_generation,
        surface,
        owner: materialization.intent.owner.kind_name(),
        application_digest: &application_digest,
        grant_digest: grant_digest.map(|(digest, _)| digest),
        grant_revision: grant_digest.map_or(0, |(_, revision)| revision),
    })?;
    let package_ref = PluginPackageId::parse(package_id.to_owned())
        .map_err(|_| projection_error("A capability package identity is invalid."))?;
    let invocation_ref = InvocationRef::derive(
        &package_ref,
        surface,
        lifecycle_generation,
        &evidence_digest,
    )
    .map_err(|_| projection_error("A capability invocation reference is invalid."))?;
    let endpoint_ref = has_endpoint
        .then(|| {
            EndpointRef::derive(
                &package_ref,
                surface,
                lifecycle_generation,
                &evidence_digest,
            )
            .map_err(|_| projection_error("A capability endpoint reference is invalid."))
        })
        .transpose()?;
    let artifact_ref = artifact_source
        .map(|digest| {
            if !valid_sha256(digest) {
                return Err(projection_error(
                    "A capability materialization digest is invalid.",
                ));
            }
            ArtifactRef::derive(&package_ref, surface, lifecycle_generation, digest)
                .map_err(|_| projection_error("A capability artifact reference is invalid."))
        })
        .transpose()?;
    let resource_ref = ResourceRef::derive(
        &package_ref,
        surface,
        lifecycle_generation,
        &evidence_digest,
    )
    .map_err(|_| projection_error("A capability resource reference is invalid."))?;
    Ok(ControlCapabilityRouteBinding {
        evidence_digest,
        invocation_ref,
        endpoint_ref,
        artifact_ref,
        resource_ref,
        runtime_schema_attestation,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteEvidenceMaterial<'a> {
    schema: &'static str,
    installation: &'a InstallationId,
    installation_generation: u64,
    capability_generation: u64,
    capability_descriptor_digest: &'a str,
    snapshot_digest: &'a str,
    package_id: &'a str,
    lifecycle_generation: u64,
    surface: &'a PluginSurfaceRef,
    owner: &'static str,
    application_digest: &'a str,
    grant_digest: Option<&'a str>,
    grant_revision: u64,
}

fn route_evidence_digest(material: RouteEvidenceMaterial<'_>) -> UseResult<String> {
    if !valid_sha256(material.capability_descriptor_digest)
        || !valid_sha256(material.snapshot_digest)
        || !valid_sha256(material.application_digest)
        || material
            .grant_digest
            .is_some_and(|digest| !valid_sha256(digest))
    {
        return Err(projection_error(
            "A capability route evidence digest is invalid.",
        ));
    }
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    material.serialize(&mut serializer).map_err(|error| {
        projection_error(format!(
            "Failed to encode capability route evidence: {error}"
        ))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(ROUTE_BINDING_DOMAIN);
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn projection_failure(error_code: &str) -> ControlEffectFailure {
    let mut hasher = Sha256::new();
    hasher.update(DESCRIPTOR_FAILURE_DOMAIN);
    hasher.update(error_code.as_bytes());
    ControlEffectFailure {
        evidence_digest: format!("sha256:{:x}", hasher.finalize()),
        error_code: DESCRIPTOR_PROJECTION_ERROR.to_owned(),
    }
}

fn projection_error(message: impl Into<String>) -> UseError {
    UseError::new(DESCRIPTOR_PROJECTION_ERROR, message)
}

fn valid_signer_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
        && !value
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
}
