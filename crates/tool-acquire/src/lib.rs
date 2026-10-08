//! Machine-local installer for a closed set of command-line tool specs.
//!
//! This crate records installs under a host-supplied root. It does not publish
//! a package generation, and its receipts are not package-manager
//! authority.

mod acquire;
mod catalog;
mod error;
mod http;
mod lock;
mod receipt;
mod source;
mod spec;
mod store;

pub use acquire::AcquireRequest;
pub use catalog::{catalog, catalog_by_name, CatalogTool};
pub use error::ToolAcquireError;
pub use http::{FetchPolicy, HttpSource};
pub use receipt::{ToolFailureRecord, ToolReceipt, FAILURE_SCHEMA, RECEIPT_SCHEMA};
pub use source::{CompanionFile, StaticSource, ToolPayload, ToolQuery, ToolSource};
pub use spec::{parse_tool_spec, ToolBackend, ToolSpec};
pub use store::{LocalApply, ToolStore};
