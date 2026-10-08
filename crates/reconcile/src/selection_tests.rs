#![cfg(test)]

use std::fs;
use std::sync::{Arc, Barrier};

use crate::{
    files_sha256, PackageFile, PackageSpec, Payload, PublicationCursor, ReconcileError,
    ReconcileStore, SelectionSnapshot, SurfaceKind, SurfaceSpec,
};

fn spec(id: &str, body: &str) -> PackageSpec {
    let files = vec![PackageFile {
        path: "echo".into(),
        bytes: body.as_bytes().to_vec(),
    }];
    PackageSpec {
        package_id: id.into(),
        surfaces: vec![SurfaceSpec {
            kind: SurfaceKind::Tool,
            id: "echo".into(),
            sha256: files_sha256(&files).unwrap(),
            entry: Some("echo".into()),
            payload: Payload::Files(files),
        }],
    }
}

fn setup() -> (tempfile::TempDir, ReconcileStore, Vec<PublicationCursor>) {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let packages = ["acme/a", "acme/b"]
        .into_iter()
        .map(|id| store.apply_sync(&spec(id, "old")).unwrap().cursor())
        .collect();
    (root, store, packages)
}

fn publish(store: &ReconcileStore, packages: &[PublicationCursor]) -> SelectionSnapshot {
    store
        .publish_selection_sync("workspace", None, packages)
        .unwrap()
}

#[test]
fn complete_selection_is_canonical_and_retry_is_idempotent() {
    let (_root, store, mut packages) = setup();
    packages.reverse();
    let snapshot = publish(&store, &packages);
    assert_eq!(snapshot.packages()[0].package_id, "acme/a");
    let next = store
        .publish_selection_sync("workspace", Some(&snapshot.cursor()), &packages)
        .unwrap();
    assert_eq!(next.cursor(), snapshot.cursor());
    let lease = store.acquire_selection_sync(&next.cursor()).unwrap();
    assert_eq!(lease.publications().len(), 2);
    assert!(lease
        .publications()
        .all(|p| fs::read(p.surfaces[0].entry.as_ref().unwrap()).unwrap() == b"old"));
    assert_eq!(
        store.selection_sync("workspace").unwrap().unwrap(),
        snapshot
    );
}

#[test]
fn replacing_and_withdrawing_packages_retains_the_whole_accepted_selection() {
    let (_root, store, packages) = setup();
    let snapshot = publish(&store, &packages);
    let lease = store.acquire_selection_sync(&snapshot.cursor()).unwrap();
    let entries: Vec<_> = lease
        .publications()
        .map(|p| p.surfaces[0].entry.clone().unwrap())
        .collect();
    let newer = store.apply_sync(&spec("acme/a", "new")).unwrap();
    store.withdraw_sync("acme/b").unwrap();
    store.collect_retired_sync("acme/a").unwrap();
    assert!(entries.iter().all(|path| fs::read(path).unwrap() == b"old"));
    assert!(matches!(
        store.acquire_selection_sync(&snapshot.cursor()),
        Err(ReconcileError::StalePublication)
    ));
    let next = store
        .publish_selection_sync("workspace", Some(&snapshot.cursor()), &[newer.cursor()])
        .unwrap();
    assert!(next.generation() > snapshot.generation());
    assert!(matches!(
        store.acquire_selection_sync(&snapshot.cursor()),
        Err(ReconcileError::StaleSelection)
    ));
    assert!(lease.release_sync().into_iter().all(|r| r.result.is_ok()));
    assert!(entries.iter().all(|path| !path.exists()));
}

#[test]
fn failed_candidate_leaves_selection_receipt_and_admission_unchanged() {
    let (root, store, packages) = setup();
    let snapshot = publish(&store, &packages[..1]);
    let receipt = root.path().join("selections/workspace/snapshot.json");
    let before = fs::read(&receipt).unwrap();
    let mut candidate = packages;
    candidate[1].revision = format!("sha256:{}", "ab".repeat(32));
    assert!(matches!(
        store.publish_selection_sync("workspace", Some(&snapshot.cursor()), &candidate),
        Err(ReconcileError::StalePublication)
    ));
    assert_eq!(fs::read(&receipt).unwrap(), before);
    let lease = store.acquire_selection_sync(&snapshot.cursor()).unwrap();
    assert_eq!(lease.publications().len(), 1);
}

#[test]
fn tampered_content_is_rejected_even_on_equal_selection_retry() {
    let (_root, store, packages) = setup();
    let snapshot = publish(&store, &packages);
    let entry = store.current_sync("acme/b").unwrap().unwrap().surfaces[0]
        .entry
        .clone()
        .unwrap();
    fs::write(entry, b"tampered").unwrap();
    assert!(matches!(
        store.publish_selection_sync("workspace", Some(&snapshot.cursor()), &packages),
        Err(ReconcileError::DigestMismatch)
    ));
    assert!(matches!(
        store.acquire_selection_sync(&snapshot.cursor()),
        Err(ReconcileError::DigestMismatch)
    ));
    assert_eq!(
        store.selection_sync("workspace").unwrap().unwrap().cursor(),
        snapshot.cursor()
    );
}

#[test]
fn racing_publishers_cannot_both_replace_one_expected_selection() {
    let (_root, store, packages) = setup();
    let initial = publish(&store, &[]);
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = packages
        .into_iter()
        .map(|cursor| {
            let store = store.clone();
            let expected = initial.cursor();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                store.publish_selection_sync("workspace", Some(&expected), &[cursor])
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(ReconcileError::StaleSelection)))
            .count(),
        1
    );
    let snapshot = store.selection_sync("workspace").unwrap().unwrap();
    assert_eq!(snapshot.generation(), initial.generation() + 1);
    assert_eq!(
        store
            .acquire_selection_sync(&snapshot.cursor())
            .unwrap()
            .publications()
            .len(),
        1
    );
}

#[test]
fn withdrawal_preserves_counter_and_other_selection_admission() {
    let (_root, store, packages) = setup();
    let snapshot = publish(&store, &packages);
    let other = store
        .publish_selection_sync("other", None, &packages)
        .unwrap();
    let lease = store.acquire_selection_sync(&snapshot.cursor()).unwrap();
    let hidden = store.withdraw_selection_sync(&snapshot.cursor()).unwrap();
    assert!(!hidden.is_published());
    assert_ne!(hidden.cursor(), snapshot.cursor());
    assert!(matches!(
        store.acquire_selection_sync(&hidden.cursor()),
        Err(ReconcileError::StaleSelection)
    ));
    assert!(matches!(
        store.publish_selection_sync("workspace", None, &packages),
        Err(ReconcileError::StaleSelection)
    ));
    assert!(store.acquire_selection_sync(&other.cursor()).is_ok());
    assert_eq!(lease.publications().len(), 2);
    let next = store
        .publish_selection_sync("workspace", Some(&hidden.cursor()), &packages)
        .unwrap();
    assert!(next.generation() > snapshot.generation());
    assert!(matches!(
        store.acquire_selection_sync(&snapshot.cursor()),
        Err(ReconcileError::StaleSelection)
    ));
}

#[test]
fn missing_and_duplicate_packages_never_publish_a_partial_selection() {
    let (_root, store, packages) = setup();
    assert!(matches!(
        store.publish_selection_sync(
            "workspace",
            None,
            &[packages[0].clone(), packages[0].clone()]
        ),
        Err(ReconcileError::DuplicateSelectionPackage(_))
    ));
    let mut missing = packages[1].clone();
    missing.package_id = "acme/missing".into();
    assert!(matches!(
        store.publish_selection_sync("workspace", None, &[packages[0].clone(), missing]),
        Err(ReconcileError::StalePublication)
    ));
    assert!(store.selection_sync("workspace").unwrap().is_none());
}

#[test]
fn release_reports_a_failure_and_still_retires_remaining_packages() {
    let (_root, store, packages) = setup();
    let snapshot = publish(&store, &packages);
    let lease = store.acquire_selection_sync(&snapshot.cursor()).unwrap();
    let entries: Vec<_> = lease
        .publications()
        .map(|p| p.surfaces[0].entry.clone().unwrap())
        .collect();
    for package in &packages {
        store.withdraw_sync(&package.package_id).unwrap();
    }
    let bad = crate::store::package_dir(&store.root, "acme/b")
        .unwrap()
        .join("snapshot.json");
    fs::write(bad, b"broken receipt").unwrap();
    let results = lease.release_sync();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].package_id, "acme/b");
    assert!(results[0].result.is_err());
    assert_eq!(results[1].package_id, "acme/a");
    assert!(results[1].result.is_ok());
    assert!(!entries[0].exists());
    assert!(entries[1].exists());
}

#[test]
fn invalid_and_oversized_receipts_fail_closed() {
    let (root, store, packages) = setup();
    let snapshot = publish(&store, &packages);
    let receipt = root.path().join("selections/workspace/snapshot.json");
    let mut value = serde_json::to_value(&snapshot).unwrap();
    value["schema"] = "unsupported".into();
    fs::write(&receipt, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(
        store.selection_sync("workspace"),
        Err(ReconcileError::InvalidSelectionSnapshot)
    ));
    fs::write(&receipt, vec![b' '; 65537]).unwrap();
    assert!(matches!(
        store.selection_sync("workspace"),
        Err(ReconcileError::SelectionTooLarge)
    ));
    assert!(matches!(
        store.acquire_selection_sync(&snapshot.cursor()),
        Err(ReconcileError::SelectionTooLarge)
    ));
}

#[test]
fn selection_paths_and_size_limits_are_enforced_before_publication() {
    let (root, store, packages) = setup();
    for id in ["", "../escape", ".", "a/b", "a\\b"] {
        assert!(matches!(
            store.publish_selection_sync(id, None, &[]),
            Err(ReconcileError::InvalidSelectionId(_))
        ));
    }
    assert!(matches!(
        store.publish_selection_sync("oversize", None, &vec![packages[0].clone(); 129]),
        Err(ReconcileError::SelectionTooLarge)
    ));
    let parent = root.path().join("selections");
    fs::create_dir(&parent).unwrap();
    for index in 0..256 {
        fs::create_dir(parent.join(format!("s{index}"))).unwrap();
    }
    assert!(matches!(
        store.publish_selection_sync("overflow", None, &[]),
        Err(ReconcileError::SelectionTooLarge)
    ));
    assert!(!parent.join("overflow").exists());
}

#[cfg(unix)]
#[test]
fn linked_selection_state_is_refused_without_touching_its_target() {
    use std::os::unix::fs::symlink;
    let (root, store, packages) = setup();
    let snapshot = publish(&store, &packages);
    let receipt = root.path().join("selections/workspace/snapshot.json");
    let external = root.path().join("external");
    fs::write(&external, b"untouched").unwrap();
    fs::remove_file(&receipt).unwrap();
    symlink(&external, &receipt).unwrap();
    assert!(matches!(
        store.acquire_selection_sync(&snapshot.cursor()),
        Err(ReconcileError::LinkRefused { .. })
    ));
    assert!(matches!(
        store.publish_selection_sync("workspace", Some(&snapshot.cursor()), &[]),
        Err(ReconcileError::LinkRefused { .. })
    ));
    assert_eq!(fs::read(external).unwrap(), b"untouched");
}

#[cfg(unix)]
#[test]
fn reapply_prepares_non_executable_tool_as_a_new_generation_without_mutating_a_lease() {
    use std::os::unix::fs::PermissionsExt;
    let (_root, store, packages) = setup();
    let lease = store.acquire_sync(&packages[0]).unwrap();
    let old_entry = lease.publication().surfaces[0].entry.clone().unwrap();
    fs::set_permissions(&old_entry, fs::Permissions::from_mode(0o600)).unwrap();
    let next = store.apply_sync(&spec("acme/a", "old")).unwrap();
    assert!(next.generation > lease.publication().generation);
    let entry = next.surfaces[0].entry.as_ref().unwrap();
    assert_eq!(
        fs::metadata(entry).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    assert_eq!(
        fs::metadata(&old_entry).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    assert_eq!(fs::read(old_entry).unwrap(), b"old");
}

#[tokio::test]
async fn async_api_retains_and_releases_the_same_native_resources() {
    let (_root, store, packages) = setup();
    let snapshot = store
        .publish_selection("workspace", None, &packages)
        .await
        .unwrap();
    let lease = store.acquire_selection(&snapshot.cursor()).await.unwrap();
    let entries: Vec<_> = lease
        .publications()
        .map(|p| p.surfaces[0].entry.clone().unwrap())
        .collect();
    store.withdraw_selection(&snapshot.cursor()).await.unwrap();
    for package in &packages {
        store.withdraw(&package.package_id).await.unwrap();
    }
    assert!(entries.iter().all(|p| p.exists()));
    assert!(lease
        .release()
        .await
        .unwrap()
        .into_iter()
        .all(|r| r.result.is_ok()));
    assert!(entries.iter().all(|p| !p.exists()));
    assert!(!store
        .selection("workspace")
        .await
        .unwrap()
        .unwrap()
        .is_published());
}
