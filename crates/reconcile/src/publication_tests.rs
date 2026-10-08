#![cfg(test)]

use std::fs;

use serde_json::{json, Value};

use crate::{
    files_sha256, PackageFile, PackageSpec, Payload, ReconcileError, ReconcileStore, SurfaceKind,
    SurfaceSpec,
};

fn published() -> (tempfile::TempDir, ReconcileStore, crate::Publication) {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let files = vec![PackageFile {
        path: "bin/echo".into(),
        bytes: b"echo ready".to_vec(),
    }];
    let publication = store
        .apply_sync(&PackageSpec {
            package_id: "acme/echo".into(),
            surfaces: vec![SurfaceSpec {
                kind: SurfaceKind::Tool,
                id: "echo".into(),
                sha256: files_sha256(&files).unwrap(),
                entry: Some("bin/echo".into()),
                payload: Payload::Files(files),
            }],
        })
        .unwrap();
    (root, store, publication)
}

fn receipt(root: &tempfile::TempDir) -> std::path::PathBuf {
    root.path().join("packages/acme%2Fecho/snapshot.json")
}

// An edited receipt must not manufacture an identity, duplicate or escaping entry.
#[test]
fn current_rejects_invalid_publication_identity_and_entries() {
    for (field, value, code) in [
        (
            "id",
            json!("../../outside"),
            "use.reconcile.invalid_surface_id",
        ),
        (
            "kind",
            json!("extension"),
            "use.reconcile.invalid_surface_kind",
        ),
        ("sha256", json!("invalid"), "use.reconcile.digest_mismatch"),
        (
            "entry",
            json!("../../outside"),
            "use.reconcile.invalid_path",
        ),
        ("entry", Value::Null, "use.reconcile.entry_required"),
    ] {
        let (root, store, _) = published();
        let path = receipt(&root);
        let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record["surfaces"][0][field] = value;
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(
            store.current_sync("acme/echo").unwrap_err().code(),
            code,
            "{field}"
        );
    }
    let (root, store, _) = published();
    let path = receipt(&root);
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let duplicate = record["surfaces"][0].clone();
    record["surfaces"].as_array_mut().unwrap().push(duplicate);
    fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(matches!(
        store.current_sync("acme/echo"),
        Err(ReconcileError::DuplicateSurface { .. })
    ));
}

#[test]
fn current_does_not_publish_a_missing_entry() {
    let (_root, store, publication) = published();
    fs::remove_file(publication.surfaces[0].entry.as_ref().unwrap()).unwrap();
    assert!(store.current_sync("acme/echo").is_err());
}

// Content edits and injected files must not pass the original files digest.
#[test]
fn file_payload_verification_rejects_changed_and_additional_bytes() {
    let (_root, _store, publication) = published();
    let surface = &publication.surfaces[0];
    surface.verify_files_payload().unwrap();
    fs::write(surface.entry.as_ref().unwrap(), b"changed").unwrap();
    assert!(matches!(
        surface.verify_files_payload(),
        Err(ReconcileError::DigestMismatch)
    ));
    fs::write(surface.entry.as_ref().unwrap(), b"echo ready").unwrap();
    fs::write(surface.directory.join(".injected"), b"hidden").unwrap();
    assert!(matches!(
        surface.verify_files_payload(),
        Err(ReconcileError::DigestMismatch)
    ));
}

#[test]
fn file_payload_verification_refuses_forged_mcp_projection() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let files = vec![PackageFile {
        path: "server.json".into(),
        bytes: br#"{"type":"stdio","command":"safe"}"#.to_vec(),
    }];
    let mut publication = store
        .apply_sync(&PackageSpec {
            package_id: "acme/mcp".into(),
            surfaces: vec![SurfaceSpec {
                kind: SurfaceKind::Mcp,
                id: "server".into(),
                sha256: files_sha256(&files).unwrap(),
                entry: None,
                payload: Payload::Files(files),
            }],
        })
        .unwrap();
    publication.surfaces[0].verify_files_payload().unwrap();
    publication.surfaces[0].mcp = Some(json!({ "type": "stdio", "command": "unadmitted" }));
    assert!(matches!(
        publication.surfaces[0].verify_files_payload(),
        Err(ReconcileError::McpInvalid)
    ));
}

#[test]
fn file_payload_verification_bounds_injected_tree_entries() {
    let (_root, _store, publication) = published();
    let surface = &publication.surfaces[0];
    for index in 0..512 {
        fs::write(surface.directory.join(format!("extra-{index}")), b"").unwrap();
    }
    assert!(matches!(
        surface.verify_files_payload(),
        Err(ReconcileError::PayloadTooLarge)
    ));
}

#[test]
fn file_payload_verification_bounds_injected_file_bytes() {
    let (_root, _store, publication) = published();
    let surface = &publication.surfaces[0];
    let file = fs::File::create(surface.directory.join("oversized")).unwrap();
    file.set_len(crate::unpack::MAX_UNPACKED + 1).unwrap();
    assert!(matches!(
        surface.verify_files_payload(),
        Err(ReconcileError::PayloadTooLarge)
    ));
}

#[cfg(unix)]
#[test]
fn current_refuses_linked_receipts_generations_and_entries() {
    for location in [
        "packages",
        "package",
        "receipt",
        "generations",
        "generation",
        "kind",
        "surface",
        "entry-parent",
        "entry",
    ] {
        let (root, store, publication) = published();
        let surface = &publication.surfaces[0];
        let package = receipt(&root).parent().unwrap().to_path_buf();
        let path = match location {
            "packages" => root.path().join("packages"),
            "package" => package.clone(),
            "receipt" => receipt(&root),
            "generations" => package.join("g"),
            "generation" => package.join("g/1"),
            "kind" => package.join("g/1/tool"),
            "surface" => surface.directory.clone(),
            "entry-parent" => surface.directory.join("bin"),
            "entry" => surface.entry.clone().unwrap(),
            _ => unreachable!(),
        };
        let target = root.path().join("original");
        fs::rename(&path, &target).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(
            matches!(
                store.current_sync("acme/echo"),
                Err(ReconcileError::LinkRefused { .. })
            ),
            "{location}"
        );
    }
}

#[cfg(unix)]
#[test]
fn file_payload_verification_refuses_linked_non_entry_files() {
    let (_root, _store, publication) = published();
    let surface = &publication.surfaces[0];
    std::os::unix::fs::symlink(
        surface.entry.as_ref().unwrap(),
        surface.directory.join("extra"),
    )
    .unwrap();
    assert!(matches!(
        surface.verify_files_payload(),
        Err(ReconcileError::LinkRefused { .. })
    ));
}
