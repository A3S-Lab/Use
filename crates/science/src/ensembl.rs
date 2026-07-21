use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use serde::Deserialize;

use crate::models::{EnsemblGene, EnsemblHomolog};
use crate::ScienceClient;

const ENSEMBL_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Debug, Deserialize)]
struct GeneResponse {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    species: Option<String>,
    #[serde(default)]
    biotype: Option<String>,
    #[serde(default)]
    seq_region_name: Option<String>,
    #[serde(default)]
    start: Option<u64>,
    #[serde(default)]
    end: Option<u64>,
    #[serde(default)]
    strand: Option<i8>,
}

#[derive(Debug, Deserialize)]
struct HomologyEnvelope {
    #[serde(default)]
    data: Vec<HomologyData>,
}

#[derive(Debug, Deserialize)]
struct HomologyData {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    homologies: Vec<RawHomology>,
}

#[derive(Debug, Deserialize)]
struct RawHomology {
    #[serde(default, rename = "type")]
    homology_type: Option<String>,
    target: HomologyTarget,
}

#[derive(Debug, Deserialize)]
struct HomologyTarget {
    id: String,
    #[serde(default)]
    species: Option<String>,
    #[serde(default)]
    protein_id: Option<String>,
    #[serde(default)]
    perc_id: Option<f64>,
    #[serde(default)]
    perc_pos: Option<f64>,
}

impl ScienceClient {
    pub async fn ensembl_lookup_gene(&self, species: &str, symbol: &str) -> UseResult<EnsemblGene> {
        validate_species(species)?;
        validate_symbol(symbol)?;
        let url = self.endpoint_url(
            &self.endpoints.ensembl,
            &["lookup", "symbol", species, symbol],
        )?;
        let response: GeneResponse = self
            .get_json(
                "Ensembl",
                self.http
                    .get(url)
                    .header(reqwest::header::ACCEPT, "application/json"),
                ENSEMBL_INTERVAL,
            )
            .await?;
        Ok(EnsemblGene {
            id: response.id,
            display_name: response.display_name,
            description: response.description,
            species: response.species,
            biotype: response.biotype,
            chromosome: response.seq_region_name,
            start: response.start,
            end: response.end,
            strand: response.strand,
        })
    }

    pub async fn ensembl_homologs(
        &self,
        species: &str,
        symbol: &str,
        target_species: Option<&str>,
        limit: usize,
    ) -> UseResult<Vec<EnsemblHomolog>> {
        validate_species(species)?;
        validate_symbol(symbol)?;
        if let Some(target_species) = target_species {
            validate_species(target_species)?;
        }
        if !(1..=200).contains(&limit) {
            return Err(UseError::new(
                "use.science.limit_invalid",
                "Ensembl homolog result limit must be between 1 and 200.",
            ));
        }
        let url = self.endpoint_url(
            &self.endpoints.ensembl,
            &["homology", "symbol", species, symbol],
        )?;
        let mut query = vec![
            ("type", "orthologues".to_string()),
            ("format", "condensed".to_string()),
        ];
        if let Some(target_species) = target_species {
            query.push(("target_species", target_species.to_string()));
        }
        let envelope: HomologyEnvelope = self
            .get_json(
                "Ensembl",
                self.http
                    .get(url)
                    .query(&query)
                    .header(reqwest::header::ACCEPT, "application/json"),
                ENSEMBL_INTERVAL,
            )
            .await?;
        let mut items = Vec::new();
        for data in envelope.data {
            for homology in data.homologies {
                items.push(EnsemblHomolog {
                    homology_type: homology.homology_type,
                    source_gene_id: data.id.clone(),
                    target_gene_id: homology.target.id,
                    target_species: homology.target.species,
                    target_protein_id: homology.target.protein_id,
                    identity_percent: homology.target.perc_id,
                    positive_percent: homology.target.perc_pos,
                });
                if items.len() == limit {
                    return Ok(items);
                }
            }
        }
        Ok(items)
    }
}

fn validate_species(species: &str) -> UseResult<()> {
    if species.is_empty()
        || species.len() > 100
        || !species
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    {
        return Err(UseError::new(
            "use.science.input_invalid",
            "Ensembl species must use a lowercase identifier such as homo_sapiens.",
        ));
    }
    Ok(())
}

fn validate_symbol(symbol: &str) -> UseResult<()> {
    if symbol.is_empty()
        || symbol.len() > 100
        || !symbol
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(UseError::new(
            "use.science.input_invalid",
            "Ensembl gene symbol contains unsupported characters.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_species_and_symbols_without_accepting_paths() {
        assert!(validate_species("homo_sapiens").is_ok());
        assert!(validate_symbol("TP53").is_ok());
        assert_eq!(
            validate_species("../human").unwrap_err().code,
            "use.science.input_invalid"
        );
        assert_eq!(
            validate_symbol("TP53/../../x").unwrap_err().code,
            "use.science.input_invalid"
        );
    }
}
