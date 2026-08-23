use a3s_use_core::UseResult;
use sha2::{Digest, Sha256};

use super::diagnostic::tests::completed_operation_diagnostic;
use super::diagnostic_history::PluginOperationHistoryStore;
use super::{
    PluginOperationDiagnostic, PluginRetainedOperationOutcome,
    MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES,
};

fn operation(sequence: usize) -> PluginOperationDiagnostic {
    let mut diagnostic = completed_operation_diagnostic();
    diagnostic.observed_at_ms = 20 + sequence as u64;
    diagnostic.operation.operation_id = format!("install:acme-root:{sequence:04}");
    diagnostic.operation.plan_digest = format!(
        "sha256:{:x}",
        Sha256::digest(format!("operation-plan-{sequence}").as_bytes())
    );
    diagnostic.validate().unwrap();
    diagnostic
}

#[tokio::test]
async fn history_store_retains_newest_first_and_deduplicates_replay() -> UseResult<()> {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginOperationHistoryStore::new(temp.path());
    let first = operation(1);
    assert!(
        store
            .retain(&first, PluginRetainedOperationOutcome::Completed)
            .await?
    );

    let mut replay = first.clone();
    replay.observed_at_ms += 10;
    replay.validate().unwrap();
    assert!(
        !store
            .retain(&replay, PluginRetainedOperationOutcome::Completed)
            .await?
    );

    let second = operation(2);
    assert!(
        store
            .retain(&second, PluginRetainedOperationOutcome::Completed)
            .await?
    );
    let history = store.get(&first.scope, &first.package_id).await?;
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].diagnostic, second);
    assert_eq!(history[1].diagnostic, first);
    Ok(())
}

#[tokio::test]
async fn history_store_prunes_the_oldest_operation_at_the_fixed_limit() -> UseResult<()> {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginOperationHistoryStore::new(temp.path());
    let total = super::MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS + 1;
    for sequence in 0..total {
        assert!(
            store
                .retain(
                    &operation(sequence),
                    PluginRetainedOperationOutcome::Completed,
                )
                .await?
        );
    }

    let sample = operation(0);
    let history = store.get(&sample.scope, &sample.package_id).await?;
    assert_eq!(
        history.len(),
        super::MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS
    );
    assert_eq!(
        history.first().unwrap().diagnostic.operation.operation_id,
        format!("install:acme-root:{:04}", total - 1)
    );
    assert_eq!(
        history.last().unwrap().diagnostic.operation.operation_id,
        "install:acme-root:0001"
    );
    Ok(())
}

#[tokio::test]
async fn history_store_accepts_repeated_textual_ids_and_rejects_unknown_fields() -> UseResult<()> {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginOperationHistoryStore::new(temp.path());
    let first = operation(1);
    assert!(
        store
            .retain(&first, PluginRetainedOperationOutcome::Completed)
            .await?
    );

    let mut conflicting = operation(2);
    conflicting.operation.operation_id = first.operation.operation_id.clone();
    conflicting.validate().unwrap();
    assert!(
        store
            .retain(&conflicting, PluginRetainedOperationOutcome::Completed,)
            .await?
    );
    assert_eq!(store.get(&first.scope, &first.package_id).await?.len(), 2);

    let scope_digest = format!(
        "{:x}",
        Sha256::digest(format!("{}\n{}", first.scope.kind.as_str(), first.scope.id).as_bytes())
    );
    let path = temp
        .path()
        .join("operations/package-diagnostic-history/scopes")
        .join(scope_digest)
        .join("acme/root.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value.as_object_mut().unwrap().insert(
        "credential".to_owned(),
        serde_json::json!("history-secret-sentinel"),
    );
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        store
            .get(&first.scope, &first.package_id)
            .await
            .unwrap_err()
            .code,
        "use.plugin.operation_history_store_invalid"
    );
    Ok(())
}

#[tokio::test]
async fn history_store_rejects_an_oversized_record() -> UseResult<()> {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginOperationHistoryStore::new(temp.path());
    let first = operation(1);
    assert!(
        store
            .retain(&first, PluginRetainedOperationOutcome::Completed)
            .await?
    );

    let scope_digest = format!(
        "{:x}",
        Sha256::digest(format!("{}\n{}", first.scope.kind.as_str(), first.scope.id).as_bytes())
    );
    let path = temp
        .path()
        .join("operations/package-diagnostic-history/scopes")
        .join(scope_digest)
        .join("acme/root.json");
    std::fs::write(
        &path,
        vec![b' '; MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES + 1],
    )
    .unwrap();
    assert_eq!(
        store
            .get(&first.scope, &first.package_id)
            .await
            .unwrap_err()
            .code,
        "use.plugin.operation_history_store_invalid"
    );
    Ok(())
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn history_store_rejects_a_linked_record() -> UseResult<()> {
    let temp = tempfile::tempdir().unwrap();
    let store = PluginOperationHistoryStore::new(temp.path());
    let first = operation(1);
    assert!(
        store
            .retain(&first, PluginRetainedOperationOutcome::Completed)
            .await?
    );

    let scope_digest = format!(
        "{:x}",
        Sha256::digest(format!("{}\n{}", first.scope.kind.as_str(), first.scope.id).as_bytes())
    );
    let path = temp
        .path()
        .join("operations/package-diagnostic-history/scopes")
        .join(scope_digest)
        .join("acme/root.json");
    let target = temp.path().join("escaped-operation-history");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&target).unwrap();
    crate::test_filesystem::create_directory_link(&target, &path);
    assert_eq!(
        store
            .get(&first.scope, &first.package_id)
            .await
            .unwrap_err()
            .code,
        "use.plugin.operation_history_store_invalid"
    );
    Ok(())
}
