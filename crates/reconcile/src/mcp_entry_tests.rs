#![cfg(test)]

use crate::*;

fn spec(command: &str, entry: Option<&str>) -> PackageSpec {
    let files = vec![
        PackageFile {
            path: "server.json".into(),
            bytes: serde_json::to_vec(&serde_json::json!({"type":"stdio","command":command}))
                .unwrap(),
        },
        PackageFile {
            path: "server".into(),
            bytes: b"#!/bin/sh\nexit 0\n".to_vec(),
        },
    ];
    PackageSpec {
        package_id: "acme/mcp".into(),
        surfaces: vec![SurfaceSpec {
            kind: SurfaceKind::Mcp,
            id: "catalog".into(),
            sha256: files_sha256(&files).unwrap(),
            entry: entry.map(str::to_owned),
            payload: Payload::Files(files),
        }],
    }
}

#[test]
fn only_the_declared_in_package_stdio_command_gets_a_managed_entry() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let owned = store.apply_sync(&spec("server", None)).unwrap();
    assert!(owned.surfaces[0]
        .entry
        .as_ref()
        .unwrap()
        .ends_with("mcp/catalog/server"));
    let external = store.apply_sync(&spec("node", None)).unwrap();
    assert!(external.surfaces[0].entry.is_none());
    assert!(store.apply_sync(&spec("node", Some("server"))).is_err());
    assert_eq!(
        store.current_sync("acme/mcp").unwrap().unwrap().cursor(),
        external.cursor()
    );
}

#[cfg(unix)]
#[test]
fn lost_mcp_execution_permission_repairs_with_a_new_retained_generation() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path()).unwrap();
    let package = spec("server", None);
    let old = store.apply_sync(&package).unwrap();
    let lease = store.acquire_current_sync("acme/mcp").unwrap().unwrap();
    let entry = old.surfaces[0].entry.as_ref().unwrap();
    assert_eq!(
        std::fs::metadata(entry).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::set_permissions(entry, std::fs::Permissions::from_mode(0o600)).unwrap();
    let next = store.apply_sync(&package).unwrap();
    assert!(next.generation > old.generation);
    assert_eq!(
        std::fs::metadata(next.surfaces[0].entry.as_ref().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(entry).unwrap().permissions().mode() & 0o777,
        0o600
    );
    lease.release_sync().unwrap();
    assert!(!entry.exists());
}
