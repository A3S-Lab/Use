//! Standard MCP tools for the process-isolated Science extension.

use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::{Deserialize, Serialize};

use crate::{ScienceClient, UseError, UseResult};

const DEFAULT_LIMIT: usize = 20;

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct PubMedSearchInput {
    #[schemars(description = "PubMed search expression")]
    query: String,
    #[schemars(description = "Maximum article summaries to return; defaults to 20, maximum 100")]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct PubMedGetInput {
    #[schemars(description = "PubMed identifier containing 1 to 12 digits")]
    pmid: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ChemblSearchInput {
    #[schemars(description = "Free-text ChEMBL search expression")]
    query: String,
    #[schemars(description = "Maximum records to return; defaults to 20, maximum 100")]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ChemblMoleculeInput {
    #[schemars(description = "ChEMBL molecule identifier, such as CHEMBL25")]
    chembl_id: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ChemblActivitiesInput {
    #[schemars(description = "Optional ChEMBL molecule identifier")]
    molecule_chembl_id: Option<String>,
    #[schemars(description = "Optional ChEMBL target identifier")]
    target_chembl_id: Option<String>,
    #[schemars(description = "Maximum activities to return; defaults to 20, maximum 100")]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ClinicalTrialsSearchInput {
    #[schemars(description = "ClinicalTrials.gov query expression")]
    query: String,
    #[schemars(description = "Optional uppercase statuses, such as RECRUITING")]
    statuses: Option<Vec<String>>,
    #[schemars(description = "Maximum studies to return; defaults to 20, maximum 100")]
    limit: Option<usize>,
    #[schemars(description = "Opaque next-page token from a prior response")]
    page_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ClinicalTrialGetInput {
    #[schemars(description = "ClinicalTrials.gov identifier, such as NCT01234567")]
    nct_id: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct BioRxivSearchInput {
    #[schemars(description = "Inclusive range start in YYYY-MM-DD form")]
    from_date: String,
    #[schemars(description = "Inclusive range end in YYYY-MM-DD form")]
    to_date: String,
    #[schemars(description = "Optional case-insensitive title, author, abstract, or DOI filter")]
    query: Option<String>,
    #[schemars(description = "Optional exact bioRxiv category filter")]
    category: Option<String>,
    #[schemars(description = "Maximum preprints to return; defaults to 20, maximum 100")]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct BioRxivGetInput {
    #[schemars(description = "bioRxiv DOI beginning with 10.1101/")]
    doi: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct EnsemblLookupInput {
    #[schemars(description = "Lowercase Ensembl species identifier, such as homo_sapiens")]
    species: String,
    #[schemars(description = "Gene symbol, such as TP53")]
    symbol: String,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct EnsemblHomologsInput {
    #[schemars(description = "Lowercase source species identifier")]
    species: String,
    #[schemars(description = "Source gene symbol")]
    symbol: String,
    #[schemars(description = "Optional lowercase target species identifier")]
    target_species: Option<String>,
    #[schemars(description = "Maximum homologs to return; defaults to 50, maximum 200")]
    limit: Option<usize>,
}

#[derive(Clone)]
pub struct ScienceMcpServer {
    client: ScienceClient,
    tool_router: ToolRouter<Self>,
}

impl ScienceMcpServer {
    pub fn new(client: ScienceClient) -> Self {
        Self {
            client,
            tool_router: Self::tool_router(),
        }
    }

    pub fn from_env() -> UseResult<Self> {
        Ok(Self::new(ScienceClient::from_env()?))
    }

    /// Serve standard MCP framing over stdin/stdout until the peer disconnects.
    pub async fn serve_stdio(self) -> UseResult<()> {
        let service = self
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| mcp_error("start", error))?;
        service
            .waiting()
            .await
            .map_err(|error| mcp_error("run", error))?;
        Ok(())
    }
}

#[tool_router]
impl ScienceMcpServer {
    #[tool(
        name = "science_doctor",
        description = "Inspect Science extension configuration without making a network request"
    )]
    async fn science_doctor(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(Ok(self.client.diagnostic())))
    }

    #[tool(
        name = "science_pubmed_search",
        description = "Search PubMed and return typed article summaries with stable identifiers"
    )]
    async fn science_pubmed_search(
        &self,
        Parameters(input): Parameters<PubMedSearchInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client
                .pubmed_search(&input.query, input.limit.unwrap_or(DEFAULT_LIMIT))
                .await,
        ))
    }

    #[tool(
        name = "science_pubmed_get",
        description = "Retrieve one PubMed article summary by PMID"
    )]
    async fn science_pubmed_get(
        &self,
        Parameters(input): Parameters<PubMedGetInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(self.client.pubmed_get(&input.pmid).await))
    }

    #[tool(
        name = "science_chembl_search_molecules",
        description = "Search ChEMBL molecules and return normalized identifiers and structures"
    )]
    async fn science_chembl_search_molecules(
        &self,
        Parameters(input): Parameters<ChemblSearchInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client
                .chembl_search_molecules(&input.query, input.limit.unwrap_or(DEFAULT_LIMIT))
                .await,
        ))
    }

    #[tool(
        name = "science_chembl_get_molecule",
        description = "Retrieve one normalized ChEMBL molecule record"
    )]
    async fn science_chembl_get_molecule(
        &self,
        Parameters(input): Parameters<ChemblMoleculeInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client.chembl_get_molecule(&input.chembl_id).await,
        ))
    }

    #[tool(
        name = "science_chembl_search_targets",
        description = "Search ChEMBL targets and return normalized target records"
    )]
    async fn science_chembl_search_targets(
        &self,
        Parameters(input): Parameters<ChemblSearchInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client
                .chembl_search_targets(&input.query, input.limit.unwrap_or(DEFAULT_LIMIT))
                .await,
        ))
    }

    #[tool(
        name = "science_chembl_activities",
        description = "Retrieve ChEMBL bioactivity records for a molecule, target, or both"
    )]
    async fn science_chembl_activities(
        &self,
        Parameters(input): Parameters<ChemblActivitiesInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client
                .chembl_activities(
                    input.molecule_chembl_id.as_deref(),
                    input.target_chembl_id.as_deref(),
                    input.limit.unwrap_or(DEFAULT_LIMIT),
                )
                .await,
        ))
    }

    #[tool(
        name = "science_clinical_trials_search",
        description = "Search ClinicalTrials.gov and return normalized protocol summaries"
    )]
    async fn science_clinical_trials_search(
        &self,
        Parameters(input): Parameters<ClinicalTrialsSearchInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client
                .clinical_trials_search(
                    &input.query,
                    input.statuses.as_deref().unwrap_or_default(),
                    input.limit.unwrap_or(DEFAULT_LIMIT),
                    input.page_token.as_deref(),
                )
                .await,
        ))
    }

    #[tool(
        name = "science_clinical_trial_get",
        description = "Retrieve one ClinicalTrials.gov study by NCT identifier"
    )]
    async fn science_clinical_trial_get(
        &self,
        Parameters(input): Parameters<ClinicalTrialGetInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client.clinical_trial_get(&input.nct_id).await,
        ))
    }

    #[tool(
        name = "science_biorxiv_search",
        description = "Search a bounded bioRxiv date range and return matching preprints"
    )]
    async fn science_biorxiv_search(
        &self,
        Parameters(input): Parameters<BioRxivSearchInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client
                .biorxiv_search(
                    &input.from_date,
                    &input.to_date,
                    input.query.as_deref(),
                    input.category.as_deref(),
                    input.limit.unwrap_or(DEFAULT_LIMIT),
                )
                .await,
        ))
    }

    #[tool(
        name = "science_biorxiv_get",
        description = "Retrieve all returned versions of one bioRxiv DOI"
    )]
    async fn science_biorxiv_get(
        &self,
        Parameters(input): Parameters<BioRxivGetInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(self.client.biorxiv_get(&input.doi).await))
    }

    #[tool(
        name = "science_ensembl_lookup_gene",
        description = "Look up an Ensembl gene by species and symbol"
    )]
    async fn science_ensembl_lookup_gene(
        &self,
        Parameters(input): Parameters<EnsemblLookupInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client
                .ensembl_lookup_gene(&input.species, &input.symbol)
                .await,
        ))
    }

    #[tool(
        name = "science_ensembl_homologs",
        description = "Retrieve Ensembl orthologs for one gene symbol"
    )]
    async fn science_ensembl_homologs(
        &self,
        Parameters(input): Parameters<EnsemblHomologsInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(tool_result(
            self.client
                .ensembl_homologs(
                    &input.species,
                    &input.symbol,
                    input.target_species.as_deref(),
                    input.limit.unwrap_or(50),
                )
                .await,
        ))
    }
}

#[tool_handler]
impl ServerHandler for ScienceMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "a3s-use-science".to_string(),
                title: Some("A3S Use Science".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: Some("https://github.com/A3S-Lab/Use".to_string()),
            },
            instructions: Some(
                "Use science_doctor before retrieval. Preserve returned source identifiers, distinguish bioRxiv preprints from peer-reviewed literature, and do not present public database records as medical advice. PubMed requires A3S_SCIENCE_CONTACT_EMAIL."
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}

fn tool_result<T>(result: UseResult<T>) -> CallToolResult
where
    T: Serialize,
{
    match result {
        Ok(output) => match serde_json::to_value(output) {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => tool_error(UseError::new(
                "use.science.output_invalid",
                format!("Failed to encode Science MCP output: {error}"),
            )),
        },
        Err(error) => tool_error(error),
    }
}

fn tool_error(error: UseError) -> CallToolResult {
    CallToolResult::structured_error(serde_json::to_value(error).unwrap_or_else(|_| {
        serde_json::json!({
            "code": "use.error_encoding_failed",
            "message": "Failed to encode A3S Use error."
        })
    }))
}

fn mcp_error(action: &str, error: impl std::fmt::Display) -> UseError {
    UseError::new(
        "use.science.mcp_failed",
        format!("Failed to {action} the Science MCP server: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_exposes_only_typed_read_tools() {
        let client = ScienceClient::builder().build().unwrap();
        let server = ScienceMcpServer::new(client);
        let mut names = server
            .tool_router
            .list_all()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "science_biorxiv_get",
                "science_biorxiv_search",
                "science_chembl_activities",
                "science_chembl_get_molecule",
                "science_chembl_search_molecules",
                "science_chembl_search_targets",
                "science_clinical_trial_get",
                "science_clinical_trials_search",
                "science_doctor",
                "science_ensembl_homologs",
                "science_ensembl_lookup_gene",
                "science_pubmed_get",
                "science_pubmed_search",
            ]
        );
    }
}
