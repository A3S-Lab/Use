use std::collections::BTreeMap;

use a3s_use_core::{
    ReleaseDependency, ReleaseKind, ReleaseResolution, ToolReleaseDescriptor, ToolServiceInterface,
    ToolServiceNetwork, ToolTaskInterface, ToolWorkloadContract,
};

const TASK_FIXTURE: &[u8] = include_bytes!("../fixtures/releases/tool-task-release-v1.json");
const SERVICE_FIXTURE: &[u8] = include_bytes!("../fixtures/releases/tool-service-release-v1.json");
const TASK_DESCRIPTOR_DIGEST: &str =
    include_str!("../fixtures/releases/tool-task-release-v1.sha256").trim_ascii_end();
const SERVICE_DESCRIPTOR_DIGEST: &str =
    include_str!("../fixtures/releases/tool-service-release-v1.sha256").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

#[test]
fn canonical_tool_fixtures_have_cross_sdk_digest_goldens() {
    let task = ToolReleaseDescriptor::from_json(TASK_FIXTURE).unwrap();
    let service = ToolReleaseDescriptor::from_json(SERVICE_FIXTURE).unwrap();

    assert_eq!(
        task.canonical_bytes().unwrap(),
        canonical_fixture(TASK_FIXTURE)
    );
    assert_eq!(
        service.canonical_bytes().unwrap(),
        canonical_fixture(SERVICE_FIXTURE)
    );
    assert_eq!(task.descriptor_digest().unwrap(), TASK_DESCRIPTOR_DIGEST);
    assert_eq!(
        service.descriptor_digest().unwrap(),
        SERVICE_DESCRIPTOR_DIGEST
    );

    assert!(matches!(
        &task.workload,
        ToolWorkloadContract::Task {
            interface: ToolTaskInterface::Cli,
            ..
        }
    ));
    assert!(matches!(
        &service.workload,
        ToolWorkloadContract::Service {
            interface: ToolServiceInterface::Http,
            network: ToolServiceNetwork::Private,
            ..
        }
    ));

    let reordered = serde_json::to_vec_pretty(
        &serde_json::from_slice::<serde_json::Value>(SERVICE_FIXTURE).unwrap(),
    )
    .unwrap();
    assert_eq!(
        ToolReleaseDescriptor::from_json(&reordered)
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        SERVICE_DESCRIPTOR_DIGEST
    );

    let mut with_tool_dependency = task;
    with_tool_dependency.dependencies.push(ReleaseDependency {
        kind: ReleaseKind::Tool,
        name: "a3s/helper-tool".to_string(),
        version: "1.0.0".to_string(),
        descriptor_digest:
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
    });
    with_tool_dependency.validate().unwrap();
}

#[test]
fn tool_workload_contracts_fail_closed() {
    let mut task: serde_json::Value = serde_json::from_slice(TASK_FIXTURE).unwrap();
    task["workload"]["endpointUrl"] = serde_json::json!("https://public.example");
    assert_eq!(
        ToolReleaseDescriptor::from_json(&serde_json::to_vec(&task).unwrap())
            .unwrap_err()
            .code,
        "use.release.descriptor_invalid"
    );

    let mut task = ToolReleaseDescriptor::from_json(TASK_FIXTURE).unwrap();
    let ToolWorkloadContract::Task { interactive, .. } = &mut task.workload else {
        panic!("fixture must describe a Tool Task");
    };
    *interactive = true;
    assert_eq!(
        task.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );

    let mut task = ToolReleaseDescriptor::from_json(TASK_FIXTURE).unwrap();
    let ToolWorkloadContract::Task {
        success_exit_codes, ..
    } = &mut task.workload
    else {
        panic!("fixture must describe a Tool Task");
    };
    success_exit_codes.push(0);
    assert_eq!(
        task.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );

    let mut task = ToolReleaseDescriptor::from_json(TASK_FIXTURE).unwrap();
    let ToolWorkloadContract::Task { entrypoint, .. } = &mut task.workload else {
        panic!("fixture must describe a Tool Task");
    };
    entrypoint.clear();
    assert_eq!(
        task.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );

    let mut service: serde_json::Value = serde_json::from_slice(SERVICE_FIXTURE).unwrap();
    service["workload"]["network"] = serde_json::json!("public");
    assert_eq!(
        ToolReleaseDescriptor::from_json(&serde_json::to_vec(&service).unwrap())
            .unwrap_err()
            .code,
        "use.release.descriptor_invalid"
    );

    let mut service = ToolReleaseDescriptor::from_json(SERVICE_FIXTURE).unwrap();
    let ToolWorkloadContract::Service {
        api_contract_digest,
        ..
    } = &mut service.workload
    else {
        panic!("fixture must describe a Tool Service");
    };
    *api_contract_digest =
        Some("sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string());
    assert_eq!(
        service.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );

    let mut service = ToolReleaseDescriptor::from_json(SERVICE_FIXTURE).unwrap();
    service.artifact.media_type = "application/vnd.a3s.skill.bundle.v1+tar+gzip".to_string();
    assert_eq!(
        service.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );
}

#[test]
fn tool_resolution_is_verified_before_runtime_mapping() {
    let service = ToolReleaseDescriptor::from_json(SERVICE_FIXTURE).unwrap();
    let mut resolution = ReleaseResolution {
        components: BTreeMap::from([
            ("a3s-runtime".to_string(), "0.2.0".to_string()),
            ("a3s-use".to_string(), "0.3.0".to_string()),
        ]),
        dependencies: Vec::new(),
    };
    service.verify_resolution(&resolution).unwrap();

    resolution.components.remove("a3s-runtime");
    assert_eq!(
        service.verify_resolution(&resolution).unwrap_err().code,
        "use.release.compatibility_missing"
    );
}

#[test]
fn tool_decode_diagnostics_do_not_echo_descriptor_values() {
    let secret_marker = "do-not-echo-super-secret";
    let mut task: serde_json::Value = serde_json::from_slice(TASK_FIXTURE).unwrap();
    task["workload"]["interface"] = serde_json::json!(secret_marker);

    let error = ToolReleaseDescriptor::from_json(&serde_json::to_vec(&task).unwrap()).unwrap_err();

    assert_eq!(error.code, "use.release.descriptor_invalid");
    assert!(!error.message.contains(secret_marker));
}
