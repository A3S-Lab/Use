use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use a3s_use_core::RiskClass;
use a3s_use_extension::{
    ExtensionManifest, ExtensionPaths, ExtensionRegistry, InstallOptions, McpTransport,
};
use a3s_use_science::{ScienceClient, ScienceEndpoints};
use axum::extract::{OriginalUri, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tokio::task::JoinHandle;
use url::Url;

#[derive(Clone, Default)]
struct RequestLog(Arc<Mutex<Vec<RecordedRequest>>>);

#[derive(Debug)]
struct RecordedRequest {
    uri: Uri,
    query: HashMap<String, String>,
    user_agent: Option<String>,
}

struct MockServer {
    base: Url,
    log: RequestLog,
    task: JoinHandle<()>,
}

impl MockServer {
    async fn start() -> Self {
        let log = RequestLog::default();
        let app = Router::new()
            .route("/pubmed/esearch.fcgi", get(pubmed_search))
            .route("/pubmed/esummary.fcgi", get(pubmed_summary))
            .route("/chembl/molecule/search.json", get(chembl_failure))
            .with_state(log.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            base: Url::parse(&format!("http://{address}/")).unwrap(),
            log,
            task,
        }
    }

    fn endpoints(&self) -> ScienceEndpoints {
        ScienceEndpoints {
            pubmed: self.base.join("pubmed/").unwrap(),
            chembl: self.base.join("chembl/").unwrap(),
            clinical_trials: self.base.join("clinical-trials/").unwrap(),
            biorxiv: self.base.join("biorxiv/").unwrap(),
            ensembl: self.base.join("ensembl/").unwrap(),
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn pubmed_search(
    State(log): State<RequestLog>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    record(&log, uri, query, &headers);
    Json(json!({
        "esearchresult": {
            "count": "1",
            "idlist": ["12345678"]
        }
    }))
}

async fn pubmed_summary(
    State(log): State<RequestLog>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Json<serde_json::Value> {
    record(&log, uri, query, &headers);
    Json(json!({
        "result": {
            "uids": ["12345678"],
            "12345678": {
                "title": "A typed science result",
                "authors": [{"name": "A. Researcher"}],
                "fulljournalname": "Journal of Tests",
                "pubdate": "2026",
                "articleids": [{"idtype": "doi", "value": "10.1000/test"}]
            }
        }
    }))
}

async fn chembl_failure(
    State(log): State<RequestLog>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    record(&log, uri, query, &headers);
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "upstream failure ".repeat(100),
    )
}

fn record(log: &RequestLog, uri: Uri, query: HashMap<String, String>, headers: &HeaderMap) {
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    log.0.lock().unwrap().push(RecordedRequest {
        uri,
        query,
        user_agent,
    });
}

#[tokio::test]
async fn pubmed_uses_two_typed_requests_and_encodes_contact_metadata() {
    let server = MockServer::start().await;
    let client = ScienceClient::builder()
        .endpoints(server.endpoints())
        .contact_email("researcher@example.org")
        .ncbi_api_key("test-key")
        .build()
        .unwrap();

    let page = client
        .pubmed_search("gene therapy & safety", 7)
        .await
        .unwrap();
    assert_eq!(page.total, Some(1));
    assert_eq!(page.items[0].pmid, "12345678");
    assert_eq!(page.items[0].doi.as_deref(), Some("10.1000/test"));

    let requests = server.log.0.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].uri.to_string().contains("%26"));
    assert_eq!(requests[0].query["term"], "gene therapy & safety");
    assert_eq!(requests[0].query["retmax"], "7");
    assert_eq!(requests[0].query["email"], "researcher@example.org");
    assert_eq!(requests[0].query["api_key"], "test-key");
    assert_eq!(
        requests[0].user_agent.as_deref(),
        Some(concat!("a3s-use-science/", env!("CARGO_PKG_VERSION")))
    );
    assert_eq!(requests[1].query["id"], "12345678");
}

#[tokio::test]
async fn upstream_http_failures_use_a_stable_bounded_error() {
    let server = MockServer::start().await;
    let client = ScienceClient::builder()
        .endpoints(server.endpoints())
        .build()
        .unwrap();

    let error = client
        .chembl_search_molecules("aspirin", 3)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.science.upstream_error");
    assert_eq!(error.details["service"], "ChEMBL");
    assert_eq!(error.details["status"], 503);
    let body = error.details["body"].as_str().unwrap();
    assert_eq!(body.chars().count(), 1_025);
    assert!(body.ends_with('…'));

    let requests = server.log.0.lock().unwrap();
    assert_eq!(requests[0].query["q"], "aspirin");
    assert_eq!(requests[0].query["limit"], "3");
}

#[test]
fn packaged_manifest_declares_native_read_only_surfaces() {
    let manifest_text = include_str!("../package/a3s-use-extension.acl");
    let manifest = ExtensionManifest::parse_acl(manifest_text).unwrap();
    assert_eq!(manifest.package_id, "a3s/science");
    assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest.route, "science");
    assert_eq!(manifest.actions, [RiskClass::Read]);
    assert!(manifest.cli.as_ref().unwrap().json_output);
    assert_eq!(
        manifest.mcp.as_ref().unwrap().transport,
        McpTransport::Stdio
    );
    assert_eq!(
        manifest.mcp.as_ref().unwrap().args,
        ["serve".to_string(), "--mcp".to_string()]
    );
    assert_eq!(
        manifest.skill.as_ref().unwrap().path,
        Path::new("skills/a3s-use-science/SKILL.md")
    );
    manifest
        .validate_package_root(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("package")
                .as_path(),
        )
        .unwrap();
}

#[test]
fn binary_emits_versioned_diagnostics_and_errors() {
    let binary = env!("CARGO_BIN_EXE_a3s-use-science");
    let diagnostic = Command::new(binary)
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(diagnostic.status.success());
    let value: serde_json::Value = serde_json::from_slice(&diagnostic.stdout).unwrap();
    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["data"]["sources"].as_array().unwrap().len(), 5);

    let invalid = Command::new(binary)
        .args(["pubmed", "get", "../escape", "--json"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    let value: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(value["error"]["code"], "use.science.identifier_invalid");
}

#[tokio::test]
async fn real_science_package_installs_hot_upgrades_dispatches_and_uninstalls() {
    let temp = tempfile::tempdir().unwrap();
    let first_package = temp.path().join("science-package-v1");
    let second_package = temp.path().join("science-package-v2");
    create_science_package(&first_package);
    create_science_package(&second_package);
    let registry = ExtensionRegistry::new(ExtensionPaths::new(
        temp.path().join("data"),
        temp.path().join("state"),
    ));

    let installed = registry
        .install_local(
            "a3s/science",
            &first_package,
            InstallOptions {
                allow_unsigned: true,
                ..InstallOptions::default()
            },
        )
        .await
        .unwrap();
    assert!(installed.changed);
    let first_root = installed.extension.receipt.package_root.clone();
    let lease = registry.acquire_route("science").await.unwrap().unwrap();
    let executable = lease.extension().cli_executable().unwrap();
    let diagnostic = Command::new(executable)
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(diagnostic.status.success());
    let value: serde_json::Value = serde_json::from_slice(&diagnostic.stdout).unwrap();
    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["data"]["sources"].as_array().unwrap().len(), 5);

    let upgraded = registry
        .install_local(
            "a3s/science",
            &second_package,
            InstallOptions {
                force: true,
                allow_unsigned: true,
            },
        )
        .await
        .unwrap();
    assert!(upgraded.changed);
    assert_ne!(upgraded.extension.receipt.package_root, first_root);
    assert!(
        first_root.exists(),
        "the generation pinned by an active route lease was removed"
    );
    drop(lease);

    let removed = registry.uninstall("a3s/science").await.unwrap();
    assert!(removed.changed);
    assert!(registry.get("a3s/science").await.unwrap().is_none());
    assert!(!first_root.parent().unwrap().exists());
}

fn create_science_package(root: &Path) {
    let binary = root.join("bin/a3s-use-science");
    let skill = root.join("skills/a3s-use-science/SKILL.md");
    std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_a3s-use-science"), &binary).unwrap();
    std::fs::write(
        root.join("a3s-use-extension.acl"),
        include_str!("../package/a3s-use-extension.acl"),
    )
    .unwrap();
    std::fs::write(
        &skill,
        include_str!("../package/skills/a3s-use-science/SKILL.md"),
    )
    .unwrap();
}
