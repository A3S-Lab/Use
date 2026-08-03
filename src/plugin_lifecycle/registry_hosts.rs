use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, ExtensionLifecyclePackage, ExtensionLifecycleResult,
    ExtensionRegistry,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::{
    PluginCapabilityLifecycleHost, PluginLifecycleAction, PluginLifecycleEvidence,
    PluginLifecycleIntent, PluginPackageLifecycleHost,
};

const DEFAULT_LIFECYCLE_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Production immutable-package adapter for the schema-v3 lifecycle saga.
///
/// Install owns a previously validated package candidate. Enable and disable
/// do not execute package checkpoints, while uninstall uses `for_installed`
/// and removes only the generation bound by its lifecycle intent.
pub struct ExtensionPackageLifecycleHost {
    registry: ExtensionRegistry,
    candidate: Option<ExtensionLifecyclePackage>,
    remove_timeout: Duration,
}

impl ExtensionPackageLifecycleHost {
    pub fn new(registry: ExtensionRegistry, candidate: ExtensionLifecyclePackage) -> Self {
        Self {
            registry,
            candidate: Some(candidate),
            remove_timeout: DEFAULT_LIFECYCLE_DRAIN_TIMEOUT,
        }
    }

    pub fn for_installed(registry: ExtensionRegistry) -> Self {
        Self {
            registry,
            candidate: None,
            remove_timeout: DEFAULT_LIFECYCLE_DRAIN_TIMEOUT,
        }
    }

    pub fn with_remove_timeout(mut self, timeout: Duration) -> Self {
        self.remove_timeout = timeout;
        self
    }

    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }
}

#[async_trait]
impl PluginPackageLifecycleHost for ExtensionPackageLifecycleHost {
    async fn commit_package(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_action(
            intent,
            &[
                PluginLifecycleAction::Install,
                PluginLifecycleAction::Upgrade,
            ],
            "package commit",
        )?;
        if intent.action == PluginLifecycleAction::Upgrade {
            return Err(UseError::new(
                "use.plugin.package_generation_retirement_required",
                "Cognitive-package upgrade requires dual-generation retirement before candidate commit can be enabled.",
            ));
        }
        let candidate = self.candidate.as_ref().ok_or_else(|| {
            UseError::new(
                "use.plugin.package_candidate_missing",
                "The lifecycle package host has no validated install candidate.",
            )
        })?;
        let identity = lifecycle_identity(intent)?;
        let result = self
            .registry
            .commit_lifecycle_package(&identity, candidate)
            .await?;
        result_evidence("package-committed", intent, idempotency_key, &result)
    }

    async fn remove_package(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_action(
            intent,
            &[PluginLifecycleAction::Uninstall],
            "package removal",
        )?;
        let identity = lifecycle_identity(intent)?;
        self.registry
            .remove_lifecycle_package(&identity, self.remove_timeout)
            .await?;
        checkpoint_evidence(
            "package-removed",
            intent,
            idempotency_key,
            &identity.descriptor_digest()?,
        )
    }
}

/// Production atomic capability adapter backed by the immutable registry
/// snapshot and the package route lease.
#[derive(Debug, Clone)]
pub struct ExtensionCapabilityLifecycleHost {
    registry: ExtensionRegistry,
    drain_timeout: Duration,
}

impl ExtensionCapabilityLifecycleHost {
    pub fn new(registry: ExtensionRegistry) -> Self {
        Self {
            registry,
            drain_timeout: DEFAULT_LIFECYCLE_DRAIN_TIMEOUT,
        }
    }

    pub fn with_drain_timeout(mut self, timeout: Duration) -> Self {
        self.drain_timeout = timeout;
        self
    }

    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }
}

#[async_trait]
impl PluginCapabilityLifecycleHost for ExtensionCapabilityLifecycleHost {
    async fn publish_capability(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_action(
            intent,
            &[
                PluginLifecycleAction::Install,
                PluginLifecycleAction::Upgrade,
                PluginLifecycleAction::Enable,
            ],
            "capability publication",
        )?;
        let identity = lifecycle_identity(intent)?;
        let result = self.registry.publish_lifecycle_package(&identity).await?;
        result_evidence("capability-published", intent, idempotency_key, &result)
    }

    async fn hide_capability(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_action(
            intent,
            &[
                PluginLifecycleAction::Disable,
                PluginLifecycleAction::Uninstall,
            ],
            "capability hiding",
        )?;
        let identity = lifecycle_identity(intent)?;
        let result = self.registry.hide_lifecycle_package(&identity).await?;
        result_evidence("capability-hidden", intent, idempotency_key, &result)
    }

    async fn drain_calls(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_action(
            intent,
            &[
                PluginLifecycleAction::Disable,
                PluginLifecycleAction::Uninstall,
            ],
            "capability drain",
        )?;
        let identity = lifecycle_identity(intent)?;
        let result = self
            .registry
            .drain_lifecycle_package(&identity, self.drain_timeout)
            .await?;
        result_evidence("calls-drained", intent, idempotency_key, &result)
    }
}

fn lifecycle_identity(intent: &PluginLifecycleIntent) -> UseResult<ExtensionLifecycleIdentity> {
    intent.validate()?;
    ExtensionLifecycleIdentity::new(
        &intent.package_id,
        intent.package_digest.clone(),
        intent.manifest_digest.clone(),
        intent.generation,
    )
}

fn validate_action(
    intent: &PluginLifecycleIntent,
    allowed: &[PluginLifecycleAction],
    operation: &str,
) -> UseResult<()> {
    intent.validate()?;
    if !allowed.contains(&intent.action) {
        return Err(UseError::new(
            "use.plugin.lifecycle_action_invalid",
            format!(
                "Lifecycle {operation} does not accept the '{}' action.",
                intent.action.name()
            ),
        ));
    }
    Ok(())
}

fn result_evidence(
    label: &str,
    intent: &PluginLifecycleIntent,
    idempotency_key: &str,
    result: &ExtensionLifecycleResult,
) -> UseResult<PluginLifecycleEvidence> {
    checkpoint_evidence(
        label,
        intent,
        idempotency_key,
        &result.extension.receipt.descriptor_digest()?,
    )
}

fn checkpoint_evidence(
    label: &str,
    intent: &PluginLifecycleIntent,
    idempotency_key: &str,
    subject_digest: &str,
) -> UseResult<PluginLifecycleEvidence> {
    let identity = format!(
        "{label}\n{idempotency_key}\n{}\n{subject_digest}",
        intent.descriptor_digest()?
    );
    PluginLifecycleEvidence::new(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_lifecycle::test_support::intent;

    #[test]
    fn lifecycle_identity_preserves_the_exact_intent_generation() {
        let intent = intent(PluginLifecycleAction::Install);
        let identity = lifecycle_identity(&intent).unwrap();
        assert_eq!(identity.package_id(), intent.package_id);
        assert_eq!(identity.package_digest(), intent.package_digest);
        assert_eq!(identity.manifest_digest(), intent.manifest_digest);
        assert_eq!(identity.generation(), intent.generation);
    }

    #[test]
    fn production_registry_hosts_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ExtensionPackageLifecycleHost>();
        assert_send_sync::<ExtensionCapabilityLifecycleHost>();
    }

    #[test]
    fn checkpoint_evidence_is_stable_and_binds_the_idempotency_key() {
        let intent = intent(PluginLifecycleAction::Install);
        let subject = format!("sha256:{}", "a".repeat(64));
        let first = checkpoint_evidence("package-committed", &intent, "key-a", &subject).unwrap();
        let replay = checkpoint_evidence("package-committed", &intent, "key-a", &subject).unwrap();
        let different =
            checkpoint_evidence("package-committed", &intent, "key-b", &subject).unwrap();
        assert_eq!(first, replay);
        assert_ne!(first, different);
    }

    #[tokio::test]
    async fn upgrade_rejects_before_a_candidate_or_registry_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let registry = ExtensionRegistry::new(a3s_use_extension::ExtensionPaths::new(
            temp.path().join("data"),
            temp.path().join("state"),
        ));
        let host = ExtensionPackageLifecycleHost::for_installed(registry.clone());
        let intent = intent(PluginLifecycleAction::Upgrade);
        let error = host
            .commit_package(&intent, &intent.checkpoints[0].idempotency_key)
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            "use.plugin.package_generation_retirement_required"
        );
        assert!(registry.list().await.unwrap().is_empty());
    }
}
