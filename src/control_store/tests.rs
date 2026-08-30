use std::sync::Arc;

use a3s_use_core::{InstallationId, InstallationKind};

use super::*;

fn installation(kind: InstallationKind) -> InstallationId {
    InstallationId::new(kind, "shared/current").unwrap()
}

fn assert_send_sync<T: Send + Sync>() {}

#[tokio::test]
async fn clean_initialization_is_idempotent_installation_bound_and_durable() {
    assert_send_sync::<ControlStore>();
    assert_eq!(MAX_QUEUED_CONTROL_STORE_OPERATIONS, 16);

    let temporary = tempfile::tempdir().unwrap();
    let extension_paths = crate::test_extension_paths(temporary.path());
    let path_bound_store = ControlStore::from_extension_paths(&extension_paths).unwrap();
    assert_eq!(path_bound_store.installation, crate::test_installation());
    assert_eq!(
        path_bound_store.state_root,
        extension_paths.installation_state_root()
    );

    let state_root = temporary.path().join("state");
    let expected = installation(InstallationKind::Workspace);
    let store = ControlStore::new(&state_root, expected.clone()).unwrap();

    let initialized = store.initialize().await.unwrap();
    assert_eq!(initialized.installation, expected);
    assert_eq!(initialized.schema_version, CONTROL_STORE_SCHEMA_VERSION);
    assert_eq!(initialized.current_generation, 0);

    let replay = store.initialize().await.unwrap();
    assert_eq!(replay, initialized);
    let inspection = store.inspect().await.unwrap();
    assert_eq!(inspection.metadata, initialized);
    assert_eq!(inspection.journal_mode, "wal");
    assert!(inspection.foreign_keys_enabled);
    assert_eq!(inspection.synchronous, SQLITE_SYNCHRONOUS_FULL);
    assert!(store.database_path().is_file());

    let other = ControlStore::new(&state_root, installation(InstallationKind::User)).unwrap();
    let error = other.initialize().await.unwrap_err();
    assert_eq!(error.code, "use.control_store.identity_mismatch");
}

#[tokio::test]
async fn initialization_rejects_legacy_authority_before_database_creation() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("state");
    tokio::fs::create_dir_all(&state_root).await.unwrap();
    tokio::fs::write(state_root.join("installation-snapshot.json"), b"legacy")
        .await
        .unwrap();
    let store = ControlStore::new(&state_root, installation(InstallationKind::Workspace)).unwrap();

    let error = store.initialize().await.unwrap_err();
    assert_eq!(error.code, "use.control_store.legacy_state_unsupported");
    assert!(!store.database_path().exists());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn initialization_rejects_a_linked_database_without_following_it() {
    let temporary = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("state");
    tokio::fs::create_dir_all(&state_root).await.unwrap();
    tokio::fs::write(external.path().join("sentinel"), b"outside")
        .await
        .unwrap();
    crate::test_filesystem::create_directory_link(
        external.path(),
        &state_root.join(CONTROL_STORE_DATABASE_FILE),
    );
    let store = ControlStore::new(&state_root, installation(InstallationKind::Workspace)).unwrap();

    let error = store.initialize().await.unwrap_err();
    assert_eq!(error.code, "use.control_store.path_invalid");
    assert_eq!(
        tokio::fs::read(external.path().join("sentinel"))
            .await
            .unwrap(),
        b"outside"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn initialization_resolves_an_ancestor_alias_before_nofollow_open() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let physical_parent = tempfile::tempdir().unwrap();
    let alias = temporary.path().join("state-alias");
    symlink(physical_parent.path(), &alias).unwrap();
    let state_root = alias.join("installation");
    let store = ControlStore::new(&state_root, installation(InstallationKind::Workspace)).unwrap();

    store.initialize().await.unwrap();

    assert!(physical_parent
        .path()
        .join("installation")
        .join(CONTROL_STORE_DATABASE_FILE)
        .is_file());
}

#[tokio::test]
async fn deterministic_export_is_canonical_scope_bound_and_offline_verifiable() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("state");
    let expected = installation(InstallationKind::Workspace);
    let store = ControlStore::new(&state_root, expected.clone()).unwrap();
    store.initialize().await.unwrap();

    let first = store.export().await.unwrap();
    let second = store.export().await.unwrap();
    assert_eq!(first, second);
    assert!(!first.ends_with(b"\n"));

    tokio::fs::remove_file(store.database_path()).await.unwrap();
    let verified = store.verify_export(first.clone()).await.unwrap();
    assert_eq!(verified.export.installation, expected);
    assert_eq!(verified.export.current_generation, 0);
    assert_eq!(verified.export.published_capability_generation, 0);
    assert!(verified.export.authority.generations.is_empty());
    assert!(verified.export.authority.operations.is_empty());
    assert!(verified.export.authority.effects.is_empty());
    assert!(verified.descriptor_digest.starts_with("sha256:"));

    let mut noncanonical = first.clone();
    noncanonical.push(b'\n');
    let error = store.verify_export(noncanonical).await.unwrap_err();
    assert_eq!(error.code, "use.control_store.export_invalid");

    let error = store
        .verify_export(vec![b'x'; 1024 * 1024 + 1])
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.control_store.export_invalid");

    let other = ControlStore::new(
        temporary.path().join("other"),
        installation(InstallationKind::User),
    )
    .unwrap();
    let error = other.verify_export(first).await.unwrap_err();
    assert_eq!(error.code, "use.control_store.identity_mismatch");
}

#[tokio::test]
async fn inspection_fails_closed_on_schema_and_database_corruption() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("state");
    let expected = installation(InstallationKind::Workspace);
    let store = ControlStore::new(&state_root, expected.clone()).unwrap();
    store.initialize().await.unwrap();

    {
        let connection = rusqlite::Connection::open(store.database_path()).unwrap();
        connection.pragma_update(None, "user_version", 999).unwrap();
    }
    let error = store.inspect().await.unwrap_err();
    assert_eq!(error.code, "use.control_store.schema_unsupported");

    let corrupt_store =
        ControlStore::new(temporary.path().join("corrupt-state"), expected).unwrap();
    corrupt_store.initialize().await.unwrap();
    tokio::fs::write(corrupt_store.database_path(), b"not a sqlite database")
        .await
        .unwrap();
    let error = corrupt_store.inspect().await.unwrap_err();
    assert_eq!(error.code, "use.control_store.corrupt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_executor_keeps_concurrent_async_callers_progressing() {
    let temporary = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ControlStore::new(
            temporary.path().join("state"),
            installation(InstallationKind::Workspace),
        )
        .unwrap(),
    );
    store.initialize().await.unwrap();
    let export = Arc::new(store.export().await.unwrap());

    let mut callers = Vec::new();
    for _ in 0..64 {
        let store = store.clone();
        let export = export.clone();
        callers.push(tokio::spawn(async move {
            store.verify_export(export.as_ref().clone()).await.unwrap()
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        for caller in callers {
            caller.await.unwrap();
        }
    })
    .await
    .unwrap();
}
