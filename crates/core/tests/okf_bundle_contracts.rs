use a3s_use_core::{
    inspect_okf_bundle, OkfBundleContract, OkfBundleFile, OkfBundleLimits, OkfDiagnosticCode,
    OkfFormatVersion, OKF_BUNDLE_CONTRACT_SCHEMA,
};

const CONTRACT: &[u8] = include_bytes!("../fixtures/okf/bundle-contract-v1.json");
const CONTRACT_DIGEST: &str =
    include_str!("../fixtures/okf/bundle-contract-v1.sha256").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

fn fixture_files() -> Vec<OkfBundleFile> {
    [
        (
            "index.md",
            include_bytes!("../fixtures/okf/bundle-v02/index.md").as_slice(),
        ),
        (
            "log.md",
            include_bytes!("../fixtures/okf/bundle-v02/log.md").as_slice(),
        ),
        (
            "concepts/package-lifecycle.md",
            include_bytes!("../fixtures/okf/bundle-v02/concepts/package-lifecycle.md").as_slice(),
        ),
        (
            "concepts/runtime-boundary.md",
            include_bytes!("../fixtures/okf/bundle-v02/concepts/runtime-boundary.md").as_slice(),
        ),
        (
            "computations/revenue.md",
            include_bytes!("../fixtures/okf/bundle-v02/computations/revenue.md").as_slice(),
        ),
        (
            "references/run-revenue.md",
            include_bytes!("../fixtures/okf/bundle-v02/references/run-revenue.md").as_slice(),
        ),
        (
            "references/attesters/revenue.py",
            include_bytes!("../fixtures/okf/bundle-v02/references/attesters/revenue.py").as_slice(),
        ),
    ]
    .into_iter()
    .map(|(path, content)| OkfBundleFile::new(path, content))
    .collect()
}

#[test]
fn canonical_okf_contract_fixture_has_a_stable_digest() {
    let contract = OkfBundleContract::from_json(CONTRACT).unwrap();

    assert_eq!(contract.schema, OKF_BUNDLE_CONTRACT_SCHEMA);
    assert_eq!(contract.format_version, OkfFormatVersion::V0_2);
    assert_eq!(
        contract.canonical_bytes().unwrap(),
        canonical_fixture(CONTRACT)
    );
    assert_eq!(contract.descriptor_digest().unwrap(), CONTRACT_DIGEST);
}

#[test]
fn inspects_v02_without_rejecting_extensions_or_safe_dangling_links() {
    let inspection = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        OkfBundleLimits::default(),
        fixture_files(),
    )
    .unwrap();
    let contract = OkfBundleContract::from_json(CONTRACT).unwrap();

    contract.verify_inspection(&inspection).unwrap();
    assert_eq!(inspection.concept_count, 4);
    assert_eq!(inspection.file_count, 7);
    assert!(inspection
        .concepts
        .iter()
        .any(|concept| concept.type_name == "Project-Specific Decision"));
    assert!(inspection
        .concepts
        .iter()
        .any(|concept| concept.type_name == "Attested Computation"));
    assert_eq!(inspection.diagnostics.len(), 1);
    assert_eq!(
        inspection.diagnostics[0].code,
        OkfDiagnosticCode::DanglingLink
    );
    assert_eq!(
        inspection.diagnostics[0].target,
        "concepts/future-generation.md"
    );
}

#[test]
fn accepts_v01_timestamp_and_body_citations_fallbacks() {
    let files = vec![
        OkfBundleFile::new(
            "index.md",
            b"---\nokf_version: \"0.1\"\n---\n\n# Concepts\n",
        ),
        OkfBundleFile::new(
            "metric.md",
            br#"---
type: Metric
timestamp: '2026-05-28T22:53:05+00:00'
---

# Definition

Revenue for a fiscal year.

# Citations

- https://example.com/policy
"#,
        ),
    ];

    let inspection =
        inspect_okf_bundle(OkfFormatVersion::V0_1, OkfBundleLimits::default(), files).unwrap();

    assert_eq!(inspection.format_version, OkfFormatVersion::V0_1);
    assert_eq!(inspection.concept_count, 1);
    assert!(inspection.diagnostics.is_empty());
}

#[test]
fn rejects_malformed_concepts_and_unsafe_resolution() {
    for content in [
        b"# Missing frontmatter\n".as_slice(),
        b"---\ntitle: Missing type\n---\n".as_slice(),
        b"---\ntype: \"\"\n---\n".as_slice(),
        b"---\ntype: [Metric]\n---\n".as_slice(),
        b"---\ntype: Metric\ninvalid: [\n---\n".as_slice(),
        b" ---\ntype: Metric\n---\n".as_slice(),
        b"---\ntype: Metric\n--- \n".as_slice(),
    ] {
        let error = inspect_okf_bundle(
            OkfFormatVersion::V0_2,
            OkfBundleLimits::default(),
            vec![OkfBundleFile::new("concept.md", content)],
        )
        .unwrap_err();
        assert_eq!(error.code, "use.okf.bundle_invalid");
    }

    let path_escape = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        OkfBundleLimits::default(),
        vec![OkfBundleFile::new(
            "../concept.md",
            b"---\ntype: Metric\n---\n",
        )],
    )
    .unwrap_err();
    assert_eq!(path_escape.code, "use.okf.path_escape");

    let link_escape = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        OkfBundleLimits::default(),
        vec![OkfBundleFile::new(
            "concepts/concept.md",
            b"---\ntype: Metric\n---\n\n[escape](../../outside.md)\n",
        )],
    )
    .unwrap_err();
    assert_eq!(link_escape.code, "use.okf.path_escape");

    let resource_escape = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        OkfBundleLimits::default(),
        vec![OkfBundleFile::new(
            "computations/revenue.md",
            b"---\ntype: Attested Computation\nruntime: python\ncomputation: ../../outside.py\n---\n",
        )],
    )
    .unwrap_err();
    assert_eq!(resource_escape.code, "use.okf.path_escape");
}

#[test]
fn enforces_declared_bounds_before_content_can_be_projected() {
    let limits = OkfBundleLimits {
        max_files: 1,
        max_concepts: 1,
        max_expanded_bytes: 64,
        max_document_bytes: 64,
        max_links_per_document: 1,
    };
    let error = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        limits,
        vec![
            OkfBundleFile::new("first.md", b"---\ntype: Metric\n---\n"),
            OkfBundleFile::new("second.md", b"---\ntype: Metric\n---\n"),
        ],
    )
    .unwrap_err();

    assert_eq!(error.code, "use.okf.limit_exceeded");
}

#[test]
fn contract_rejects_observation_drift() {
    let contract = OkfBundleContract::from_json(CONTRACT).unwrap();
    let mut inspection = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        contract.limits.clone(),
        fixture_files(),
    )
    .unwrap();
    inspection.content_digest = format!("sha256:{}", "f".repeat(64));

    let error = contract.verify_inspection(&inspection).unwrap_err();
    assert_eq!(error.code, "use.okf.contract_mismatch");
}

#[test]
fn deterministic_identity_is_order_independent_and_content_sensitive() {
    let limits = OkfBundleLimits::default();
    let first =
        inspect_okf_bundle(OkfFormatVersion::V0_2, limits.clone(), fixture_files()).unwrap();
    let mut reversed = fixture_files();
    reversed.reverse();
    let second = inspect_okf_bundle(OkfFormatVersion::V0_2, limits.clone(), reversed).unwrap();
    assert_eq!(first, second);

    let mut changed = fixture_files();
    changed
        .iter_mut()
        .find(|file| file.path.ends_with("revenue.py"))
        .unwrap()
        .content
        .extend_from_slice(b"\n# changed\n");
    let changed = inspect_okf_bundle(OkfFormatVersion::V0_2, limits, changed).unwrap();
    assert_ne!(first.content_digest, changed.content_digest);
}

#[test]
fn rejects_reserved_file_drift_encoded_escape_and_unsafe_uri_schemes() {
    let version_mismatch = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        OkfBundleLimits::default(),
        vec![
            OkfBundleFile::new(
                "index.md",
                b"---\nokf_version: \"0.1\"\n---\n\n# Concepts\n",
            ),
            OkfBundleFile::new("concept.md", b"---\ntype: Metric\n---\n"),
        ],
    )
    .unwrap_err();
    assert_eq!(version_mismatch.code, "use.okf.bundle_invalid");

    let non_root_index = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        OkfBundleLimits::default(),
        vec![
            OkfBundleFile::new("concept.md", b"---\ntype: Metric\n---\n"),
            OkfBundleFile::new(
                "nested/index.md",
                b"---\nokf_version: \"0.2\"\n---\n\n# Nested\n",
            ),
        ],
    )
    .unwrap_err();
    assert_eq!(non_root_index.code, "use.okf.bundle_invalid");

    let invalid_log = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        OkfBundleLimits::default(),
        vec![
            OkfBundleFile::new("concept.md", b"---\ntype: Metric\n---\n"),
            OkfBundleFile::new(
                "log.md",
                b"# History\n\n## 2026-07-30\n\n- Older\n\n## 2026-07-31\n\n- Newer\n",
            ),
        ],
    )
    .unwrap_err();
    assert_eq!(invalid_log.code, "use.okf.bundle_invalid");

    let log_code_fence = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        OkfBundleLimits::default(),
        vec![
            OkfBundleFile::new("concept.md", b"---\ntype: Metric\n---\n"),
            OkfBundleFile::new(
                "log.md",
                b"# History\n\n```markdown\n## not-a-date\n```\n\n## 2026-07-31\n\n- Current\n",
            ),
        ],
    )
    .unwrap();
    assert!(log_code_fence.diagnostics.is_empty());

    for target in ["%2e%2e/outside.md", "javascript:alert(1)"] {
        let content = format!("---\ntype: Metric\n---\n\n[unsafe]({target})\n");
        let error = inspect_okf_bundle(
            OkfFormatVersion::V0_2,
            OkfBundleLimits::default(),
            vec![OkfBundleFile::new("concept.md", content)],
        )
        .unwrap_err();
        assert!(matches!(
            error.code.as_str(),
            "use.okf.path_escape" | "use.okf.bundle_invalid"
        ));
    }

    let windows_path = inspect_okf_bundle(
        OkfFormatVersion::V0_2,
        OkfBundleLimits::default(),
        vec![OkfBundleFile::new(
            "concept.md",
            b"---\ntype: Metric\n---\n\n[unsafe](C:\\outside.md)\n",
        )],
    )
    .unwrap_err();
    assert_eq!(windows_path.code, "use.okf.path_escape");
}

#[test]
fn contract_json_fails_closed_on_unknown_or_noncanonical_evidence() {
    let mut unknown: serde_json::Value = serde_json::from_slice(CONTRACT).unwrap();
    unknown["executor"] = serde_json::json!("ambient");
    let error = OkfBundleContract::from_json(&serde_json::to_vec(&unknown).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.okf.contract_invalid");

    let mut escaping: serde_json::Value = serde_json::from_slice(CONTRACT).unwrap();
    escaping["root"] = serde_json::json!("../knowledge");
    let error = OkfBundleContract::from_json(&serde_json::to_vec(&escaping).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.okf.path_escape");

    let mut oversized: serde_json::Value = serde_json::from_slice(CONTRACT).unwrap();
    oversized["limits"]["maxFiles"] = serde_json::json!(1);
    let error = OkfBundleContract::from_json(&serde_json::to_vec(&oversized).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.okf.contract_invalid");
}
