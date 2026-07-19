use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use serde::Deserialize;
use serde_json::Value;

use crate::models::{ChemblActivity, ChemblMolecule, ChemblTarget, Page};
use crate::ScienceClient;

const CHEMBL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Default, Deserialize)]
struct PageMeta {
    #[serde(default)]
    total_count: Option<u64>,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MoleculeEnvelope {
    #[serde(default)]
    molecules: Vec<Value>,
    #[serde(default)]
    page_meta: PageMeta,
}

#[derive(Debug, Deserialize)]
struct TargetEnvelope {
    #[serde(default)]
    targets: Vec<Value>,
    #[serde(default)]
    page_meta: PageMeta,
}

#[derive(Debug, Deserialize)]
struct ActivityEnvelope {
    #[serde(default)]
    activities: Vec<Value>,
    #[serde(default)]
    page_meta: PageMeta,
}

impl ScienceClient {
    pub async fn chembl_search_molecules(
        &self,
        query: &str,
        limit: usize,
    ) -> UseResult<Page<ChemblMolecule>> {
        let query = required_query(query)?;
        let limit = bounded_limit(limit)?;
        let url = self.endpoint_url(&self.endpoints.chembl, &["molecule", "search.json"])?;
        let params = [("q", query.to_string()), ("limit", limit.to_string())];
        let envelope: MoleculeEnvelope = self
            .get_json("ChEMBL", self.http.get(url).query(&params), CHEMBL_INTERVAL)
            .await?;
        Ok(Page {
            total: envelope.page_meta.total_count,
            next_page_token: envelope.page_meta.next,
            items: envelope
                .molecules
                .iter()
                .filter_map(parse_molecule)
                .collect(),
        })
    }

    pub async fn chembl_get_molecule(&self, chembl_id: &str) -> UseResult<ChemblMolecule> {
        validate_chembl_id(chembl_id)?;
        let url = self.endpoint_url(
            &self.endpoints.chembl,
            &["molecule", &format!("{chembl_id}.json")],
        )?;
        let value: Value = self
            .get_json("ChEMBL", self.http.get(url), CHEMBL_INTERVAL)
            .await?;
        parse_molecule(&value).ok_or_else(|| {
            UseError::new(
                "use.science.response_invalid",
                "ChEMBL returned a molecule without a molecule_chembl_id.",
            )
        })
    }

    pub async fn chembl_search_targets(
        &self,
        query: &str,
        limit: usize,
    ) -> UseResult<Page<ChemblTarget>> {
        let query = required_query(query)?;
        let limit = bounded_limit(limit)?;
        let url = self.endpoint_url(&self.endpoints.chembl, &["target", "search.json"])?;
        let params = [("q", query.to_string()), ("limit", limit.to_string())];
        let envelope: TargetEnvelope = self
            .get_json("ChEMBL", self.http.get(url).query(&params), CHEMBL_INTERVAL)
            .await?;
        Ok(Page {
            total: envelope.page_meta.total_count,
            next_page_token: envelope.page_meta.next,
            items: envelope.targets.iter().filter_map(parse_target).collect(),
        })
    }

    pub async fn chembl_activities(
        &self,
        molecule_chembl_id: Option<&str>,
        target_chembl_id: Option<&str>,
        limit: usize,
    ) -> UseResult<Page<ChemblActivity>> {
        let limit = bounded_limit(limit)?;
        if molecule_chembl_id.is_none() && target_chembl_id.is_none() {
            return Err(UseError::new(
                "use.science.input_invalid",
                "ChEMBL activities require a molecule or target ChEMBL ID.",
            ));
        }
        if let Some(identifier) = molecule_chembl_id {
            validate_chembl_id(identifier)?;
        }
        if let Some(identifier) = target_chembl_id {
            validate_chembl_id(identifier)?;
        }
        let url = self.endpoint_url(&self.endpoints.chembl, &["activity.json"])?;
        let mut query = vec![("limit", limit.to_string())];
        if let Some(identifier) = molecule_chembl_id {
            query.push(("molecule_chembl_id", identifier.to_string()));
        }
        if let Some(identifier) = target_chembl_id {
            query.push(("target_chembl_id", identifier.to_string()));
        }
        let envelope: ActivityEnvelope = self
            .get_json("ChEMBL", self.http.get(url).query(&query), CHEMBL_INTERVAL)
            .await?;
        Ok(Page {
            total: envelope.page_meta.total_count,
            next_page_token: envelope.page_meta.next,
            items: envelope.activities.iter().map(parse_activity).collect(),
        })
    }
}

fn parse_molecule(value: &Value) -> Option<ChemblMolecule> {
    Some(ChemblMolecule {
        chembl_id: value.get("molecule_chembl_id")?.as_str()?.to_string(),
        preferred_name: value_string(value.get("pref_name")),
        molecule_type: value_string(value.get("molecule_type")),
        max_phase: value.get("max_phase").and_then(value_f64),
        canonical_smiles: value
            .get("molecule_structures")
            .and_then(|structures| structures.get("canonical_smiles"))
            .and_then(Value::as_str)
            .map(str::to_string),
        standard_inchi_key: value
            .get("molecule_structures")
            .and_then(|structures| structures.get("standard_inchi_key"))
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn parse_target(value: &Value) -> Option<ChemblTarget> {
    Some(ChemblTarget {
        chembl_id: value.get("target_chembl_id")?.as_str()?.to_string(),
        preferred_name: value_string(value.get("pref_name")),
        target_type: value_string(value.get("target_type")),
        organism: value_string(value.get("organism")),
    })
}

fn parse_activity(value: &Value) -> ChemblActivity {
    ChemblActivity {
        activity_id: value_string(value.get("activity_id")),
        molecule_chembl_id: value_string(value.get("molecule_chembl_id")),
        target_chembl_id: value_string(value.get("target_chembl_id")),
        assay_chembl_id: value_string(value.get("assay_chembl_id")),
        standard_type: value_string(value.get("standard_type")),
        standard_relation: value_string(value.get("standard_relation")),
        standard_value: value_string(value.get("standard_value")),
        standard_units: value_string(value.get("standard_units")),
        pchembl_value: value_string(value.get("pchembl_value")),
    }
}

fn value_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn required_query(query: &str) -> UseResult<&str> {
    let query = query.trim();
    if query.is_empty() {
        return Err(UseError::new(
            "use.science.input_invalid",
            "ChEMBL query cannot be empty.",
        ));
    }
    Ok(query)
}

fn bounded_limit(limit: usize) -> UseResult<usize> {
    if !(1..=100).contains(&limit) {
        return Err(UseError::new(
            "use.science.limit_invalid",
            "ChEMBL result limit must be between 1 and 100.",
        ));
    }
    Ok(limit)
}

fn validate_chembl_id(identifier: &str) -> UseResult<()> {
    let suffix = identifier.strip_prefix("CHEMBL").unwrap_or_default();
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(UseError::new(
            "use.science.identifier_invalid",
            "A ChEMBL identifier must use the form CHEMBL followed by digits.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typed_chembl_records() {
        let molecule = serde_json::json!({
            "molecule_chembl_id": "CHEMBL25",
            "pref_name": "ASPIRIN",
            "molecule_type": "Small molecule",
            "max_phase": 4,
            "molecule_structures": {
                "canonical_smiles": "CC(=O)OC1=CC=CC=C1C(=O)O",
                "standard_inchi_key": "BSYNRYMUTXBXSQ-UHFFFAOYSA-N"
            }
        });
        let parsed = parse_molecule(&molecule).unwrap();
        assert_eq!(parsed.chembl_id, "CHEMBL25");
        assert_eq!(parsed.max_phase, Some(4.0));

        let activity = parse_activity(&serde_json::json!({
            "activity_id": 42,
            "standard_value": "12.5"
        }));
        assert_eq!(activity.activity_id.as_deref(), Some("42"));
    }

    #[test]
    fn rejects_unbounded_or_malformed_inputs() {
        assert_eq!(
            validate_chembl_id("../CHEMBL25").unwrap_err().code,
            "use.science.identifier_invalid"
        );
        assert_eq!(
            bounded_limit(101).unwrap_err().code,
            "use.science.limit_invalid"
        );
    }
}
