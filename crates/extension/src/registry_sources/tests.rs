use super::*;
use crate::VerifiedTargetObservationStatus;
use sha2::{Digest, Sha256};

fn store(root: &std::path::Path) -> RegistrySourceStore {
    RegistrySourceStore::new(UsePaths::new(root.join("data"), root.join("state")))
}

fn input(name: &str, url: &str, root: &str) -> RegistrySourceInput {
    RegistrySourceInput::new(name, url, root, None, VerifiedTargetCachePolicy::default())
}

#[tokio::test]
async fn source_mutations_are_revision_bound_and_preserve_isolated_datastores() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let empty = store.snapshot().await.unwrap();
    assert!(empty.sources.is_empty());
    assert_eq!(empty.revision.len(), 64);

    let first = store
        .add(input(
            "primary",
            "https://registry.example/a3s",
            &"a".repeat(64),
        ))
        .await
        .unwrap();
    assert!(first.changed);
    assert_eq!(first.snapshot.default_registry.as_deref(), Some("primary"));
    assert_eq!(
        first.snapshot.sources[0].registry_url,
        "https://registry.example/a3s/"
    );
    assert_ne!(first.previous_revision, first.snapshot.revision);

    let repeated = store
        .add(input(
            "primary",
            "https://registry.example/a3s/",
            &"a".repeat(64),
        ))
        .await
        .unwrap();
    assert!(!repeated.changed);
    assert_eq!(repeated.snapshot.revision, first.snapshot.revision);

    let second = store
        .add(input(
            "mirror",
            "https://mirror.example/a3s/",
            &"b".repeat(64),
        ))
        .await
        .unwrap();
    assert_eq!(second.snapshot.default_registry.as_deref(), Some("primary"));
    let stale = store
        .set_default("mirror", &first.snapshot.revision)
        .await
        .unwrap_err();
    assert_eq!(
        stale.code,
        "use.extension.registry_sources_revision_mismatch"
    );

    let selected = store
        .set_default("mirror", &second.snapshot.revision)
        .await
        .unwrap();
    let resolved = store.resolve(None).await.unwrap();
    assert_eq!(resolved.root().name(), "mirror");
    assert_eq!(
        resolved.root().source_identity(),
        selected
            .snapshot
            .sources
            .iter()
            .find(|source| source.name == "mirror")
            .unwrap()
            .source_identity
    );
    assert_eq!(resolved.dependencies().len(), 1);
    assert_eq!(resolved.dependencies()[0].name(), "primary");
    assert_eq!(resolved.source_revision(), selected.snapshot.revision);
    assert_ne!(
        resolved.root().datastore(),
        resolved.dependencies()[0].datastore()
    );

    let conflict = store
        .remove("mirror", &selected.snapshot.revision)
        .await
        .unwrap_err();
    assert_eq!(
        conflict.code,
        "use.extension.registry_source_default_conflict"
    );
    let primary = store
        .set_default("primary", &selected.snapshot.revision)
        .await
        .unwrap();
    let disabled = store
        .disable("mirror", &primary.snapshot.revision)
        .await
        .unwrap();
    assert!(!disabled.snapshot.sources[0].enabled);
    let resolved = store.resolve(None).await.unwrap();
    assert!(resolved.dependencies().is_empty());
    let enabled = store
        .enable("mirror", &disabled.snapshot.revision)
        .await
        .unwrap();
    assert_eq!(
        enabled.snapshot.default_registry.as_deref(),
        Some("primary")
    );
    assert_eq!(store.resolve(None).await.unwrap().dependencies().len(), 1);
    let removed = store
        .remove("mirror", &enabled.snapshot.revision)
        .await
        .unwrap();
    assert_eq!(removed.snapshot.sources.len(), 1);
    assert_eq!(removed.snapshot.sources[0].name, "primary");
}

#[tokio::test]
async fn host_network_policy_is_applied_to_every_resolved_source() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path()).with_network_policy(RegistryNetworkPolicy::PublicInternet);
    store
        .add(input(
            "primary",
            "https://registry.example/",
            &"a".repeat(64),
        ))
        .await
        .unwrap();
    store
        .add(input(
            "dependency",
            "https://dependency.example/",
            &"b".repeat(64),
        ))
        .await
        .unwrap();

    let resolved = store.resolve(Some("primary")).await.unwrap();

    assert_eq!(
        resolved.root().network_policy(),
        RegistryNetworkPolicy::PublicInternet
    );
    assert!(resolved
        .dependencies()
        .iter()
        .all(|registry| { registry.network_policy() == RegistryNetworkPolicy::PublicInternet }));
}

#[tokio::test]
async fn disabling_the_only_source_removes_selection_without_deleting_trust_state() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let added = store
        .add(input(
            "packages",
            "https://registry.example/",
            &"a".repeat(64),
        ))
        .await
        .unwrap();
    let disabled = store
        .disable("packages", &added.snapshot.revision)
        .await
        .unwrap();
    assert!(disabled.changed);
    assert!(disabled.snapshot.default_registry.is_none());
    assert!(!disabled.snapshot.sources[0].enabled);
    assert_eq!(
        store.resolve(None).await.unwrap_err().code,
        "use.extension.registry_source_default_missing"
    );
    assert_eq!(
        store.resolve(Some("packages")).await.unwrap_err().code,
        "use.extension.registry_source_disabled"
    );
    let enabled = store
        .enable("packages", &disabled.snapshot.revision)
        .await
        .unwrap();
    assert_eq!(
        enabled.snapshot.default_registry.as_deref(),
        Some("packages")
    );
    assert_eq!(store.resolve(None).await.unwrap().root().name(), "packages");
}

#[tokio::test]
async fn removing_the_default_is_allowed_when_every_other_source_is_disabled() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    store
        .add(input(
            "primary",
            "https://registry.example/",
            &"a".repeat(64),
        ))
        .await
        .unwrap();
    let mirror = store
        .add(input("mirror", "https://mirror.example/", &"b".repeat(64)))
        .await
        .unwrap();
    let selected = store
        .set_default("mirror", &mirror.snapshot.revision)
        .await
        .unwrap();
    let disabled = store
        .disable("primary", &selected.snapshot.revision)
        .await
        .unwrap();

    let removed = store
        .remove("mirror", &disabled.snapshot.revision)
        .await
        .unwrap();

    assert!(removed.snapshot.default_registry.is_none());
    assert_eq!(removed.snapshot.sources.len(), 1);
    assert_eq!(removed.snapshot.sources[0].name, "primary");
    assert!(!removed.snapshot.sources[0].enabled);
}

#[tokio::test]
async fn source_replacement_changes_identity_without_rewriting_an_old_datastore() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let added = store
        .add(input("packages", "https://one.example/", &"1".repeat(64)))
        .await
        .unwrap();
    let first = store.resolve(None).await.unwrap();
    std::fs::create_dir_all(first.root().datastore()).unwrap();
    std::fs::write(first.root().datastore().join("evidence"), b"old").unwrap();
    let archive_digest = "3".repeat(64);
    let cache = first.root().datastore().join("verified-targets/sha256");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join(&archive_digest), b"old!").unwrap();
    let retained_provenance = a3s_use_core::VerifiedCatalogProvenance {
        registry_name: "packages".to_owned(),
        registry_url: "https://one.example/".to_owned(),
        root_sha256: format!("sha256:{}", "1".repeat(64)),
        root_version: 1,
        timestamp_version: 2,
        snapshot_version: 3,
        targets_version: 4,
        catalog_record_digest: format!("sha256:{}", "4".repeat(64)),
    };

    let replaced = store
        .replace(
            &added.snapshot.revision,
            input("packages", "https://two.example/", &"2".repeat(64)),
        )
        .await
        .unwrap();
    assert!(replaced.changed);
    let next = store.resolve(None).await.unwrap();
    assert_ne!(first.root().datastore(), next.root().datastore());
    assert_eq!(
        std::fs::read(first.root().datastore().join("evidence")).unwrap(),
        b"old"
    );
    assert!(!next.root().datastore().exists());
    let observation = store
        .observe_retained_target(&retained_provenance, 4, &archive_digest)
        .await
        .unwrap();
    assert_eq!(observation.registry_name, "packages");
    assert_eq!(observation.retained_bytes, 4);
    assert_eq!(
        observation.status,
        VerifiedTargetObservationStatus::Complete
    );
}

#[tokio::test]
async fn imported_trusted_roots_are_digest_bound_and_revalidated() {
    let temporary = tempfile::tempdir().unwrap();
    let root_path = temporary.path().join("root.json");
    std::fs::write(&root_path, b"{\"signed\":{}}").unwrap();
    let digest = format!("{:x}", Sha256::digest(std::fs::read(&root_path).unwrap()));
    let store = store(temporary.path());
    let input = RegistrySourceInput::new(
        "packages",
        "https://registry.example/",
        &digest,
        Some(root_path),
        VerifiedTargetCachePolicy::default(),
    );
    let added = store.add(input).await.unwrap();
    assert!(added.snapshot.sources[0].imported_trusted_root);
    store.resolve(None).await.unwrap();

    let managed = temporary
        .path()
        .join("state/registry-trust-roots/sha256")
        .join(format!("{digest}.json"));
    std::fs::write(&managed, b"tampered").unwrap();
    let error = store.resolve(None).await.unwrap_err();
    assert_eq!(error.code, "use.extension.registry_root_mismatch");
}

#[tokio::test]
async fn noncanonical_or_unknown_acl_fails_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let state = temporary.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        state.join("registries.acl"),
        "registries {\n  schema_version = 1\n  mystery = true\n}\n",
    )
    .unwrap();
    let error = store(temporary.path()).snapshot().await.unwrap_err();
    assert_eq!(error.code, "use.extension.registry_sources_invalid");
    assert!(error.message.contains("unknown attribute"));
}

#[tokio::test]
async fn replacing_or_removing_unknown_sources_fails_without_writing() {
    let temporary = tempfile::tempdir().unwrap();
    let store = store(temporary.path());
    let snapshot = store.snapshot().await.unwrap();
    let replace = store
        .replace(
            &snapshot.revision,
            input("missing", "https://registry.example/", &"a".repeat(64)),
        )
        .await
        .unwrap_err();
    assert_eq!(replace.code, "use.extension.registry_source_not_found");
    let remove = store
        .remove("missing", &snapshot.revision)
        .await
        .unwrap_err();
    assert_eq!(remove.code, "use.extension.registry_source_not_found");
    assert!(!temporary.path().join("state/registries.acl").exists());
}

#[tokio::test]
async fn source_mutations_fail_closed_while_the_configuration_lock_is_held() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let store = RegistrySourceStore::new(paths.clone());
    let _lock = io::RegistrySourcesLock::acquire(&paths).unwrap();

    let error = store
        .add(input(
            "packages",
            "https://registry.example/",
            &"a".repeat(64),
        ))
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.extension.registry_sources_busy");
    assert!(!temporary.path().join("state/registries.acl").exists());
}
