// Plugin lifecycle coordinator helpers (included into coordinator).

fn failure_evidence_digest(checkpoint: &PluginLifecycleCheckpoint, error_code: &str) -> String {
    let identity = format!("{}\n{error_code}", checkpoint.idempotency_key);
    format!("sha256:{:x}", Sha256::digest(identity.as_bytes()))
}

fn rollback_checkpoint_key(
    intent: &PluginLifecycleIntent,
    checkpoint: &PluginLifecycleCheckpoint,
) -> String {
    let identity = format!(
        "{}\n{}\ncandidate-surface-rollback",
        intent.operation_id, checkpoint.idempotency_key
    );
    format!("sha256:{:x}", Sha256::digest(identity.as_bytes()))
}

fn surface_missing() -> UseError {
    coordinator_error("A lifecycle checkpoint references a missing manifest surface.")
}

pub(super) fn coordinator_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.lifecycle_coordinator_invalid", message)
}
