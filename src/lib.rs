//! Typed facade for A3S application capabilities.

#[cfg(test)]
pub(crate) fn test_installation() -> a3s_use_core::InstallationId {
    a3s_use_core::InstallationId::new(a3s_use_core::InstallationKind::User, "user/current")
        .expect("the fixed test installation must be valid")
}

#[cfg(all(test, feature = "extensions"))]
pub(crate) fn test_extension_paths(root: &std::path::Path) -> a3s_use_extension::ExtensionPaths {
    a3s_use_extension::ExtensionPaths::new(
        root.join("data"),
        root.join("state"),
        test_installation(),
    )
    .expect("the fixed test installation paths must be valid")
}

#[cfg(feature = "extensions")]
pub mod artifact_reachability;
#[cfg(feature = "browser")]
mod browser_cli;
#[cfg(feature = "browser")]
mod browser_driver;
#[cfg(all(feature = "browser", feature = "mcp"))]
mod browser_session_cli;
pub mod capability_registry;
pub mod cli;
#[cfg(feature = "extensions")]
pub mod cognitive_package;
mod component_route;
#[cfg(feature = "extensions")]
#[cfg_attr(not(test), allow(dead_code))]
mod control_store;
mod extension_cli;
mod first_use;
#[cfg(feature = "extensions")]
pub mod flow_runtime;
#[cfg(feature = "extensions")]
mod installation_state_layout;
#[cfg(feature = "extensions")]
pub mod plugin_lifecycle;
#[cfg(feature = "extensions")]
pub mod plugin_manager;
#[cfg(feature = "extensions")]
pub mod plugin_runtime;
#[cfg(feature = "extensions")]
pub mod state_backup;
#[cfg(feature = "extensions")]
pub mod state_restore;
#[cfg(feature = "extensions")]
mod surface_graph;
#[cfg(feature = "extensions")]
mod surface_reconciler;

#[cfg(all(test, feature = "extensions", any(unix, windows)))]
mod test_filesystem;

#[cfg(feature = "ocr")]
mod ocr_builtin;

#[cfg(feature = "mcp")]
mod mcp;

#[cfg(feature = "extensions")]
mod extension_host;

#[cfg(feature = "extensions")]
pub mod okf_knowledge;

pub use a3s_use_core as core;

#[cfg(feature = "browser")]
pub use a3s_use_browser as browser;

#[cfg(feature = "ocr")]
pub use a3s_use_ocr as ocr;

#[cfg(feature = "extensions")]
pub use a3s_use_extension as extension;
