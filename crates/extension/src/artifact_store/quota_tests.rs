use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use sha2::{Digest, Sha256};
use tokio::fs;

use super::*;

const CHILD_ROOT_ENV: &str = "A3S_USE_TEST_STORAGE_QUOTA_ROOT";
const CHILD_SEED_ENV: &str = "A3S_USE_TEST_STORAGE_QUOTA_SEED";
const CHILD_QUOTA_EXIT: i32 = 42;
const CROSS_PROCESS_BODY_BYTES: usize = 1024 * 1024;

fn assert_send_sync<T: Send + Sync>() {}

#[tokio::test]
async fn quota_policy_is_canonical_revision_guarded_global_state() {
    assert_send_sync::<ArtifactStorageQuotaPolicy>();
    assert_send_sync::<ArtifactStorageQuotaSnapshot>();
    assert_send_sync::<ArtifactStorageQuotaMutation>();

    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let initial = store.storage_quota().await.unwrap();
    assert_eq!(
        initial.schema_version,
        ARTIFACT_STORAGE_QUOTA_POLICY_SCHEMA_VERSION
    );
    assert!(initial.policy.is_none());
    assert_eq!(initial.revision.len(), 64);

    let policy = ArtifactStorageQuotaPolicy::new(4096, 8).unwrap();
    let conflict = store
        .set_storage_quota(&"0".repeat(64), policy)
        .await
        .unwrap_err();
    assert_eq!(conflict.code, "use.artifact_store.quota_revision_conflict");
    assert!(initial.policy.is_none());

    let mutation = store
        .set_storage_quota(&initial.revision, policy)
        .await
        .unwrap();
    assert!(mutation.changed);
    assert_eq!(mutation.action, ArtifactStorageQuotaAction::Set);
    assert_eq!(mutation.previous_revision, initial.revision);
    assert_eq!(mutation.snapshot.policy, Some(policy));
    let encoded = fs::read_to_string(store.root().join("storage-quota.acl"))
        .await
        .unwrap();
    assert!(encoded.ends_with('\n'));
    assert!(encoded.contains("artifact_storage_quota"));
    assert!(encoded.contains("max_physical_bytes = \"4096\""));

    let current = store.storage_quota().await.unwrap();
    assert_eq!(current, mutation.snapshot);
    let replay = store
        .set_storage_quota(&current.revision, policy)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.snapshot, current);

    let cleared = store.clear_storage_quota(&current.revision).await.unwrap();
    assert!(cleared.changed);
    assert_eq!(cleared.action, ArtifactStorageQuotaAction::Clear);
    assert!(cleared.snapshot.policy.is_none());
    assert!(!store.root().join("storage-quota.acl").exists());

    let staging = store.root().join(".storage-quota.tmp");
    fs::write(&staging, []).await.unwrap();
    let clear_replay = store
        .clear_storage_quota(&cleared.snapshot.revision)
        .await
        .unwrap();
    assert!(!clear_replay.changed);
    assert!(!staging.exists());
}

#[tokio::test]
async fn blob_publication_enforces_quota_and_allows_non_increasing_cleanup() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    configure(&store, ArtifactStorageQuotaPolicy::new(4, 1).unwrap()).await;

    let first_body = b"one!";
    let first_sha256 = sha256(first_body);
    commit(&store, temporary.path(), "first", first_body)
        .await
        .unwrap();

    let error = commit(&store, temporary.path(), "second", b"two?")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_store.quota_exceeded");
    assert_eq!(error.details["expectedContentBytes"], "4");
    assert_eq!(error.details["replacedStagingBytes"], "0");
    assert_eq!(error.details["currentPhysicalBytes"], "4");
    assert_eq!(error.details["projectedPhysicalBytes"], "8");
    assert_eq!(error.details["projectedPhysicalArtifacts"], "2");

    let container = store
        .blob_path(&format!("sha256:{first_sha256}"))
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    fs::write(container.join(".artifact-staging-stale.tmp"), b"old")
        .await
        .unwrap();

    commit(&store, temporary.path(), "first-replay", first_body)
        .await
        .unwrap();
    assert!(!container.join(".artifact-staging-stale.tmp").exists());

    let collection = store.acquire_collection().await.unwrap();
    let inventory = store.inspect_inventory(&collection).await.unwrap();
    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(
        inventory.entries[0].digest,
        format!("sha256:{first_sha256}")
    );
    assert_eq!(inventory.entries[0].content_bytes, 4);
    assert_eq!(inventory.entries[0].staging_bytes, 0);
}

#[tokio::test]
async fn malformed_quota_policy_fails_writes_without_hiding_physical_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let _ = store.storage_quota().await.unwrap();
    fs::write(
        store.root().join("storage-quota.acl"),
        b"artifact_storage_quota { schema_version = 1 }\n",
    )
    .await
    .unwrap();

    let error = commit(&store, temporary.path(), "blocked", b"body")
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_store.quota_config_invalid");

    let collection = store.acquire_collection().await.unwrap();
    let inventory = store.inspect_inventory(&collection).await.unwrap();
    assert!(inventory.entries.is_empty());
}

#[tokio::test]
async fn quota_serializes_distinct_publishers_across_processes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("data");
    let store = ArtifactStore::from_data_root(&root);
    configure(
        &store,
        ArtifactStorageQuotaPolicy::new(CROSS_PROCESS_BODY_BYTES as u64, 1).unwrap(),
    )
    .await;

    let first = quota_child(&root, "41");
    let second = quota_child(&root, "42");
    let (first, second) = tokio::join!(first, second);
    let mut codes = [
        child_code(first.expect("run first quota child")),
        child_code(second.expect("run second quota child")),
    ];
    codes.sort_unstable();
    assert_eq!(codes, [0, CHILD_QUOTA_EXIT]);

    let collection = store.acquire_collection().await.unwrap();
    let inventory = store.inspect_inventory(&collection).await.unwrap();
    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(
        inventory.entries[0].content_bytes,
        CROSS_PROCESS_BODY_BYTES as u64
    );
}

#[tokio::test]
#[ignore = "subprocess helper for cross-process Artifact Store quota admission"]
async fn artifact_storage_quota_commit_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let seed = std::env::var(CHILD_SEED_ENV)
        .expect("quota child seed is missing")
        .parse::<u8>()
        .expect("quota child seed is invalid");
    let source_root = root.parent().unwrap().join(format!("source-{seed}"));
    fs::create_dir_all(&source_root).await.unwrap();
    let body = vec![seed; CROSS_PROCESS_BODY_BYTES];
    let store = ArtifactStore::from_data_root(&root);
    match commit(&store, &source_root, "child", &body).await {
        Ok(_) => {}
        Err(error) if error.code == "use.artifact_store.quota_exceeded" => {
            std::process::exit(CHILD_QUOTA_EXIT)
        }
        Err(error) => panic!("quota child failed unexpectedly: {error:?}"),
    }
}

async fn configure(store: &ArtifactStore, policy: ArtifactStorageQuotaPolicy) {
    let snapshot = store.storage_quota().await.unwrap();
    store
        .set_storage_quota(&snapshot.revision, policy)
        .await
        .unwrap();
}

async fn commit(
    store: &ArtifactStore,
    source_root: &Path,
    name: &str,
    body: &[u8],
) -> UseResult<()> {
    fs::create_dir_all(source_root)
        .await
        .map_err(test_io_error)?;
    let source_path = source_root.join(format!("{name}.part"));
    fs::write(&source_path, body).await.map_err(test_io_error)?;
    let mut source = fs::File::open(&source_path).await.map_err(test_io_error)?;
    let digest = sha256(body);
    let admission = store.acquire_reference_admission().await?;
    store
        .commit_blob(&admission, &mut source, body.len() as u64, &digest)
        .await?;
    Ok(())
}

fn sha256(body: &[u8]) -> String {
    format!("{:x}", Sha256::digest(body))
}

async fn quota_child(root: &Path, seed: &str) -> std::io::Result<std::process::Output> {
    tokio::process::Command::new(std::env::current_exe()?)
        .arg("artifact_storage_quota_commit_child")
        .arg("--ignored")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(CHILD_ROOT_ENV, root)
        .env(CHILD_SEED_ENV, seed)
        .output()
        .await
}

fn child_code(output: std::process::Output) -> i32 {
    output.status.code().unwrap_or_else(|| {
        panic!(
            "quota child was terminated: status={:?}, stdout={}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn test_io_error(error: std::io::Error) -> UseError {
    UseError::new("use.artifact_store.test_io", error.to_string())
}
