//! Typed facade for A3S application capabilities.

#[cfg(feature = "browser")]
mod browser_cli;
#[cfg(feature = "browser")]
mod browser_driver;
#[cfg(all(feature = "browser", feature = "mcp"))]
mod browser_session_cli;
mod capability_registry;
pub mod cli;
mod component_route;
mod extension_cli;

#[cfg(feature = "office")]
mod office_artifact;
#[cfg(feature = "office")]
mod office_native_cli;

#[cfg(all(feature = "browser", feature = "office"))]
pub mod office_screenshot;

#[cfg(feature = "mcp")]
mod mcp;

#[cfg(feature = "extensions")]
mod extension_host;

pub use a3s_use_core as core;

#[cfg(feature = "browser")]
pub use a3s_use_browser as browser;

#[cfg(feature = "office")]
pub use a3s_use_office as office;

#[cfg(feature = "extensions")]
pub use a3s_use_extension as extension;
