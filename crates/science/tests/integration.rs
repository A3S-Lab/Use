use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use a3s_use_core::RiskClass;
use a3s_use_extension::{ExtensionManifest, PluginMcpLaunch, ToolTaskSource, ToolWorkload};
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
    let ToolWorkload::Task(tool) = &manifest.tools[0].workload else {
        panic!("science must be a Tool Task");
    };
    assert!(tool.json_output);
    assert_eq!(
        tool.source,
        ToolTaskSource::Executable {
            executable: PathBuf::from("bin/a3s-use-science")
        }
    );
    let PluginMcpLaunch::Stdio { args, .. } = &manifest.mcp_servers[0].launch else {
        panic!("science MCP must use stdio");
    };
    assert_eq!(args, &["serve".to_string(), "--mcp".to_string()]);
    assert_eq!(
        manifest.skills[0].path,
        Path::new("skills/a3s-use-science/SKILL.md")
    );
    let activity = &manifest.ui[0];
    assert_eq!(activity.id, "research");
    assert_eq!(activity.title, "科研");
    assert_eq!(activity.icon, "flask-conical");
    assert_eq!(activity.entry, Path::new("web/activity.html"));
    assert_eq!(activity.styles, [PathBuf::from("web/activity.css")]);
    assert_eq!(activity.scripts, [PathBuf::from("web/activity.js")]);
    assert_eq!(activity.skill.as_deref(), Some("science"));
    manifest
        .validate_package_root(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("package")
                .as_path(),
        )
        .unwrap();
}

#[test]
fn packaged_research_activity_declares_multiple_disciplines_and_subfields() {
    let activity = include_str!("../package/web/activity.html");
    let styles = include_str!("../package/web/activity.css");
    let script = include_str!("../package/web/activity.js");
    let catalog = activity
        .split_once("<script type=\"application/json\" id=\"discipline-catalog\">")
        .and_then(|(_, remainder)| remainder.split_once("</script>"))
        .map(|(json, _)| json)
        .expect("research Activity must embed its discipline catalog");
    let catalog: serde_json::Value = serde_json::from_str(catalog).unwrap();
    let disciplines = catalog
        .as_array()
        .expect("discipline catalog must be a JSON array");
    assert!(disciplines.len() >= 10);
    for discipline in disciplines {
        assert!(discipline["id"].as_str().is_some());
        assert!(discipline["label"].as_str().is_some());
        assert!(discipline["subfields"].as_array().unwrap().len() >= 4);
        let sources = discipline["sources"].as_array().unwrap();
        assert!(sources.len() >= 3);
        assert_eq!(
            sources
                .iter()
                .map(|source| source["id"].as_str().unwrap())
                .collect::<BTreeSet<_>>()
                .len(),
            sources.len(),
            "source IDs must be unique within a discipline"
        );
    }

    let package_skill_sources = disciplines
        .iter()
        .flat_map(|discipline| {
            let discipline_id = discipline["id"].as_str().unwrap();
            discipline["sources"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|source| source["packageSkill"] == true)
                .map(move |source| (discipline_id, source["id"].as_str().unwrap()))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        package_skill_sources,
        BTreeSet::from([
            ("agriculture-food", "ensembl"),
            ("chemistry-materials", "chembl"),
            ("life-sciences", "biorxiv"),
            ("life-sciences", "ensembl"),
            ("life-sciences", "pubmed"),
            ("medicine-health", "chembl"),
            ("medicine-health", "clinical-trials"),
            ("medicine-health", "pubmed"),
        ]),
        "package-backed sources must match the extension's implemented data sources"
    );
    assert!(!activity.contains("href=\"./activity.css\""));
    assert!(!activity.contains("src=\"./activity.js\""));
    assert!(activity.contains("id=\"project-name\""));
    assert!(activity.contains("id=\"validation-criteria\""));
    assert!(activity.contains("id=\"submit-research\" type=\"button\""));
    assert!(activity.contains("可复核科研闭环"));
    assert!(activity.contains("provenance-card"));
    assert!(!activity.contains("<style>"));
    assert!(!activity.contains("<script>"));
    assert!(styles.contains(".discipline-options"));
    assert!(script.contains("provenance note"));
    assert!(script.contains("usePackageSkill"));
    assert!(script.contains("a3s.activity.v1"));
    assert!(script.contains("submitButton.addEventListener('click'"));
    assert!(!script.contains("form.addEventListener('submit'"));
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
