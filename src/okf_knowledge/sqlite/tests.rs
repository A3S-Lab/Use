use std::sync::Arc;

use a3s_use_core::{
    inspect_okf_bundle_files, OkfBundleContract, OkfBundleFile, OkfBundleLimits,
    OkfCapabilityProjection, OkfFormatVersion, PlanQualifiedSurfaceRef, PlanScope, PlanScopeKind,
    PluginSurfaceKind, PluginSurfaceRef, OKF_BUNDLE_CONTRACT_SCHEMA,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;
use crate::okf_knowledge::{OkfKnowledgeClient, OkfKnowledgeStageSpec};

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

#[cfg(unix)]
#[tokio::test]
async fn backend_rejects_symlinked_database_roots() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::new(temporary.path()));
    std::fs::create_dir_all(adapter.root().parent().unwrap()).unwrap();
    symlink(outside.path(), adapter.root()).unwrap();
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
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);

    assert!(super::schema::open(&path, false).is_err());
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .unwrap();
    assert_eq!(
        version, 2,
        "opening must not rewrite or migrate the database"
    );
}

async fn stage_and_promote(
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

fn stage_spec(
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

fn knowledge_files(throughput: &str, latency: &str) -> Vec<OkfBundleFile> {
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

fn scope(kind: PlanScopeKind) -> PlanScope {
    PlanScope {
        kind,
        id: "shared-scope".to_owned(),
    }
}
