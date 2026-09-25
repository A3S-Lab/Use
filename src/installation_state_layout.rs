use a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER;

pub(crate) const CONTROL_INSTALLATION_RESTORE_ATTEMPT_DIRECTORY: &str =
    ".control-installation-restore";

const STATE_DIRECTORIES: &[&str] = &[
    "bindings",
    "capability-gateway",
    "capability-index",
    "extension-generations",
    "extensions",
    "generation-leases",
    "grants",
    "knowledge",
    "operations",
    "package-enablement",
    "plugin-host-manager",
    "runtime-plans",
];

const DERIVED_STATE_DIRECTORIES: &[&str] = &["capability-index"];

const STATE_FILES: &[&str] = &[
    "control.sqlite3",
    "installation-snapshot.json",
    "registry.json",
];

const STATE_ROOT_LOCKS: &[&str] = &[
    ".installation-mutation.lock",
    ".maintenance.lock",
    ".package-graph.lock",
];

/// Operational SQLite sidecars and restore staging that are not authority leaves.
const OPERATIONAL_STATE_FILES: &[&str] = &[
    ".control-restore.sqlite3",
    "control.sqlite3-journal",
    "control.sqlite3-shm",
    "control.sqlite3-wal",
];

const OPERATION_DIRECTORIES: &[&str] = &[
    "package-diagnostic-history",
    "package-downloads",
    "package-graphs",
    "package-resolutions",
    "plugins",
    "state-restores",
];

pub(crate) fn supported_root_entry(name: &str, directory: bool) -> bool {
    if directory {
        STATE_DIRECTORIES.binary_search(&name).is_ok()
    } else {
        STATE_FILES.binary_search(&name).is_ok()
            || STATE_ROOT_LOCKS.binary_search(&name).is_ok()
            || OPERATIONAL_STATE_FILES.binary_search(&name).is_ok()
            || name == ACTIVE_STATE_RESTORE_MARKER
    }
}

pub(crate) fn supported_operation_directory(name: &str) -> bool {
    OPERATION_DIRECTORIES.binary_search(&name).is_ok()
}

pub(crate) fn supported_binding_directory(name: &str) -> bool {
    matches!(name, "flow" | "knowledge" | "runtime")
}

pub(crate) fn excluded_root_lock(name: &str) -> bool {
    STATE_ROOT_LOCKS.binary_search(&name).is_ok()
}

pub(crate) fn excluded_operational_state_file(name: &str) -> bool {
    OPERATIONAL_STATE_FILES.binary_search(&name).is_ok()
}

pub(crate) fn excluded_derived_root(name: &str) -> bool {
    DERIVED_STATE_DIRECTORIES.binary_search(&name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_state_layout_is_sorted_and_classified_once() {
        assert!(STATE_DIRECTORIES.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(STATE_FILES.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(STATE_ROOT_LOCKS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(OPERATIONAL_STATE_FILES
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(DERIVED_STATE_DIRECTORIES
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(OPERATION_DIRECTORIES
            .windows(2)
            .all(|pair| pair[0] < pair[1]));
        assert!(supported_root_entry("operations", true));
        assert!(supported_root_entry("control.sqlite3", false));
        assert!(supported_root_entry("installation-snapshot.json", false));
        assert!(supported_root_entry("control.sqlite3-wal", false));
        assert!(excluded_operational_state_file("control.sqlite3-wal"));
        assert!(supported_operation_directory("package-graphs"));
        assert!(supported_binding_directory("knowledge"));
        assert!(excluded_derived_root("capability-index"));
        assert!(!supported_root_entry(
            CONTROL_INSTALLATION_RESTORE_ATTEMPT_DIRECTORY,
            true
        ));
        assert!(!supported_root_entry("future-authority", true));
        // Host Registry transport/config stays outside the installation cutover
        // unit (backup may still scan them via ExtensionPaths).
        for host_registry_leaf in [
            "registries.acl",
            "registry-trust-roots",
            "remote-registries",
        ] {
            assert!(
                !supported_root_entry(host_registry_leaf, false)
                    && !supported_root_entry(host_registry_leaf, true),
                "{host_registry_leaf} must remain outside installation-state cutover leaves"
            );
        }
    }
}
