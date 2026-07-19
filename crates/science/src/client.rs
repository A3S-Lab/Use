use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use reqwest::{RequestBuilder, Response};
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use tokio::time::Instant;
use url::Url;

use crate::models::ScienceDiagnostic;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const ERROR_BODY_LIMIT: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScienceEndpoints {
    pub pubmed: Url,
    pub chembl: Url,
    pub clinical_trials: Url,
    pub biorxiv: Url,
    pub ensembl: Url,
}

impl ScienceEndpoints {
    /// Return the public upstream endpoints used by the toolkit.
    pub fn public() -> UseResult<Self> {
        Ok(Self {
            pubmed: parse_endpoint("PubMed", "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/")?,
            chembl: parse_endpoint("ChEMBL", "https://www.ebi.ac.uk/chembl/api/data/")?,
            clinical_trials: parse_endpoint(
                "ClinicalTrials.gov",
                "https://clinicaltrials.gov/api/v2/",
            )?,
            biorxiv: parse_endpoint("bioRxiv", "https://api.biorxiv.org/")?,
            ensembl: parse_endpoint("Ensembl", "https://rest.ensembl.org/")?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScienceClientBuilder {
    endpoints: Option<ScienceEndpoints>,
    contact_email: Option<String>,
    ncbi_api_key: Option<String>,
    timeout: Duration,
    user_agent: Option<String>,
}

impl Default for ScienceClientBuilder {
    fn default() -> Self {
        Self {
            endpoints: None,
            contact_email: None,
            ncbi_api_key: None,
            timeout: DEFAULT_TIMEOUT,
            user_agent: None,
        }
    }
}

impl ScienceClientBuilder {
    pub fn endpoints(mut self, endpoints: ScienceEndpoints) -> Self {
        self.endpoints = Some(endpoints);
        self
    }

    pub fn contact_email(mut self, contact_email: impl Into<String>) -> Self {
        self.contact_email = Some(contact_email.into());
        self
    }

    pub fn ncbi_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.ncbi_api_key = Some(api_key.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    pub fn build(self) -> UseResult<ScienceClient> {
        if self.timeout.is_zero() {
            return Err(UseError::new(
                "use.science.config_invalid",
                "Science request timeout must be greater than zero.",
            ));
        }
        if let Some(email) = self.contact_email.as_deref() {
            if !email.contains('@') || email.chars().any(char::is_whitespace) {
                return Err(UseError::new(
                    "use.science.contact_email_invalid",
                    "A3S_SCIENCE_CONTACT_EMAIL must contain a valid contact email address.",
                ));
            }
        }
        let endpoints = match self.endpoints {
            Some(endpoints) => endpoints,
            None => ScienceEndpoints::public()?,
        };
        for (name, endpoint) in endpoint_pairs(&endpoints) {
            if !matches!(endpoint.scheme(), "http" | "https") {
                return Err(UseError::new(
                    "use.science.config_invalid",
                    format!("The {name} endpoint must use HTTP or HTTPS."),
                ));
            }
        }

        let user_agent = self
            .user_agent
            .unwrap_or_else(|| format!("a3s-use-science/{}", env!("CARGO_PKG_VERSION")));
        let http = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(user_agent)
            .build()
            .map_err(|error| {
                UseError::new(
                    "use.science.client_invalid",
                    format!("Failed to construct the science HTTP client: {error}"),
                )
            })?;

        Ok(ScienceClient {
            http,
            endpoints,
            contact_email: self.contact_email,
            ncbi_api_key: self.ncbi_api_key,
            gate: Arc::new(RequestGate::default()),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScienceClient {
    pub(crate) http: reqwest::Client,
    pub(crate) endpoints: ScienceEndpoints,
    pub(crate) contact_email: Option<String>,
    pub(crate) ncbi_api_key: Option<String>,
    gate: Arc<RequestGate>,
}

impl ScienceClient {
    pub fn builder() -> ScienceClientBuilder {
        ScienceClientBuilder::default()
    }

    pub fn from_env() -> UseResult<Self> {
        let mut builder = Self::builder();
        if let Some(contact_email) = optional_env("A3S_SCIENCE_CONTACT_EMAIL")? {
            builder = builder.contact_email(contact_email);
        }
        if let Some(api_key) = optional_env("NCBI_API_KEY")? {
            builder = builder.ncbi_api_key(api_key);
        }
        builder.build()
    }

    pub fn diagnostic(&self) -> ScienceDiagnostic {
        ScienceDiagnostic {
            version: env!("CARGO_PKG_VERSION").to_string(),
            contact_email_configured: self.contact_email.is_some(),
            ncbi_api_key_configured: self.ncbi_api_key.is_some(),
            sources: vec![
                "PubMed".to_string(),
                "ChEMBL".to_string(),
                "ClinicalTrials.gov".to_string(),
                "bioRxiv".to_string(),
                "Ensembl".to_string(),
            ],
            message: "Read-only public life-science data sources are configured; no network request was made."
                .to_string(),
        }
    }

    pub(crate) async fn get_json<T>(
        &self,
        service: &'static str,
        request: RequestBuilder,
        min_interval: Duration,
    ) -> UseResult<T>
    where
        T: DeserializeOwned,
    {
        self.gate.wait(service, min_interval).await;
        let response = request.send().await.map_err(|error| {
            UseError::new(
                "use.science.upstream_unavailable",
                format!("{service} request failed: {error}"),
            )
            .with_detail("service", service)
        })?;
        parse_json(service, response).await
    }

    pub(crate) fn endpoint_url(&self, base: &Url, segments: &[&str]) -> UseResult<Url> {
        let mut url = base.clone();
        {
            let mut path = url.path_segments_mut().map_err(|_| {
                UseError::new(
                    "use.science.config_invalid",
                    format!("Endpoint '{base}' cannot be used as a hierarchical URL."),
                )
            })?;
            path.pop_if_empty();
            path.extend(segments.iter().copied());
        }
        Ok(url)
    }
}

async fn parse_json<T>(service: &'static str, response: Response) -> UseResult<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    if !status.is_success() {
        let body = bounded_response_text(response, ERROR_BODY_LIMIT).await;
        return Err(UseError::new(
            "use.science.upstream_error",
            format!("{service} returned HTTP {status}."),
        )
        .with_detail("service", service)
        .with_detail("status", u64::from(status.as_u16()))
        .with_detail("body", body));
    }
    response.json::<T>().await.map_err(|error| {
        UseError::new(
            "use.science.response_invalid",
            format!("{service} returned an invalid JSON response: {error}"),
        )
        .with_detail("service", service)
    })
}

async fn bounded_response_text(mut response: Response, max_bytes: usize) -> String {
    let mut bytes = Vec::with_capacity(max_bytes.saturating_add(1));
    while bytes.len() <= max_bytes {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) if !chunk.is_empty() => chunk,
            Ok(Some(_)) | Ok(None) | Err(_) => break,
        };
        let remaining = max_bytes.saturating_add(1).saturating_sub(bytes.len());
        let take = remaining.min(chunk.len());
        bytes.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            break;
        }
    }
    let truncated = bytes.len() > max_bytes;
    bytes.truncate(max_bytes);
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        output.push('…');
    }
    output
}

fn endpoint_pairs(endpoints: &ScienceEndpoints) -> [(&'static str, &Url); 5] {
    [
        ("PubMed", &endpoints.pubmed),
        ("ChEMBL", &endpoints.chembl),
        ("ClinicalTrials.gov", &endpoints.clinical_trials),
        ("bioRxiv", &endpoints.biorxiv),
        ("Ensembl", &endpoints.ensembl),
    ]
}

fn parse_endpoint(service: &'static str, value: &'static str) -> UseResult<Url> {
    Url::parse(value).map_err(|error| {
        UseError::new(
            "use.science.config_invalid",
            format!("The built-in {service} endpoint is invalid: {error}"),
        )
    })
}

fn optional_env(name: &'static str) -> UseResult<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(UseError::new(
            "use.science.config_invalid",
            format!("Environment variable {name} must contain valid UTF-8."),
        )),
    }
}

#[derive(Debug, Default)]
struct RequestGate {
    next_allowed: Mutex<HashMap<&'static str, Instant>>,
}

impl RequestGate {
    async fn wait(&self, service: &'static str, min_interval: Duration) {
        if min_interval.is_zero() {
            return;
        }
        let now = Instant::now();
        let start = {
            let mut next_allowed = self.next_allowed.lock().await;
            let start = next_allowed.get(service).copied().unwrap_or(now).max(now);
            next_allowed.insert(service, start + min_interval);
            start
        };
        if start > now {
            tokio::time::sleep_until(start).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn rejects_invalid_contact_email_and_non_http_endpoints() {
        assert_eq!(
            ScienceClient::builder()
                .contact_email("not-an-email")
                .build()
                .unwrap_err()
                .code,
            "use.science.contact_email_invalid"
        );

        let mut endpoints = ScienceEndpoints::public().unwrap();
        endpoints.pubmed = Url::parse("file:///tmp/pubmed").unwrap();
        assert_eq!(
            ScienceClient::builder()
                .endpoints(endpoints)
                .build()
                .unwrap_err()
                .code,
            "use.science.config_invalid"
        );
    }

    #[test]
    fn endpoint_builder_percent_encodes_untrusted_segments() {
        let client = ScienceClient::builder().build().unwrap();
        let url = client
            .endpoint_url(
                &client.endpoints.ensembl,
                &["lookup", "symbol", "human", "A/B"],
            )
            .unwrap();
        assert!(url.as_str().ends_with("/lookup/symbol/human/A%2FB"));
    }

    #[test]
    fn public_clients_are_send_and_sync() {
        assert_send_sync::<ScienceClient>();
        assert_send_sync::<ScienceClientBuilder>();
    }
}
