use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub total: Option<u64>,
    pub next_page_token: Option<String>,
    pub items: Vec<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PubMedArticle {
    pub pmid: String,
    pub title: String,
    pub authors: Vec<String>,
    pub journal: Option<String>,
    pub publication_date: Option<String>,
    pub doi: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChemblMolecule {
    pub chembl_id: String,
    pub preferred_name: Option<String>,
    pub molecule_type: Option<String>,
    pub max_phase: Option<f64>,
    pub canonical_smiles: Option<String>,
    pub standard_inchi_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChemblTarget {
    pub chembl_id: String,
    pub preferred_name: Option<String>,
    pub target_type: Option<String>,
    pub organism: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChemblActivity {
    pub activity_id: Option<String>,
    pub molecule_chembl_id: Option<String>,
    pub target_chembl_id: Option<String>,
    pub assay_chembl_id: Option<String>,
    pub standard_type: Option<String>,
    pub standard_relation: Option<String>,
    pub standard_value: Option<String>,
    pub standard_units: Option<String>,
    pub pchembl_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClinicalTrial {
    pub nct_id: String,
    pub brief_title: String,
    pub official_title: Option<String>,
    pub overall_status: Option<String>,
    pub study_type: Option<String>,
    pub phases: Vec<String>,
    pub conditions: Vec<String>,
    pub interventions: Vec<String>,
    pub lead_sponsor: Option<String>,
    pub enrollment: Option<u64>,
    pub start_date: Option<String>,
    pub completion_date: Option<String>,
}

pub type ClinicalTrialPage = Page<ClinicalTrial>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BioRxivRecord {
    pub doi: String,
    pub title: String,
    pub authors: String,
    pub abstract_text: Option<String>,
    pub category: Option<String>,
    pub date: Option<String>,
    pub version: Option<String>,
    pub published_doi: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BioRxivPage {
    pub total_upstream: Option<u64>,
    pub scanned: usize,
    pub items: Vec<BioRxivRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsemblGene {
    pub id: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub species: Option<String>,
    pub biotype: Option<String>,
    pub chromosome: Option<String>,
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub strand: Option<i8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsemblHomolog {
    pub homology_type: Option<String>,
    pub source_gene_id: Option<String>,
    pub target_gene_id: String,
    pub target_species: Option<String>,
    pub target_protein_id: Option<String>,
    pub identity_percent: Option<f64>,
    pub positive_percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScienceDiagnostic {
    pub version: String,
    pub contact_email_configured: bool,
    pub ncbi_api_key_configured: bool,
    pub sources: Vec<String>,
    pub message: String,
}
