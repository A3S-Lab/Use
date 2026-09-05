use std::io;
use std::path::{Path, PathBuf};

use crate::control_store::aggregate_tests::fixtures::{
    catalog_binding, claim, control_installation, initialized_store, observation, operation,
    transition,
};
use crate::control_store::effect_port::{
    ControlCapabilityCutoverRequest, ControlEffectRequestIdentity,
};
use crate::control_store::model::{
    ControlEffectAuthority, ControlEffectOutcome, ControlEffectSubject,
    ControlPublishedCapabilityPackage,
};

use super::index::ControlCapabilityIndexStore;
use super::lease::ControlGenerationLeaseStore;
use super::model::ControlCapabilityIndexDocument;

struct CandidateIndexFixture {
    _temporary: tempfile::TempDir,
    state_root: PathBuf,
    document: ControlCapabilityIndexDocument,
}

#[tokio::test]
async fn immutable_index_replay_retires_exact_crash_staging_without_rewriting_the_target() {
    let fixture = candidate_index_fixture().await;
    let store = ControlCapabilityIndexStore::new(&fixture.state_root);
    let receipt = fixture.document.receipt_digest().unwrap();
    let staging = staging_path(&fixture.state_root, &receipt);
    tokio::fs::create_dir_all(staging.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&staging, b"incomplete first attempt")
        .await
        .unwrap();

    assert_eq!(store.materialize(&fixture.document).await.unwrap(), receipt);
    let target = document_path(&fixture.state_root, &receipt);
    let first = tokio::fs::read(&target).await.unwrap();

    tokio::fs::write(&staging, b"unlink lost after publication")
        .await
        .unwrap();
    assert_eq!(store.materialize(&fixture.document).await.unwrap(), receipt);
    assert_eq!(tokio::fs::read(&target).await.unwrap(), first);
    assert!(matches!(
        tokio::fs::symlink_metadata(&staging).await,
        Err(error) if error.kind() == io::ErrorKind::NotFound
    ));
    assert_eq!(store.read(&receipt).await.unwrap(), fixture.document);
}

#[tokio::test]
async fn immutable_index_rejects_content_substitution_at_an_existing_receipt() {
    let fixture = candidate_index_fixture().await;
    let store = ControlCapabilityIndexStore::new(&fixture.state_root);
    let receipt = store.materialize(&fixture.document).await.unwrap();
    tokio::fs::write(document_path(&fixture.state_root, &receipt), b"{}")
        .await
        .unwrap();

    let error = store.materialize(&fixture.document).await.unwrap_err();

    assert_eq!(error.code, "use.control.capability_index_conflict");
}

#[tokio::test]
async fn index_and_generation_leases_reject_linked_directory_substitution() {
    let index_fixture = candidate_index_fixture().await;
    let outside_index = index_fixture._temporary.path().join("outside-index");
    std::fs::create_dir(&outside_index).unwrap();
    let index_link = index_fixture.state_root.join("capability-index");
    create_directory_link(&outside_index, &index_link);
    let index_error = ControlCapabilityIndexStore::new(&index_fixture.state_root)
        .materialize(&index_fixture.document)
        .await
        .unwrap_err();
    remove_directory_link(&index_link);
    assert_eq!(
        index_error.code,
        "use.control.capability_index_path_invalid"
    );

    let lease_temporary = tempfile::tempdir().unwrap();
    let lease_root = lease_temporary.path().join("state");
    let publisher_root = lease_root.join("generation-leases");
    std::fs::create_dir_all(&publisher_root).unwrap();
    let outside_lease = lease_temporary.path().join("outside-lease");
    std::fs::create_dir(&outside_lease).unwrap();
    let publisher_link = publisher_root.join("acme");
    create_directory_link(&outside_lease, &publisher_link);
    let package = ControlPublishedCapabilityPackage {
        package_id: "acme/knowledge".to_string(),
        lifecycle_generation: 1,
        package_digest: crate::control_store::aggregate_tests::fixtures::digest('1'),
        manifest_digest: crate::control_store::aggregate_tests::fixtures::digest('2'),
    };
    let Err(lease_error) = ControlGenerationLeaseStore::new(&lease_root)
        .try_acquire_shared(&[package])
        .await
    else {
        panic!("a linked publisher directory must reject lease acquisition");
    };
    remove_directory_link(&publisher_link);
    assert_eq!(
        lease_error.code,
        "use.control.invocation_lease_path_invalid"
    );
}

async fn candidate_index_fixture() -> CandidateIndexFixture {
    let (temporary, store) = initialized_store().await;
    let reviewed = operation("operation:capability-index:fixture");
    store.register_operation(reviewed.clone()).await.unwrap();
    store
        .commit_transition(transition(control_installation(), &reviewed))
        .await
        .unwrap();
    for sequence in 0..2_u32 {
        let now_ms = 30 + u64::from(sequence) * 20;
        let token = format!("claim:capability-index:prepare:{sequence}");
        let claimed = store
            .claim_next_effect(claim(
                reviewed.operation_id(),
                &token,
                now_ms,
                now_ms + 10,
                false,
            ))
            .await
            .unwrap()
            .unwrap();
        store
            .record_effect_observation(observation(
                reviewed.operation_id(),
                &claimed.intent,
                &claimed.claim_token,
                ControlEffectOutcome::Applied,
                char::from_digit(sequence, 16).unwrap(),
                now_ms + 5,
            ))
            .await
            .unwrap();
    }
    let claimed = store
        .claim_next_effect(claim(
            reviewed.operation_id(),
            "claim:capability-index:cutover",
            70,
            80,
            false,
        ))
        .await
        .unwrap()
        .unwrap();
    let ControlEffectAuthority::CapabilityIndex(authority) = claimed.authority else {
        panic!("the final install effect must carry Capability Index authority");
    };
    let ControlEffectSubject::Installation {
        expected_capability_generation,
        capability_generation,
        descriptor_digest,
    } = &claimed.intent.subject
    else {
        panic!("the final install effect must have an installation subject");
    };
    let request = ControlCapabilityCutoverRequest {
        identity: ControlEffectRequestIdentity {
            operation_id: reviewed.operation_id().to_string(),
            installation: claimed.intent.installation.clone(),
            plan_digest: claimed.intent.plan_digest.clone(),
            operation_action: claimed.intent.operation_action,
            installation_generation: claimed.intent.installation_generation,
            sequence: claimed.intent.sequence,
            idempotency_key: claimed.intent.idempotency_key.clone(),
            required: claimed.intent.required,
            attempt: claimed.attempt,
            deadline_at_ms: claimed.lease_until_ms,
        },
        authority,
        expected_capability_generation: *expected_capability_generation,
        capability_generation: *capability_generation,
        descriptor_digest: descriptor_digest.clone(),
    };
    let catalog = catalog_binding(
        &request.identity.installation,
        request.capability_generation,
    );
    CandidateIndexFixture {
        state_root: store.state_root.clone(),
        document: ControlCapabilityIndexDocument::from_request(&request, catalog).unwrap(),
        _temporary: temporary,
    }
}

fn staging_path(state_root: &Path, receipt: &str) -> PathBuf {
    state_root
        .join("capability-index")
        .join(".staging")
        .join(format!("{}.tmp", receipt.strip_prefix("sha256:").unwrap()))
}

fn document_path(state_root: &Path, receipt: &str) -> PathBuf {
    let digest = receipt.strip_prefix("sha256:").unwrap();
    state_root
        .join("capability-index")
        .join("sha256")
        .join(&digest[..2])
        .join(format!("{digest}.json"))
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mklink /J failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn remove_directory_link(link: &Path) {
    std::fs::remove_file(link).unwrap();
}

#[cfg(windows)]
fn remove_directory_link(link: &Path) {
    std::fs::remove_dir(link).unwrap();
}
