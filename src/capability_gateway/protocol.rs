//! Private MCP protocol helpers for the Capability Gateway boundary.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use a3s_use_core::{
    CapabilityDescriptor, CapabilityDescriptorKind, CapabilityGatewayCatalog,
    CapabilityPromptArgument, CapabilityToolAnnotations, ResourceRef, UseError, UseResult,
};
use base64::Engine as _;
use jsonschema::{Draft, Validator};
use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::model::AnnotateAble;
use rmcp::model::{
    CallToolResult, GetPromptResult, Prompt, PromptArgument, PromptMessageContent, RawResource,
    Resource, ResourceContents, Tool, ToolAnnotations,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::CancellationToken;

use crate::capability_registry::{CapabilitySnapshotCursor, CapabilitySnapshotLease};

use super::admission::{AdmissionFailure, GatewayAdmission};
use super::server::CapabilityGatewayMcpServer;

pub(super) const MCP_ERROR: &str = "use.plugin.capability_gateway_mcp_invalid";
pub(super) const MCP_SCHEMA_ERROR: &str = "use.plugin.capability_gateway_schema_violation";
pub(super) const MCP_AUTHORIZATION_ERROR: &str = "use.plugin.capability_gateway_forbidden";
pub(super) const MCP_INVOCATION_ERROR: &str = "use.plugin.capability_gateway_invocation_failed";
pub(super) const MCP_RATE_LIMIT_ERROR: &str = "use.plugin.capability_gateway_rate_limited";
pub(super) const MCP_DISCOVERY_ERROR: &str = "use.plugin.capability_gateway_discovery_unavailable";
pub(super) const MCP_DISCOVERY_CURSOR_INVALID: &str =
    "use.plugin.capability_gateway_discovery_cursor_invalid";
pub(super) const MCP_DISCOVERY_CURSOR_STALE: &str =
    "use.plugin.capability_gateway_discovery_cursor_stale";
pub(super) const MCP_CANCELLED_ERROR: &str = "use.plugin.capability_gateway_cancelled";
pub(super) const MAX_CAPABILITY_VALUE_BYTES: usize = 256 * 1024;
pub(super) const MAX_CAPABILITY_VALUE_DEPTH: usize = 32;
pub(super) const MAX_CAPABILITY_VALUE_ELEMENTS: usize = 4_096;
pub(super) const MAX_CAPABILITY_RESOURCE_SIZE: u32 = 256 * 1024;
pub(super) const MAX_DISCOVERY_ITEMS_PER_PAGE: usize = 64;
pub(super) const MAX_DISCOVERY_CURSOR_BYTES: usize = 128;
pub(super) const DISCOVERY_CURSOR_VERSION: &str = "v2";
#[derive(Clone)]
pub(super) struct CompiledCapabilitySchema {
    validator: Arc<Validator>,
}

impl std::fmt::Debug for CompiledCapabilitySchema {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompiledCapabilitySchema")
            .finish_non_exhaustive()
    }
}

impl CompiledCapabilitySchema {
    pub(super) fn compile(schema: &Value) -> UseResult<Self> {
        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            .with_retriever(NoExternalSchemaRetriever)
            .build(schema)
            .map_err(|_| {
                mcp_error(
                    "The Capability Gateway schema cannot be compiled by the fixed validator.",
                )
            })?;
        Ok(Self {
            validator: Arc::new(validator),
        })
    }

    pub(super) fn validate(&self, value: &Value) -> UseResult<()> {
        let encoded = serde_json::to_vec(value).map_err(|_| schema_value_error())?;
        if encoded.len() > MAX_CAPABILITY_VALUE_BYTES {
            return Err(schema_value_error());
        }
        validate_value_bounds(value, 0)?;
        if self.validator.is_valid(value) {
            Ok(())
        } else {
            Err(schema_value_error())
        }
    }
}

/// Capability schemas are self-contained contract data.  An agent-visible
/// descriptor must never make validator construction read a URL or local
/// file, even if a future schema keyword introduces another reference form.
#[derive(Debug, Clone, Copy)]
struct NoExternalSchemaRetriever;

impl jsonschema::Retrieve for NoExternalSchemaRetriever {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external Capability Gateway schema retrieval is disabled".into())
    }
}

#[derive(Clone)]
pub(super) struct CapabilityGatewayTool {
    pub(super) descriptor_index: usize,
    pub(super) input_schema: CompiledCapabilitySchema,
    pub(super) output_schema: CompiledCapabilitySchema,
}

#[derive(Clone)]
pub(super) struct CapabilityGatewayResource {
    pub(super) descriptor_index: usize,
    pub(super) resource: Resource,
}

#[derive(Clone)]
pub(super) struct CapabilityGatewayPrompt {
    pub(super) descriptor_index: usize,
    pub(super) prompt: Prompt,
    pub(super) arguments: Vec<CapabilityPromptArgument>,
}
pub(super) fn validate_snapshot_binding(
    catalog: &CapabilityGatewayCatalog,
    lease: &CapabilitySnapshotLease,
) -> UseResult<()> {
    let snapshot = lease.snapshot();
    if snapshot.cursor() != lease.cursor() {
        return Err(mcp_error(
            "The Capability Gateway snapshot lease contains inconsistent cursor evidence.",
        ));
    }
    validate_snapshot_binding_identity(
        catalog,
        snapshot.installation.clone(),
        snapshot.generation,
        lease.cursor(),
    )
}

pub(super) fn validate_snapshot_binding_identity(
    catalog: &CapabilityGatewayCatalog,
    installation: a3s_use_core::InstallationId,
    generation: u64,
    cursor: &CapabilitySnapshotCursor,
) -> UseResult<()> {
    cursor
        .validate()
        .map_err(|_| mcp_error("The Capability Gateway snapshot lease cursor is invalid."))?;
    if catalog.installation() != &installation
        || cursor.installation != installation
        || cursor.generation != generation
        || catalog.generation() != generation
        || !cursor.is_fully_leasable()
    {
        return Err(mcp_error(
            "The Capability Gateway catalog is not bound to the exact Use snapshot lease.",
        ));
    }

    for descriptor in catalog.descriptors() {
        let package_id = descriptor.package_id.to_string();
        let Some(package) = cursor
            .packages
            .iter()
            .find(|package| package.package_id == package_id)
        else {
            return Err(mcp_error(
                "The Capability Gateway catalog contains a package outside the Use snapshot lease.",
            ));
        };
        if package.lifecycle_generation != descriptor.generation
            || package.package_digest != descriptor.package_digest
            || package.manifest_digest != descriptor.manifest_digest
        {
            return Err(mcp_error(
                "The Capability Gateway descriptor does not match the Use snapshot lease.",
            ));
        }
    }
    Ok(())
}

pub(super) fn frozen_tool_router(
    catalog: &Arc<CapabilityGatewayCatalog>,
) -> UseResult<ToolRouter<CapabilityGatewayMcpServer>> {
    let mut router = ToolRouter::<CapabilityGatewayMcpServer>::new();
    for descriptor in catalog
        .descriptors()
        .iter()
        .filter(|descriptor| descriptor.is_agent_tool())
    {
        let name = descriptor.tool_name().ok_or_else(|| {
            mcp_error("The Capability Gateway catalog contains a non-Tool route.")
        })?;
        if router.has_route(name) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate Tool name `{name}`."
            )));
        }
        let tool = mcp_tool(descriptor)?;
        let route_name = name.to_owned();
        router.add_route(ToolRoute::new_dyn(
            tool,
            move |context: rmcp::handler::server::tool::ToolCallContext<
                '_,
                CapabilityGatewayMcpServer,
            >| {
                let route_name = route_name.clone();
                Box::pin(async move {
                    let gateway_context =
                        context.service.request_context(&context.request_context)?;
                    let cancellation = context.request_context.ct.clone();
                    context
                        .service
                        .dispatch(
                            &route_name,
                            context.arguments,
                            &gateway_context,
                            &cancellation,
                        )
                        .await
                })
            },
        ));
    }
    Ok(router)
}

pub(super) fn compile_tools(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<BTreeMap<String, CapabilityGatewayTool>> {
    let mut tools = BTreeMap::new();
    for (descriptor_index, descriptor) in catalog.descriptors().iter().enumerate() {
        if !descriptor.is_agent_tool() {
            continue;
        }
        let name = descriptor.tool_name().ok_or_else(|| {
            mcp_error("The Capability Gateway catalog contains a non-Tool route.")
        })?;
        if tools.contains_key(name) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate Tool name `{name}`."
            )));
        }
        let (input_schema, output_schema) = match &descriptor.capability {
            CapabilityDescriptorKind::Tool {
                input_schema,
                output_schema,
                ..
            } => (input_schema, output_schema),
            CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Resource { .. }
            | CapabilityDescriptorKind::Prompt { .. }
            | CapabilityDescriptorKind::Flow { .. }
            | CapabilityDescriptorKind::Knowledge { .. }
            | CapabilityDescriptorKind::Ui { .. } => {
                return Err(mcp_error(
                    "Only Tool descriptors can be compiled for the MCP Gateway.",
                ));
            }
        };
        tools.insert(
            name.to_owned(),
            CapabilityGatewayTool {
                descriptor_index,
                input_schema: CompiledCapabilitySchema::compile(input_schema)?,
                output_schema: CompiledCapabilitySchema::compile(output_schema)?,
            },
        );
    }
    Ok(tools)
}

pub(super) fn compile_resources(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<BTreeMap<String, CapabilityGatewayResource>> {
    let mut resources = BTreeMap::new();
    for (descriptor_index, descriptor) in catalog.descriptors().iter().enumerate() {
        let CapabilityDescriptorKind::Resource {
            name,
            uri,
            mime_type,
            size,
        } = &descriptor.capability
        else {
            continue;
        };
        if size.is_some_and(|value| value > MAX_CAPABILITY_RESOURCE_SIZE) {
            return Err(mcp_error(
                "The Capability Gateway resource metadata exceeds its size bound.",
            ));
        }
        if resources.contains_key(uri.as_str()) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate resource URI `{}`.",
                uri.as_str()
            )));
        }
        let mut raw = RawResource::new(uri.as_str(), name.clone());
        raw.title = Some(descriptor.title.clone());
        raw.description = Some(descriptor.description.clone());
        raw.mime_type = mime_type.clone();
        raw.size = *size;
        resources.insert(
            uri.as_str().to_owned(),
            CapabilityGatewayResource {
                descriptor_index,
                resource: raw.no_annotation(),
            },
        );
    }
    Ok(resources)
}

pub(super) fn compile_prompts(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<BTreeMap<String, CapabilityGatewayPrompt>> {
    let mut prompts = BTreeMap::new();
    for (descriptor_index, descriptor) in catalog.descriptors().iter().enumerate() {
        let CapabilityDescriptorKind::Prompt { name, arguments } = &descriptor.capability else {
            continue;
        };
        if prompts.contains_key(name) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate prompt name `{name}`."
            )));
        }
        let prompt_arguments = arguments
            .iter()
            .map(|argument| PromptArgument {
                name: argument.name.clone(),
                title: argument.title.clone(),
                description: argument.description.clone(),
                required: Some(argument.required),
            })
            .collect::<Vec<_>>();
        let prompt = Prompt {
            name: name.clone(),
            title: Some(descriptor.title.clone()),
            description: Some(descriptor.description.clone()),
            arguments: (!prompt_arguments.is_empty()).then_some(prompt_arguments),
            icons: None,
        };
        prompts.insert(
            name.clone(),
            CapabilityGatewayPrompt {
                descriptor_index,
                prompt,
                arguments: arguments.clone(),
            },
        );
    }
    Ok(prompts)
}

fn mcp_tool(descriptor: &CapabilityDescriptor) -> UseResult<Tool> {
    let CapabilityDescriptorKind::Tool {
        name,
        input_schema,
        output_schema,
        annotations,
        ..
    } = &descriptor.capability
    else {
        return Err(mcp_error("Only Tool descriptors can be exposed over MCP."));
    };
    let input_schema = input_schema
        .as_object()
        .cloned()
        .ok_or_else(|| mcp_error("A Capability Gateway input schema is not an object."))?;
    let output_schema = output_schema
        .as_object()
        .cloned()
        .ok_or_else(|| mcp_error("A Capability Gateway output schema is not an object."))?;
    Ok(Tool {
        name: Cow::Owned(name.clone()),
        title: Some(descriptor.title.clone()),
        description: Some(Cow::Owned(descriptor.description.clone())),
        input_schema: Arc::new(input_schema),
        output_schema: Some(Arc::new(output_schema)),
        annotations: Some(mcp_annotations(*annotations)),
        icons: None,
    })
}

fn mcp_annotations(annotations: CapabilityToolAnnotations) -> ToolAnnotations {
    ToolAnnotations {
        title: None,
        read_only_hint: Some(annotations.read_only_hint),
        destructive_hint: Some(annotations.destructive_hint),
        idempotent_hint: Some(annotations.idempotent_hint),
        open_world_hint: Some(annotations.open_world_hint),
    }
}

pub(super) fn content_admission(
    admission: &GatewayAdmission,
) -> Result<OwnedSemaphorePermit, rmcp::ErrorData> {
    match admission.try_acquire() {
        Ok(permit) => Ok(permit),
        Err(AdmissionFailure::InFlight | AdmissionFailure::RateLimited) => {
            Err(rmcp::ErrorData::internal_error(
                "The Capability Gateway is temporarily rate limited.",
                None,
            ))
        }
        Err(AdmissionFailure::StatePoisoned) => Err(rmcp::ErrorData::internal_error(
            "The Capability Gateway admission state is unavailable.",
            None,
        )),
    }
}

pub(super) fn descriptor_is_visible(view: &[usize], descriptor_index: usize) -> bool {
    view.binary_search(&descriptor_index).is_ok()
}

pub(super) fn discovery_policy_error() -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(
        "The Capability Gateway discovery policy is unavailable.",
        Some(serde_json::json!({ "code": MCP_DISCOVERY_ERROR })),
    )
}

pub(super) fn discovery_page(
    cursor: Option<String>,
    surface: &str,
    fingerprint: &str,
    item_count: usize,
) -> Result<(usize, usize, Option<String>), rmcp::ErrorData> {
    let start = match cursor {
        None => 0,
        Some(cursor) => {
            if cursor.len() > MAX_DISCOVERY_CURSOR_BYTES || cursor.is_empty() {
                return Err(discovery_cursor_error(
                    MCP_DISCOVERY_CURSOR_INVALID,
                    "The Capability Gateway discovery cursor is invalid.",
                ));
            }
            let mut parts = cursor.split('.');
            let version = parts.next();
            let cursor_surface = parts.next();
            let cursor_fingerprint = parts.next();
            let offset = parts.next();
            if version != Some(DISCOVERY_CURSOR_VERSION)
                || cursor_surface != Some(surface)
                || cursor_fingerprint.is_none_or(|value| !valid_discovery_fingerprint(value))
                || offset.is_none_or(|value| {
                    value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit())
                })
                || parts.next().is_some()
            {
                return Err(discovery_cursor_error(
                    MCP_DISCOVERY_CURSOR_INVALID,
                    "The Capability Gateway discovery cursor is invalid.",
                ));
            }
            if cursor_fingerprint != Some(fingerprint) {
                return Err(discovery_cursor_error(
                    MCP_DISCOVERY_CURSOR_STALE,
                    "The Capability Gateway catalog or visibility view changed; restart discovery pagination.",
                ));
            }
            offset
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| {
                    discovery_cursor_error(
                        MCP_DISCOVERY_CURSOR_INVALID,
                        "The Capability Gateway discovery cursor is invalid.",
                    )
                })?
        }
    };
    if start > item_count {
        return Err(discovery_cursor_error(
            MCP_DISCOVERY_CURSOR_INVALID,
            "The Capability Gateway discovery cursor is outside the catalog.",
        ));
    }
    let end = start
        .saturating_add(MAX_DISCOVERY_ITEMS_PER_PAGE)
        .min(item_count);
    let next_cursor = (end < item_count)
        .then(|| format!("{DISCOVERY_CURSOR_VERSION}.{surface}.{fingerprint}.{end}"));
    Ok((start, end, next_cursor))
}

fn discovery_cursor_error(code: &'static str, message: &'static str) -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_params(message, Some(serde_json::json!({ "code": code })))
}

fn valid_discovery_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn update_discovery_digest_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

pub(super) fn validate_resource_contents(
    uri: &str,
    contents: &[ResourceContents],
) -> UseResult<()> {
    if contents.len() > MAX_CAPABILITY_VALUE_ELEMENTS {
        return Err(schema_value_error());
    }
    for content in contents {
        let content_uri = match content {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                ..
            } => {
                validate_content_text(text)?;
                validate_content_mime(mime_type.as_deref())?;
                uri
            }
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => {
                validate_content_blob(blob)?;
                validate_content_mime(mime_type.as_deref())?;
                uri
            }
        };
        if content_uri != uri || ResourceRef::parse(content_uri.clone()).is_err() {
            return Err(schema_value_error());
        }
    }
    let encoded = serde_json::to_vec(contents).map_err(|_| schema_value_error())?;
    if encoded.len() > MAX_CAPABILITY_VALUE_BYTES {
        return Err(schema_value_error());
    }
    Ok(())
}

pub(super) fn validate_prompt_arguments(
    arguments: &Value,
    declarations: &[CapabilityPromptArgument],
) -> UseResult<()> {
    let Value::Object(arguments) = arguments else {
        return Err(schema_value_error());
    };
    let encoded = serde_json::to_vec(arguments).map_err(|_| schema_value_error())?;
    if encoded.len() > MAX_CAPABILITY_VALUE_BYTES {
        return Err(schema_value_error());
    }
    validate_value_bounds(&Value::Object(arguments.clone()), 0)?;
    for key in arguments.keys() {
        if !declarations
            .iter()
            .any(|declaration| declaration.name == *key)
        {
            return Err(schema_value_error());
        }
    }
    if arguments
        .values()
        .any(|value| !value.as_str().is_some_and(valid_content_text))
    {
        return Err(schema_value_error());
    }
    for declaration in declarations {
        if declaration.required && !arguments.contains_key(&declaration.name) {
            return Err(schema_value_error());
        }
    }
    Ok(())
}

pub(super) fn validate_prompt_result(
    result: &GetPromptResult,
    resource_uris: &std::collections::BTreeSet<String>,
) -> UseResult<()> {
    let encoded = serde_json::to_vec(result).map_err(|_| schema_value_error())?;
    if encoded.len() > MAX_CAPABILITY_VALUE_BYTES
        || result.messages.len() > MAX_CAPABILITY_VALUE_ELEMENTS
    {
        return Err(schema_value_error());
    }
    if result
        .description
        .as_deref()
        .is_some_and(|description| !valid_content_text(description))
    {
        return Err(schema_value_error());
    }
    for message in &result.messages {
        match &message.content {
            PromptMessageContent::Text { text } => validate_content_text(text)?,
            PromptMessageContent::Image { image } => {
                validate_content_blob(&image.data)?;
                validate_content_text(&image.mime_type)?;
            }
            PromptMessageContent::Resource { resource } => {
                validate_prompt_resource_contents(&resource.resource, resource_uris)?;
            }
            PromptMessageContent::ResourceLink { link } => {
                if ResourceRef::parse(link.uri.clone()).is_err()
                    || !resource_uris.contains(link.uri.as_str())
                    || !valid_content_text(&link.name)
                    || link
                        .description
                        .as_deref()
                        .is_some_and(|description| !valid_content_text(description))
                {
                    return Err(schema_value_error());
                }
            }
        }
    }
    Ok(())
}

fn validate_prompt_resource_contents(
    content: &ResourceContents,
    resource_uris: &std::collections::BTreeSet<String>,
) -> UseResult<()> {
    let uri = match content {
        ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            ..
        } => {
            validate_content_text(text)?;
            validate_content_mime(mime_type.as_deref())?;
            uri
        }
        ResourceContents::BlobResourceContents {
            uri,
            mime_type,
            blob,
            ..
        } => {
            validate_content_blob(blob)?;
            validate_content_mime(mime_type.as_deref())?;
            uri
        }
    };
    if ResourceRef::parse(uri.clone()).is_err() || !resource_uris.contains(uri.as_str()) {
        return Err(schema_value_error());
    }
    Ok(())
}

fn validate_content_mime(mime_type: Option<&str>) -> UseResult<()> {
    if mime_type.is_some_and(|value| !valid_content_text(value)) {
        return Err(schema_value_error());
    }
    Ok(())
}

fn validate_content_text(value: &str) -> UseResult<()> {
    if valid_content_text(value) {
        Ok(())
    } else {
        Err(schema_value_error())
    }
}

fn validate_content_blob(value: &str) -> UseResult<()> {
    if !valid_content_text(value) {
        return Err(schema_value_error());
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| schema_value_error())?;
    if decoded.len() > MAX_CAPABILITY_VALUE_BYTES
        || base64::engine::general_purpose::STANDARD.encode(decoded) != value
    {
        return Err(schema_value_error());
    }
    Ok(())
}

fn valid_content_text(value: &str) -> bool {
    value.len() <= MAX_CAPABILITY_VALUE_BYTES && !value.chars().any(char::is_control)
}

pub(super) fn tool_result(
    result: UseResult<Value>,
    output_schema: &CompiledCapabilitySchema,
) -> CallToolResult {
    match result {
        Ok(value) => match output_schema.validate(&value) {
            Ok(()) => CallToolResult::structured(value),
            Err(_) => structured_error(
                MCP_SCHEMA_ERROR,
                "The Capability Gateway provider returned a value outside the published schema.",
            ),
        },
        Err(_) => structured_error(
            MCP_INVOCATION_ERROR,
            "The Capability Gateway provider could not complete the invocation.",
        ),
    }
}

pub(super) fn structured_error(code: &str, message: &str) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({
        "code": code,
        "message": message,
    }))
}

pub(super) fn cancellation_error() -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_request(
        "The Capability Gateway request was cancelled.",
        Some(serde_json::json!({ "code": MCP_CANCELLED_ERROR })),
    )
}

pub(super) async fn run_until_cancelled<F, T>(
    cancellation: &CancellationToken,
    future: F,
) -> Option<T>
where
    F: Future<Output = T>,
{
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => None,
        output = future => Some(output),
    }
}

fn schema_value_error() -> UseError {
    UseError::new(
        MCP_SCHEMA_ERROR,
        "The capability value does not satisfy its published schema.",
    )
}

fn validate_value_bounds(value: &Value, depth: usize) -> UseResult<()> {
    if depth > MAX_CAPABILITY_VALUE_DEPTH {
        return Err(schema_value_error());
    }
    match value {
        Value::Array(values) => {
            if values.len() > MAX_CAPABILITY_VALUE_ELEMENTS {
                return Err(schema_value_error());
            }
            for value in values {
                validate_value_bounds(value, depth + 1)?;
            }
        }
        Value::Object(object) => {
            if object.len() > MAX_CAPABILITY_VALUE_ELEMENTS {
                return Err(schema_value_error());
            }
            for (key, value) in object {
                if key.len() > 4 * 1024 || key.chars().any(char::is_control) {
                    return Err(schema_value_error());
                }
                validate_value_bounds(value, depth + 1)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

pub(super) fn mcp_error(message: impl Into<String>) -> UseError {
    UseError::new(MCP_ERROR, message)
}
