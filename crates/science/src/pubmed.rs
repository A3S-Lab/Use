use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use serde::Deserialize;
use serde_json::Value;

use crate::models::{Page, PubMedArticle};
use crate::ScienceClient;

const PUBMED_KEYLESS_INTERVAL: Duration = Duration::from_millis(350);
const PUBMED_KEYED_INTERVAL: Duration = Duration::from_millis(110);

#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    esearchresult: SearchResult,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    count: String,
    #[serde(default)]
    idlist: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SummaryEnvelope {
    result: serde_json::Map<String, Value>,
}

impl ScienceClient {
    pub async fn pubmed_search(&self, query: &str, limit: usize) -> UseResult<Page<PubMedArticle>> {
        let query = required_text("PubMed query", query)?;
        let limit = bounded_limit(limit, 100)?;
        let contact_email = self.pubmed_contact_email()?;
        let url = self.endpoint_url(&self.endpoints.pubmed, &["esearch.fcgi"])?;
        let mut params = vec![
            ("db", "pubmed".to_string()),
            ("term", query.to_string()),
            ("retmode", "json".to_string()),
            ("retmax", limit.to_string()),
            ("tool", "a3s-use-science".to_string()),
            ("email", contact_email.to_string()),
        ];
        if let Some(api_key) = &self.ncbi_api_key {
            params.push(("api_key", api_key.clone()));
        }
        let envelope: SearchEnvelope = self
            .get_json(
                "PubMed",
                self.http.get(url).query(&params),
                self.pubmed_interval(),
            )
            .await?;
        let total = envelope.esearchresult.count.parse().ok();
        if envelope.esearchresult.idlist.is_empty() {
            return Ok(Page {
                total,
                next_page_token: None,
                items: Vec::new(),
            });
        }
        let items = self
            .pubmed_summaries(&envelope.esearchresult.idlist)
            .await?;
        Ok(Page {
            total,
            next_page_token: None,
            items,
        })
    }

    pub async fn pubmed_get(&self, pmid: &str) -> UseResult<PubMedArticle> {
        validate_pmid(pmid)?;
        self.pubmed_summaries(&[pmid.to_string()])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                UseError::new(
                    "use.science.not_found",
                    format!("PubMed did not return PMID {pmid}."),
                )
                .with_detail("service", "PubMed")
                .with_detail("pmid", pmid)
            })
    }

    async fn pubmed_summaries(&self, pmids: &[String]) -> UseResult<Vec<PubMedArticle>> {
        let contact_email = self.pubmed_contact_email()?;
        let url = self.endpoint_url(&self.endpoints.pubmed, &["esummary.fcgi"])?;
        let mut params = vec![
            ("db", "pubmed".to_string()),
            ("id", pmids.join(",")),
            ("retmode", "json".to_string()),
            ("version", "2.0".to_string()),
            ("tool", "a3s-use-science".to_string()),
            ("email", contact_email.to_string()),
        ];
        if let Some(api_key) = &self.ncbi_api_key {
            params.push(("api_key", api_key.clone()));
        }
        let envelope: SummaryEnvelope = self
            .get_json(
                "PubMed",
                self.http.get(url).query(&params),
                self.pubmed_interval(),
            )
            .await?;
        Ok(parse_summaries(pmids, &envelope.result))
    }

    fn pubmed_contact_email(&self) -> UseResult<&str> {
        self.contact_email.as_deref().ok_or_else(|| {
            UseError::new(
                "use.science.contact_email_required",
                "PubMed requests require a contact email for responsible NCBI E-utilities use.",
            )
            .with_suggestion("Set A3S_SCIENCE_CONTACT_EMAIL and retry.")
        })
    }

    fn pubmed_interval(&self) -> Duration {
        if self.ncbi_api_key.is_some() {
            PUBMED_KEYED_INTERVAL
        } else {
            PUBMED_KEYLESS_INTERVAL
        }
    }
}

fn parse_summaries(
    pmids: &[String],
    result: &serde_json::Map<String, Value>,
) -> Vec<PubMedArticle> {
    pmids
        .iter()
        .filter_map(|pmid| {
            let record = result.get(pmid)?.as_object()?;
            let authors = record
                .get("authors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|author| author.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            let doi = record
                .get("articleids")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|identifier| identifier.get("idtype").and_then(Value::as_str) == Some("doi"))
                .and_then(|identifier| identifier.get("value"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(PubMedArticle {
                pmid: pmid.clone(),
                title: string_field(record, "title").unwrap_or_default(),
                authors,
                journal: string_field(record, "fulljournalname")
                    .or_else(|| string_field(record, "source")),
                publication_date: string_field(record, "pubdate"),
                doi,
            })
        })
        .collect()
}

fn string_field(record: &serde_json::Map<String, Value>, name: &str) -> Option<String> {
    record
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn required_text<'a>(label: &str, value: &'a str) -> UseResult<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        return Err(UseError::new(
            "use.science.input_invalid",
            format!("{label} cannot be empty."),
        ));
    }
    Ok(value)
}

fn bounded_limit(limit: usize, maximum: usize) -> UseResult<usize> {
    if !(1..=maximum).contains(&limit) {
        return Err(UseError::new(
            "use.science.limit_invalid",
            format!("Result limit must be between 1 and {maximum}."),
        ));
    }
    Ok(limit)
}

fn validate_pmid(pmid: &str) -> UseResult<()> {
    if pmid.is_empty() || pmid.len() > 12 || !pmid.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(UseError::new(
            "use.science.identifier_invalid",
            "A PMID must contain only 1 to 12 digits.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pubmed_summaries_in_requested_order() {
        let result = serde_json::json!({
            "2": {
                "title": "Second",
                "authors": [{"name": "B Author"}],
                "articleids": [{"idtype": "doi", "value": "10.1/second"}]
            },
            "1": {
                "title": "First",
                "authors": [{"name": "A Author"}],
                "fulljournalname": "Journal",
                "pubdate": "2026",
                "articleids": []
            }
        });
        let articles = parse_summaries(
            &["1".to_string(), "2".to_string()],
            result.as_object().unwrap(),
        );
        assert_eq!(articles[0].pmid, "1");
        assert_eq!(articles[0].authors, ["A Author"]);
        assert_eq!(articles[1].doi.as_deref(), Some("10.1/second"));
    }

    #[test]
    fn validates_pubmed_inputs() {
        assert_eq!(
            validate_pmid("../1").unwrap_err().code,
            "use.science.identifier_invalid"
        );
        assert_eq!(
            bounded_limit(0, 100).unwrap_err().code,
            "use.science.limit_invalid"
        );
    }
}
