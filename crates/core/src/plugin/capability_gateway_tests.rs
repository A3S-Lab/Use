use super::*;

fn package() -> super::super::PluginPackageId {
    super::super::PluginPackageId::parse("acme/assistant").unwrap()
}

fn surface(kind: PluginSurfaceKind, id: &str) -> PluginSurfaceRef {
    PluginSurfaceRef {
        kind,
        id: id.to_owned(),
    }
}

fn digest(letter: char) -> String {
    format!("sha256:{}", letter.to_string().repeat(64))
}

fn schema() -> Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "query": {"type": "string", "maxLength": 128}
        },
        "required": ["query"]
    })
}

fn tool() -> CapabilityDescriptor {
    let package = package();
    let surface = surface(PluginSurfaceKind::Tool, "search");
    CapabilityDescriptor {
        schema: CAPABILITY_DESCRIPTOR_SCHEMA_V1.to_owned(),
        invocation_ref: InvocationRef::derive(&package, &surface, 7, &digest('a')).unwrap(),
        artifact_ref: Some(ArtifactRef::derive(&package, &surface, 7, &digest('b')).unwrap()),
        endpoint_ref: Some(EndpointRef::derive(&package, &surface, 7, &digest('c')).unwrap()),
        package_id: package,
        surface,
        generation: 7,
        package_digest: digest('d'),
        manifest_digest: digest('e'),
        title: "Search".to_owned(),
        description: "Search verified knowledge.".to_owned(),
        dependencies: Vec::new(),
        publication: CapabilityPublicationEvidence {
            catalog_record_digest: digest('f'),
            signature_digest: digest('0'),
        },
        capability: CapabilityDescriptorKind::Tool {
            name: "search".to_owned(),
            input_schema: schema(),
            output_schema: schema(),
            annotations: CapabilityToolAnnotations::new(true, false, true, false),
        },
    }
}

fn resource() -> CapabilityDescriptor {
    let package = package();
    let surface = surface(PluginSurfaceKind::Skill, "knowledge");
    CapabilityDescriptor {
        schema: CAPABILITY_DESCRIPTOR_SCHEMA_V1.to_owned(),
        invocation_ref: InvocationRef::derive(&package, &surface, 7, &digest('a')).unwrap(),
        artifact_ref: None,
        endpoint_ref: None,
        package_id: package.clone(),
        surface: surface.clone(),
        generation: 7,
        package_digest: digest('d'),
        manifest_digest: digest('e'),
        title: "Knowledge index".to_owned(),
        description: "Verified knowledge resource.".to_owned(),
        dependencies: Vec::new(),
        publication: CapabilityPublicationEvidence {
            catalog_record_digest: digest('f'),
            signature_digest: digest('0'),
        },
        capability: CapabilityDescriptorKind::Resource {
            name: "knowledge".to_owned(),
            uri: ResourceRef::derive(&package, &surface, 7, &digest('1')).unwrap(),
            mime_type: Some("text/plain".to_owned()),
            size: Some(128),
        },
    }
}

fn prompt() -> CapabilityDescriptor {
    let package = package();
    let surface = surface(PluginSurfaceKind::Skill, "prompts");
    CapabilityDescriptor {
        schema: CAPABILITY_DESCRIPTOR_SCHEMA_V1.to_owned(),
        invocation_ref: InvocationRef::derive(&package, &surface, 7, &digest('a')).unwrap(),
        artifact_ref: None,
        endpoint_ref: None,
        package_id: package.clone(),
        surface: surface.clone(),
        generation: 7,
        package_digest: digest('d'),
        manifest_digest: digest('e'),
        title: "Research prompt".to_owned(),
        description: "Compose a bounded research request.".to_owned(),
        dependencies: Vec::new(),
        publication: CapabilityPublicationEvidence {
            catalog_record_digest: digest('f'),
            signature_digest: digest('0'),
        },
        capability: CapabilityDescriptorKind::Prompt {
            name: "research".to_owned(),
            arguments: vec![CapabilityPromptArgument::new("topic", true)],
        },
    }
}

#[test]
fn opaque_references_are_deterministic_and_domain_separated() {
    let package = package();
    let surface = surface(PluginSurfaceKind::Tool, "search");
    let invocation = InvocationRef::derive(&package, &surface, 1, &digest('a')).unwrap();
    assert_eq!(
        invocation,
        InvocationRef::derive(&package, &surface, 1, &digest('a')).unwrap()
    );
    assert_ne!(
        invocation.as_str(),
        ArtifactRef::derive(&package, &surface, 1, &digest('a'))
            .unwrap()
            .as_str()
    );
    assert!(InvocationRef::parse("/tmp/private").is_err());
}

#[test]
fn descriptor_round_trips_without_private_authority_fields() {
    let descriptor = tool();
    descriptor.validate().unwrap();
    let encoded = serde_json::to_value(&descriptor).unwrap();
    assert!(encoded.get("path").is_none());
    assert!(encoded.get("packageRoot").is_none());
    assert!(encoded.get("executable").is_none());
    assert!(encoded.get("secret").is_none());
    let decoded: CapabilityDescriptor = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, descriptor);
    assert_eq!(
        decoded.descriptor_digest().unwrap(),
        descriptor.descriptor_digest().unwrap()
    );

    let mut unknown = serde_json::to_value(&descriptor).unwrap();
    unknown["path"] = serde_json::json!("/private");
    assert!(serde_json::from_value::<CapabilityDescriptor>(unknown).is_err());
}

#[test]
fn descriptor_rejects_executable_only_and_external_schema_authority() {
    let mut descriptor = tool();
    descriptor.capability = CapabilityDescriptorKind::Tool {
        name: "bad/name".to_owned(),
        input_schema: schema(),
        output_schema: schema(),
        annotations: CapabilityToolAnnotations::new(false, true, false, true),
    };
    assert!(descriptor.validate().is_err());

    let mut descriptor = tool();
    descriptor.capability = CapabilityDescriptorKind::Tool {
        name: "search".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "$ref": "https://example.invalid/schema"
        }),
        output_schema: schema(),
        annotations: CapabilityToolAnnotations::new(true, false, true, false),
    };
    assert!(descriptor.validate().is_err());

    for reference_keyword in ["$dynamicRef", "$recursiveRef", "$id"] {
        let mut descriptor = tool();
        let mut input_schema = serde_json::Map::new();
        input_schema.insert("type".to_owned(), Value::String("object".to_owned()));
        input_schema.insert("additionalProperties".to_owned(), Value::Bool(false));
        input_schema.insert(
            reference_keyword.to_owned(),
            Value::String("https://example.invalid/schema".to_owned()),
        );
        descriptor.capability = CapabilityDescriptorKind::Tool {
            name: "search".to_owned(),
            input_schema: Value::Object(input_schema),
            output_schema: schema(),
            annotations: CapabilityToolAnnotations::new(true, false, true, false),
        };
        assert!(
            descriptor.validate().is_err(),
            "{reference_keyword} escaped"
        );
    }
}

#[test]
fn resources_and_prompts_round_trip_and_share_a_surface_generation() {
    let resource = resource();
    let prompt = prompt();
    resource.validate().unwrap();
    prompt.validate().unwrap();
    let resource_json = serde_json::to_value(&resource).unwrap();
    let prompt_json = serde_json::to_value(&prompt).unwrap();
    assert_eq!(
        serde_json::from_value::<CapabilityDescriptor>(resource_json).unwrap(),
        resource
    );
    assert_eq!(
        serde_json::from_value::<CapabilityDescriptor>(prompt_json).unwrap(),
        prompt
    );

    let installation =
        InstallationId::new(super::super::InstallationKind::User, "user/current").unwrap();
    let catalog = CapabilityGatewayCatalog::new(installation, 7, vec![resource, prompt]).unwrap();
    assert!(catalog
        .find_resource(catalog.descriptors()[0].resource_uri().unwrap().as_str())
        .is_some());
    assert!(catalog.find_prompt("research").is_some());
}

#[test]
fn resource_refs_remain_opaque_and_cross_kind_fields_are_rejected() {
    let descriptor = resource();
    let encoded = serde_json::to_value(&descriptor).unwrap();
    assert!(encoded["uri"]
        .as_str()
        .unwrap()
        .starts_with("resource:v1:sha256:"));
    assert!(ResourceRef::parse("https://example.invalid/private").is_err());
    let mut tampered = encoded;
    tampered["inputSchema"] = serde_json::json!({});
    assert!(serde_json::from_value::<CapabilityDescriptor>(tampered).is_err());
}

#[test]
fn catalog_is_immutable_publication_and_revision_checked() {
    let installation =
        InstallationId::new(super::super::InstallationKind::User, "user/current").unwrap();
    let first = tool();
    let mut second = tool();
    second.surface.id = "z-last".to_owned();
    if let CapabilityDescriptorKind::Tool { name, .. } = &mut second.capability {
        *name = "z-last".to_owned();
    }
    second.invocation_ref = InvocationRef::derive(
        &second.package_id,
        &second.surface,
        second.generation,
        &digest('a'),
    )
    .unwrap();
    let catalog =
        CapabilityGatewayCatalog::new(installation.clone(), 7, vec![second.clone(), first.clone()])
            .unwrap();
    assert_eq!(catalog.descriptors()[0].surface.id, "search");
    assert_eq!(catalog.find_tool("search").unwrap().surface.id, "search");
    let mut tampered = serde_json::to_value(&catalog).unwrap();
    tampered["generation"] = serde_json::json!(8);
    assert!(CapabilityGatewayCatalog::from_json(&serde_json::to_vec(&tampered).unwrap()).is_err());

    let mut foreign = catalog.clone();
    foreign.installation = InstallationId::new(
        super::super::InstallationKind::Workspace,
        "workspace/current",
    )
    .unwrap();
    assert!(foreign.validate().is_err());
}

#[test]
fn catalog_publication_generation_is_independent_from_package_lifecycle_generation() {
    let installation =
        InstallationId::new(super::super::InstallationKind::User, "user/current").unwrap();
    let first = tool();
    let mut second = tool();
    second.package_id = super::super::PluginPackageId::parse("acme/second").unwrap();
    second.surface = surface(PluginSurfaceKind::Tool, "convert");
    second.generation = 11;
    second.invocation_ref = InvocationRef::derive(
        &second.package_id,
        &second.surface,
        second.generation,
        &digest('a'),
    )
    .unwrap();
    second.artifact_ref = Some(
        ArtifactRef::derive(
            &second.package_id,
            &second.surface,
            second.generation,
            &digest('b'),
        )
        .unwrap(),
    );
    second.endpoint_ref = Some(
        EndpointRef::derive(
            &second.package_id,
            &second.surface,
            second.generation,
            &digest('c'),
        )
        .unwrap(),
    );
    if let CapabilityDescriptorKind::Tool { name, .. } = &mut second.capability {
        *name = "convert".to_owned();
    }

    let catalog = CapabilityGatewayCatalog::new(installation, 42, vec![second, first]).unwrap();
    assert_eq!(catalog.generation(), 42);
    assert_eq!(catalog.descriptors()[0].generation, 7);
    assert_eq!(catalog.descriptors()[1].generation, 11);
}

#[test]
fn catalog_rejects_two_lifecycle_generations_of_one_surface() {
    let installation =
        InstallationId::new(super::super::InstallationKind::User, "user/current").unwrap();
    let first = tool();
    let mut second = first.clone();
    second.generation = 8;
    second.invocation_ref = InvocationRef::derive(
        &second.package_id,
        &second.surface,
        second.generation,
        &digest('a'),
    )
    .unwrap();
    assert!(CapabilityGatewayCatalog::new(installation, 42, vec![first, second]).is_err());
}

#[test]
fn description_proof_binds_the_exact_agent_visible_descriptor() {
    let proof = CapabilityDescriptionProof::from_verified(tool(), "registry/official").unwrap();
    proof.validate().unwrap();
    let encoded = serde_json::to_vec(&proof).unwrap();
    let decoded = CapabilityDescriptionProof::from_json(&encoded).unwrap();
    assert_eq!(decoded, proof);

    let mut tampered = proof.clone();
    tampered.descriptor.title = "Different title".to_owned();
    assert!(tampered.validate().is_err());

    let mut tampered = proof;
    tampered.signer_id = "../untrusted".to_owned();
    assert!(tampered.validate().is_err());
}

#[test]
fn verified_descriptions_are_the_only_inputs_to_the_proof_catalog_constructor() {
    let installation =
        InstallationId::new(super::super::InstallationKind::User, "user/current").unwrap();
    let proof = CapabilityDescriptionProof::from_verified(tool(), "registry/official").unwrap();
    let catalog =
        CapabilityGatewayCatalog::from_verified_descriptions(installation.clone(), 7, vec![proof])
            .unwrap();
    assert_eq!(catalog.descriptors().len(), 1);

    let mut invalid =
        CapabilityDescriptionProof::from_verified(tool(), "registry/official").unwrap();
    invalid.descriptor_digest = digest('e');
    assert!(
        CapabilityGatewayCatalog::from_verified_descriptions(installation, 7, vec![invalid])
            .is_err()
    );
}

#[test]
fn public_gateway_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<InvocationRef>();
    assert_send_sync::<ArtifactRef>();
    assert_send_sync::<EndpointRef>();
    assert_send_sync::<ResourceRef>();
    assert_send_sync::<CapabilityPromptArgument>();
    assert_send_sync::<CapabilityDescriptionProof>();
    assert_send_sync::<CapabilityDescriptor>();
    assert_send_sync::<CapabilityGatewayCatalog>();
}
