use a3s_use_core::{OkfKnowledgeObservation, OkfProjectionReceipt, UseError, UseResult};
use serde::{Deserialize, Serialize};

pub const OKF_KNOWLEDGE_BINDING_SCHEMA: &str = "a3s.use.okf-knowledge-binding.v1";

/// Durable exact-generation evidence owned by the A3S Knowledge boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeBinding {
    pub schema: String,
    pub receipt: OkfProjectionReceipt,
    pub observation: OkfKnowledgeObservation,
}

impl OkfKnowledgeBinding {
    pub fn new(
        receipt: OkfProjectionReceipt,
        observation: OkfKnowledgeObservation,
    ) -> UseResult<Self> {
        let binding = Self {
            schema: OKF_KNOWLEDGE_BINDING_SCHEMA.to_owned(),
            receipt,
            observation,
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != OKF_KNOWLEDGE_BINDING_SCHEMA {
            return Err(binding_error(
                "The OKF Knowledge binding schema is unsupported.",
            ));
        }
        self.observation.validate_for_receipt(&self.receipt)
    }
}

fn binding_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_binding_invalid", message)
}
