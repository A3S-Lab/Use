use a3s_use_core::{
    inspect_okf_bundle_files, OkfBundleContract, OkfBundleFile, OkfBundleLimits, OkfFormatVersion,
    OkfKnowledgeObservation, OkfKnowledgeObservedState, OkfProjectionReceipt,
    OkfSelectedGeneration, PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef,
    OKF_BUNDLE_CONTRACT_SCHEMA, OKF_KNOWLEDGE_OBSERVATION_SCHEMA, OKF_PROJECTION_RECEIPT_SCHEMA,
};

use super::{OkfKnowledgeBinding, OkfKnowledgeStageSpec};

pub(super) const PACKAGE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(super) const MANIFEST_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

pub(super) fn surface() -> PlanQualifiedSurfaceRef {
    PlanQualifiedSurfaceRef {
        package_id: "acme/research".to_owned(),
        surface: PluginSurfaceRef {
            kind: PluginSurfaceKind::Okf,
            id: "domain-knowledge".to_owned(),
        },
    }
}

pub(super) fn files() -> Vec<OkfBundleFile> {
    vec![OkfBundleFile::new(
        "concept.md",
        b"---\ntype: Metric\n---\n\n# Throughput\n",
    )]
}

pub(super) fn bundle() -> OkfBundleContract {
    let limits = OkfBundleLimits::default();
    let files = files();
    let inspection =
        inspect_okf_bundle_files(OkfFormatVersion::V0_2, limits.clone(), &files).unwrap();
    OkfBundleContract {
        schema: OKF_BUNDLE_CONTRACT_SCHEMA.to_owned(),
        format_version: inspection.format_version,
        root: "knowledge".to_owned(),
        content_digest: inspection.content_digest,
        concept_count: inspection.concept_count,
        file_count: inspection.file_count,
        expanded_bytes: inspection.expanded_bytes,
        limits,
    }
}

pub(super) fn stage_spec(generation: u64) -> OkfKnowledgeStageSpec {
    OkfKnowledgeStageSpec {
        operation_id: format!("operation-{generation}"),
        scope_id: "workspace-01".to_owned(),
        surface: surface(),
        generation,
        package_digest: PACKAGE_DIGEST.to_owned(),
        manifest_digest: MANIFEST_DIGEST.to_owned(),
        bundle: bundle(),
    }
}

pub(super) fn receipt(generation: u64) -> OkfProjectionReceipt {
    OkfProjectionReceipt {
        schema: OKF_PROJECTION_RECEIPT_SCHEMA.to_owned(),
        operation_id: format!("operation-{generation}"),
        scope_id: "workspace-01".to_owned(),
        surface: surface(),
        generation,
        package_digest: PACKAGE_DIGEST.to_owned(),
        manifest_digest: MANIFEST_DIGEST.to_owned(),
        bundle: bundle(),
        projection_id: format!("projection-{generation}"),
        index_schema: "okf-v1".to_owned(),
        index_build_id: format!("build-{generation}"),
        staged_at_ms: generation * 1_000,
    }
}

pub(super) fn index_digest(generation: u64) -> String {
    format!("sha256:{generation:064x}")
}

pub(super) fn selected(receipt: &OkfProjectionReceipt) -> OkfSelectedGeneration {
    OkfSelectedGeneration {
        generation: receipt.generation,
        package_digest: receipt.package_digest.clone(),
        bundle_digest: receipt.bundle.content_digest.clone(),
        projection_receipt_digest: receipt.descriptor_digest().unwrap(),
        index_schema: receipt.index_schema.clone(),
        index_build_id: receipt.index_build_id.clone(),
        index_digest: index_digest(receipt.generation),
    }
}

pub(super) fn observation(
    receipt: &OkfProjectionReceipt,
    state: OkfKnowledgeObservedState,
    selected_generation: Option<&OkfProjectionReceipt>,
    observed_at_ms: u64,
) -> OkfKnowledgeObservation {
    let selected = selected_generation.map(selected);
    let index_digest = match state {
        OkfKnowledgeObservedState::Removed => None,
        OkfKnowledgeObservedState::Failed if selected_generation.is_some() => {
            Some(index_digest(receipt.generation))
        }
        OkfKnowledgeObservedState::Failed => None,
        OkfKnowledgeObservedState::Promoted | OkfKnowledgeObservedState::Staged => {
            Some(index_digest(receipt.generation))
        }
    };
    OkfKnowledgeObservation {
        schema: OKF_KNOWLEDGE_OBSERVATION_SCHEMA.to_owned(),
        scope_id: receipt.scope_id.clone(),
        surface: receipt.surface.clone(),
        generation: receipt.generation,
        package_digest: receipt.package_digest.clone(),
        bundle_digest: receipt.bundle.content_digest.clone(),
        projection_receipt_digest: receipt.descriptor_digest().unwrap(),
        index_schema: receipt.index_schema.clone(),
        index_build_id: receipt.index_build_id.clone(),
        state,
        observed_at_ms,
        index_digest,
        selected,
    }
}

pub(super) fn binding(
    receipt: &OkfProjectionReceipt,
    state: OkfKnowledgeObservedState,
    selected_generation: Option<&OkfProjectionReceipt>,
    observed_at_ms: u64,
) -> OkfKnowledgeBinding {
    OkfKnowledgeBinding::new(
        receipt.clone(),
        observation(receipt, state, selected_generation, observed_at_ms),
    )
    .unwrap()
}
