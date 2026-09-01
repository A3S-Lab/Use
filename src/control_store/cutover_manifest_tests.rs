use std::collections::BTreeSet;
use std::path::{Component, Path};

use a3s_acl::{Block, Value};

use super::payload_owner::ControlPayloadOwnerId;

const MANIFEST: &str = include_str!("../../docs/control-store-cutover.acl");

const AUTHORITY_IDS: &[&str] = &[
    "capability-registry",
    "enablement-operations",
    "flow-bindings",
    "installation-graph",
    "knowledge-bindings",
    "lifecycle-journal",
    "reviewed-graph-operations",
    "runtime-bindings",
    "workspace-grants",
];

const CONSUMER_IDS: &[&str] = &[
    "artifact-reachability",
    "capability-index-and-leases",
    "operation-diagnostics",
    "package-lifecycle-orchestrator",
    "state-backup",
    "state-layout-and-clean-initialization",
    "state-restore",
];

const OPERATIONAL_STATE_IDS: &[&str] = &[
    "complete-restore-attempt",
    "generation-leases",
    "legacy-package-graph-lock",
    "outer-fences",
    "restore-marker",
];

const AUTHORITY_PATHS: &[&str] = &[
    "bindings/flow",
    "bindings/knowledge",
    "bindings/runtime",
    "extension-generations",
    "extensions",
    "grants",
    "installation-snapshot.json",
    "operations/package-graphs",
    "operations/plugins",
    "package-enablement",
    "registry.json",
];

const EXTERNAL_OWNER_PATHS: &[&str] = &[
    "knowledge",
    "operations/package-diagnostic-history",
    "operations/package-downloads",
    "operations/package-resolutions",
    "operations/state-restores",
    "plugin-host-manager",
];

const OPERATIONAL_STATE_PATHS: &[&str] = &[
    ".control-installation-restore",
    ".installation-mutation.lock",
    ".maintenance.lock",
    ".maintenance.restore.json",
    ".package-graph.lock",
    "generation-leases",
];

#[test]
fn cutover_manifest_freezes_the_complete_current_state_layout() {
    let root = parse_root();
    assert_known_attributes(
        &root,
        &[
            "schema_version",
            "status",
            "production_authority",
            "control_store_activation",
            "migration_policy",
            "cutover_unit",
            "dual_write_allowed",
            "fallback_read_allowed",
        ],
    );
    assert_eq!(number(&root, "schema_version"), 1.0);
    assert_eq!(string(&root, "status"), "inventory-frozen");
    assert_eq!(string(&root, "production_authority"), "legacy-files");
    assert_eq!(string(&root, "control_store_activation"), "inactive");
    assert_eq!(string(&root, "migration_policy"), "clean-state-only");
    assert_eq!(string(&root, "cutover_unit"), "one-installation-aggregate");
    assert!(!boolean(&root, "dual_write_allowed"));
    assert!(!boolean(&root, "fallback_read_allowed"));

    let authorities = blocks(&root, "authority");
    let consumers = blocks(&root, "consumer");
    let external_owners = blocks(&root, "external_owner");
    let operational_states = blocks(&root, "operational_state");
    assert_eq!(block_ids(&authorities), expected(AUTHORITY_IDS));
    assert_eq!(block_ids(&consumers), expected(CONSUMER_IDS));
    assert_eq!(
        block_ids(&external_owners),
        ControlPayloadOwnerId::ALL
            .into_iter()
            .map(ControlPayloadOwnerId::as_str)
            .map(str::to_owned)
            .collect()
    );
    assert_eq!(
        block_ids(&operational_states),
        expected(OPERATIONAL_STATE_IDS)
    );
    assert_eq!(
        root.blocks.len(),
        authorities.len() + consumers.len() + external_owners.len() + operational_states.len(),
        "the cutover manifest contains an unknown block kind"
    );

    let authority_paths = collect_unique_paths(&authorities, "legacy_paths");
    let external_paths = collect_unique_paths(&external_owners, "state_paths");
    let operational_paths = collect_unique_paths(&operational_states, "state_paths");
    assert_eq!(authority_paths, expected(AUTHORITY_PATHS));
    assert_eq!(external_paths, expected(EXTERNAL_OWNER_PATHS));
    assert_eq!(operational_paths, expected(OPERATIONAL_STATE_PATHS));
    assert!(authority_paths.is_disjoint(&external_paths));
    assert!(authority_paths.is_disjoint(&operational_paths));
    assert!(external_paths.is_disjoint(&operational_paths));

    for path in authority_paths
        .iter()
        .chain(external_paths.iter())
        .chain(operational_paths.iter())
    {
        assert_supported_state_path(path);
    }
}

#[test]
fn cutover_manifest_references_real_code_and_forbids_partial_activation() {
    let root = parse_root();
    for authority in blocks(&root, "authority") {
        assert_known_attributes(
            authority,
            &[
                "legacy_paths",
                "facts",
                "implementation",
                "readers",
                "writers",
                "target_owner",
                "cutover",
                "legacy_disposition",
            ],
        );
        assert_eq!(string(authority, "target_owner"), "control-store");
        assert_eq!(string(authority, "cutover"), "same-commit");
        assert_eq!(string(authority, "legacy_disposition"), "delete");
        assert!(!string_list(authority, "facts").is_empty());
        assert_code_files(authority, &["implementation", "readers", "writers"]);
    }

    for consumer in blocks(&root, "consumer") {
        assert_known_attributes(
            consumer,
            &[
                "implementation",
                "required_change",
                "cutover",
                "legacy_fallback",
            ],
        );
        assert_eq!(string(consumer, "cutover"), "same-commit");
        assert!(!boolean(consumer, "legacy_fallback"));
        assert!(!string(consumer, "required_change").is_empty());
        assert_code_files(consumer, &["implementation"]);
    }

    for owner in blocks(&root, "external_owner") {
        assert_known_attributes(
            owner,
            &[
                "state_paths",
                "implementation",
                "payload",
                "backup_policy",
                "registered_before_activation",
                "may_choose_desired_state",
            ],
        );
        assert!(boolean(owner, "registered_before_activation"));
        assert!(!boolean(owner, "may_choose_desired_state"));
        assert!(!string(owner, "payload").is_empty());
        let owner_id = ControlPayloadOwnerId::parse(block_id(owner))
            .expect("the owner set was checked against the typed registry");
        assert_eq!(
            string(owner, "backup_policy"),
            owner_id.backup_policy().as_str()
        );
        assert_code_files(owner, &["implementation"]);
    }

    for operational in blocks(&root, "operational_state") {
        assert_known_attributes(
            operational,
            &[
                "state_paths",
                "implementation",
                "disposition",
                "backup_policy",
            ],
        );
        assert_eq!(string(operational, "backup_policy"), "excluded");
        assert!(matches!(
            string(operational, "disposition"),
            "delete"
                | "retain"
                | "retain-outside-database"
                | "rebind-to-control-generation"
                | "retire-staging-retain-terminal"
        ));
        assert_code_files(operational, &["implementation"]);
    }
}

fn parse_root() -> Block {
    let document = a3s_acl::parse_acl(MANIFEST).expect("the cutover ACL must parse");
    let [root] = document.blocks.as_slice() else {
        panic!("the cutover ACL must contain exactly one root block");
    };
    assert_eq!(root.name, "control_store_cutover");
    assert!(root.labels.is_empty());
    root.clone()
}

fn blocks<'a>(root: &'a Block, name: &str) -> Vec<&'a Block> {
    root.blocks
        .iter()
        .filter(|block| block.name == name)
        .collect()
}

fn block_ids(blocks: &[&Block]) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for block in blocks {
        let [id] = block.labels.as_slice() else {
            panic!("every cutover child block must have exactly one label");
        };
        assert!(!id.is_empty());
        assert!(block.blocks.is_empty());
        assert!(ids.insert(id.clone()), "duplicate cutover block '{id}'");
    }
    ids
}

fn block_id(block: &Block) -> &str {
    let [id] = block.labels.as_slice() else {
        panic!("every cutover child block must have exactly one label");
    };
    id
}

fn expected(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn collect_unique_paths(blocks: &[&Block], attribute: &str) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for block in blocks {
        for path in string_list(block, attribute) {
            assert_normalized_relative_path(&path);
            assert!(
                paths.insert(path.clone()),
                "state path '{path}' is assigned more than once"
            );
        }
    }
    paths
}

fn assert_supported_state_path(path: &str) {
    let parts = path.split('/').collect::<Vec<_>>();
    let first = parts[0];
    if first == crate::installation_state_layout::CONTROL_INSTALLATION_RESTORE_ATTEMPT_DIRECTORY {
        assert_eq!(parts.len(), 1);
        assert!(!crate::installation_state_layout::supported_root_entry(
            first, true
        ));
        return;
    }
    let root_is_directory = !matches!(
        first,
        ".installation-mutation.lock"
            | ".maintenance.lock"
            | ".maintenance.restore.json"
            | ".package-graph.lock"
            | "installation-snapshot.json"
            | "registry.json"
    );
    assert!(
        crate::installation_state_layout::supported_root_entry(first, root_is_directory),
        "manifest path '{path}' is absent from the current state layout"
    );
    match parts.as_slice() {
        ["operations", family] => assert!(
            crate::installation_state_layout::supported_operation_directory(family),
            "manifest operation family '{family}' is unsupported"
        ),
        ["bindings", family] => assert!(
            crate::installation_state_layout::supported_binding_directory(family),
            "manifest binding family '{family}' is unsupported"
        ),
        [_] => {}
        _ => panic!("manifest state path '{path}' is not a classified state-layout leaf"),
    }
}

fn assert_code_files(block: &Block, attributes: &[&str]) {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    for attribute in attributes {
        let files = string_list(block, attribute);
        assert!(
            !files.is_empty(),
            "{} '{}' must list at least one {attribute} file",
            block.name,
            block.labels.join("/")
        );
        let mut unique = BTreeSet::new();
        for file in files {
            assert_normalized_relative_path(&file);
            assert!(file.ends_with(".rs"), "code reference '{file}' is not Rust");
            assert!(
                unique.insert(file.clone()),
                "duplicate code reference '{file}'"
            );
            assert!(
                repository.join(&file).is_file(),
                "cutover code reference '{file}' does not exist"
            );
        }
    }
}

fn assert_normalized_relative_path(value: &str) {
    assert!(!value.is_empty());
    assert!(!value.contains('\\'));
    let path = Path::new(value);
    assert!(!path.is_absolute());
    assert!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "path '{value}' is not normalized and relative"
    );
}

fn assert_known_attributes(block: &Block, expected_names: &[&str]) {
    let actual = block
        .attributes
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected = expected_names.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        actual, expected,
        "{} has an unknown or missing field",
        block.name
    );
}

fn value<'a>(block: &'a Block, name: &str) -> &'a Value {
    block
        .attributes
        .get(name)
        .unwrap_or_else(|| panic!("{} omits '{name}'", block.name))
}

fn string<'a>(block: &'a Block, name: &str) -> &'a str {
    value(block, name)
        .as_str()
        .unwrap_or_else(|| panic!("{} field '{name}' must be a string", block.name))
}

fn number(block: &Block, name: &str) -> f64 {
    value(block, name)
        .as_number()
        .unwrap_or_else(|| panic!("{} field '{name}' must be a number", block.name))
}

fn boolean(block: &Block, name: &str) -> bool {
    value(block, name)
        .as_bool()
        .unwrap_or_else(|| panic!("{} field '{name}' must be a boolean", block.name))
}

fn string_list(block: &Block, name: &str) -> Vec<String> {
    let Value::List(values) = value(block, name) else {
        panic!("{} field '{name}' must be a list", block.name);
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("{} field '{name}' must contain strings", block.name))
                .to_owned()
        })
        .collect()
}
