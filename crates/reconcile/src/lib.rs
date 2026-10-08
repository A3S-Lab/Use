//! One package generation for the surfaces Studio materializes.
//!
//! A package may declare `skill`, `okf`, `ui`, `mcp`, `tool`, and `flow`.
//! Omitted surfaces are valid. This crate publishes the flow source. It does
//! not install environment-dependency tools.

mod error;
mod lease;
mod selection;
mod spec;
mod store;
mod unpack;
mod verification;

pub use error::ReconcileError;
pub use lease::{GenerationLease, RetirementReport};
pub use selection::{
    SelectionCursor, SelectionLease, SelectionRetirement, SelectionSnapshot, SELECTION_SCHEMA,
};
pub use spec::{
    files_sha256, payload_sha256, PackageFile, PackageSpec, Payload, SurfaceKind, SurfaceSpec,
    SCHEMA,
};
pub use store::{Publication, PublicationCursor, PublishedSurface, ReconcileStore};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "publication_tests.rs"]
mod publication_tests;

#[cfg(test)]
#[path = "lease_tests.rs"]
mod lease_tests;

#[cfg(test)]
#[path = "selection_tests.rs"]
mod selection_tests;

#[cfg(test)]
#[path = "mcp_entry_tests.rs"]
mod mcp_entry_tests;
