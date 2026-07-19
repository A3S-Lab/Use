use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use serde::Deserialize;

use crate::models::{ClinicalTrial, ClinicalTrialPage};
use crate::ScienceClient;

const CLINICAL_TRIALS_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StudiesEnvelope {
    #[serde(default)]
    studies: Vec<Study>,
    #[serde(default)]
    next_page_token: Option<String>,
    #[serde(default)]
    total_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Study {
    protocol_section: ProtocolSection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolSection {
    identification_module: IdentificationModule,
    #[serde(default)]
    status_module: StatusModule,
    #[serde(default)]
    design_module: DesignModule,
    #[serde(default)]
    conditions_module: ConditionsModule,
    #[serde(default)]
    arms_interventions_module: ArmsInterventionsModule,
    #[serde(default)]
    sponsor_collaborators_module: SponsorCollaboratorsModule,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentificationModule {
    nct_id: String,
    brief_title: String,
    #[serde(default)]
    official_title: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusModule {
    #[serde(default)]
    overall_status: Option<String>,
    #[serde(default)]
    start_date_struct: Option<DateStruct>,
    #[serde(default)]
    completion_date_struct: Option<DateStruct>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DateStruct {
    date: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesignModule {
    #[serde(default)]
    study_type: Option<String>,
    #[serde(default)]
    phases: Vec<String>,
    #[serde(default)]
    enrollment_info: Option<EnrollmentInfo>,
}

#[derive(Debug, Deserialize)]
struct EnrollmentInfo {
    #[serde(default)]
    count: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct ConditionsModule {
    #[serde(default)]
    conditions: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ArmsInterventionsModule {
    #[serde(default)]
    interventions: Vec<Intervention>,
}

#[derive(Debug, Deserialize)]
struct Intervention {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SponsorCollaboratorsModule {
    #[serde(default)]
    lead_sponsor: Option<LeadSponsor>,
}

#[derive(Debug, Deserialize)]
struct LeadSponsor {
    name: String,
}

impl ScienceClient {
    pub async fn clinical_trials_search(
        &self,
        query: &str,
        statuses: &[String],
        limit: usize,
        page_token: Option<&str>,
    ) -> UseResult<ClinicalTrialPage> {
        let query = query.trim();
        if query.is_empty() {
            return Err(UseError::new(
                "use.science.input_invalid",
                "ClinicalTrials.gov query cannot be empty.",
            ));
        }
        let limit = bounded_limit(limit)?;
        for status in statuses {
            validate_status(status)?;
        }
        if let Some(token) = page_token {
            validate_page_token(token)?;
        }
        let url = self.endpoint_url(&self.endpoints.clinical_trials, &["studies"])?;
        let mut params = vec![
            ("query.term", query.to_string()),
            ("pageSize", limit.to_string()),
            ("countTotal", "true".to_string()),
            ("format", "json".to_string()),
        ];
        if !statuses.is_empty() {
            params.push(("filter.overallStatus", statuses.join("|")));
        }
        if let Some(token) = page_token {
            params.push(("pageToken", token.to_string()));
        }
        let envelope: StudiesEnvelope = self
            .get_json(
                "ClinicalTrials.gov",
                self.http.get(url).query(&params),
                CLINICAL_TRIALS_INTERVAL,
            )
            .await?;
        Ok(ClinicalTrialPage {
            total: envelope.total_count,
            next_page_token: envelope.next_page_token,
            items: envelope.studies.into_iter().map(flatten_study).collect(),
        })
    }

    pub async fn clinical_trial_get(&self, nct_id: &str) -> UseResult<ClinicalTrial> {
        validate_nct_id(nct_id)?;
        let url = self.endpoint_url(&self.endpoints.clinical_trials, &["studies", nct_id])?;
        let study: Study = self
            .get_json(
                "ClinicalTrials.gov",
                self.http.get(url).query(&[("format", "json")]),
                CLINICAL_TRIALS_INTERVAL,
            )
            .await?;
        Ok(flatten_study(study))
    }
}

fn flatten_study(study: Study) -> ClinicalTrial {
    let protocol = study.protocol_section;
    ClinicalTrial {
        nct_id: protocol.identification_module.nct_id,
        brief_title: protocol.identification_module.brief_title,
        official_title: protocol.identification_module.official_title,
        overall_status: protocol.status_module.overall_status,
        study_type: protocol.design_module.study_type,
        phases: protocol.design_module.phases,
        conditions: protocol.conditions_module.conditions,
        interventions: protocol
            .arms_interventions_module
            .interventions
            .into_iter()
            .map(|intervention| intervention.name)
            .collect(),
        lead_sponsor: protocol
            .sponsor_collaborators_module
            .lead_sponsor
            .map(|sponsor| sponsor.name),
        enrollment: protocol
            .design_module
            .enrollment_info
            .and_then(|enrollment| enrollment.count),
        start_date: protocol
            .status_module
            .start_date_struct
            .map(|date| date.date),
        completion_date: protocol
            .status_module
            .completion_date_struct
            .map(|date| date.date),
    }
}

fn validate_nct_id(nct_id: &str) -> UseResult<()> {
    let digits = nct_id.strip_prefix("NCT").unwrap_or_default();
    if digits.len() != 8 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(UseError::new(
            "use.science.identifier_invalid",
            "A ClinicalTrials.gov identifier must use the form NCT followed by eight digits.",
        ));
    }
    Ok(())
}

fn validate_status(status: &str) -> UseResult<()> {
    if status.is_empty()
        || !status
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
    {
        return Err(UseError::new(
            "use.science.input_invalid",
            "Clinical trial statuses must use uppercase API values such as RECRUITING.",
        ));
    }
    Ok(())
}

fn validate_page_token(token: &str) -> UseResult<()> {
    if token.is_empty()
        || token.len() > 2_048
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
    {
        return Err(UseError::new(
            "use.science.input_invalid",
            "ClinicalTrials.gov page token contains unsupported characters.",
        ));
    }
    Ok(())
}

fn bounded_limit(limit: usize) -> UseResult<usize> {
    if !(1..=100).contains(&limit) {
        return Err(UseError::new(
            "use.science.limit_invalid",
            "Clinical trial result limit must be between 1 and 100.",
        ));
    }
    Ok(limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_clinical_trial_modules() {
        let study: Study = serde_json::from_value(serde_json::json!({
            "protocolSection": {
                "identificationModule": {
                    "nctId": "NCT12345678",
                    "briefTitle": "Trial"
                },
                "statusModule": {
                    "overallStatus": "RECRUITING",
                    "startDateStruct": {"date": "2026-01"}
                },
                "designModule": {
                    "studyType": "INTERVENTIONAL",
                    "phases": ["PHASE2"],
                    "enrollmentInfo": {"count": 120}
                },
                "conditionsModule": {"conditions": ["Cancer"]},
                "armsInterventionsModule": {
                    "interventions": [{"name": "Drug A"}]
                },
                "sponsorCollaboratorsModule": {
                    "leadSponsor": {"name": "A3S Lab"}
                }
            }
        }))
        .unwrap();
        let trial = flatten_study(study);
        assert_eq!(trial.nct_id, "NCT12345678");
        assert_eq!(trial.enrollment, Some(120));
        assert_eq!(trial.interventions, ["Drug A"]);
    }

    #[test]
    fn validates_trial_identifiers_and_statuses() {
        assert_eq!(
            validate_nct_id("NCT123").unwrap_err().code,
            "use.science.identifier_invalid"
        );
        assert_eq!(
            validate_status("recruiting").unwrap_err().code,
            "use.science.input_invalid"
        );
    }
}
