use std::collections::HashSet;
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use serde::Deserialize;
use serde_json::Value;

use crate::models::{BioRxivPage, BioRxivRecord};
use crate::ScienceClient;

const BIORXIV_INTERVAL: Duration = Duration::from_millis(200);
const MAX_SCAN_RECORDS: usize = 500;

#[derive(Debug, Deserialize)]
struct BioRxivEnvelope {
    #[serde(default)]
    messages: Vec<BioRxivMessage>,
    #[serde(default)]
    collection: Vec<RawBioRxivRecord>,
}

#[derive(Debug, Default, Deserialize)]
struct BioRxivMessage {
    #[serde(default)]
    total: Value,
    #[serde(default)]
    count: Value,
}

#[derive(Debug, Deserialize)]
struct RawBioRxivRecord {
    doi: String,
    title: String,
    authors: String,
    #[serde(default, rename = "abstract")]
    abstract_text: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default, rename = "published")]
    published_doi: Option<String>,
}

impl ScienceClient {
    pub async fn biorxiv_search(
        &self,
        from_date: &str,
        to_date: &str,
        query: Option<&str>,
        category: Option<&str>,
        limit: usize,
    ) -> UseResult<BioRxivPage> {
        validate_date_range(from_date, to_date)?;
        let limit = bounded_limit(limit)?;
        let query = query
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(str::to_lowercase);
        let category = category
            .map(str::trim)
            .filter(|category| !category.is_empty())
            .map(str::to_lowercase);

        let mut cursor = 0_usize;
        let mut scanned = 0_usize;
        let mut total_upstream = None;
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        while scanned < MAX_SCAN_RECORDS && items.len() < limit {
            let cursor_text = cursor.to_string();
            let url = self.endpoint_url(
                &self.endpoints.biorxiv,
                &[
                    "details",
                    "biorxiv",
                    from_date,
                    to_date,
                    &cursor_text,
                    "json",
                ],
            )?;
            let envelope: BioRxivEnvelope = self
                .get_json("bioRxiv", self.http.get(url), BIORXIV_INTERVAL)
                .await?;
            let message = envelope.messages.first();
            total_upstream =
                total_upstream.or_else(|| message.and_then(|message| flexible_u64(&message.total)));
            let reported_count = message
                .and_then(|message| flexible_u64(&message.count))
                .map(|count| count as usize)
                .unwrap_or(envelope.collection.len());
            let received = envelope.collection.len();
            if received == 0 {
                break;
            }
            scanned = scanned.saturating_add(received);
            cursor = cursor.saturating_add(reported_count.max(received));
            for record in envelope.collection {
                if !matches_filters(&record, query.as_deref(), category.as_deref()) {
                    continue;
                }
                let key = format!(
                    "{}#{}",
                    record.doi,
                    record.version.as_deref().unwrap_or_default()
                );
                if seen.insert(key) {
                    items.push(convert_record(record));
                    if items.len() == limit {
                        break;
                    }
                }
            }
            if reported_count == 0 || total_upstream.is_some_and(|total| cursor as u64 >= total) {
                break;
            }
        }
        Ok(BioRxivPage {
            total_upstream,
            scanned,
            items,
        })
    }

    pub async fn biorxiv_get(&self, doi: &str) -> UseResult<Vec<BioRxivRecord>> {
        let suffix = validate_biorxiv_doi(doi)?;
        let url = self.endpoint_url(
            &self.endpoints.biorxiv,
            &["details", "biorxiv", "10.1101", suffix, "na", "json"],
        )?;
        let envelope: BioRxivEnvelope = self
            .get_json("bioRxiv", self.http.get(url), BIORXIV_INTERVAL)
            .await?;
        if envelope.collection.is_empty() {
            return Err(UseError::new(
                "use.science.not_found",
                format!("bioRxiv did not return DOI {doi}."),
            )
            .with_detail("service", "bioRxiv")
            .with_detail("doi", doi));
        }
        Ok(envelope
            .collection
            .into_iter()
            .map(convert_record)
            .collect())
    }
}

fn convert_record(record: RawBioRxivRecord) -> BioRxivRecord {
    BioRxivRecord {
        doi: record.doi,
        title: record.title,
        authors: record.authors,
        abstract_text: non_empty(record.abstract_text),
        category: non_empty(record.category),
        date: non_empty(record.date),
        version: non_empty(record.version),
        published_doi: non_empty(record.published_doi).filter(|value| value != "NA"),
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn matches_filters(record: &RawBioRxivRecord, query: Option<&str>, category: Option<&str>) -> bool {
    let query_matches = query.is_none_or(|query| {
        [
            record.title.as_str(),
            record.authors.as_str(),
            record.abstract_text.as_deref().unwrap_or_default(),
            record.doi.as_str(),
        ]
        .iter()
        .any(|value| value.to_lowercase().contains(query))
    });
    let category_matches = category.is_none_or(|category| {
        record
            .category
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(category))
    });
    query_matches && category_matches
}

fn flexible_u64(value: &Value) -> Option<u64> {
    match value {
        Value::Number(value) => value.as_u64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn validate_date_range(from_date: &str, to_date: &str) -> UseResult<()> {
    if !valid_iso_date(from_date) || !valid_iso_date(to_date) || from_date > to_date {
        return Err(UseError::new(
            "use.science.date_invalid",
            "bioRxiv dates must form an ordered YYYY-MM-DD range.",
        ));
    }
    Ok(())
}

fn valid_iso_date(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    let (Ok(year), Ok(month), Ok(day)) = (
        parts.first().unwrap_or(&"").parse::<u32>(),
        parts.get(1).unwrap_or(&"").parse::<u32>(),
        parts.get(2).unwrap_or(&"").parse::<u32>(),
    ) else {
        return false;
    };
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1900..=9999).contains(&year) && (1..=max_day).contains(&day)
}

fn validate_biorxiv_doi(doi: &str) -> UseResult<&str> {
    let suffix = doi.strip_prefix("10.1101/").unwrap_or_default();
    if suffix.is_empty()
        || suffix.len() > 200
        || !suffix.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'(' | b')')
        })
    {
        return Err(UseError::new(
            "use.science.identifier_invalid",
            "A bioRxiv DOI must start with 10.1101/ and contain a safe DOI suffix.",
        ));
    }
    Ok(suffix)
}

fn bounded_limit(limit: usize) -> UseResult<usize> {
    if !(1..=100).contains(&limit) {
        return Err(UseError::new(
            "use.science.limit_invalid",
            "bioRxiv result limit must be between 1 and 100.",
        ));
    }
    Ok(limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_real_calendar_dates_and_dois() {
        assert!(valid_iso_date("2024-02-29"));
        assert!(!valid_iso_date("2025-02-29"));
        assert!(validate_biorxiv_doi("10.1101/2026.01.01.123456").is_ok());
        assert_eq!(
            validate_biorxiv_doi("https://example.com")
                .unwrap_err()
                .code,
            "use.science.identifier_invalid"
        );
    }

    #[test]
    fn filters_records_without_losing_case_insensitivity() {
        let record = RawBioRxivRecord {
            doi: "10.1101/example".to_string(),
            title: "Protein Design".to_string(),
            authors: "A. Author".to_string(),
            abstract_text: Some("A diffusion model".to_string()),
            category: Some("Bioinformatics".to_string()),
            date: None,
            version: None,
            published_doi: None,
        };
        assert!(matches_filters(
            &record,
            Some("protein"),
            Some("bioinformatics")
        ));
        assert!(!matches_filters(&record, Some("genome"), None));
    }
}
