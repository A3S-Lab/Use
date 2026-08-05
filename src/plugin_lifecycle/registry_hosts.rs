use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, ExtensionLifecyclePackage, ExtensionLifecycleResult,
    ExtensionRegistry,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::{
    PluginCapabilityCutoverEvidence, PluginCapabilityLifecycleHost,
    PluginGraphCapabilityLifecycleHost, PluginGraphCapabilityPublication, PluginLifecycleAction,
    PluginLifecycleEvidence, PluginLifecycleIntent, PluginPackageLifecycleHost,
    PluginPackagePublicationEvidence, PluginPackageRollbackEvidence,
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

/// Atomic dependency-closure publication adapter backed by one immutable
/// Registry snapshot cutover.
#[derive(Debug, Clone)]
pub struct ExtensionGraphCapabilityLifecycleHost {
    registry: ExtensionRegistry,
}

impl ExtensionGraphCapabilityLifecycleHost {
    pub fn new(registry: ExtensionRegistry) -> Self {
        Self { registry }
    }

    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }
}

#[async_trait]
impl PluginGraphCapabilityLifecycleHost for ExtensionGraphCapabilityLifecycleHost {
    async fn publish_capabilities(
        &self,
        package_lock: &a3s_use_core::PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        idempotency_key: &str,
    ) -> UseResult<Vec<PluginPackagePublicationEvidence>> {
        let identities = intents
            .iter()
            .map(lifecycle_identity)
            .collect::<UseResult<Vec<_>>>()?;
        let results = self
            .registry
            .publish_lifecycle_package_graph(package_lock, &identities)
            .await?;
        if results.len() != intents.len() {
            return Err(UseError::new(
                "use.plugin.package_graph_publication_invalid",
                "The Registry omitted a package from dependency-closure publication.",
            ));
        }
        intents
            .iter()
            .zip(results)
            .map(|(intent, result)| {
                let evidence = checkpoint_evidence(
                    "package-graph-capability-published",
                    intent,
                    idempotency_key,
                    &result.extension.receipt.descriptor_digest()?,
                )?;
                PluginPackagePublicationEvidence::new(&intent.package_id, evidence)
            })
            .collect()
    }

    async fn publish_capabilities_with_cutover(
        &self,
        package_lock: &a3s_use_core::PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        idempotency_key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication> {
        let identities = intents
            .iter()
            .map(lifecycle_identity)
            .collect::<UseResult<Vec<_>>>()?;
        let publication = self
            .registry
            .publish_lifecycle_package_graph_with_evidence(package_lock, &identities)
            .await?;
        let packages = publication_evidence(
            intents,
            publication.packages,
            idempotency_key,
            "package-graph-capability-published",
        )?;
        let cutover = registry_cutover_evidence(
            publication.registry_generation,
            publication.registry_snapshot_digest,
        )?;
        Ok(PluginGraphCapabilityPublication::new(packages, cutover))
    }

    async fn publish_upgrade_capabilities(
        &self,
        package_lock: &a3s_use_core::PluginPackageLock,
        candidate_intents: &[PluginLifecycleIntent],
        removed_intents: &[PluginLifecycleIntent],
        idempotency_key: &str,
    ) -> UseResult<Vec<PluginPackagePublicationEvidence>> {
        let candidates = candidate_intents
            .iter()
            .map(lifecycle_identity)
            .collect::<UseResult<Vec<_>>>()?;
        let removed = removed_intents
            .iter()
            .map(lifecycle_identity)
            .collect::<UseResult<Vec<_>>>()?;
        let results = self
            .registry
            .publish_lifecycle_package_graph_transition(package_lock, &candidates, &removed)
            .await?;
        if results.len() != candidate_intents.len() {
            return Err(UseError::new(
                "use.plugin.package_graph_publication_invalid",
                "The Registry omitted a candidate from dependency-graph upgrade publication.",
            ));
        }
        candidate_intents
            .iter()
            .zip(results)
            .map(|(intent, result)| {
                let evidence = checkpoint_evidence(
                    "package-graph-capability-published",
                    intent,
                    idempotency_key,
                    &result.extension.receipt.descriptor_digest()?,
                )?;
                PluginPackagePublicationEvidence::new(&intent.package_id, evidence)
            })
            .collect()
    }

    async fn publish_upgrade_capabilities_with_cutover(
        &self,
        package_lock: &a3s_use_core::PluginPackageLock,
        candidate_intents: &[PluginLifecycleIntent],
        removed_intents: &[PluginLifecycleIntent],
        idempotency_key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication> {
        let candidates = candidate_intents
            .iter()
            .map(lifecycle_identity)
            .collect::<UseResult<Vec<_>>>()?;
        let removed = removed_intents
            .iter()
            .map(lifecycle_identity)
            .collect::<UseResult<Vec<_>>>()?;
        let publication = self
            .registry
            .publish_lifecycle_package_graph_transition_with_evidence(
                package_lock,
                &candidates,
                &removed,
            )
            .await?;
        let packages = publication_evidence(
            candidate_intents,
            publication.packages,
            idempotency_key,
            "package-graph-capability-published",
        )?;
        let cutover = registry_cutover_evidence(
            publication.registry_generation,
            publication.registry_snapshot_digest,
        )?;
        Ok(PluginGraphCapabilityPublication::new(packages, cutover))
    }

    async fn hide_capabilities_with_cutover(
        &self,
        _package_lock: &a3s_use_core::PluginPackageLock,
        intents: &[PluginLifecycleIntent],
        idempotency_key: &str,
    ) -> UseResult<PluginGraphCapabilityPublication> {
        let identities = intents
            .iter()
            .map(lifecycle_identity)
            .collect::<UseResult<Vec<_>>>()?;
        let publication = self
            .registry
            .hide_lifecycle_package_graph_with_evidence(&identities)
            .await?;
        let packages = intents
            .iter()
            .map(|intent| {
                let evidence = checkpoint_evidence(
                    "package-graph-capability-hidden",
                    intent,
                    idempotency_key,
                    &publication.registry_snapshot_digest,
                )?;
                PluginPackagePublicationEvidence::new(&intent.package_id, evidence)
            })
            .collect::<UseResult<Vec<_>>>()?;
        let cutover = registry_cutover_evidence(
            publication.registry_generation,
            publication.registry_snapshot_digest,
        )?;
        Ok(PluginGraphCapabilityPublication::new(packages, cutover))
    }

    async fn rollback_candidates(
        &self,
        candidate_lock: &a3s_use_core::PluginPackageLock,
        candidate_intents: &[PluginLifecycleIntent],
        prior_intents: &[PluginLifecycleIntent],
        idempotency_key: &str,
    ) -> UseResult<Vec<PluginPackageRollbackEvidence>> {
        candidate_lock.validate()?;
        let candidates = candidate_intents
            .iter()
            .map(|intent| Ok((intent.package_id.as_str(), lifecycle_identity(intent)?)))
            .collect::<UseResult<std::collections::BTreeMap<_, _>>>()?;
        let priors = prior_intents
            .iter()
            .map(|intent| Ok((intent.package_id.as_str(), lifecycle_identity(intent)?)))
            .collect::<UseResult<std::collections::BTreeMap<_, _>>>()?;
        if candidates.len() != candidate_intents.len()
            || priors.len() != prior_intents.len()
            || priors
                .keys()
                .any(|package_id| !candidates.contains_key(package_id))
        {
            return Err(UseError::new(
                "use.plugin.package_graph_rollback_invalid",
                "Candidate rollback contains duplicate or unrelated package identities.",
            ));
        }
        let ordered_candidates = candidate_lock
            .removal_order()?
            .into_iter()
            .filter_map(|package| candidates.get(package.package_id()).cloned())
            .collect::<Vec<_>>();
        if ordered_candidates.len() != candidates.len() {
            return Err(UseError::new(
                "use.plugin.package_graph_rollback_invalid",
                "A rollback candidate is absent from its exact dependency lock.",
            ));
        }
        let prior_identities = ordered_candidates
            .iter()
            .filter_map(|candidate| priors.get(candidate.package_id()).cloned())
            .collect::<Vec<_>>();
        let results = self
            .registry
            .rollback_lifecycle_package_graph(&ordered_candidates, &prior_identities)
            .await?;
        if results.len() != ordered_candidates.len() {
            return Err(UseError::new(
                "use.plugin.package_graph_rollback_invalid",
                "The Registry omitted a candidate from graph rollback evidence.",
            ));
        }
        ordered_candidates
            .iter()
            .zip(results)
            .map(|(identity, result)| {
                if result.package_id != identity.package_id() {
                    return Err(UseError::new(
                        "use.plugin.package_graph_rollback_invalid",
                        "The Registry changed candidate rollback evidence order.",
                    ));
                }
                let subject = format!(
                    "{}\n{}\n{}",
                    identity.descriptor_digest()?,
                    result.registry_generation,
                    result.changed
                );
                let evidence = checkpoint_evidence(
                    "package-graph-candidate-rolled-back",
                    candidate_intents
                        .iter()
                        .find(|intent| intent.package_id == result.package_id)
                        .ok_or_else(|| {
                            UseError::new(
                                "use.plugin.package_graph_rollback_invalid",
                                "Candidate rollback evidence lost its lifecycle intent.",
                            )
                        })?,
                    idempotency_key,
                    &format!("sha256:{:x}", Sha256::digest(subject.as_bytes())),
                )?;
                PluginPackageRollbackEvidence::new(result.package_id, evidence)
            })
            .collect()
    }
}

fn publication_evidence(
    intents: &[PluginLifecycleIntent],
    results: Vec<a3s_use_extension::ExtensionLifecycleResult>,
    idempotency_key: &str,
    label: &str,
) -> UseResult<Vec<PluginPackagePublicationEvidence>> {
    if results.len() != intents.len() {
        return Err(UseError::new(
            "use.plugin.package_graph_publication_invalid",
            "The Registry omitted a package from dependency-graph capability publication.",
        ));
    }
    intents
        .iter()
        .zip(results)
        .map(|(intent, result)| {
            if result.extension.receipt.package_id != intent.package_id {
                return Err(UseError::new(
                    "use.plugin.package_graph_publication_invalid",
                    "The Registry changed package order in capability publication evidence.",
                ));
            }
            let evidence = checkpoint_evidence(
                label,
                intent,
                idempotency_key,
                &result.extension.receipt.descriptor_digest()?,
            )?;
            PluginPackagePublicationEvidence::new(&intent.package_id, evidence)
        })
        .collect()
}

fn registry_cutover_evidence(
    registry_generation_after: u64,
    registry_snapshot_digest: String,
) -> UseResult<PluginCapabilityCutoverEvidence> {
    let registry_generation_before = registry_generation_after
        .checked_sub(1)
        .ok_or_else(capability_generation_invalid)?;
    PluginCapabilityCutoverEvidence::new(
        registry_generation_before,
        registry_generation_after,
        registry_snapshot_digest,
    )
}

fn capability_generation_invalid() -> UseError {
    UseError::new(
        "use.plugin.package_graph_generation_invalid",
        "Registry cutover did not produce a positive next capability generation.",
    )
}

#[cfg(test)]
mod cutover_tests {
    use super::*;

    #[test]
    fn first_registry_cutover_preserves_the_exact_zero_to_one_generation() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let evidence = registry_cutover_evidence(1, digest.clone()).unwrap();
        assert_eq!(evidence.capability_generation_before(), 0);
        assert_eq!(evidence.capability_generation_after(), 1);
        assert_eq!(evidence.capability_snapshot_digest(), digest);
    }
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
        assert_send_sync::<ExtensionGraphCapabilityLifecycleHost>();
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
    async fn upgrade_commits_a_disabled_candidate_while_the_prior_generation_stays_published() {
        let temp = tempfile::tempdir().unwrap();
        let registry = ExtensionRegistry::new(a3s_use_extension::ExtensionPaths::new(
            temp.path().join("data"),
            temp.path().join("state"),
        ));
        let package_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates/extension/fixtures/packages/plugin-v3-cognitive/package");
        let candidate =
            ExtensionLifecyclePackage::prepare_local("acme/cognitive", &package_root, true)
                .await
                .unwrap();
        let first = ExtensionLifecycleIdentity::new(
            candidate.manifest().package_id.clone(),
            candidate.package_digest().to_string(),
            candidate.manifest_digest().to_string(),
            1,
        )
        .unwrap();
        registry
            .commit_lifecycle_package(&first, &candidate)
            .await
            .unwrap();
        registry.publish_lifecycle_package(&first).await.unwrap();

        let intent = PluginLifecycleIntent::from_manifest(
            crate::plugin_lifecycle::PluginLifecycleIntentSpec {
                operation_id: "upgrade:acme-cognitive:2".to_string(),
                plan_digest: format!("sha256:{}", "1".repeat(64)),
                scope_id: "workspace:cognitive".to_string(),
                package_id: candidate.manifest().package_id.clone(),
                package_digest: candidate.package_digest().to_string(),
                manifest_digest: candidate.manifest_digest().to_string(),
                generation: 2,
                action: PluginLifecycleAction::Upgrade,
            },
            candidate.manifest(),
        )
        .unwrap();
        let host = ExtensionPackageLifecycleHost::new(registry.clone(), candidate);
        host.commit_package(&intent, &intent.checkpoints[0].idempotency_key)
            .await
            .unwrap();

        let selected = registry.get("acme/cognitive").await.unwrap().unwrap();
        assert_eq!(selected.receipt.lifecycle_generation, Some(2));
        assert!(!selected.receipt.enabled);
        assert_eq!(
            registry.snapshot().await.unwrap().routes[0].lifecycle_generation,
            Some(1)
        );
        assert!(registry
            .get_lifecycle_generation(&first)
            .await
            .unwrap()
            .is_some());
    }
}
