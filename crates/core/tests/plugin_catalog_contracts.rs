use a3s_use_core::{
    CatalogPlanningTarget, CatalogSurface, ExecutablePlanningSurface, InstalledPluginPlanEvidence,
    McpReleaseDescriptor, OkfBundleContract, PlanPackageRole, PlanningArtifactRef,
    PlanningSurfaceActivation, PluginCatalogRecord, PluginPermissionCeiling, PluginPlanSource,
    PluginPlanningBundle, PluginSurfaceKind, PluginSurfaceRef, ToolReleaseDescriptor,
    ToolWorkloadClass, VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
    INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA, PLUGIN_CATALOG_SCHEMA_V2, PLUGIN_CATALOG_SCHEMA_V3,
    PLUGIN_PLANNING_BUNDLE_SCHEMA,
};
use sha2::{Digest, Sha256};

const PERMISSION_CEILING: &[u8] = include_bytes!("../fixtures/plugins/permission-ceiling-v1.json");
const CATALOG_RECORD: &[u8] = include_bytes!("../fixtures/plugins/catalog-record-v1.json");
const OKF_CATALOG_RECORD: &[u8] = include_bytes!("../fixtures/plugins/catalog-record-okf-v3.json");
const OKF_CATALOG_RECORD_DIGEST: &str =
    include_str!("../fixtures/plugins/catalog-record-okf-v3.sha256").trim_ascii_end();
const COMPLETE_PACKAGE_CATALOG: &[u8] =
    include_bytes!("../fixtures/plugins/complete-package-catalog-v1.json");
const COMPLETE_PACKAGE_CATALOG_DIGEST: &str =
    include_str!("../fixtures/plugins/complete-package-catalog-v1.sha256").trim_ascii_end();
const PERMISSION_DIGEST: &str =
    include_str!("../fixtures/plugins/permission-ceiling-v1.sha256").trim_ascii_end();
const CATALOG_DIGEST: &str =
    include_str!("../fixtures/plugins/catalog-record-v1.sha256").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

#[test]
fn canonical_plugin_contract_fixtures_have_cross_sdk_digests() {
    let ceiling = PluginPermissionCeiling::from_json(PERMISSION_CEILING).unwrap();
    let catalog = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();

    assert_eq!(
        ceiling.canonical_bytes().unwrap(),
        canonical_fixture(PERMISSION_CEILING)
    );
    assert_eq!(
        catalog.canonical_bytes().unwrap(),
        canonical_fixture(CATALOG_RECORD)
    );
    assert_eq!(ceiling.descriptor_digest().unwrap(), PERMISSION_DIGEST);
    assert_eq!(catalog.descriptor_digest().unwrap(), CATALOG_DIGEST);

    assert_eq!(ceiling.surfaces.len(), 4);
    assert_eq!(ceiling.surfaces[0].surface.kind, PluginSurfaceKind::Mcp);
    assert!(ceiling.surfaces[1].native_execution);
    assert_eq!(ceiling.surfaces[3].ui_http[0].tool_id, "index");
    assert_eq!(catalog.package_id, "acme/research");
    assert_eq!(catalog.surfaces.len(), 5);
    assert_eq!(catalog.surfaces[2].workload, Some(ToolWorkloadClass::Task));
    assert_eq!(
        catalog.permission_ceiling_digest,
        catalog.permission_ceiling.descriptor_digest().unwrap()
    );

    let reordered = serde_json::to_vec_pretty(
        &serde_json::from_slice::<serde_json::Value>(CATALOG_RECORD).unwrap(),
    )
    .unwrap();
    assert_eq!(
        PluginCatalogRecord::from_json(&reordered)
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        CATALOG_DIGEST
    );
}

#[test]
fn permission_ceiling_rejects_ambient_or_unscoped_authority() {
    let mut unsafe_path: serde_json::Value = serde_json::from_slice(PERMISSION_CEILING).unwrap();
    unsafe_path["surfaces"][1]["filesystem"][0]["path"] = serde_json::json!("/etc/passwd");
    assert_eq!(
        PluginPermissionCeiling::from_json(&serde_json::to_vec(&unsafe_path).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.permission_invalid"
    );

    let mut ambient_ui: serde_json::Value = serde_json::from_slice(PERMISSION_CEILING).unwrap();
    ambient_ui["surfaces"][3]["nativeExecution"] = serde_json::json!(true);
    assert_eq!(
        PluginPermissionCeiling::from_json(&serde_json::to_vec(&ambient_ui).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.permission_invalid"
    );

    let mut missing_resources: serde_json::Value =
        serde_json::from_slice(PERMISSION_CEILING).unwrap();
    missing_resources["surfaces"][1]
        .as_object_mut()
        .unwrap()
        .remove("resources");
    assert_eq!(
        PluginPermissionCeiling::from_json(&serde_json::to_vec(&missing_resources).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.permission_invalid"
    );
}

#[test]
fn catalog_record_binds_permissions_surfaces_and_archive() {
    let mut changed: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    changed["permissionCeiling"]["surfaces"][1]["nativeExecution"] = serde_json::json!(false);
    assert_eq!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&changed).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.catalog_invalid"
    );

    let mut unsorted: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    unsorted["surfaces"].as_array_mut().unwrap().swap(0, 1);
    assert_eq!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&unsorted).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.catalog_invalid"
    );

    let mut public_service: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    public_service["permissionCeiling"]["surfaces"][2]["privateService"] = serde_json::json!(false);
    let permissions: PluginPermissionCeiling =
        serde_json::from_value(public_service["permissionCeiling"].clone()).unwrap();
    public_service["permissionCeilingDigest"] =
        serde_json::json!(permissions.descriptor_digest().unwrap());
    assert_eq!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&public_service).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.catalog_invalid"
    );
}

#[test]
fn catalog_v2_binds_manifest_and_resolves_only_the_surface_dependency_closure() {
    let mut value: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    value["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    value["package"]["manifestSha256"] = serde_json::json!(format!("sha256:{}", "c".repeat(64)));
    for surface in value["surfaces"].as_array_mut().unwrap() {
        surface["optional"] = serde_json::json!(true);
    }
    value["surfaces"][1]["requires"] = serde_json::json!([
        {"kind": "tool", "id": "convert"}
    ]);
    value["surfaces"][4]["requires"] = serde_json::json!([
        {"kind": "skill", "id": "review"},
        {"kind": "tool", "id": "index"}
    ]);
    let catalog = PluginCatalogRecord::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        catalog.descriptor_digest().unwrap(),
        "sha256:3b2bbd9a4dbd0c1e16468cf4a5c971ee83fabc721d116439e76e5ab759df90ef"
    );
    let resolved = catalog
        .resolve_surfaces(&[PluginSurfaceRef {
            kind: PluginSurfaceKind::Ui,
            id: "review".to_string(),
        }])
        .unwrap();

    assert_eq!(
        resolved
            .iter()
            .map(|surface| surface.reference())
            .collect::<Vec<_>>(),
        vec![
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Skill,
                id: "review".to_string(),
            },
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_string(),
            },
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "index".to_string(),
            },
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Ui,
                id: "review".to_string(),
            },
        ]
    );
    let requested = PluginSurfaceRef {
        kind: PluginSurfaceKind::Ui,
        id: "review".to_string(),
    };
    assert!(catalog
        .resolve_surfaces(&[requested.clone(), requested])
        .is_err());
    assert!(catalog
        .resolve_surfaces(&[PluginSurfaceRef {
            kind: PluginSurfaceKind::Skill,
            id: "missing".to_string(),
        }])
        .is_err());
}

#[test]
fn catalog_versions_fail_closed_across_new_evidence_fields() {
    let mut missing_manifest: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    missing_manifest["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    assert!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&missing_manifest).unwrap()).is_err()
    );

    let mut v1_dependency: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    v1_dependency["surfaces"][1]["requires"] = serde_json::json!([
        {"kind": "tool", "id": "convert"}
    ]);
    assert!(PluginCatalogRecord::from_json(&serde_json::to_vec(&v1_dependency).unwrap()).is_err());

    let mut forbidden_back_edge: serde_json::Value =
        serde_json::from_slice(CATALOG_RECORD).unwrap();
    forbidden_back_edge["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    forbidden_back_edge["package"]["manifestSha256"] =
        serde_json::json!(format!("sha256:{}", "c".repeat(64)));
    forbidden_back_edge["surfaces"][1]["requires"] = serde_json::json!([
        {"kind": "tool", "id": "convert"}
    ]);
    forbidden_back_edge["surfaces"][2]["requires"] = serde_json::json!([
        {"kind": "skill", "id": "review"}
    ]);
    assert!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&forbidden_back_edge).unwrap()).is_err()
    );

    let mut missing_package_digest: serde_json::Value =
        serde_json::from_slice(CATALOG_RECORD).unwrap();
    missing_package_digest["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    missing_package_digest["package"]["manifestSha256"] =
        serde_json::json!(format!("sha256:{}", "c".repeat(64)));
    missing_package_digest["package"]
        .as_object_mut()
        .unwrap()
        .remove("sha256");
    assert!(
        PluginCatalogRecord::from_json(&serde_json::to_vec(&missing_package_digest).unwrap())
            .is_err()
    );

    let mut v2_flow: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    v2_flow["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    v2_flow["package"]["manifestSha256"] = serde_json::json!(format!("sha256:{}", "c".repeat(64)));
    v2_flow["surfaces"].as_array_mut().unwrap().insert(
        0,
        serde_json::json!({
            "kind": "flow",
            "id": "review-flow",
            "optional": true,
            "requires": []
        }),
    );
    assert!(PluginCatalogRecord::from_json(&serde_json::to_vec(&v2_flow).unwrap()).is_err());
}

#[test]
fn catalog_v3_binds_one_small_deterministic_planning_target() {
    let mut value = serde_json::to_value(plan_ready_catalog().record).unwrap();
    value["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V3);
    value["planning"] = serde_json::json!({
        "targetName":
            "extensions/acme/research/2.0.0/stable/linux-x86_64/planning-v1.json",
        "length": 4096,
        "sha256": format!("sha256:{}", "d".repeat(64))
    });
    let catalog = PluginCatalogRecord::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();

    assert!(catalog.is_package_plan_ready());
    assert_eq!(
        catalog.planning,
        Some(CatalogPlanningTarget {
            target_name: "extensions/acme/research/2.0.0/stable/linux-x86_64/planning-v1.json"
                .to_owned(),
            length: 4096,
            sha256: format!("sha256:{}", "d".repeat(64)),
        })
    );

    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove("planning");
    assert!(PluginCatalogRecord::from_json(&serde_json::to_vec(&missing).unwrap()).is_err());

    let mut wrong_path = value.clone();
    wrong_path["planning"]["targetName"] =
        serde_json::json!("extensions/acme/research/2.0.0/stable/linux-x86_64/other.json");
    assert!(PluginCatalogRecord::from_json(&serde_json::to_vec(&wrong_path).unwrap()).is_err());

    let mut oversized = value.clone();
    oversized["planning"]["length"] = serde_json::json!(512 * 1024 + 1);
    assert!(PluginCatalogRecord::from_json(&serde_json::to_vec(&oversized).unwrap()).is_err());

    value["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    assert!(PluginCatalogRecord::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn catalog_v3_binds_okf_and_skill_dependency_closure_without_runtime_authority() {
    let bundle =
        OkfBundleContract::from_json(include_bytes!("../fixtures/okf/bundle-contract-v1.json"))
            .unwrap();
    let mut record = plan_ready_catalog().record;
    record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    record.planning = Some(CatalogPlanningTarget {
        target_name: "extensions/acme/research/2.0.0/stable/linux-x86_64/planning-v1.json"
            .to_owned(),
        length: 4096,
        sha256: format!("sha256:{}", "d".repeat(64)),
    });
    record.surfaces.insert(
        1,
        CatalogSurface {
            kind: PluginSurfaceKind::Okf,
            id: "domain-knowledge".to_owned(),
            optional: true,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: Some(bundle.clone()),
            requires: Vec::new(),
        },
    );
    record.surfaces[2].requires.insert(
        0,
        PluginSurfaceRef {
            kind: PluginSurfaceKind::Okf,
            id: "domain-knowledge".to_owned(),
        },
    );
    record.validate().unwrap();

    let selected = record
        .resolve_surfaces(&[PluginSurfaceRef {
            kind: PluginSurfaceKind::Skill,
            id: "review".to_owned(),
        }])
        .unwrap();
    assert!(selected.iter().any(|surface| {
        surface.kind == PluginSurfaceKind::Okf
            && surface.id == "domain-knowledge"
            && surface.okf_bundle.as_ref() == Some(&bundle)
    }));

    let mut legacy = record.clone();
    legacy.schema = PLUGIN_CATALOG_SCHEMA_V2.to_owned();
    legacy.planning = None;
    assert!(legacy.validate().is_err());

    let mut permission = record.permission_ceiling.clone();
    let mut unauthorized = permission.surfaces[0].clone();
    unauthorized.surface = PluginSurfaceRef {
        kind: PluginSurfaceKind::Okf,
        id: "domain-knowledge".to_owned(),
    };
    permission.surfaces.insert(1, unauthorized);
    assert!(permission.validate().is_err());
}

#[test]
fn catalog_v3_resolves_flow_between_okf_and_skill_ui_consumers() {
    let mut record = okf_only_catalog();
    for surface in &mut record.surfaces {
        surface.optional = true;
    }
    record.surfaces.insert(
        0,
        CatalogSurface {
            kind: PluginSurfaceKind::Flow,
            id: "review".to_owned(),
            optional: true,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires: vec![PluginSurfaceRef {
                kind: PluginSurfaceKind::Okf,
                id: "domain-knowledge".to_owned(),
            }],
        },
    );
    record.surfaces[2].requires = vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Flow,
        id: "review".to_owned(),
    }];
    record.surfaces.push(CatalogSurface {
        kind: PluginSurfaceKind::Ui,
        id: "review".to_owned(),
        optional: true,
        workload: None,
        mcp_transport: None,
        mcp_tool_count: None,
        okf_bundle: None,
        requires: vec![
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Flow,
                id: "review".to_owned(),
            },
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Skill,
                id: "research".to_owned(),
            },
        ],
    });
    record.validate().unwrap();

    let selected = record
        .resolve_surfaces(&[PluginSurfaceRef {
            kind: PluginSurfaceKind::Ui,
            id: "review".to_owned(),
        }])
        .unwrap();
    assert_eq!(
        selected
            .iter()
            .map(CatalogSurface::reference)
            .collect::<Vec<_>>(),
        record
            .surfaces
            .iter()
            .map(CatalogSurface::reference)
            .collect::<Vec<_>>()
    );

    let mut invalid = record;
    invalid.surfaces[0].requires = vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Skill,
        id: "research".to_owned(),
    }];
    assert!(invalid.validate().is_err());
}

#[test]
fn okf_only_catalog_v3_does_not_invent_an_executable_planning_target() {
    let record = okf_only_catalog();
    assert_eq!(
        record.canonical_bytes().unwrap(),
        canonical_fixture(OKF_CATALOG_RECORD)
    );
    assert_eq!(
        record.descriptor_digest().unwrap(),
        OKF_CATALOG_RECORD_DIGEST
    );
    assert_eq!(
        PluginCatalogRecord::from_json(OKF_CATALOG_RECORD).unwrap(),
        record
    );
    let state = VerifiedPluginCatalogRecord::new(
        record.clone(),
        VerifiedCatalogProvenance {
            registry_name: "official".to_owned(),
            registry_url: "https://plugins.a3s.dev/catalog".to_owned(),
            root_sha256: format!("sha256:{}", "f".repeat(64)),
            root_version: 7,
            timestamp_version: 42,
            snapshot_version: 41,
            targets_version: 39,
            catalog_record_digest: record.descriptor_digest().unwrap(),
        },
    )
    .unwrap()
    .selected_state(&[])
    .unwrap();
    assert!(state.permissions.surfaces.is_empty());
    assert_eq!(
        state.release.surfaces[0].okf_bundle.as_ref().unwrap().root,
        "okf/domain-knowledge"
    );
    assert_eq!(
        state.release.surfaces[1].requires[0].kind,
        PluginSurfaceKind::Okf
    );
}

#[test]
fn planning_bundle_binds_release_backed_executables_to_catalog_v3() {
    let bundle = planning_bundle();
    let bytes = bundle.canonical_bytes().unwrap();
    let catalog = catalog_v3_for_planning_target(&bytes);
    let parsed = PluginPlanningBundle::from_catalog_target(&bytes, &catalog).unwrap();

    assert_eq!(parsed, bundle);
    assert_eq!(parsed.surfaces.len(), 3);
    assert!(parsed.descriptor_digest().unwrap().starts_with("sha256:"));

    let mut changed_bytes = bytes.clone();
    changed_bytes.push(b'\n');
    assert!(PluginPlanningBundle::from_catalog_target(&changed_bytes, &catalog).is_err());

    let mut incomplete = bundle.clone();
    incomplete.surfaces.pop();
    let incomplete_bytes = incomplete.canonical_bytes().unwrap();
    let incomplete_catalog = catalog_v3_for_planning_target(&incomplete_bytes);
    assert!(
        PluginPlanningBundle::from_catalog_target(&incomplete_bytes, &incomplete_catalog).is_err()
    );
}

#[test]
fn planning_bundle_rejects_mutable_artifacts_and_unsupported_catalog_surfaces() {
    let mut bundle = planning_bundle();
    let ExecutablePlanningSurface::McpService { artifact, .. } = &mut bundle.surfaces[0] else {
        panic!("first planning surface should be MCP");
    };
    artifact.uri = "oci://registry.example/acme/library:latest".to_owned();
    assert!(bundle.validate().is_err());

    let mut bundle = planning_bundle();
    let initial_bytes = bundle.canonical_bytes().unwrap();
    let mut catalog = catalog_v3_for_planning_target(&initial_bytes);
    catalog.record.surfaces[0].mcp_transport = Some(a3s_use_core::CatalogMcpTransport::Stdio);
    catalog.record.permission_ceiling.surfaces[0].native_execution = true;
    catalog.record.permission_ceiling.surfaces[0].private_service = false;
    catalog.record.permission_ceiling_digest = catalog
        .record
        .permission_ceiling
        .descriptor_digest()
        .unwrap();
    bundle.permission_ceiling_digest = catalog.record.permission_ceiling_digest.clone();
    let bytes = bundle.canonical_bytes().unwrap();
    let planning = catalog.record.planning.as_mut().unwrap();
    planning.length = bytes.len() as u64;
    planning.sha256 = format!("sha256:{:x}", Sha256::digest(&bytes));
    catalog.provenance.catalog_record_digest = catalog.record.descriptor_digest().unwrap();
    assert!(catalog.validate().is_ok());
    assert!(PluginPlanningBundle::from_catalog_target(&bytes, &catalog).is_err());
}

#[test]
fn verified_catalog_v2_derives_a_plan_ready_selected_install_transition() {
    let verified = plan_ready_catalog();
    let transition = verified
        .install_transition(
            PlanPackageRole::Root,
            &[PluginSurfaceRef {
                kind: PluginSurfaceKind::Ui,
                id: "review".to_string(),
            }],
        )
        .unwrap();
    let after = transition.after.as_ref().unwrap();

    assert_eq!(after.release.surfaces.len(), 4);
    assert_eq!(after.permissions.surfaces.len(), 3);
    assert!(after
        .release
        .surfaces
        .iter()
        .all(|surface| surface.id != "library"));
    assert_eq!(
        after.release.permission_ceiling_digest,
        after.permissions.descriptor_digest().unwrap()
    );
    assert!(matches!(
        transition.source,
        Some(PluginPlanSource::Registry { .. })
    ));
}

#[test]
fn verified_catalog_v2_derives_remove_and_replace_from_observed_surfaces() {
    let installed = plan_ready_catalog();
    let active_surfaces = [PluginSurfaceRef {
        kind: PluginSurfaceKind::Ui,
        id: "review".to_owned(),
    }];
    let removal = installed
        .remove_transition(PlanPackageRole::Root, &active_surfaces)
        .unwrap();
    assert!(removal.after.is_none());
    assert_eq!(removal.before.as_ref().unwrap().release.surfaces.len(), 4);
    assert_eq!(removal.surfaces.len(), 4);
    assert!(removal.source.is_none());

    let mut candidate_record = installed.record.clone();
    candidate_record.version = "2.1.0".to_owned();
    candidate_record.archive.target_name = candidate_record
        .archive
        .target_name
        .replace("/2.0.0/", "/2.1.0/")
        .replace("-2.0.0-", "-2.1.0-");
    candidate_record.validate().unwrap();
    let candidate = VerifiedPluginCatalogRecord::new(
        candidate_record.clone(),
        VerifiedCatalogProvenance {
            catalog_record_digest: candidate_record.descriptor_digest().unwrap(),
            ..installed.provenance.clone()
        },
    )
    .unwrap();
    let replacement = candidate
        .replace_transition(
            &installed,
            PlanPackageRole::Root,
            &active_surfaces,
            &[PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_owned(),
            }],
        )
        .unwrap();

    assert_eq!(
        replacement.before.as_ref().unwrap().release.version,
        "2.0.0"
    );
    assert_eq!(replacement.after.as_ref().unwrap().release.version, "2.1.0");
    assert_eq!(
        replacement.after.as_ref().unwrap().release.surfaces.len(),
        1
    );
    assert!(matches!(
        replacement.source,
        Some(PluginPlanSource::Registry { .. })
    ));
}

#[test]
fn installed_plan_evidence_binds_catalog_receipt_and_capability_state() {
    let evidence = InstalledPluginPlanEvidence {
        schema: INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA.to_owned(),
        component_id: "use/acme/research".to_owned(),
        package_id: "acme/research".to_owned(),
        version: "2.0.0".to_owned(),
        capability_generation: 19,
        capability_revision: "a".repeat(64),
        receipt_digest: format!("sha256:{}", "b".repeat(64)),
        desired_enabled: true,
        selected_surfaces: vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_owned(),
        }],
        verified_catalog: plan_ready_catalog(),
    };

    evidence.validate().unwrap();
    let decoded =
        InstalledPluginPlanEvidence::from_json(&evidence.canonical_bytes().unwrap()).unwrap();
    assert_eq!(decoded, evidence);
    assert!(evidence.descriptor_digest().unwrap().starts_with("sha256:"));
}

#[test]
fn installed_plan_evidence_rejects_catalog_or_capability_drift() {
    let mut evidence = InstalledPluginPlanEvidence {
        schema: INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA.to_owned(),
        component_id: "use/acme/research".to_owned(),
        package_id: "acme/research".to_owned(),
        version: "2.0.0".to_owned(),
        capability_generation: 19,
        capability_revision: "a".repeat(64),
        receipt_digest: format!("sha256:{}", "b".repeat(64)),
        desired_enabled: true,
        selected_surfaces: vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_owned(),
        }],
        verified_catalog: plan_ready_catalog(),
    };

    evidence.capability_revision = "A".repeat(64);
    assert!(evidence.validate().is_err());
    evidence.capability_revision = "a".repeat(64);
    evidence.verified_catalog.record.version = "2.1.0".to_owned();
    assert!(evidence.validate().is_err());
}

fn planning_bundle() -> PluginPlanningBundle {
    let catalog = plan_ready_catalog();
    let mcp =
        McpReleaseDescriptor::from_json(include_bytes!("../fixtures/releases/mcp-release-v1.json"))
            .unwrap();
    let task = ToolReleaseDescriptor::from_json(include_bytes!(
        "../fixtures/releases/tool-task-release-v1.json"
    ))
    .unwrap();
    let service = ToolReleaseDescriptor::from_json(include_bytes!(
        "../fixtures/releases/tool-service-release-v1.json"
    ))
    .unwrap();
    PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
        package_id: catalog.record.package_id,
        version: catalog.record.version,
        channel: catalog.record.channel,
        target: catalog.record.target,
        archive_sha256: catalog.record.archive.sha256,
        package_sha256: catalog.record.package.sha256.unwrap(),
        manifest_sha256: catalog.record.package.manifest_sha256.unwrap(),
        permission_ceiling_digest: catalog.record.permission_ceiling_digest,
        surfaces: vec![
            ExecutablePlanningSurface::McpService {
                id: "library".to_owned(),
                activation: PlanningSurfaceActivation::Eager,
                artifact: planning_artifact(
                    "library",
                    &mcp.artifact.digest,
                    &mcp.artifact.media_type,
                ),
                descriptor: mcp,
            },
            ExecutablePlanningSurface::ToolTask {
                id: "convert".to_owned(),
                activation: PlanningSurfaceActivation::Lazy,
                command: "acme-convert".to_owned(),
                json_output: true,
                timeout_ms: 120_000,
                artifact: planning_artifact(
                    "convert",
                    &task.artifact.digest,
                    &task.artifact.media_type,
                ),
                descriptor: task,
            },
            ExecutablePlanningSurface::ToolService {
                id: "index".to_owned(),
                activation: PlanningSurfaceActivation::Eager,
                base_path: "/api".to_owned(),
                artifact: planning_artifact(
                    "index",
                    &service.artifact.digest,
                    &service.artifact.media_type,
                ),
                descriptor: service,
            },
        ],
    }
}

fn planning_artifact(name: &str, digest: &str, media_type: &str) -> PlanningArtifactRef {
    PlanningArtifactRef {
        uri: format!("oci://registry.example/acme/{name}@{digest}"),
        digest: digest.to_owned(),
        media_type: media_type.to_owned(),
    }
}

fn catalog_v3_for_planning_target(bytes: &[u8]) -> VerifiedPluginCatalogRecord {
    let mut catalog = plan_ready_catalog();
    catalog.record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    catalog.record.planning = Some(CatalogPlanningTarget {
        target_name: "extensions/acme/research/2.0.0/stable/linux-x86_64/planning-v1.json"
            .to_owned(),
        length: bytes.len() as u64,
        sha256: format!("sha256:{:x}", Sha256::digest(bytes)),
    });
    catalog.provenance.catalog_record_digest = catalog.record.descriptor_digest().unwrap();
    catalog.validate().unwrap();
    catalog
}

fn plan_ready_catalog() -> VerifiedPluginCatalogRecord {
    let mut value: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    value["schema"] = serde_json::json!(PLUGIN_CATALOG_SCHEMA_V2);
    value["package"]["manifestSha256"] = serde_json::json!(format!("sha256:{}", "c".repeat(64)));
    for surface in value["surfaces"].as_array_mut().unwrap() {
        surface["optional"] = serde_json::json!(true);
    }
    value["surfaces"][1]["requires"] = serde_json::json!([
        {"kind": "tool", "id": "convert"}
    ]);
    value["surfaces"][4]["requires"] = serde_json::json!([
        {"kind": "skill", "id": "review"},
        {"kind": "tool", "id": "index"}
    ]);
    let record = PluginCatalogRecord::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "official".to_owned(),
        registry_url: "https://plugins.a3s.dev/catalog".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 7,
        timestamp_version: 42,
        snapshot_version: 41,
        targets_version: 39,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    VerifiedPluginCatalogRecord::new(record, provenance).unwrap()
}

fn okf_only_catalog() -> PluginCatalogRecord {
    let mut record = plan_ready_catalog().record;
    record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    record.package_id = "acme/knowledge".to_owned();
    record.display_name = "A3S Knowledge Pack".to_owned();
    record.description =
        "Cited package knowledge with an exact promoted OKF generation.".to_owned();
    record.keywords = vec!["knowledge".to_owned(), "okf".to_owned()];
    record.categories = vec!["knowledge".to_owned()];
    record.version = "1.0.0".to_owned();
    record.planning = None;
    record.surfaces = vec![
        CatalogSurface {
            kind: PluginSurfaceKind::Okf,
            id: "domain-knowledge".to_owned(),
            optional: false,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: Some(
                OkfBundleContract::from_json(include_bytes!(
                    "../fixtures/okf/bundle-contract-v1.json"
                ))
                .unwrap(),
            ),
            requires: Vec::new(),
        },
        CatalogSurface {
            kind: PluginSurfaceKind::Skill,
            id: "research".to_owned(),
            optional: false,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires: vec![PluginSurfaceRef {
                kind: PluginSurfaceKind::Okf,
                id: "domain-knowledge".to_owned(),
            }],
        },
    ];
    record.permission_ceiling.surfaces.clear();
    record.permission_ceiling_digest = record.permission_ceiling.descriptor_digest().unwrap();
    record.archive.target_name =
        "extensions/acme/knowledge/1.0.0/stable/linux-x86_64/acme-knowledge-1.0.0-linux-x86_64.tar.gz"
            .to_owned();
    record.archive.length = 1_838;
    record.archive.sha256 =
        "sha256:b9bd4d35c77237ad6408ba09716fa1392ed71f34d6776f438bcb26c77e9fa0ac".to_owned();
    record.package.expanded_bytes = 3_366;
    record.package.file_count = 10;
    record.package.sha256 =
        Some("sha256:e030e6e8c3afa6383536b4973395e12b05257d2b7b10bf042f3bfaf71b420fe2".to_owned());
    record.package.manifest_sha256 =
        Some("sha256:16d065341edebaf45e0d633e46614155a60b853738290bbac8c5b62d9230bce2".to_owned());
    record.repository = "https://github.com/acme/knowledge".to_owned();
    record.validate().unwrap();
    record
}

#[test]
fn catalog_v1_remains_searchable_but_cannot_claim_plan_ready_evidence() {
    let record = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "official".to_owned(),
        registry_url: "https://plugins.a3s.dev/catalog".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 7,
        timestamp_version: 42,
        snapshot_version: 41,
        targets_version: 39,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    let error = verified
        .install_transition(PlanPackageRole::Root, &[])
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.catalog_plan_evidence_missing");
}

#[test]
fn privilege_bearing_unknown_fields_fail_closed_without_echo() {
    let secret_marker = "do-not-echo-super-secret";
    let mut permissions: serde_json::Value = serde_json::from_slice(PERMISSION_CEILING).unwrap();
    permissions["surfaces"][1]["environment"] = serde_json::json!({"TOKEN": secret_marker});
    let error =
        PluginPermissionCeiling::from_json(&serde_json::to_vec(&permissions).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.plugin.permission_invalid");
    assert!(!error.message.contains(secret_marker));

    let mut catalog: serde_json::Value = serde_json::from_slice(CATALOG_RECORD).unwrap();
    catalog["endpointUrl"] = serde_json::json!("https://public.example");
    let error = PluginCatalogRecord::from_json(&serde_json::to_vec(&catalog).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.plugin.catalog_invalid");
}

#[test]
fn verified_catalog_provenance_binds_outer_tuf_evidence_to_the_record() {
    let record = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "official".to_owned(),
        registry_url: "https://plugins.a3s.dev/catalog".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 7,
        timestamp_version: 42,
        snapshot_version: 41,
        targets_version: 39,
        catalog_record_digest: CATALOG_DIGEST.to_owned(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    verified.validate().unwrap();

    let mut drift = verified;
    drift.provenance.catalog_record_digest =
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned();
    assert!(drift.validate().is_err());
}

#[test]
fn verified_catalog_provenance_accepts_only_secure_or_loopback_registries() {
    let record = PluginCatalogRecord::from_json(CATALOG_RECORD).unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "fixture".to_owned(),
        registry_url: "http://127.0.0.1:43210/".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: CATALOG_DIGEST.to_owned(),
    };
    VerifiedPluginCatalogRecord::new(record.clone(), provenance).unwrap();

    let insecure = VerifiedCatalogProvenance {
        registry_name: "fixture".to_owned(),
        registry_url: "http://plugins.example/".to_owned(),
        root_sha256: "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .to_owned(),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: CATALOG_DIGEST.to_owned(),
    };
    assert!(VerifiedPluginCatalogRecord::new(record, insecure).is_err());
}

#[test]
fn complete_package_catalog_fixture_is_canonical() {
    let catalog = PluginCatalogRecord::from_json(COMPLETE_PACKAGE_CATALOG).unwrap();
    assert_eq!(
        catalog.canonical_bytes().unwrap(),
        canonical_fixture(COMPLETE_PACKAGE_CATALOG)
    );
    assert_eq!(
        catalog.descriptor_digest().unwrap(),
        COMPLETE_PACKAGE_CATALOG_DIGEST
    );
}
