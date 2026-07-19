//! Typed, read-only life-science data retrieval for A3S Use.
//!
//! The crate deliberately exposes public upstream contracts rather than a
//! generic action envelope. [ScienceClient] is suitable for embedding, while
//! [ScienceMcpServer] presents the same operations as standard MCP tools.

mod biorxiv;
mod chembl;
pub mod cli;
mod client;
mod clinical_trials;
mod ensembl;
pub mod mcp;
mod models;
mod pubmed;

pub use client::{ScienceClient, ScienceClientBuilder, ScienceEndpoints};
pub use mcp::ScienceMcpServer;
pub use models::{
    BioRxivPage, BioRxivRecord, ChemblActivity, ChemblMolecule, ChemblTarget, ClinicalTrial,
    ClinicalTrialPage, EnsemblGene, EnsemblHomolog, Page, PubMedArticle, ScienceDiagnostic,
};

pub use a3s_use_core::{UseError, UseResult};
