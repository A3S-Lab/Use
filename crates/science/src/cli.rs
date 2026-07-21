use a3s_use_core::{UseError, UseResult};
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::{ScienceClient, ScienceMcpServer};

#[derive(Debug)]
pub struct CommandOutput {
    pub human: String,
    pub json: serde_json::Value,
    pub exit_code: u8,
    pub should_print: bool,
}

impl CommandOutput {
    fn data<T>(value: T) -> UseResult<Self>
    where
        T: Serialize,
    {
        let data = serde_json::to_value(value).map_err(output_error)?;
        let human = serde_json::to_string_pretty(&data).map_err(output_error)?;
        Ok(Self {
            human,
            json: serde_json::json!({
                "schemaVersion": 1,
                "ok": true,
                "data": data,
            }),
            exit_code: 0,
            should_print: true,
        })
    }

    fn text(value: String) -> Self {
        Self {
            human: value.clone(),
            json: serde_json::json!({
                "schemaVersion": 1,
                "ok": true,
                "data": { "text": value },
            }),
            exit_code: 0,
            should_print: true,
        }
    }

    fn silent() -> Self {
        Self {
            human: String::new(),
            json: serde_json::Value::Null,
            exit_code: 0,
            should_print: false,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "a3s-use-science",
    version,
    about = "Read-only life-science data tools for A3S Use",
    arg_required_else_help = true
)]
struct Cli {
    /// Emit one versioned JSON document.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Inspect local configuration without making a network request.
    Doctor,
    /// Search or retrieve PubMed article summaries.
    Pubmed(PubmedArgs),
    /// Search ChEMBL molecules, targets, and activities.
    Chembl(ChemblArgs),
    /// Search or retrieve ClinicalTrials.gov studies.
    #[command(name = "clinical-trials")]
    ClinicalTrials(ClinicalTrialsArgs),
    /// Search or retrieve bioRxiv preprints.
    Biorxiv(BioRxivArgs),
    /// Look up Ensembl genes and homologs.
    Ensembl(EnsemblArgs),
    /// Run an extension protocol surface.
    Serve(ServeArgs),
}

#[derive(Debug, Args)]
struct PubmedArgs {
    #[command(subcommand)]
    command: PubmedCommand,
}

#[derive(Debug, Subcommand)]
enum PubmedCommand {
    /// Search PubMed and return article summaries.
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Retrieve one PubMed article summary by PMID.
    Get { pmid: String },
}

#[derive(Debug, Args)]
struct ChemblArgs {
    #[command(subcommand)]
    command: ChemblCommand,
}

#[derive(Debug, Subcommand)]
enum ChemblCommand {
    /// Search ChEMBL molecules.
    #[command(name = "search-molecules")]
    SearchMolecules {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Retrieve one ChEMBL molecule.
    #[command(name = "get-molecule")]
    GetMolecule { chembl_id: String },
    /// Search ChEMBL targets.
    #[command(name = "search-targets")]
    SearchTargets {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Retrieve bioactivity records for a molecule, target, or both.
    Activities {
        #[arg(long = "molecule")]
        molecule_chembl_id: Option<String>,
        #[arg(long = "target")]
        target_chembl_id: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Debug, Args)]
struct ClinicalTrialsArgs {
    #[command(subcommand)]
    command: ClinicalTrialsCommand,
}

#[derive(Debug, Subcommand)]
enum ClinicalTrialsCommand {
    /// Search ClinicalTrials.gov studies.
    Search {
        query: String,
        #[arg(long = "status")]
        statuses: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        page_token: Option<String>,
    },
    /// Retrieve one study by NCT identifier.
    Get { nct_id: String },
}

#[derive(Debug, Args)]
struct BioRxivArgs {
    #[command(subcommand)]
    command: BioRxivCommand,
}

#[derive(Debug, Subcommand)]
enum BioRxivCommand {
    /// Search a bounded bioRxiv date range.
    Search {
        #[arg(long = "from")]
        from_date: String,
        #[arg(long = "to")]
        to_date: String,
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        category: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Retrieve all returned versions of one bioRxiv DOI.
    Get { doi: String },
}

#[derive(Debug, Args)]
struct EnsemblArgs {
    #[command(subcommand)]
    command: EnsemblCommand,
}

#[derive(Debug, Subcommand)]
enum EnsemblCommand {
    /// Look up one gene by species and symbol.
    Lookup { species: String, symbol: String },
    /// Retrieve orthologs for one gene symbol.
    Homologs {
        species: String,
        symbol: String,
        #[arg(long)]
        target_species: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

#[derive(Debug, Args)]
struct ServeArgs {
    /// Serve standard MCP over stdin/stdout.
    #[arg(long)]
    mcp: bool,
}

pub async fn run(args: Vec<String>) -> UseResult<CommandOutput> {
    let mut argv = vec!["a3s-use-science".to_string()];
    argv.extend(args);
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return Ok(CommandOutput::text(error.to_string()));
        }
        Err(error) => return Err(usage_error(error.to_string())),
    };

    if let Command::Serve(serve) = &cli.command {
        if !serve.mcp {
            return Err(usage_error("serve requires --mcp"));
        }
        if cli.json {
            return Err(usage_error("--json cannot be combined with serve --mcp"));
        }
        ScienceMcpServer::from_env()?.serve_stdio().await?;
        return Ok(CommandOutput::silent());
    }

    let client = ScienceClient::from_env()?;
    match cli.command {
        Command::Doctor => CommandOutput::data(client.diagnostic()),
        Command::Pubmed(args) => match args.command {
            PubmedCommand::Search { query, limit } => {
                CommandOutput::data(client.pubmed_search(&query, limit).await?)
            }
            PubmedCommand::Get { pmid } => CommandOutput::data(client.pubmed_get(&pmid).await?),
        },
        Command::Chembl(args) => match args.command {
            ChemblCommand::SearchMolecules { query, limit } => {
                CommandOutput::data(client.chembl_search_molecules(&query, limit).await?)
            }
            ChemblCommand::GetMolecule { chembl_id } => {
                CommandOutput::data(client.chembl_get_molecule(&chembl_id).await?)
            }
            ChemblCommand::SearchTargets { query, limit } => {
                CommandOutput::data(client.chembl_search_targets(&query, limit).await?)
            }
            ChemblCommand::Activities {
                molecule_chembl_id,
                target_chembl_id,
                limit,
            } => CommandOutput::data(
                client
                    .chembl_activities(
                        molecule_chembl_id.as_deref(),
                        target_chembl_id.as_deref(),
                        limit,
                    )
                    .await?,
            ),
        },
        Command::ClinicalTrials(args) => match args.command {
            ClinicalTrialsCommand::Search {
                query,
                statuses,
                limit,
                page_token,
            } => CommandOutput::data(
                client
                    .clinical_trials_search(&query, &statuses, limit, page_token.as_deref())
                    .await?,
            ),
            ClinicalTrialsCommand::Get { nct_id } => {
                CommandOutput::data(client.clinical_trial_get(&nct_id).await?)
            }
        },
        Command::Biorxiv(args) => match args.command {
            BioRxivCommand::Search {
                from_date,
                to_date,
                query,
                category,
                limit,
            } => CommandOutput::data(
                client
                    .biorxiv_search(
                        &from_date,
                        &to_date,
                        query.as_deref(),
                        category.as_deref(),
                        limit,
                    )
                    .await?,
            ),
            BioRxivCommand::Get { doi } => CommandOutput::data(client.biorxiv_get(&doi).await?),
        },
        Command::Ensembl(args) => match args.command {
            EnsemblCommand::Lookup { species, symbol } => {
                CommandOutput::data(client.ensembl_lookup_gene(&species, &symbol).await?)
            }
            EnsemblCommand::Homologs {
                species,
                symbol,
                target_species,
                limit,
            } => CommandOutput::data(
                client
                    .ensembl_homologs(&species, &symbol, target_species.as_deref(), limit)
                    .await?,
            ),
        },
        Command::Serve(_) => Err(UseError::new(
            "use.science.command_invalid",
            "Science MCP command dispatch reached an invalid state.",
        )),
    }
}

fn output_error(error: serde_json::Error) -> UseError {
    UseError::new(
        "use.science.output_invalid",
        format!("Failed to encode science command output: {error}"),
    )
}

fn usage_error(message: impl Into<String>) -> UseError {
    UseError::new("use.science.usage_invalid", message)
        .with_suggestion("Run 'a3s use science --help'.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn doctor_is_versioned_and_does_not_require_network_configuration() {
        let output = run(vec!["doctor".to_string(), "--json".to_string()])
            .await
            .unwrap();
        assert_eq!(output.json["schemaVersion"], 1);
        assert_eq!(output.json["ok"], true);
        assert_eq!(output.json["data"]["sources"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn serve_requires_an_explicit_protocol() {
        let error = run(vec!["serve".to_string()]).await.unwrap_err();
        assert_eq!(error.code, "use.science.usage_invalid");
    }
}
