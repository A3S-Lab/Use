// CapabilityDescriptor encode/validate (included into capability_gateway).

impl CapabilityDescriptor {
    /// Decode and validate one bounded descriptor document.
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "Capability descriptor",
            CAPABILITY_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != CAPABILITY_DESCRIPTOR_SCHEMA_V1
            || !super::PluginPackageId::is_valid(self.package_id.as_str())
            || validate_surface(&self.surface).is_err()
            || self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.manifest_digest)
            || !valid_capability_text(&self.title, MAX_CAPABILITY_TEXT_BYTES)
            || !valid_capability_text(&self.description, MAX_CAPABILITY_TEXT_BYTES)
            || self.dependencies.len() > MAX_CAPABILITY_DEPENDENCIES
            || !strictly_sorted_unique(&self.dependencies)
            || self.required_extensions.len() > MAX_CAPABILITY_CONSUMER_EXTENSIONS
            || !strictly_sorted_unique(&self.required_extensions)
            || self
                .dependencies
                .iter()
                .any(|dependency| validate_surface(dependency).is_err())
        {
            return Err(capability_error(
                "The capability descriptor identity, text, digest, or dependency set is invalid.",
            ));
        }

        self.publication.validate()?;
        validate_opaque_ref(
            self.invocation_ref.as_str(),
            "invocation:v1:",
            "InvocationRef",
        )?;
        if let Some(reference) = &self.artifact_ref {
            validate_opaque_ref(reference.as_str(), "artifact:v1:", "ArtifactRef")?;
        }
        if let Some(reference) = &self.endpoint_ref {
            validate_opaque_ref(reference.as_str(), "endpoint:v1:", "EndpointRef")?;
        }
        if self
            .dependencies
            .iter()
            .any(|dependency| dependency == &self.surface)
        {
            return Err(capability_error(
                "A capability descriptor cannot depend on its own surface.",
            ));
        }

        match &self.capability {
            CapabilityDescriptorKind::Tool {
                name,
                input_schema,
                output_schema,
                annotations: _,
                runtime_descriptor_digest,
            } => {
                if self.surface.kind != PluginSurfaceKind::Tool
                    || !valid_tool_name(name)
                    || validate_agent_schema(input_schema, true).is_err()
                    || validate_agent_schema(output_schema, true).is_err()
                    || runtime_descriptor_digest
                        .as_deref()
                        .is_some_and(|digest| !valid_sha256(digest))
                {
                    return Err(capability_error(
                        "A Tool descriptor must bind a schema-valid Tool surface.",
                    ));
                }
            }
            CapabilityDescriptorKind::McpServer {
                server_name,
                transport: CapabilityMcpTransport::StreamableHttp,
                protocol_version,
            } => {
                if self.surface.kind != PluginSurfaceKind::Mcp
                    || !valid_tool_name(server_name)
                    || protocol_version.is_empty()
                    || protocol_version.len() > MAX_CAPABILITY_PROTOCOL_BYTES
                    || protocol_version.chars().any(char::is_control)
                    || self.endpoint_ref.is_none()
                {
                    return Err(capability_error(
                        "An MCP Server descriptor must bind a streamable HTTP endpoint.",
                    ));
                }
            }
            CapabilityDescriptorKind::Resource {
                name,
                uri,
                mime_type,
                size,
            } => {
                if !valid_tool_name(name)
                    || validate_opaque_ref(uri.as_str(), "resource:v1:", "ResourceRef").is_err()
                    || size.is_some_and(|value| value > 256 * 1024)
                    || mime_type.as_deref().is_some_and(|value| {
                        value.is_empty()
                            || value.len() > MAX_CAPABILITY_PROTOCOL_BYTES
                            || value.chars().any(char::is_control)
                    })
                {
                    return Err(capability_error(
                        "A Resource descriptor must bind a valid opaque resource reference.",
                    ));
                }
            }
            CapabilityDescriptorKind::Prompt { name, arguments } => {
                if !valid_tool_name(name)
                    || arguments.len() > MAX_CAPABILITY_DEPENDENCIES
                    || !strictly_sorted_unique(
                        &arguments
                            .iter()
                            .map(|argument| argument.name.as_str())
                            .collect::<Vec<_>>(),
                    )
                {
                    return Err(capability_error(
                        "A Prompt descriptor has an invalid or unordered argument set.",
                    ));
                }
                for argument in arguments {
                    argument.validate()?;
                }
            }
            CapabilityDescriptorKind::Flow {
                engine,
                runtime,
                export_name,
                artifact_digest,
                requires_tools,
                requires_mcp,
                requires_knowledge,
            } => {
                if self.surface.kind != PluginSurfaceKind::Flow
                    || !self
                        .required_extensions
                        .contains(&CapabilityConsumerExtension::Flow)
                    || engine != "a3s-flow"
                    || runtime != "native-ts"
                    || !valid_tool_name(export_name)
                    || !valid_sha256(artifact_digest)
                    || requires_tools.len() > MAX_CAPABILITY_DEPENDENCIES
                    || requires_mcp.len() > MAX_CAPABILITY_DEPENDENCIES
                    || requires_knowledge.len() > MAX_CAPABILITY_DEPENDENCIES
                    || !strictly_sorted_unique(requires_tools)
                    || !strictly_sorted_unique(requires_mcp)
                    || !strictly_sorted_unique(requires_knowledge)
                    || requires_tools.iter().any(|id| !valid_tool_name(id))
                    || requires_mcp.iter().any(|id| !valid_tool_name(id))
                    || requires_knowledge.iter().any(|id| !valid_tool_name(id))
                {
                    return Err(capability_error(
                        "A Flow descriptor must bind a path-free Flow surface with the flow extension.",
                    ));
                }
            }
            CapabilityDescriptorKind::Knowledge {
                bundle_digest,
                okf_version,
            } => {
                if self.surface.kind != PluginSurfaceKind::Okf
                    || !self
                        .required_extensions
                        .contains(&CapabilityConsumerExtension::Knowledge)
                    || !valid_sha256(bundle_digest)
                    || okf_version.as_deref().is_some_and(|value| {
                        value.is_empty()
                            || value.len() > MAX_CAPABILITY_PROTOCOL_BYTES
                            || value.chars().any(char::is_control)
                    })
                {
                    return Err(capability_error(
                        "A Knowledge descriptor must bind a path-free OKF surface with the knowledge extension.",
                    ));
                }
            }
            CapabilityDescriptorKind::Ui {
                icon,
                order: _,
                entry_digest,
                bind_tools,
                bind_mcp,
                bind_flows,
            } => {
                if self.surface.kind != PluginSurfaceKind::Ui
                    || !self
                        .required_extensions
                        .contains(&CapabilityConsumerExtension::Ui)
                    || !valid_capability_text(icon, MAX_CAPABILITY_TEXT_BYTES)
                    || !valid_sha256(entry_digest)
                    || bind_tools.len() > MAX_CAPABILITY_DEPENDENCIES
                    || bind_mcp.len() > MAX_CAPABILITY_DEPENDENCIES
                    || bind_flows.len() > MAX_CAPABILITY_DEPENDENCIES
                    || !strictly_sorted_unique(bind_tools)
                    || !strictly_sorted_unique(bind_mcp)
                    || !strictly_sorted_unique(bind_flows)
                    || bind_tools.iter().any(|id| !valid_tool_name(id))
                    || bind_mcp.iter().any(|id| !valid_tool_name(id))
                    || bind_flows.iter().any(|id| !valid_tool_name(id))
                {
                    return Err(capability_error(
                        "A UI descriptor must bind a path-free UI surface with the ui extension.",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "capability descriptor", CAPABILITY_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    /// Return the canonical digests of a Tool's input and output schemas.
    /// Non-Tool descriptors have no schema contract.
    pub fn tool_schema_digests(&self) -> UseResult<Option<(String, String)>> {
        let CapabilityDescriptorKind::Tool {
            input_schema,
            output_schema,
            ..
        } = &self.capability
        else {
            return Ok(None);
        };
        self.validate()?;
        Ok(Some((
            capability_schema_digest(input_schema)?,
            capability_schema_digest(output_schema)?,
        )))
    }

    /// Return the optional consumer extensions required to understand this
    /// descriptor. Requirements are descriptive publication metadata; they
    /// are never treated as an authorization grant.
    pub fn required_extensions(&self) -> &[CapabilityConsumerExtension] {
        &self.required_extensions
    }

    pub fn requires_extension(&self, extension: CapabilityConsumerExtension) -> bool {
        self.required_extensions.contains(&extension)
    }

    pub fn tool_name(&self) -> Option<&str> {
        match &self.capability {
            CapabilityDescriptorKind::Tool { name, .. } => Some(name),
            CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Resource { .. }
            | CapabilityDescriptorKind::Prompt { .. }
            | CapabilityDescriptorKind::Flow { .. }
            | CapabilityDescriptorKind::Knowledge { .. }
            | CapabilityDescriptorKind::Ui { .. } => None,
        }
    }

    pub fn mcp_server_name(&self) -> Option<&str> {
        match &self.capability {
            CapabilityDescriptorKind::McpServer { server_name, .. } => Some(server_name),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::Resource { .. }
            | CapabilityDescriptorKind::Prompt { .. }
            | CapabilityDescriptorKind::Flow { .. }
            | CapabilityDescriptorKind::Knowledge { .. }
            | CapabilityDescriptorKind::Ui { .. } => None,
        }
    }

    pub fn resource_name(&self) -> Option<&str> {
        match &self.capability {
            CapabilityDescriptorKind::Resource { name, .. } => Some(name),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Prompt { .. }
            | CapabilityDescriptorKind::Flow { .. }
            | CapabilityDescriptorKind::Knowledge { .. }
            | CapabilityDescriptorKind::Ui { .. } => None,
        }
    }

    pub fn resource_uri(&self) -> Option<&ResourceRef> {
        match &self.capability {
            CapabilityDescriptorKind::Resource { uri, .. } => Some(uri),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Prompt { .. }
            | CapabilityDescriptorKind::Flow { .. }
            | CapabilityDescriptorKind::Knowledge { .. }
            | CapabilityDescriptorKind::Ui { .. } => None,
        }
    }

    pub fn prompt_name(&self) -> Option<&str> {
        match &self.capability {
            CapabilityDescriptorKind::Prompt { name, .. } => Some(name),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Resource { .. }
            | CapabilityDescriptorKind::Flow { .. }
            | CapabilityDescriptorKind::Knowledge { .. }
            | CapabilityDescriptorKind::Ui { .. } => None,
        }
    }

    pub fn prompt_arguments(&self) -> Option<&[CapabilityPromptArgument]> {
        match &self.capability {
            CapabilityDescriptorKind::Prompt { arguments, .. } => Some(arguments),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Resource { .. }
            | CapabilityDescriptorKind::Flow { .. }
            | CapabilityDescriptorKind::Knowledge { .. }
            | CapabilityDescriptorKind::Ui { .. } => None,
        }
    }

    pub fn is_agent_tool(&self) -> bool {
        matches!(self.capability, CapabilityDescriptorKind::Tool { .. })
    }

    pub fn is_resource(&self) -> bool {
        matches!(self.capability, CapabilityDescriptorKind::Resource { .. })
    }

    pub fn is_prompt(&self) -> bool {
        matches!(self.capability, CapabilityDescriptorKind::Prompt { .. })
    }

    /// True when this descriptor is A3S extension metadata (Flow, Knowledge, or UI).
    pub fn is_extension_metadata(&self) -> bool {
        matches!(
            self.capability,
            CapabilityDescriptorKind::Flow { .. }
                | CapabilityDescriptorKind::Knowledge { .. }
                | CapabilityDescriptorKind::Ui { .. }
        )
    }

    /// Return the consumer extension this metadata kind requires, if any.
    pub fn extension_metadata_kind(&self) -> Option<CapabilityConsumerExtension> {
        match &self.capability {
            CapabilityDescriptorKind::Flow { .. } => Some(CapabilityConsumerExtension::Flow),
            CapabilityDescriptorKind::Knowledge { .. } => {
                Some(CapabilityConsumerExtension::Knowledge)
            }
            CapabilityDescriptorKind::Ui { .. } => Some(CapabilityConsumerExtension::Ui),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Resource { .. }
            | CapabilityDescriptorKind::Prompt { .. } => None,
        }
    }
}
