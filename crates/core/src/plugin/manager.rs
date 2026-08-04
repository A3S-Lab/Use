use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::UseResult;

use super::{
    canonical_digest, canonical_json, contract_error, parse_contract,
    PLUGIN_MANAGER_TOOLSET_SCHEMA, PLUGIN_MANAGER_TOOLSET_SCHEMA_V2,
    PLUGIN_MANAGER_TOOLSET_SCHEMA_V3,
};

const MANAGER_ERROR: &str = "use.plugin.manager_toolset_invalid";
const PACKAGE_ID_PATTERN: &str = "^[a-z][a-z0-9-]{0,62}/[a-z][a-z0-9-]{0,62}$";
const MACHINE_ID_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._:/@-]{0,255}$";
const DIGEST_PATTERN: &str = "^sha256:[0-9a-f]{64}$";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerToolset {
    pub schema: String,
    pub tools: Vec<PluginManagerToolDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub annotations: PluginManagerToolAnnotations,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginManagerToolAnnotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
    pub open_world_hint: bool,
}

impl PluginManagerToolset {
    pub fn v1() -> Self {
        Self::contract(PLUGIN_MANAGER_TOOLSET_SCHEMA, false, false)
    }

    pub fn v2() -> Self {
        Self::contract(PLUGIN_MANAGER_TOOLSET_SCHEMA_V2, true, false)
    }

    pub fn v3() -> Self {
        Self::contract(PLUGIN_MANAGER_TOOLSET_SCHEMA_V3, true, true)
    }

    fn contract(schema: &str, okf: bool, flow: bool) -> Self {
        Self {
            schema: schema.to_owned(),
            tools: vec![
                tool(
                    "plugin_search",
                    "Search verified plugin catalog metadata without installing packages.",
                    search_schema(okf, flow),
                    annotations(true, false, true, true),
                ),
                tool(
                    "plugin_inspect",
                    "Inspect one verified plugin release, its surfaces, and permission ceiling.",
                    inspect_schema(),
                    annotations(true, false, true, true),
                ),
                tool(
                    "plugin_list_installed",
                    "List plugins installed in one bounded user or workspace scope.",
                    list_schema(),
                    annotations(true, false, true, false),
                ),
                tool(
                    "plugin_status",
                    "Read lifecycle, health, receipt, and enablement state for one installed plugin.",
                    package_scope_schema(),
                    annotations(true, false, true, false),
                ),
                tool(
                    "plugin_plan_install",
                    "Resolve an install and return a digest-bound plan without applying it.",
                    plan_schema(okf, flow),
                    annotations(true, false, false, true),
                ),
                tool(
                    "plugin_plan_upgrade",
                    "Resolve an upgrade and return a digest-bound plan without applying it.",
                    plan_schema(okf, flow),
                    annotations(true, false, false, true),
                ),
                tool(
                    "plugin_plan_uninstall",
                    "Resolve an uninstall and return a digest-bound plan without applying it.",
                    package_scope_schema(),
                    annotations(true, false, false, false),
                ),
                tool(
                    "plugin_apply_plan",
                    "Apply exactly one reviewed operation ID and canonical plan digest.",
                    apply_schema(),
                    annotations(false, true, true, true),
                ),
                tool(
                    "plugin_enable",
                    "Idempotently enable an installed plugin in one bounded scope.",
                    package_scope_schema(),
                    annotations(false, false, true, false),
                ),
                tool(
                    "plugin_disable",
                    "Idempotently disable an installed plugin while retaining installed data.",
                    package_scope_schema(),
                    annotations(false, false, true, false),
                ),
            ],
        }
    }

    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin manager MCP toolset",
            MANAGER_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self != &Self::v1() && self != &Self::v2() && self != &Self::v3() {
            return Err(manager_error(
                "The plugin manager MCP tool inventory differs from the frozen v1/v2/v3 contracts.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin manager MCP toolset", MANAGER_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn tool(&self, name: &str) -> Option<&PluginManagerToolDefinition> {
        self.tools.iter().find(|tool| tool.name == name)
    }
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    annotations: PluginManagerToolAnnotations,
) -> PluginManagerToolDefinition {
    PluginManagerToolDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema,
        annotations,
    }
}

const fn annotations(
    read_only_hint: bool,
    destructive_hint: bool,
    idempotent_hint: bool,
    open_world_hint: bool,
) -> PluginManagerToolAnnotations {
    PluginManagerToolAnnotations {
        read_only_hint,
        destructive_hint,
        idempotent_hint,
        open_world_hint,
    }
}

fn search_schema(okf: bool, flow: bool) -> Value {
    object_schema(
        vec![
            (
                "query",
                json!({"type":"string","minLength":1,"maxLength":256}),
            ),
            ("kind", surface_kind_schema(okf, flow)),
            ("channel", channel_schema()),
            ("cursor", bounded_string(512)),
            (
                "limit",
                json!({"type":"integer","minimum":1,"maximum":50,"default":20}),
            ),
        ],
        &["query"],
    )
}

fn inspect_schema() -> Value {
    object_schema(
        vec![
            ("packageId", package_id_schema()),
            ("version", bounded_string(64)),
            ("channel", channel_schema()),
        ],
        &["packageId"],
    )
}

fn list_schema() -> Value {
    object_schema(
        vec![
            ("scopeKind", scope_kind_schema()),
            ("scopeId", machine_id_schema()),
            ("cursor", bounded_string(512)),
            (
                "limit",
                json!({"type":"integer","minimum":1,"maximum":100,"default":50}),
            ),
        ],
        &["scopeKind", "scopeId"],
    )
}

fn package_scope_schema() -> Value {
    object_schema(
        vec![
            ("packageId", package_id_schema()),
            ("scopeKind", scope_kind_schema()),
            ("scopeId", machine_id_schema()),
        ],
        &["packageId", "scopeKind", "scopeId"],
    )
}

fn plan_schema(okf: bool, flow: bool) -> Value {
    object_schema(
        vec![
            ("packageId", package_id_schema()),
            ("versionRequirement", bounded_string(64)),
            ("channel", channel_schema()),
            ("surfaces", selected_surfaces_schema(okf, flow)),
            ("scopeKind", scope_kind_schema()),
            ("scopeId", machine_id_schema()),
        ],
        &["packageId", "scopeKind", "scopeId"],
    )
}

fn apply_schema() -> Value {
    object_schema(
        vec![
            ("operationId", machine_id_schema()),
            (
                "planDigest",
                json!({"type":"string","pattern":DIGEST_PATTERN}),
            ),
        ],
        &["operationId", "planDigest"],
    )
}

fn selected_surfaces_schema(okf: bool, flow: bool) -> Value {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 256,
        "uniqueItems": true,
        "items": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "kind": surface_kind_schema(okf, flow),
                "id": {
                    "type": "string",
                    "pattern": "^[a-z][a-z0-9-]{0,62}$"
                }
            },
            "required": ["kind", "id"]
        }
    })
}

fn package_id_schema() -> Value {
    json!({"type":"string","pattern":PACKAGE_ID_PATTERN})
}

fn machine_id_schema() -> Value {
    json!({"type":"string","pattern":MACHINE_ID_PATTERN})
}

fn surface_kind_schema(okf: bool, flow: bool) -> Value {
    if flow {
        json!({"type":"string","enum":["flow","mcp","okf","skill","tool","ui"]})
    } else if okf {
        json!({"type":"string","enum":["mcp","okf","skill","tool","ui"]})
    } else {
        json!({"type":"string","enum":["mcp","skill","tool","ui"]})
    }
}

fn channel_schema() -> Value {
    json!({"type":"string","enum":["stable","beta","nightly"]})
}

fn scope_kind_schema() -> Value {
    json!({"type":"string","enum":["user","workspace"]})
}

fn bounded_string(max_length: u64) -> Value {
    json!({"type":"string","minLength":1,"maxLength":max_length})
}

fn object_schema(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let properties = properties
        .into_iter()
        .map(|(name, schema)| (name.to_owned(), schema))
        .collect::<Map<_, _>>();
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": required
    })
}

fn manager_error(message: impl Into<String>) -> crate::UseError {
    contract_error(MANAGER_ERROR, message)
}
