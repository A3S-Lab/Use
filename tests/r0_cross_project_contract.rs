mod r0_cross_project_support;

use std::path::Path;

use a3s_use_core::{PluginPackageLock, PluginReleaseChannel};
use a3s_use_extension::{ExtensionReceipt, ExtensionRegistrySnapshot};
use olpc_cjson::CanonicalFormatter;
use r0_cross_project_support::{
    canonical_digest, read_fixture, verify_fixture_package, Contract, ContractError,
    KnowledgeBinding,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const FIXTURE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/crates/core/fixtures/agentic-ontology/r0-cross-project-v1"
);

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HandoffFixture {
    schema: String,
    project: String,
    scope: ScopeFixture,
    package: PackageFixture,
    version: String,
    route: String,
    requires_use: String,
    dependencies: Vec<serde_json::Value>,
    requested_channel: PluginReleaseChannel,
    revision: RevisionFixture,
    draft_digest: String,
    approval_digest: String,
    revision_activation_receipt_digest: String,
    blueprint_digest: String,
    surface_graph_digest: String,
    compiler_package_digest: String,
    use_package_digest: String,
    manifest_digest: String,
    file_count: u64,
    expanded_bytes: u64,
    handoff_digest: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScopeFixture {
    organization_id: String,
    workspace_id: String,
    authority_generation: u64,
    scope_id: String,
    revision: u64,
    scope_digest: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PackageFixture {
    publisher: String,
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevisionFixture {
    schema: String,
    scope: ScopeFixture,
    generation: u64,
    revision_digest: String,
    reference_digest: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HandoffIdentity<'a> {
    schema: &'a str,
    project: &'a str,
    scope: &'a ScopeFixture,
    package: &'a PackageFixture,
    version: &'a str,
    route: &'a str,
    requires_use: &'a str,
    dependencies: &'a [serde_json::Value],
    requested_channel: PluginReleaseChannel,
    revision: &'a RevisionFixture,
    draft_digest: &'a str,
    approval_digest: &'a str,
    revision_activation_receipt_digest: &'a str,
    blueprint_digest: &'a str,
    surface_graph_digest: &'a str,
    compiler_package_digest: &'a str,
    use_package_digest: &'a str,
    manifest_digest: &'a str,
    file_count: u64,
    expanded_bytes: u64,
}

impl HandoffFixture {
    fn digest(&self) -> String {
        canonical_digest(
            "agentic.ontology.a3s-use-handoff.v1",
            &HandoffIdentity {
                schema: &self.schema,
                project: &self.project,
                scope: &self.scope,
                package: &self.package,
                version: &self.version,
                route: &self.route,
                requires_use: &self.requires_use,
                dependencies: &self.dependencies,
                requested_channel: self.requested_channel,
                revision: &self.revision,
                draft_digest: &self.draft_digest,
                approval_digest: &self.approval_digest,
                revision_activation_receipt_digest: &self.revision_activation_receipt_digest,
                blueprint_digest: &self.blueprint_digest,
                surface_graph_digest: &self.surface_graph_digest,
                compiler_package_digest: &self.compiler_package_digest,
                use_package_digest: &self.use_package_digest,
                manifest_digest: &self.manifest_digest,
                file_count: self.file_count,
                expanded_bytes: self.expanded_bytes,
            },
        )
    }

    fn package_id(&self) -> String {
        format!("{}/{}", self.package.publisher, self.package.name)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GenerationReceiptFixture {
    schema: String,
    issuer: String,
    host_target: String,
    use_version: String,
    handoff_digest: String,
    package: serde_json::Value,
    version: String,
    revision: serde_json::Value,
    draft_digest: String,
    channel: PluginReleaseChannel,
    lifecycle_generation: u64,
    package_lock_digest: String,
    registry_generation: u64,
    registry_snapshot_digest: String,
    closure: Vec<LockedGenerationFixture>,
    generation_digest: String,
    receipt_digest: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LockedGenerationFixture {
    package_id: String,
    version: String,
    lifecycle_generation: u64,
    package_digest: String,
    manifest_digest: String,
    catalog_digest: String,
    extension_receipt_digest: String,
    registry_name: String,
    trust_root_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilitySnapshotFixture {
    schema: String,
    package_id: String,
    package_version: String,
    lifecycle_generation: u64,
    generation_digest: String,
    surfaces: Vec<CapabilitySurfaceFixture>,
    snapshot_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilitySurfaceFixture {
    kind: String,
    id: String,
    format_version: String,
    content_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CitationFixture {
    schema: String,
    package_id: String,
    package_version: String,
    lifecycle_generation: u64,
    generation_digest: String,
    surface_id: String,
    content_digest: String,
    document_path: String,
    heading: String,
    evidence_ids: Vec<String>,
    citation_digest: String,
}

#[test]
fn use_accepts_the_pinned_package_and_rejects_all_contract_drift() {
    let root = Path::new(FIXTURE_ROOT);
    verify_fixture_package(root).unwrap();
    Contract::parse(&read_fixture(root, "fixtures/r0-cross-project.valid.json")).unwrap();

    for (path, expected) in [
        (
            "fixtures/r0-cross-project.unknown-field.json",
            ContractError::Invalid,
        ),
        (
            "fixtures/r0-cross-project.drift-handoff-digest.json",
            ContractError::BindingMismatch,
        ),
        (
            "fixtures/r0-cross-project.drift-generation.json",
            ContractError::BindingMismatch,
        ),
        (
            "fixtures/r0-cross-project.drift-session.json",
            ContractError::DigestMismatch,
        ),
        (
            "fixtures/r0-cross-project.drift-cloud-task-proof.json",
            ContractError::BindingMismatch,
        ),
    ] {
        assert_eq!(
            Contract::parse(&read_fixture(root, path)).unwrap_err(),
            expected,
            "{path}"
        );
    }
}

#[test]
fn use_recomputes_candidate_lock_registry_and_generation_evidence() {
    let root = Path::new(FIXTURE_ROOT);
    let contract =
        Contract::parse(&read_fixture(root, "fixtures/r0-cross-project.valid.json")).unwrap();
    let handoff: HandoffFixture = read_json(root, "fixtures/a3s-use-handoff.valid.json");
    assert_eq!(handoff.schema, "agentic.ontology.a3s-use-handoff.v1");
    assert_eq!(handoff.digest(), handoff.handoff_digest);
    assert_eq!(handoff.handoff_digest, contract.candidate.handoff_digest);
    assert_eq!(handoff.package_id(), contract.candidate.package_id);
    assert_eq!(handoff.version, contract.candidate.package_version);
    assert_eq!(
        handoff.use_package_digest,
        contract.candidate.package_digest
    );
    assert_eq!(handoff.manifest_digest, contract.candidate.manifest_digest);
    let drift: HandoffFixture = read_json(root, "fixtures/a3s-use-handoff.drift.json");
    assert_ne!(drift.digest(), drift.handoff_digest);

    let package_lock: PluginPackageLock = read_json(root, "fixtures/plugin-package-lock.json");
    package_lock.validate().unwrap();
    let package_lock_digest = package_lock.descriptor_digest().unwrap();
    assert_eq!(package_lock_digest, contract.generation.package_lock_digest);
    assert_eq!(package_lock.root_package_id, contract.candidate.package_id);
    let snapshot: ExtensionRegistrySnapshot = read_json(root, "fixtures/registry-snapshot.json");
    let snapshot_digest = use_descriptor_digest(&snapshot);
    assert_eq!(
        snapshot_digest,
        contract.generation.registry_snapshot_digest
    );
    assert_eq!(snapshot.generation, contract.generation.registry_generation);
    let extension_receipt: ExtensionReceipt = read_json(root, "fixtures/extension-receipt.json");
    let extension_receipt_digest = extension_receipt.descriptor_digest().unwrap();
    let generation: GenerationReceiptFixture =
        read_json(root, "fixtures/a3s-use-generation-receipt.json");

    assert_eq!(generation.schema, contract.generation.receipt_schema);
    assert_eq!(generation.issuer, contract.generation.issuer);
    assert_eq!(generation.host_target, package_lock.host.target);
    assert_eq!(generation.use_version, package_lock.host.use_version);
    assert_eq!(generation.handoff_digest, handoff.handoff_digest);
    assert_eq!(generation.version, contract.candidate.package_version);
    assert_eq!(generation.draft_digest, handoff.draft_digest);
    assert_eq!(generation.channel, handoff.requested_channel);
    assert_eq!(
        generation.lifecycle_generation,
        contract.generation.lifecycle_generation
    );
    assert_eq!(generation.package_lock_digest, package_lock_digest);
    assert_eq!(generation.registry_generation, snapshot.generation);
    assert_eq!(generation.registry_snapshot_digest, snapshot_digest);
    assert_eq!(generation.closure.len(), 1);
    let locked = &generation.closure[0];
    assert_eq!(locked.package_id, contract.candidate.package_id);
    assert_eq!(locked.version, contract.candidate.package_version);
    assert_eq!(locked.lifecycle_generation, generation.lifecycle_generation);
    assert_eq!(locked.package_digest, contract.candidate.package_digest);
    assert_eq!(locked.manifest_digest, contract.candidate.manifest_digest);
    assert_eq!(locked.extension_receipt_digest, extension_receipt_digest);
    assert_eq!(
        locked.catalog_digest,
        package_lock.packages[0]
            .catalog
            .descriptor_digest()
            .unwrap()
    );

    let generation_digest = canonical_digest(
        "agentic.ontology.a3s-use-generation.v1",
        &(
            &generation.issuer,
            &generation.host_target,
            &generation.use_version,
            &generation.handoff_digest,
            &generation.package_lock_digest,
            generation.channel,
            &generation.closure,
        ),
    );
    assert_eq!(generation.generation_digest, generation_digest);
    assert_eq!(
        generation.generation_digest,
        contract.generation.generation_digest
    );
    assert_eq!(
        generation.receipt_digest,
        canonical_digest(
            "agentic.ontology.a3s-use-generation-receipt.v1",
            &(
                &generation_digest,
                generation.registry_generation,
                &generation.registry_snapshot_digest,
            ),
        )
    );
    assert_eq!(
        generation.receipt_digest,
        contract.generation.receipt_digest
    );
}

#[test]
fn use_projects_only_exact_generation_knowledge_and_citations() {
    let root = Path::new(FIXTURE_ROOT);
    let contract =
        Contract::parse(&read_fixture(root, "fixtures/r0-cross-project.valid.json")).unwrap();
    let knowledge: KnowledgeBinding = read_json(root, "fixtures/knowledge-lease-binding.json");
    assert_eq!(knowledge, contract.knowledge);
    let capability: CapabilitySnapshotFixture =
        read_json(root, "fixtures/capability-snapshot.json");
    assert_eq!(capability.schema, "a3s.use.capability-snapshot.v1");
    assert_eq!(capability.package_id, contract.candidate.package_id);
    assert_eq!(
        capability.package_version,
        contract.candidate.package_version
    );
    assert_eq!(
        capability.lifecycle_generation,
        contract.generation.lifecycle_generation
    );
    assert_eq!(
        capability.generation_digest,
        contract.generation.generation_digest
    );
    assert_eq!(capability.surfaces.len(), 1);
    let surface = &capability.surfaces[0];
    assert_eq!(surface.kind, "okf");
    assert_eq!(surface.id, knowledge.surface_id);
    assert_eq!(surface.format_version, knowledge.format_version);
    assert_eq!(surface.content_digest, knowledge.content_digest);
    assert_eq!(
        capability.snapshot_digest,
        canonical_digest(
            "a3s.use.capability-snapshot.v1",
            &(
                capability.package_id.as_str(),
                capability.package_version.as_str(),
                capability.lifecycle_generation,
                &capability.generation_digest,
                surface.id.as_str(),
                surface.format_version.as_str(),
                surface.content_digest.as_str(),
            ),
        )
    );
    assert_eq!(
        capability.snapshot_digest,
        contract.code.capability_snapshot_digest
    );

    let citation: CitationFixture = read_json(root, "fixtures/knowledge-citation.json");
    assert_eq!(citation.schema, knowledge.citation_schema);
    assert_eq!(citation.package_id, contract.candidate.package_id);
    assert_eq!(citation.package_version, contract.candidate.package_version);
    assert_eq!(
        citation.lifecycle_generation,
        knowledge.lifecycle_generation
    );
    assert_eq!(citation.generation_digest, knowledge.generation_digest);
    assert_eq!(citation.surface_id, knowledge.surface_id);
    assert_eq!(citation.content_digest, knowledge.content_digest);
    assert!(!citation.document_path.starts_with('/'));
    assert!(!citation.document_path.contains(".."));
    assert!(!citation.heading.trim().is_empty());
    assert!(!citation.evidence_ids.is_empty());
    assert_eq!(
        citation.citation_digest,
        canonical_digest(
            "a3s.use.okf-knowledge-citation.v1",
            &(
                citation.package_id.as_str(),
                citation.package_version.as_str(),
                citation.lifecycle_generation,
                &citation.generation_digest,
                citation.surface_id.as_str(),
                citation.content_digest.as_str(),
                citation.document_path.as_str(),
                citation.heading.as_str(),
                &citation.evidence_ids,
            ),
        )
    );
}

fn read_json<T: for<'de> Deserialize<'de>>(root: &Path, path: &str) -> T {
    serde_json::from_slice(&read_fixture(root, path)).unwrap()
}

fn use_descriptor_digest<T: Serialize>(value: &T) -> String {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).unwrap();
    format!("sha256:{:x}", Sha256::digest(bytes))
}
