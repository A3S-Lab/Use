//! Typed facade for A3S application capabilities.

#[cfg(feature = "browser")]
mod browser_cli;
#[cfg(feature = "browser")]
mod browser_driver;
#[cfg(all(feature = "browser", feature = "mcp"))]
mod browser_session_cli;
mod capability_registry;
pub mod cli;
#[cfg(feature = "extensions")]
pub mod cognitive_package;
mod component_route;
mod extension_cli;
mod first_use;
#[cfg(feature = "extensions")]
pub mod flow_runtime;
#[cfg(feature = "extensions")]
pub mod plugin_lifecycle;
#[cfg(feature = "extensions")]
pub mod plugin_runtime;
#[cfg(feature = "extensions")]
mod release_bundles;
#[cfg(feature = "extensions")]
mod surface_reconciler;

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
