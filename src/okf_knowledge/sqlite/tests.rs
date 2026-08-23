use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Arc;

use a3s_use_core::{
    inspect_okf_bundle_files, OkfBundleContract, OkfBundleFile, OkfBundleLimits,
    OkfCapabilityProjection, OkfFormatVersion, PlanQualifiedSurfaceRef, PlanScope, PlanScopeKind,
    PluginSurfaceKind, PluginSurfaceRef, OKF_BUNDLE_CONTRACT_SCHEMA,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;
use crate::okf_knowledge::{OkfKnowledgeClient, OkfKnowledgeReadRequest, OkfKnowledgeStageSpec};

const PACKAGE_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MANIFEST_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[tokio::test]
async fn stages_promotes_and_queries_cited_concepts_after_restart() {
    let temporary = TempDir::new().unwrap();
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path()));
    let client = OkfKnowledgeClient::new(adapter);
    let files = knowledge_files("1200 requests per second", "45 milliseconds");
    let spec = stage_spec(1, scope(PlanScopeKind::Workspace), "acme/research", &files);
    let promoted = stage_and_promote(&client, spec, files.clone()).await;
    let projection = projection(&promoted);
    let request = OkfKnowledgeSearchRequest::new(
        promoted.receipt.scope.clone(),
        "request throughput",
        5,
        vec![projection],
    )
    .unwrap();

    let first = client.search(&request).await.unwrap();
    assert!(!first.hits.is_empty());
    assert_eq!(first.hits[0].title, "Request throughput");
    assert_eq!(first.hits[0].citation.path, "throughput.md");
    assert_eq!(
        first.hits[0].citation.source_digest,
        format!("sha256:{:x}", Sha256::digest(&files[0].content))
    );
    let read = client
        .read(
            &OkfKnowledgeReadRequest::new(
                promoted.receipt.scope.clone(),
                request.projections[0].clone(),
                first.hits[0].citation.clone(),
                files[0].content.len(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(read.content.as_bytes(), files[0].content.as_slice());
    assert_eq!(read.byte_count, files[0].content.len());

    let restarted =
        OkfKnowledgeClient::new(Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path())));
    assert_eq!(restarted.search(&request).await.unwrap(), first);
    assert_eq!(
        restarted
            .observe(&promoted.receipt)
            .await
            .unwrap()
            .observation,
        promoted.observation
    );
}

#[tokio::test]
async fn cited_read_enforces_bounds_and_rejects_database_tampering() {
    let temporary = TempDir::new().unwrap();
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path()));
    let client = OkfKnowledgeClient::new(adapter.clone());
    let files = knowledge_files("bounded source throughput", "bounded source latency");
    let promoted = stage_and_promote(
        &client,
        stage_spec(1, workspace_scope.clone(), "acme/research", &files),
        files.clone(),
    )
    .await;
    let exact_projection = projection(&promoted);
    let search = client
        .search(
            &OkfKnowledgeSearchRequest::new(
                workspace_scope.clone(),
                "bounded source throughput",
                5,
                vec![exact_projection.clone()],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let citation = search.hits[0].citation.clone();
    let requested_bytes = files[0].content.len();

    let bounded = OkfKnowledgeReadRequest::new(
        workspace_scope.clone(),
        exact_projection.clone(),
        citation.clone(),
        requested_bytes - 1,
    )
    .unwrap();
    let error = client.read(&bounded).await.unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_read_failed");

    let exact = OkfKnowledgeReadRequest::new(
        workspace_scope.clone(),
        exact_projection,
        citation,
        requested_bytes,
    )
    .unwrap();
    assert_eq!(
        client.read(&exact).await.unwrap().content.as_bytes(),
        files[0].content.as_slice()
    );

    let path = adapter
        .scope_directory(&workspace_scope)
        .unwrap()
        .join("knowledge.sqlite3");
    let connection = super::schema::open(&path, false).unwrap();
    let substituted_digest = format!("sha256:{}", "c".repeat(64));
    connection
        .execute(
            "UPDATE knowledge_documents SET source_digest = ?1
             WHERE package_id = 'acme/research' AND surface_id = 'domain-knowledge'
               AND generation = 1 AND path = 'throughput.md'",
            rusqlite::params![substituted_digest],
        )
        .unwrap();
    drop(connection);
    let error = adapter.audit(&workspace_scope).await.unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_database_invalid");

    let original_digest = format!("sha256:{:x}", Sha256::digest(&files[0].content));
    let tampered_content = b"---\ntype: Metric\n---\n\n# Forged knowledge\n".to_vec();
    let tampered_digest = format!("sha256:{:x}", Sha256::digest(&tampered_content));
    let connection = super::schema::open(&path, false).unwrap();
    connection
        .execute(
            "UPDATE knowledge_documents SET source_digest = ?1, content = ?2
             WHERE package_id = 'acme/research' AND surface_id = 'domain-knowledge'
               AND generation = 1 AND path = 'throughput.md'",
            rusqlite::params![tampered_digest, tampered_content],
        )
        .unwrap();
    drop(connection);
    let error = adapter.audit(&workspace_scope).await.unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_database_invalid");
    let error = adapter
        .repair_search_index(&workspace_scope)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_database_invalid");
    let error = client.read(&exact).await.unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_read_failed");

    let connection = super::schema::open(&path, false).unwrap();
    connection
        .execute(
            "UPDATE knowledge_documents SET source_digest = ?1, content = ?2
             WHERE package_id = 'acme/research' AND surface_id = 'domain-knowledge'
               AND generation = 1 AND path = 'throughput.md'",
            rusqlite::params![original_digest, files[0].content],
        )
        .unwrap();
    drop(connection);
    adapter.audit(&workspace_scope).await.unwrap();
}

#[tokio::test]
async fn identical_scope_ids_are_isolated_by_scope_kind() {
    let temporary = TempDir::new().unwrap();
    let client =
        OkfKnowledgeClient::new(Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path())));
    let workspace_files = knowledge_files("workspaceexclusive throughput", "workspace latency");
    let user_files = knowledge_files("user-only throughput", "user latency");
    let workspace = stage_and_promote(
        &client,
        stage_spec(
            1,
            scope(PlanScopeKind::Workspace),
            "acme/research",
            &workspace_files,
        ),
        workspace_files,
    )
    .await;
    let user = stage_and_promote(
        &client,
        stage_spec(1, scope(PlanScopeKind::User), "acme/research", &user_files),
        user_files,
    )
    .await;

    let workspace_result = client
        .search(
            &OkfKnowledgeSearchRequest::new(
                workspace.receipt.scope.clone(),
                "workspaceexclusive",
                5,
                vec![projection(&workspace)],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(!workspace_result.hits.is_empty());
    let user_result = client
        .search(
            &OkfKnowledgeSearchRequest::new(
                user.receipt.scope.clone(),
                "workspaceexclusive",
                5,
                vec![projection(&user)],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(user_result.hits.is_empty());

    let error = OkfKnowledgeSearchRequest::new(
        user.receipt.scope.clone(),
        "throughput",
        5,
        vec![projection(&workspace)],
    )
    .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_search_request_invalid");
}

#[tokio::test]
async fn promotion_keeps_draining_projections_queryable_until_receipt_owned_removal() {
    let temporary = TempDir::new().unwrap();
    let client =
        OkfKnowledgeClient::new(Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path())));
    let first_files = knowledge_files("first generation throughput", "first latency");
    let first = stage_and_promote(
        &client,
        stage_spec(
            1,
            scope(PlanScopeKind::Workspace),
            "acme/research",
            &first_files,
        ),
        first_files,
    )
    .await;
    let first_projection = projection(&first);

    let second_files = knowledge_files("second generation throughput", "second latency");
    let second_spec = stage_spec(
        2,
        scope(PlanScopeKind::Workspace),
        "acme/research",
        &second_files,
    );
    let staged = client
        .stage(OkfKnowledgeStageRequest::new(second_spec, second_files).unwrap())
        .await
        .unwrap();
    assert_eq!(
        staged.observation.selected.as_ref().unwrap().generation,
        first.receipt.generation
    );
    assert!(!client
        .search(
            &OkfKnowledgeSearchRequest::new(
                first.receipt.scope.clone(),
                "first generation",
                5,
                vec![first_projection.clone()],
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .hits
        .is_empty());

    let second = client.promote(&staged.receipt).await.unwrap();
    let second_projection = projection(&second);
    let draining = client
        .search(
            &OkfKnowledgeSearchRequest::new(
                first.receipt.scope.clone(),
                "throughput",
                5,
                vec![first_projection],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(!draining.hits.is_empty());

    client.remove(&first.receipt).await.unwrap();
    client.remove(&first.receipt).await.unwrap();
    let removed_first = client
        .search(
            &OkfKnowledgeSearchRequest::new(
                first.receipt.scope.clone(),
                "first generation",
                5,
                vec![projection(&first)],
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(removed_first.code, "use.okf.knowledge_projection_stale");
    assert!(!client
        .search(
            &OkfKnowledgeSearchRequest::new(
                second.receipt.scope.clone(),
                "second generation",
                5,
                vec![second_projection.clone()],
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .hits
        .is_empty());

    client.remove(&second.receipt).await.unwrap();
    let removed = client
        .search(
            &OkfKnowledgeSearchRequest::new(
                second.receipt.scope.clone(),
                "second generation",
                5,
                vec![second_projection],
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(removed.code, "use.okf.knowledge_projection_stale");
}

#[tokio::test]
async fn candidate_removal_restores_last_good_queryability() {
    let temporary = TempDir::new().unwrap();
    let client =
        OkfKnowledgeClient::new(Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path())));
    let first_files = knowledge_files("last-good throughput", "last-good latency");
    let first = stage_and_promote(
        &client,
        stage_spec(
            1,
            scope(PlanScopeKind::Workspace),
            "acme/research",
            &first_files,
        ),
        first_files,
    )
    .await;
    let candidate_files = knowledge_files("candidate throughput", "candidate latency");
    let candidate = stage_and_promote(
        &client,
        stage_spec(
            2,
            scope(PlanScopeKind::Workspace),
            "acme/research",
            &candidate_files,
        ),
        candidate_files,
    )
    .await;

    client.remove(&candidate.receipt).await.unwrap();
    let result = client
        .search(
            &OkfKnowledgeSearchRequest::new(
                first.receipt.scope.clone(),
                "last-good",
                5,
                vec![projection(&first)],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(!result.hits.is_empty());
    assert_eq!(result.hits[0].citation.generation, 1);
}

#[tokio::test]
async fn stage_replay_is_idempotent_and_conflicting_operation_fails_closed() {
    let temporary = TempDir::new().unwrap();
    let client =
        OkfKnowledgeClient::new(Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path())));
    let files = knowledge_files("stable throughput", "stable latency");
    let spec = stage_spec(1, scope(PlanScopeKind::Workspace), "acme/research", &files);
    let request = || OkfKnowledgeStageRequest::new(spec.clone(), files.clone()).unwrap();
    let first = client.stage(request()).await.unwrap();
    assert_eq!(client.stage(request()).await.unwrap(), first);

    let mut conflict = spec;
    conflict.operation_id = "different-operation".to_owned();
    let error = client
        .stage(OkfKnowledgeStageRequest::new(conflict, files).unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_database_conflict");
}

#[tokio::test]
async fn scope_quota_is_atomic_reusable_after_removal_and_survives_restart() {
    let temporary = TempDir::new().unwrap();
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let first_files = knowledge_files("first quota generation", "first quota latency");
    let first_spec = stage_spec(1, workspace_scope.clone(), "acme/first", &first_files);
    let second_files = knowledge_files("second quota generation", "second quota latency");
    let second_spec = stage_spec(1, workspace_scope.clone(), "acme/second", &second_files);
    let policy = OkfKnowledgeStoragePolicy::new(
        first_spec
            .bundle
            .expanded_bytes
            .max(second_spec.bundle.expanded_bytes),
        4,
        2,
        4,
    )
    .unwrap();
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::with_policy(
        temporary.path(),
        policy,
    ));
    let client = OkfKnowledgeClient::new(adapter.clone());
    let first = stage_and_promote(&client, first_spec, first_files).await;

    let before = adapter.usage(&workspace_scope).await.unwrap();
    assert_eq!(before.retained_projections, 1);
    assert_eq!(
        before.retained_expanded_bytes,
        first.receipt.bundle.expanded_bytes
    );
    assert_eq!(before.removed_tombstones, 0);

    let error = client
        .stage(OkfKnowledgeStageRequest::new(second_spec.clone(), second_files.clone()).unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_scope_quota_exceeded");
    assert_eq!(adapter.usage(&workspace_scope).await.unwrap(), before);
    assert!(!client
        .search(
            &OkfKnowledgeSearchRequest::new(
                workspace_scope.clone(),
                "first quota generation",
                5,
                vec![projection(&first)],
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .hits
        .is_empty());

    client.remove(&first.receipt).await.unwrap();
    let removed = adapter.usage(&workspace_scope).await.unwrap();
    assert_eq!(removed.retained_projections, 0);
    assert_eq!(removed.retained_expanded_bytes, 0);
    assert_eq!(removed.removed_tombstones, 1);
    assert_eq!(removed.reclaimable_database_bytes, 0);

    drop(client);
    drop(adapter);
    let restarted_adapter = Arc::new(SqliteOkfKnowledgeAdapter::with_policy(
        temporary.path(),
        policy,
    ));
    let restarted = OkfKnowledgeClient::new(restarted_adapter.clone());
    stage_and_promote(&restarted, second_spec, second_files).await;
    let after_restart = restarted_adapter.usage(&workspace_scope).await.unwrap();
    assert_eq!(after_restart.retained_projections, 1);
    assert_eq!(after_restart.removed_tombstones, 1);
}

#[tokio::test]
async fn scope_projection_limit_and_scope_kind_are_independent() {
    let temporary = TempDir::new().unwrap();
    let policy = OkfKnowledgeStoragePolicy::new(1024 * 1024, 1, 1, 4).unwrap();
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::with_policy(
        temporary.path(),
        policy,
    ));
    let client = OkfKnowledgeClient::new(adapter.clone());
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let first_files = knowledge_files("workspace quota", "workspace latency");
    stage_and_promote(
        &client,
        stage_spec(1, workspace_scope.clone(), "acme/first", &first_files),
        first_files,
    )
    .await;

    let second_files = knowledge_files("workspace overflow", "overflow latency");
    let error = client
        .stage(
            OkfKnowledgeStageRequest::new(
                stage_spec(1, workspace_scope.clone(), "acme/second", &second_files),
                second_files,
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.okf.knowledge_scope_projection_limit_exceeded"
    );

    let user_scope = scope(PlanScopeKind::User);
    let user_files = knowledge_files("user quota", "user latency");
    stage_and_promote(
        &client,
        stage_spec(1, user_scope.clone(), "acme/second", &user_files),
        user_files,
    )
    .await;
    assert_eq!(
        adapter
            .usage(&workspace_scope)
            .await
            .unwrap()
            .retained_projections,
        1
    );
    assert_eq!(
        adapter
            .usage(&user_scope)
            .await
            .unwrap()
            .retained_projections,
        1
    );
}

#[tokio::test]
async fn removal_bounds_scope_tombstones_and_reclaims_sqlite_pages() {
    let temporary = TempDir::new().unwrap();
    let policy = OkfKnowledgeStoragePolicy::new(1024 * 1024, 4, 1, 2).unwrap();
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::with_policy(
        temporary.path(),
        policy,
    ));
    let client = OkfKnowledgeClient::new(adapter.clone());
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let mut receipts = Vec::new();
    for package in ["acme/first", "acme/second", "acme/third"] {
        let files = knowledge_files(package, "retired latency");
        let binding = stage_and_promote(
            &client,
            stage_spec(1, workspace_scope.clone(), package, &files),
            files,
        )
        .await;
        client.remove(&binding.receipt).await.unwrap();
        receipts.push(binding.receipt);
    }

    let usage = adapter.usage(&workspace_scope).await.unwrap();
    assert_eq!(usage.retained_projections, 0);
    assert_eq!(usage.removed_tombstones, 2);
    assert_eq!(usage.retained_expanded_bytes, 0);
    assert_eq!(usage.reclaimable_database_bytes, 0);

    let oldest = client.observe(&receipts[0]).await.unwrap_err();
    assert_eq!(oldest.code, "use.okf.knowledge_projection_missing");
    client.remove(&receipts[2]).await.unwrap();
}

#[tokio::test]
async fn storage_accounting_rejects_receipt_row_identity_tampering() {
    let temporary = TempDir::new().unwrap();
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path()));
    let client = OkfKnowledgeClient::new(adapter.clone());
    let files = knowledge_files("tamper-resistant quota", "tamper-resistant latency");
    let staged = client
        .stage(
            OkfKnowledgeStageRequest::new(
                stage_spec(1, workspace_scope.clone(), "acme/research", &files),
                files,
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let mut tampered = staged.receipt.clone();
    tampered.surface.package_id = "acme/other".to_owned();
    let receipt_bytes = tampered.canonical_bytes().unwrap();
    let receipt_digest = tampered.descriptor_digest().unwrap();
    let path = adapter
        .scope_directory(&workspace_scope)
        .unwrap()
        .join("knowledge.sqlite3");
    let connection = super::schema::open(&path, false).unwrap();
    connection
        .execute(
            "UPDATE knowledge_projections
             SET receipt_json = ?1, receipt_digest = ?2
             WHERE package_id = 'acme/research' AND surface_id = 'domain-knowledge'
               AND generation = 1",
            rusqlite::params![receipt_bytes, receipt_digest],
        )
        .unwrap();
    drop(connection);

    let error = adapter.usage(&workspace_scope).await.unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_database_invalid");
    let repair = adapter
        .repair_search_index(&workspace_scope)
        .await
        .unwrap_err();
    assert_eq!(repair.code, "use.okf.knowledge_database_invalid");
    let backup = temporary.path().join("tampered.a3s-okf-backup");
    let error = adapter.backup(&workspace_scope, &backup).await.unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_database_invalid");
    assert!(!backup.exists());
}

#[tokio::test]
async fn audit_and_repair_rebuild_only_the_derived_search_index() {
    let temporary = TempDir::new().unwrap();
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path()));
    let client = OkfKnowledgeClient::new(adapter.clone());
    let files = knowledge_files("auditable throughput", "auditable latency");
    let promoted = stage_and_promote(
        &client,
        stage_spec(1, workspace_scope.clone(), "acme/research", &files),
        files,
    )
    .await;

    let healthy = adapter.audit(&workspace_scope).await.unwrap();
    assert_eq!(healthy.scope, workspace_scope);
    assert_eq!(healthy.document_count, 2);
    assert_eq!(healthy.indexed_document_count, 2);
    assert_eq!(healthy.storage.retained_projections, 1);

    let path = adapter
        .scope_directory(&workspace_scope)
        .unwrap()
        .join("knowledge.sqlite3");
    let connection = super::schema::open(&path, false).unwrap();
    connection
        .execute(
            "DELETE FROM knowledge_documents_fts
             WHERE rowid = (SELECT MIN(rowid) FROM knowledge_documents_fts)",
            [],
        )
        .unwrap();
    drop(connection);

    let error = adapter.audit(&workspace_scope).await.unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_search_index_invalid");

    let repaired = adapter.repair_search_index(&workspace_scope).await.unwrap();
    assert_eq!(repaired.scope, workspace_scope);
    assert_eq!(repaired.rebuilt_document_count, 2);
    assert_eq!(repaired.after.document_count, 2);
    assert_eq!(repaired.after.indexed_document_count, 2);
    assert_eq!(repaired.after.storage.retained_projections, 1);
    assert_eq!(
        client.observe(&promoted.receipt).await.unwrap().observation,
        promoted.observation
    );
    assert!(!client
        .search(
            &OkfKnowledgeSearchRequest::new(
                workspace_scope,
                "auditable throughput",
                5,
                vec![projection(&promoted)],
            )
            .unwrap(),
        )
        .await
        .unwrap()
        .hits
        .is_empty());
}

#[tokio::test]
async fn backup_is_scope_bound_verified_and_never_overwrites() {
    let temporary = TempDir::new().unwrap();
    let workspace_scope = scope(PlanScopeKind::Workspace);
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path()));
    let client = OkfKnowledgeClient::new(adapter.clone());
    let files = knowledge_files("backed-up throughput", "backed-up latency");
    stage_and_promote(
        &client,
        stage_spec(1, workspace_scope.clone(), "acme/research", &files),
        files,
    )
    .await;
    let backup = temporary.path().join("workspace.a3s-okf-backup");

    let created = adapter.backup(&workspace_scope, &backup).await.unwrap();
    assert_eq!(created.scope, workspace_scope);
    assert_eq!(created.storage.retained_projections, 1);
    assert!(created.database_bytes > 0);
    assert!(created.database_sha256.starts_with("sha256:"));
    assert!(backup.is_file());

    let verified = SqliteOkfKnowledgeAdapter::verify_backup(&backup, Some(&workspace_scope))
        .await
        .unwrap();
    assert_eq!(verified, created);
    let mut unknown_field = serde_json::to_value(&created).unwrap();
    unknown_field["storage"]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<OkfKnowledgeBackupManifest>(unknown_field).is_err());

    let manifest_tamper = temporary.path().join("manifest-tamper.a3s-okf-backup");
    std::fs::copy(&backup, &manifest_tamper).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&manifest_tamper)
        .unwrap();
    let manifest_offset = u64::try_from(b"A3S-OKF-BACKUP\n".len() + 4 + 32).unwrap();
    file.seek(SeekFrom::Start(manifest_offset)).unwrap();
    let mut manifest_byte = [0_u8; 1];
    file.read_exact(&mut manifest_byte).unwrap();
    file.seek(SeekFrom::Start(manifest_offset)).unwrap();
    file.write_all(&[manifest_byte[0] ^ 0xff]).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let error = SqliteOkfKnowledgeAdapter::verify_backup(&manifest_tamper, Some(&workspace_scope))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_backup_invalid");

    let wrong_scope = scope(PlanScopeKind::User);
    let error = SqliteOkfKnowledgeAdapter::verify_backup(&backup, Some(&wrong_scope))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_backup_scope_mismatch");

    let overwrite = adapter.backup(&workspace_scope, &backup).await.unwrap_err();
    assert_eq!(overwrite.code, "use.okf.knowledge_backup_exists");

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&backup)
        .unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    let mut final_byte = [0_u8; 1];
    file.read_exact(&mut final_byte).unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    file.write_all(&[final_byte[0] ^ 0xff]).unwrap();
    file.sync_all().unwrap();
    drop(file);

    let error = SqliteOkfKnowledgeAdapter::verify_backup(&backup, Some(&workspace_scope))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_backup_invalid");
}

#[test]
fn storage_policy_rejects_unbounded_or_inconsistent_limits() {
    for error in [
        OkfKnowledgeStoragePolicy::new(0, 1, 1, 1).unwrap_err(),
        OkfKnowledgeStoragePolicy::new(MAX_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES + 1, 1, 1, 1)
            .unwrap_err(),
        OkfKnowledgeStoragePolicy::new(1, 0, 1, 1).unwrap_err(),
        OkfKnowledgeStoragePolicy::new(1, MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS + 1, 1, 1)
            .unwrap_err(),
        OkfKnowledgeStoragePolicy::new(1, 1, 0, 1).unwrap_err(),
        OkfKnowledgeStoragePolicy::new(1, 1, 2, 1).unwrap_err(),
        OkfKnowledgeStoragePolicy::new(
            1,
            MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS,
            crate::okf_knowledge::MAX_OKF_KNOWLEDGE_GENERATIONS + 1,
            1,
        )
        .unwrap_err(),
        OkfKnowledgeStoragePolicy::new(1, 1, 1, 0).unwrap_err(),
        OkfKnowledgeStoragePolicy::new(1, 1, 1, MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES + 1)
            .unwrap_err(),
    ] {
        assert_eq!(error.code, "use.okf.knowledge_storage_policy_invalid");
    }
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn backend_rejects_linked_database_roots() {
    let temporary = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path()));
    std::fs::create_dir_all(adapter.root().parent().unwrap()).unwrap();
    crate::test_filesystem::create_directory_link(outside.path(), adapter.root());
    let client = OkfKnowledgeClient::new(adapter);
    let files = knowledge_files("unsafe throughput", "unsafe latency");
    let error = client
        .stage(
            OkfKnowledgeStageRequest::new(
                stage_spec(1, scope(PlanScopeKind::Workspace), "acme/research", &files),
                files,
            )
            .unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.okf.knowledge_database_path_invalid");
}

#[test]
fn database_accepts_only_the_current_schema_without_migration() {
    let temporary = TempDir::new().unwrap();
    let path = temporary.path().join("knowledge.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    let unsupported_version = super::schema::DATABASE_SCHEMA_VERSION + 1;
    connection
        .pragma_update(None, "user_version", unsupported_version)
        .unwrap();
    drop(connection);

    assert!(super::schema::open(&path, false).is_err());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .unwrap();
    assert_eq!(
        version, unsupported_version,
        "opening must not rewrite or migrate the database"
    );
}

pub(super) async fn stage_and_promote(
    client: &OkfKnowledgeClient,
    spec: OkfKnowledgeStageSpec,
    files: Vec<OkfBundleFile>,
) -> OkfKnowledgeBinding {
    let staged = client
        .stage(OkfKnowledgeStageRequest::new(spec, files).unwrap())
        .await
        .unwrap();
    client.promote(&staged.receipt).await.unwrap()
}

fn projection(binding: &OkfKnowledgeBinding) -> OkfCapabilityProjection {
    OkfCapabilityProjection::from_promoted(&binding.receipt, &binding.observation).unwrap()
}

pub(super) fn stage_spec(
    generation: u64,
    scope: PlanScope,
    package_id: &str,
    files: &[OkfBundleFile],
) -> OkfKnowledgeStageSpec {
    OkfKnowledgeStageSpec {
        operation_id: format!("operation-{generation}"),
        scope,
        surface: PlanQualifiedSurfaceRef {
            package_id: package_id.to_owned(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Okf,
                id: "domain-knowledge".to_owned(),
            },
        },
        generation,
        package_digest: PACKAGE_DIGEST.to_owned(),
        manifest_digest: MANIFEST_DIGEST.to_owned(),
        bundle: bundle(files),
    }
}

fn bundle(files: &[OkfBundleFile]) -> OkfBundleContract {
    let limits = OkfBundleLimits::default();
    let inspection =
        inspect_okf_bundle_files(OkfFormatVersion::V0_2, limits.clone(), files).unwrap();
    OkfBundleContract {
        schema: OKF_BUNDLE_CONTRACT_SCHEMA.to_owned(),
        format_version: inspection.format_version,
        root: "knowledge".to_owned(),
        content_digest: inspection.content_digest,
        concept_count: inspection.concept_count,
        file_count: inspection.file_count,
        expanded_bytes: inspection.expanded_bytes,
        limits,
    }
}

pub(super) fn knowledge_files(throughput: &str, latency: &str) -> Vec<OkfBundleFile> {
    vec![
        OkfBundleFile::new(
            "throughput.md",
            format!(
                "---\ntype: Metric\n---\n\n# Request throughput\n\nThe service handles {throughput}.\n"
            ),
        ),
        OkfBundleFile::new(
            "latency.md",
            format!(
                "---\ntype: Metric\n---\n\n# Request latency\n\nThe p95 latency is {latency}.\n"
            ),
        ),
    ]
}

pub(super) fn scope(kind: PlanScopeKind) -> PlanScope {
    PlanScope {
        kind,
        id: "shared-scope".to_owned(),
    }
}
