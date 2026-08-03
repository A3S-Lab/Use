use std::path::PathBuf;

use a3s_use_core::{PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult};
use a3s_use_extension::{
    inspect_skill_surface_file, inspect_ui_surface_files, PluginSkillSurface, PluginUiSurface,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::{
    PluginLifecycleEvidence, PluginLifecycleIntent, PluginSkillLifecycleHost, PluginUiLifecycleHost,
};

/// Immutable-file adapter for Skill and UI package contributions.
///
/// Preparation revalidates and content-addresses every declared file. Stop and
/// remove are intentionally projection-owned no-ops: capability publication
/// controls visibility, and only the package host may delete immutable package
/// generations after all surface checkpoints complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticPluginSurfaceLifecycleHost {
    package_root: PathBuf,
}

impl StaticPluginSurfaceLifecycleHost {
    pub fn new(package_root: impl Into<PathBuf>) -> Self {
        Self {
            package_root: package_root.into(),
        }
    }

    pub fn package_root(&self) -> &std::path::Path {
        &self.package_root
    }
}

#[async_trait]
impl PluginSkillLifecycleHost for StaticPluginSurfaceLifecycleHost {
    async fn prepare_skill(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginSkillSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_surface(intent, PluginSurfaceKind::Skill, &surface.id)?;
        let files = inspect_skill_surface_file(surface, &self.package_root).await?;
        surface_evidence(
            "skill-prepared",
            intent,
            &surface.id,
            idempotency_key,
            files.digest(),
        )
    }

    async fn stop_skill(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginSkillSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_surface(intent, PluginSurfaceKind::Skill, &surface.id)?;
        projection_evidence("skill-hidden", intent, &surface.id, idempotency_key)
    }

    async fn remove_skill(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginSkillSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_surface(intent, PluginSurfaceKind::Skill, &surface.id)?;
        projection_evidence("skill-removed", intent, &surface.id, idempotency_key)
    }
}

#[async_trait]
impl PluginUiLifecycleHost for StaticPluginSurfaceLifecycleHost {
    async fn prepare_ui(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginUiSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_surface(intent, PluginSurfaceKind::Ui, &surface.id)?;
        let files = inspect_ui_surface_files(surface, &self.package_root).await?;
        surface_evidence(
            "ui-prepared",
            intent,
            &surface.id,
            idempotency_key,
            files.digest(),
        )
    }

    async fn stop_ui(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginUiSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_surface(intent, PluginSurfaceKind::Ui, &surface.id)?;
        projection_evidence("ui-hidden", intent, &surface.id, idempotency_key)
    }

    async fn remove_ui(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginUiSurface,
        idempotency_key: &str,
    ) -> UseResult<PluginLifecycleEvidence> {
        validate_surface(intent, PluginSurfaceKind::Ui, &surface.id)?;
        projection_evidence("ui-removed", intent, &surface.id, idempotency_key)
    }
}

fn validate_surface(
    intent: &PluginLifecycleIntent,
    kind: PluginSurfaceKind,
    surface_id: &str,
) -> UseResult<()> {
    intent.validate()?;
    let reference = PluginSurfaceRef {
        kind,
        id: surface_id.to_string(),
    };
    if !intent
        .surfaces
        .iter()
        .any(|candidate| candidate.surface == reference)
    {
        return Err(UseError::new(
            "use.plugin.static_surface_mismatch",
            "The static lifecycle call is absent from the admitted package surface inventory.",
        ));
    }
    Ok(())
}

fn projection_evidence(
    label: &str,
    intent: &PluginLifecycleIntent,
    surface_id: &str,
    idempotency_key: &str,
) -> UseResult<PluginLifecycleEvidence> {
    let subject = format!(
        "{}\n{}\n{}\n{}\n{}",
        intent.scope_id, intent.package_id, surface_id, intent.generation, intent.package_digest
    );
    let subject = format!("sha256:{:x}", Sha256::digest(subject.as_bytes()));
    surface_evidence(label, intent, surface_id, idempotency_key, &subject)
}

fn surface_evidence(
    label: &str,
    intent: &PluginLifecycleIntent,
    surface_id: &str,
    idempotency_key: &str,
    subject_digest: &str,
) -> UseResult<PluginLifecycleEvidence> {
    let identity = format!(
        "{label}\n{idempotency_key}\n{}\n{}\n{surface_id}\n{}\n{}\n{subject_digest}",
        intent.package_id, intent.scope_id, intent.generation, intent.manifest_digest
    );
    PluginLifecycleEvidence::new(format!("sha256:{:x}", Sha256::digest(identity.as_bytes())))
}

#[cfg(test)]
mod tests;
