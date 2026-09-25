// Capability gateway catalog encoding helpers (included into capability_gateway).

impl CapabilityGatewayCatalog {
    /// Build a canonical immutable catalog. Input descriptors are sorted by
    /// package, surface, and package lifecycle generation before the revision
    /// is allocated. `generation` is the publication generation, not a package
    /// lifecycle generation.
    pub fn new(
        installation: InstallationId,
        generation: u64,
        mut descriptors: Vec<CapabilityDescriptor>,
    ) -> UseResult<Self> {
        installation.validate()?;
        descriptors.sort_by(descriptor_order);
        let revision = catalog_revision(&installation, generation, &descriptors)?;
        let catalog = Self {
            schema: CAPABILITY_GATEWAY_CATALOG_SCHEMA_V1.to_owned(),
            installation,
            generation,
            revision,
            descriptors,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    /// Build a catalog only from descriptions that have crossed the host's
    /// signed-publication verification boundary.
    pub fn from_verified_descriptions(
        installation: InstallationId,
        generation: u64,
        proofs: Vec<CapabilityDescriptionProof>,
    ) -> UseResult<Self> {
        let descriptors = proofs
            .into_iter()
            .map(|proof| {
                proof.validate()?;
                Ok(proof.into_descriptor())
            })
            .collect::<UseResult<Vec<_>>>()?;
        Self::new(installation, generation, descriptors)
    }

    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "Capability Gateway catalog",
            CAPABILITY_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != CAPABILITY_GATEWAY_CATALOG_SCHEMA_V1
            || self.installation.validate().is_err()
            || self.descriptors.len() > MAX_CAPABILITY_DESCRIPTORS
            || !valid_sha256(&self.revision)
            || (self.generation == 0 && !self.descriptors.is_empty())
        {
            return Err(capability_error(
                "The Capability Gateway catalog identity or bounds are invalid.",
            ));
        }

        let mut identities = BTreeSet::new();
        let mut tool_names = BTreeSet::new();
        let mut mcp_server_names = BTreeSet::new();
        let mut resource_uris = BTreeSet::new();
        let mut prompt_names = BTreeSet::new();
        let mut surface_generations = BTreeMap::new();
        let mut previous = None;
        for descriptor in &self.descriptors {
            descriptor.validate()?;
            let identity = descriptor_identity_key(descriptor);
            if previous
                .as_ref()
                .is_some_and(|value| *value >= descriptor_order_key(descriptor))
                || !identities.insert(identity)
            {
                return Err(capability_error(
                    "Capability descriptors must be sorted and unique within one publication.",
                ));
            }
            let surface_key = descriptor_surface_key(descriptor);
            if surface_generations
                .insert(surface_key, descriptor.generation)
                .is_some_and(|generation| generation != descriptor.generation)
            {
                return Err(capability_error(
                    "A capability surface cannot publish multiple lifecycle generations in one catalog.",
                ));
            }
            if let Some(name) = descriptor.tool_name() {
                if !tool_names.insert(name) {
                    return Err(capability_error(
                        "Capability Gateway Tool names must be unique within one catalog.",
                    ));
                }
            }
            if let Some(name) = descriptor.mcp_server_name() {
                if !mcp_server_names.insert(name) {
                    return Err(capability_error(
                        "Capability Gateway MCP server names must be unique within one catalog.",
                    ));
                }
            }
            if let Some(uri) = descriptor.resource_uri() {
                if !resource_uris.insert(uri.as_str()) {
                    return Err(capability_error(
                        "Capability Gateway resource URIs must be unique within one catalog.",
                    ));
                }
            }
            if let Some(name) = descriptor.prompt_name() {
                if !prompt_names.insert(name) {
                    return Err(capability_error(
                        "Capability Gateway prompt names must be unique within one catalog.",
                    ));
                }
            }
            previous = Some(descriptor_order_key(descriptor));
        }

        let expected_revision =
            catalog_revision(&self.installation, self.generation, &self.descriptors)?;
        if self.revision != expected_revision {
            return Err(capability_error(
                "The Capability Gateway catalog revision does not match its immutable descriptors.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "Capability Gateway catalog", CAPABILITY_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn installation(&self) -> &InstallationId {
        &self.installation
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn descriptors(&self) -> &[CapabilityDescriptor] {
        &self.descriptors
    }

    pub fn find_tool(&self, name: &str) -> Option<&CapabilityDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.tool_name() == Some(name))
    }

    pub fn find_resource(&self, uri: &str) -> Option<&CapabilityDescriptor> {
        self.descriptors.iter().find(|descriptor| {
            descriptor
                .resource_uri()
                .is_some_and(|value| value.as_str() == uri)
        })
    }

    pub fn find_prompt(&self, name: &str) -> Option<&CapabilityDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.prompt_name() == Some(name))
    }

    /// Project this immutable publication for one already completed consumer
    /// negotiation. A descriptor that requires an extension the consumer did
    /// not explicitly accept is omitted from the projected catalog, including
    /// its invocation route. Rebuilding through [`Self::new`] gives the
    /// projection its own canonical revision and re-runs all catalog
    /// invariants instead of retaining a revision for a different view.
    pub fn for_consumer(&self, negotiation: &CapabilityConsumerNegotiation) -> UseResult<Self> {
        self.validate()?;
        negotiation.validate()?;
        let descriptors = self
            .descriptors
            .iter()
            .filter(|descriptor| {
                descriptor
                    .required_extensions()
                    .iter()
                    .all(|extension| negotiation.accepts(*extension))
            })
            .cloned()
            .collect();
        Self::new(self.installation.clone(), self.generation, descriptors)
    }
}

fn descriptor_order(
    left: &CapabilityDescriptor,
    right: &CapabilityDescriptor,
) -> std::cmp::Ordering {
    descriptor_order_key(left).cmp(&descriptor_order_key(right))
}

fn descriptor_order_key(
    descriptor: &CapabilityDescriptor,
) -> (String, PluginSurfaceKind, String, u64, String) {
    (
        descriptor.package_id.to_string(),
        descriptor.surface.kind,
        descriptor.surface.id.clone(),
        descriptor.generation,
        descriptor_capability_key(descriptor),
    )
}

fn descriptor_identity_key(
    descriptor: &CapabilityDescriptor,
) -> (String, PluginSurfaceKind, String, String) {
    (
        descriptor.package_id.to_string(),
        descriptor.surface.kind,
        descriptor.surface.id.clone(),
        descriptor_capability_key(descriptor),
    )
}

fn descriptor_surface_key(
    descriptor: &CapabilityDescriptor,
) -> (String, PluginSurfaceKind, String) {
    (
        descriptor.package_id.to_string(),
        descriptor.surface.kind,
        descriptor.surface.id.clone(),
    )
}

fn descriptor_capability_key(descriptor: &CapabilityDescriptor) -> String {
    match &descriptor.capability {
        CapabilityDescriptorKind::Tool { name, .. } => format!("tool:{name}"),
        CapabilityDescriptorKind::McpServer { server_name, .. } => {
            format!("mcp-server:{server_name}")
        }
        CapabilityDescriptorKind::Resource { uri, .. } => format!("resource:{}", uri.as_str()),
        CapabilityDescriptorKind::Prompt { name, .. } => format!("prompt:{name}"),
        CapabilityDescriptorKind::Flow { .. } => {
            format!("flow:{}", descriptor.surface.id)
        }
        CapabilityDescriptorKind::Knowledge { .. } => {
            format!("knowledge:{}", descriptor.surface.id)
        }
        CapabilityDescriptorKind::Ui { .. } => format!("ui:{}", descriptor.surface.id),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogRevisionMaterial<'a> {
    schema: &'static str,
    installation: &'a InstallationId,
    generation: u64,
    descriptors: &'a [CapabilityDescriptor],
}

fn catalog_revision(
    installation: &InstallationId,
    generation: u64,
    descriptors: &[CapabilityDescriptor],
) -> UseResult<String> {
    let material = CatalogRevisionMaterial {
        schema: CAPABILITY_GATEWAY_CATALOG_SCHEMA_V1,
        installation,
        generation,
        descriptors,
    };
    Ok(canonical_digest(&canonical_json(
        &material,
        "Capability Gateway catalog revision",
        CAPABILITY_ERROR,
    )?))
}

fn validate_surface(surface: &PluginSurfaceRef) -> UseResult<()> {
    if !valid_segment(&surface.id) {
        return Err(capability_error("A capability surface ID is invalid."));
    }
    Ok(())
}

fn valid_tool_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn valid_capability_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn validate_opaque_ref(value: &str, prefix: &str, label: &str) -> UseResult<()> {
    let digest = value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_prefix("sha256:"));
    if digest.is_none_or(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }) {
        return Err(capability_error(format!(
            "The {label} value is not a valid opaque reference."
        )));
    }
    Ok(())
}

fn derive_opaque_ref(
    prefix: &str,
    reference_domain: &str,
    package_id: &super::PluginPackageId,
    surface: &PluginSurfaceRef,
    generation: u64,
    binding_digest: &str,
) -> UseResult<String> {
    if !super::PluginPackageId::is_valid(package_id.as_str()) {
        return Err(capability_error("The package identity is invalid."));
    }
    validate_surface(surface)?;
    if generation == 0 || !valid_sha256(binding_digest) {
        return Err(capability_error(
            "Opaque references require a positive generation and a binding digest.",
        ));
    }
    let mut digest = Sha256::new();
    digest.update(CAPABILITY_REF_DOMAIN);
    for value in [
        reference_domain,
        package_id.as_str(),
        surface_kind_name(surface.kind),
        surface.id.as_str(),
        &generation.to_string(),
        binding_digest,
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    Ok(format!("{prefix}sha256:{:x}", digest.finalize()))
}

fn surface_kind_name(kind: PluginSurfaceKind) -> &'static str {
    match kind {
        PluginSurfaceKind::Flow => "flow",
        PluginSurfaceKind::Mcp => "mcp",
        PluginSurfaceKind::Okf => "okf",
        PluginSurfaceKind::Skill => "skill",
        PluginSurfaceKind::Tool => "tool",
        PluginSurfaceKind::Ui => "ui",
    }
}

pub(crate) fn validate_agent_schema(schema: &Value, require_object: bool) -> UseResult<()> {
    if !schema.is_object() {
        return Err(capability_error(
            "Agent input and output schemas must be JSON objects.",
        ));
    }
    let encoded = serde_json::to_vec(schema).map_err(|error| {
        capability_error(format!("Failed to encode an agent JSON schema: {error}"))
    })?;
    if encoded.len() > MAX_CAPABILITY_SCHEMA_BYTES {
        return Err(capability_error(
            "An agent JSON schema exceeds its size bound.",
        ));
    }
    validate_schema_value(schema, 0)?;
    if require_object {
        let Some(object) = schema.as_object() else {
            return Err(capability_error(
                "Agent input and output schemas must be JSON objects.",
            ));
        };
        if object.get("type") != Some(&Value::String("object".to_owned())) {
            return Err(capability_error(
                "Agent Tool schemas must declare a top-level object type.",
            ));
        }
        if object.get("additionalProperties") != Some(&Value::Bool(false)) {
            return Err(capability_error(
                "Agent Tool schemas must close top-level additional properties.",
            ));
        }
    }
    Ok(())
}

/// Compute the stable, domain-separated digest for one agent Tool schema.
///
/// The same helper is used by release planning and Control projection. It
/// validates the bounded, closed object contract before hashing canonical JSON
/// so a digest can never attest to an unsafe or merely equivalent-looking
/// schema document.
pub fn capability_schema_digest(schema: &Value) -> UseResult<String> {
    validate_agent_schema(schema, true)?;
    let bytes = canonical_json(schema, "agent JSON schema", CAPABILITY_ERROR)?;
    let mut hasher = Sha256::new();
    hasher.update(CAPABILITY_SCHEMA_DIGEST_DOMAIN);
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn validate_schema_value(value: &Value, depth: usize) -> UseResult<()> {
    if depth > MAX_CAPABILITY_SCHEMA_DEPTH {
        return Err(capability_error(
            "An agent JSON schema is nested too deeply.",
        ));
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(text) => {
            if text.len() > MAX_CAPABILITY_TEXT_BYTES || text.chars().any(char::is_control) {
                Err(capability_error(
                    "An agent JSON schema contains unbounded text.",
                ))
            } else {
                Ok(())
            }
        }
        Value::Array(values) => {
            if values.len() > MAX_CAPABILITY_SCHEMA_PROPERTIES {
                return Err(capability_error("An agent JSON schema array is too large."));
            }
            for value in values {
                validate_schema_value(value, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            if object.len() > MAX_CAPABILITY_SCHEMA_PROPERTIES {
                return Err(capability_error(
                    "An agent JSON schema object is too large.",
                ));
            }
            for (key, value) in object {
                if key.is_empty()
                    || key.len() > 128
                    || key.chars().any(char::is_control)
                    || ((matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef"))
                        && !value
                            .as_str()
                            .is_some_and(|reference| reference.starts_with('#')))
                    || (key == "$id"
                        && !value
                            .as_str()
                            .is_some_and(|identifier| identifier.starts_with('#')))
                {
                    return Err(capability_error(
                        "An agent JSON schema contains an unsafe property or external reference.",
                    ));
                }
                validate_schema_value(value, depth + 1)?;
            }
            if let Some(properties_value) = object.get("properties") {
                let properties = properties_value.as_object().ok_or_else(|| {
                    capability_error("An agent JSON schema properties value must be an object.")
                })?;
                if properties.len() > MAX_CAPABILITY_SCHEMA_PROPERTIES {
                    return Err(capability_error(
                        "An agent JSON schema properties object is too large.",
                    ));
                }
                if let Some(required) = object.get("required") {
                    let required = required.as_array().ok_or_else(|| {
                        capability_error("An agent JSON schema required value must be an array.")
                    })?;
                    if required.len() > MAX_CAPABILITY_SCHEMA_PROPERTIES {
                        return Err(capability_error(
                            "An agent JSON schema required set is too large.",
                        ));
                    }
                    let names = required
                        .iter()
                        .map(|value| {
                            value.as_str().ok_or_else(|| {
                                capability_error(
                                    "An agent JSON schema required name is not a string.",
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if !strictly_sorted_unique(&names)
                        || names.iter().any(|name| !properties.contains_key(*name))
                    {
                        return Err(capability_error(
                        "An agent JSON schema required set must be sorted and present in properties.",
                    ));
                    }
                }
            } else if object.contains_key("required") {
                return Err(capability_error(
                    "An agent JSON schema required set needs a properties object.",
                ));
            }
            Ok(())
        }
    }
}

fn capability_error(message: impl Into<String>) -> crate::UseError {
    contract_error(CAPABILITY_ERROR, message)
}

