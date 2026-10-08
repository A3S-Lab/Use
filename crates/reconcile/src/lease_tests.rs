#![cfg(test)]

use std::fs;
use std::io::{Cursor, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use zip::{write::SimpleFileOptions, ZipWriter};

use crate::{
    files_sha256, payload_sha256, PackageFile, PackageSpec, Payload, Publication, ReconcileError,
    ReconcileStore, SurfaceKind, SurfaceSpec,
};

fn spec(body: &str) -> PackageSpec {
    let files = vec![PackageFile {
        path: "bin/echo".into(),
        bytes: body.as_bytes().to_vec(),
    }];
    PackageSpec {
        package_id: "acme/echo".into(),
        surfaces: vec![SurfaceSpec {
            kind: SurfaceKind::Tool,
            id: "echo".into(),
            sha256: files_sha256(&files).unwrap(),
            entry: Some("bin/echo".into()),
            payload: Payload::Files(files),
        }],
    }
}

fn published() -> (tempfile::TempDir, ReconcileStore, Publication) {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let publication = store.apply_sync(&spec("old bytes")).unwrap();
    (root, store, publication)
}

#[test]
fn old_and_new_generations_coexist_until_their_owners_release() {
    let (_root, store, old) = published();
    let old_lease = store.acquire_sync(&old.cursor()).unwrap();
    let new = store.apply_sync(&spec("new bytes")).unwrap();
    let new_lease = store.acquire_sync(&new.cursor()).unwrap();
    let old_entry = old.surfaces[0].entry.as_ref().unwrap();
    let new_entry = new.surfaces[0].entry.as_ref().unwrap();
    let report = store.collect_retired_sync("acme/echo").unwrap();
    assert_eq!(report.retained_generations, vec![old.generation]);
    assert_eq!(fs::read(old_entry).unwrap(), b"old bytes");
    assert_eq!(fs::read(new_entry).unwrap(), b"new bytes");
    assert!(matches!(
        store.acquire_sync(&old.cursor()),
        Err(ReconcileError::StalePublication)
    ));
    let report = old_lease.release_sync().unwrap();
    assert_eq!(report.removed_generations, vec![old.generation]);
    assert!(!old_entry.exists());
    assert!(new_entry.is_file());
    new_lease.release_sync().unwrap();
    assert!(
        new_entry.is_file(),
        "the current publication remains admitted"
    );
}

#[test]
fn withdraw_stops_admission_before_all_accepted_owners_drain() {
    let (_root, store, publication) = published();
    let one = store.acquire_sync(&publication.cursor()).unwrap();
    let two = store.acquire_sync(&publication.cursor()).unwrap();
    let entry = publication.surfaces[0].entry.as_ref().unwrap();
    store.withdraw_sync("acme/echo").unwrap();
    assert!(store.current_sync("acme/echo").unwrap().is_none());
    assert!(store.acquire_current_sync("acme/echo").unwrap().is_none());
    assert!(matches!(
        store.acquire_sync(&publication.cursor()),
        Err(ReconcileError::StalePublication)
    ));
    assert_eq!(fs::read(entry).unwrap(), b"old bytes");
    assert_eq!(
        one.release_sync().unwrap().retained_generations,
        vec![publication.generation]
    );
    assert!(entry.is_file());
    assert_eq!(
        two.release_sync().unwrap().removed_generations,
        vec![publication.generation]
    );
    assert!(!entry.exists());
    store.withdraw_sync("acme/echo").unwrap();
}

#[test]
fn reinstall_cannot_reuse_a_withdrawn_generation_identity() {
    let (_root, store, publication) = published();
    store.withdraw_sync("acme/echo").unwrap();
    let next = store.apply_sync(&spec("old bytes")).unwrap();
    assert!(next.generation > publication.generation);
    assert!(matches!(
        store.acquire_sync(&publication.cursor()),
        Err(ReconcileError::StalePublication)
    ));
    let lease = store.acquire_sync(&next.cursor()).unwrap();
    assert_eq!(lease.publication().cursor(), next.cursor());
}

#[test]
fn equal_generation_with_a_different_revision_is_not_admitted() {
    let (_root, store, publication) = published();
    let mut forged = publication.cursor();
    forged.revision = format!("sha256:{}", "ab".repeat(32));
    assert!(matches!(
        store.acquire_sync(&forged),
        Err(ReconcileError::StalePublication)
    ));
}

#[test]
fn acquisition_verifies_content_before_retaining_a_generation() {
    let (_root, store, publication) = published();
    fs::write(publication.surfaces[0].entry.as_ref().unwrap(), b"modified").unwrap();
    assert!(matches!(
        store.acquire_sync(&publication.cursor()),
        Err(ReconcileError::DigestMismatch)
    ));
    assert!(matches!(
        store.acquire_current_sync("acme/echo"),
        Err(ReconcileError::DigestMismatch)
    ));
}

#[test]
fn zip_publications_have_a_separate_verified_unpacked_digest() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("SKILL.md", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"# zip skill").unwrap();
    let payload = Payload::Zip(zip.finish().unwrap().into_inner());
    let publication = store
        .apply_sync(&PackageSpec {
            package_id: "acme/skill".into(),
            surfaces: vec![SurfaceSpec {
                kind: SurfaceKind::Skill,
                id: "skill".into(),
                sha256: payload_sha256(&payload).unwrap(),
                entry: None,
                payload,
            }],
        })
        .unwrap();
    assert_ne!(
        publication.surfaces[0].sha256,
        publication.surfaces[0].content_sha256
    );
    let lease = store.acquire_sync(&publication.cursor()).unwrap();
    let entry = lease.publication().surfaces[0].directory.join("SKILL.md");
    assert_eq!(fs::read(&entry).unwrap(), b"# zip skill");
    fs::write(entry, b"changed skill").unwrap();
    assert!(matches!(
        store.acquire_sync(&publication.cursor()),
        Err(ReconcileError::DigestMismatch)
    ));
}

#[test]
fn dropping_the_last_lease_reclaims_hidden_data() {
    let (_root, store, publication) = published();
    let lease = store.acquire_sync(&publication.cursor()).unwrap();
    store.withdraw_sync("acme/echo").unwrap();
    let entry = publication.surfaces[0].entry.as_ref().unwrap();
    assert!(entry.is_file());
    drop(lease);
    assert!(!entry.exists());
}

#[test]
fn dropping_a_lease_does_not_wait_for_a_live_writer_and_cleanup_replays() {
    let (root, store, publication) = published();
    let lease = store.acquire_sync(&publication.cursor()).unwrap();
    store.withdraw_sync("acme/echo").unwrap();
    let writer = crate::store::AcquireLock::acquire(root.path()).unwrap();
    let start = Instant::now();
    drop(lease);
    assert!(start.elapsed() < Duration::from_secs(2));
    let entry = publication.surfaces[0].entry.as_ref().unwrap();
    assert!(entry.is_file());
    drop(writer);
    assert_eq!(
        store
            .collect_retired_sync("acme/echo")
            .unwrap()
            .removed_generations,
        vec![publication.generation]
    );
    assert!(!entry.exists());
}

#[test]
fn abandoned_candidates_are_not_overwritten_and_can_be_reclaimed() {
    let (root, store, publication) = published();
    let candidate = root.path().join("packages/acme%2Fecho/g/2");
    fs::create_dir_all(&candidate).unwrap();
    fs::write(candidate.join("partial"), b"abandoned").unwrap();
    let next = store.apply_sync(&spec("replacement")).unwrap();
    assert!(next.generation > 2);
    assert_eq!(fs::read(candidate.join("partial")).unwrap(), b"abandoned");
    let report = store.collect_retired_sync("acme/echo").unwrap();
    assert_eq!(report.removed_generations, vec![publication.generation, 2]);
    assert!(next.surfaces[0].entry.as_ref().unwrap().is_file());
}

#[test]
fn orphaned_guard_cleanup_does_not_remove_the_current_generation() {
    let (root, store, publication) = published();
    let guards = root.path().join("packages/acme%2Fecho/leases");
    fs::create_dir_all(&guards).unwrap();
    fs::write(guards.join("99.lock"), b"").unwrap();
    store.collect_retired_sync("acme/echo").unwrap();
    assert!(!guards.join("99.lock").exists());
    assert!(publication.surfaces[0].entry.as_ref().unwrap().is_file());
}

#[cfg(unix)]
#[test]
fn linked_writer_and_generation_guards_cannot_be_used_for_retention() {
    for location in ["writer", "leases", "guard"] {
        let (root, store, publication) = published();
        let guards = root.path().join("packages/acme%2Fecho/leases");
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let outside_file = outside.join("keep");
        fs::write(&outside_file, b"unchanged").unwrap();
        let (path, target) = match location {
            "writer" => {
                let path = root.path().join(".reconcile.lock");
                fs::remove_file(&path).unwrap();
                (path, outside_file.clone())
            }
            "leases" => (guards, outside.clone()),
            "guard" => {
                fs::create_dir(&guards).unwrap();
                (guards.join("1.lock"), outside_file.clone())
            }
            _ => unreachable!(),
        };
        std::os::unix::fs::symlink(target, path).unwrap();
        assert!(
            matches!(
                store.acquire_sync(&publication.cursor()),
                Err(ReconcileError::LinkRefused { .. })
            ),
            "{location}"
        );
        assert_eq!(fs::read(outside_file).unwrap(), b"unchanged");
    }
}

#[test]
fn old_receipts_are_refused_and_generation_overflow_does_not_publish() {
    let (root, store, _) = published();
    let path = root.path().join("packages/acme%2Fecho/snapshot.json");
    let original = fs::read(&path).unwrap();
    let mut record: Value = serde_json::from_slice(&original).unwrap();
    record["schema"] = Value::String("a3s.use.package-reconcile.v1".into());
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(store.acquire_current_sync("acme/echo").is_err());
    assert!(store.apply_sync(&spec("new")).is_err());
    record["schema"] = Value::String(crate::SCHEMA.into());
    record["generation"] = Value::from(u64::MAX);
    let overflow = serde_json::to_vec(&record).unwrap();
    fs::write(&path, &overflow).unwrap();
    assert!(matches!(
        store.apply_sync(&spec("new")),
        Err(ReconcileError::GenerationExhausted)
    ));
    assert_eq!(fs::read(path).unwrap(), overflow);
}

#[cfg(unix)]
#[test]
fn linked_pending_receipts_cannot_modify_external_bytes_or_replace_admission() {
    let (root, store, publication) = published();
    let outside = root.path().join("outside");
    fs::write(&outside, b"unchanged").unwrap();
    let pending = root.path().join("packages/acme%2Fecho/snapshot.json.new");
    std::os::unix::fs::symlink(&outside, &pending).unwrap();
    assert!(matches!(
        store.apply_sync(&spec("new bytes")),
        Err(ReconcileError::LinkRefused { .. })
    ));
    assert_eq!(fs::read(outside).unwrap(), b"unchanged");
    assert_eq!(
        store.current_sync("acme/echo").unwrap().unwrap().cursor(),
        publication.cursor()
    );
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// The parent test invokes this ignored entry in a separate, killable process.
#[test]
#[ignore = "subprocess entry exercised by cross_process_lease_survives_cutover_and_is_recovered_after_exit"]
fn hold_lease_in_child_process() {
    let root = std::env::var_os("A3S_USE_TEST_LEASE_ROOT").unwrap();
    let store = ReconcileStore::open(std::path::PathBuf::from(root)).unwrap();
    let _lease = store.acquire_current_sync("acme/echo").unwrap().unwrap();
    fs::write(store.root.join("child-leased"), b"ready").unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
}

#[test]
fn cross_process_lease_survives_cutover_and_is_recovered_after_exit() {
    let (root, store, publication) = published();
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("lease_tests::hold_lease_in_child_process")
            .arg("--ignored")
            .env("A3S_USE_TEST_LEASE_ROOT", root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    while !root.path().join("child-leased").is_file() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "lease child exited before acquisition"
        );
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "lease child did not acquire"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let next = store.apply_sync(&spec("new bytes")).unwrap();
    store.withdraw_sync("acme/echo").unwrap();
    let report = store.collect_retired_sync("acme/echo").unwrap();
    assert_eq!(report.retained_generations, vec![publication.generation]);
    let entry = publication.surfaces[0].entry.as_ref().unwrap();
    assert_eq!(fs::read(entry).unwrap(), b"old bytes");
    assert!(!next.surfaces[0].entry.as_ref().unwrap().exists());
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let reopened = ReconcileStore::open(root.path()).unwrap();
    assert_eq!(
        reopened
            .collect_retired_sync("acme/echo")
            .unwrap()
            .removed_generations,
        vec![publication.generation]
    );
    assert!(!entry.exists());
    assert!(reopened.current_sync("acme/echo").unwrap().is_none());
}
