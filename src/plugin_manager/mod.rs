mod confirmation;
#[cfg(feature = "mcp")]
mod mcp;
#[cfg(feature = "mcp")]
mod mcp_error;
mod model;
mod service;

pub use confirmation::{
    FailClosedPluginManagerConfirmationProvider, PluginManagerConfirmationProvider,
};
#[cfg(feature = "mcp")]
pub use mcp::PluginManagerMcpServer;
pub use model::{
    PluginManagerInstalledPackage, PluginManagerInstalledPage, PluginManagerSearchResult,
};
pub use service::PluginManagerService;

#[cfg(test)]
mod tests;
