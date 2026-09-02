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
}

#[test]
fn catalog_is_immutable_generation_bound_and_revision_checked() {
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
fn public_gateway_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<InvocationRef>();
    assert_send_sync::<ArtifactRef>();
    assert_send_sync::<EndpointRef>();
    assert_send_sync::<CapabilityDescriptor>();
    assert_send_sync::<CapabilityGatewayCatalog>();
}
