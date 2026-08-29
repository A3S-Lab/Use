use std::path::PathBuf;

use a3s_use_core::{UseError, UseResult, VerifiedCatalogProvenance};
use serde::Serialize;

use crate::remote::{normalize_registry_url, normalize_sha256};
use crate::{
    RegistryNetworkPolicy, TrustedRegistry, UsePaths, VerifiedTargetCachePolicy,
    VerifiedTargetObservation,
};

mod acl;
mod artifact_references;
mod github;
mod io;

pub use artifact_references::{
    RegistryArtifactReference, RegistryArtifactReferenceInventory,
    REGISTRY_ARTIFACT_REFERENCE_INVENTORY_SCHEMA,
};

pub use github::{
    GitHubRegistryRepository, DEFAULT_GITHUB_REGISTRY_PATH, DEFAULT_GITHUB_REGISTRY_REF,
};

pub const REGISTRY_SOURCE_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const MAX_CONFIGURED_REGISTRY_SOURCES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrySource {
    pub name: String,
    pub registry_url: String,
    pub root_sha256: String,
    pub enabled: bool,
    pub imported_trusted_root: bool,
    pub cache_policy: VerifiedTargetCachePolicy,
    pub source_identity: String,
}

#[derive(Debug, Clone)]
pub struct RegistrySourceInput {
    pub name: String,
    pub registry_url: String,
    pub root_sha256: String,
    pub trusted_root_path: Option<PathBuf>,
    pub cache_policy: VerifiedTargetCachePolicy,
}

impl RegistrySourceInput {
    pub fn new(
        name: impl Into<String>,
        registry_url: impl Into<String>,
        root_sha256: impl Into<String>,
        trusted_root_path: Option<PathBuf>,
        cache_policy: VerifiedTargetCachePolicy,
    ) -> Self {
        Self {
            name: name.into(),
            registry_url: registry_url.into(),
            root_sha256: root_sha256.into(),
            trusted_root_path,
            cache_policy,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrySourceSnapshot {
    pub schema_version: u32,
    pub revision: String,
    pub default_registry: Option<String>,
    pub sources: Vec<RegistrySource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrySourceMutation {
    pub schema_version: u32,
    pub action: String,
    pub changed: bool,
    pub previous_revision: String,
    pub snapshot: RegistrySourceSnapshot,
}

#[derive(Debug, Clone)]
pub struct ResolvedRegistrySources {
    source_revision: String,
    root: TrustedRegistry,
    dependencies: Vec<TrustedRegistry>,
}

impl ResolvedRegistrySources {
    pub fn source_revision(&self) -> &str {
        &self.source_revision
    }

    pub fn root(&self) -> &TrustedRegistry {
        &self.root
    }

    pub fn dependencies(&self) -> &[TrustedRegistry] {
        &self.dependencies
    }

    /// Return every enabled Registry in deterministic source-name order.
    ///
    /// The selected root remains first so callers that deduplicate a
    /// multi-Registry catalog can give its exact records deterministic
    /// precedence. The remaining sources preserve the canonical ACL order.
    pub fn all(&self) -> impl Iterator<Item = &TrustedRegistry> {
        std::iter::once(&self.root).chain(self.dependencies.iter())
    }
}

#[derive(Debug, Clone)]
pub struct RegistrySourceStore {
    paths: UsePaths,
    network_policy: RegistryNetworkPolicy,
}

impl RegistrySourceStore {
    pub fn from_env() -> UseResult<Self> {
        Ok(Self::new(UsePaths::from_env()?))
    }

    pub fn new(paths: UsePaths) -> Self {
        Self {
            paths,
            network_policy: RegistryNetworkPolicy::default(),
        }
    }

    pub fn with_network_policy(mut self, policy: RegistryNetworkPolicy) -> Self {
        self.network_policy = policy;
        self
    }

    pub async fn snapshot(&self) -> UseResult<RegistrySourceSnapshot> {
        Ok(io::load(&self.paths).await?.snapshot())
    }

    /// Derive every durable Registry blob reference, including references in
    /// preserved datastores no longer selected by current source config.
    pub async fn inspect_artifact_references(
        &self,
        collection: &crate::ArtifactCollectionGuard,
    ) -> UseResult<RegistryArtifactReferenceInventory> {
        artifact_references::inspect(&self.paths, collection).await
    }

    /// Observe one exact target in the immutable datastore selected by retained
    /// catalog provenance, even if the current source configuration was later
    /// replaced or disabled. This performs no network request or state write.
    pub async fn observe_retained_target(
        &self,
        provenance: &VerifiedCatalogProvenance,
        expected_length: u64,
        expected_sha256: &str,
    ) -> UseResult<VerifiedTargetObservation> {
        provenance.validate()?;
        let registry_url = normalize_registry_url(&provenance.registry_url)?.to_string();
        let root_sha256 = normalize_sha256(&provenance.root_sha256, "registry trust root")?;
        let identity = source_identity(&provenance.registry_name, &registry_url, &root_sha256);
        let datastore = self
            .paths
            .registry_source_datastore(&provenance.registry_name, &identity)?;
        crate::remote::observe_verified_target_cache_entry(
            &datastore,
            &self.paths.artifact_store(),
            &provenance.registry_name,
            expected_length,
            expected_sha256,
        )
        .await
    }

    pub async fn add(&self, input: RegistrySourceInput) -> UseResult<RegistrySourceMutation> {
        let _lock = io::RegistrySourcesLock::acquire(&self.paths)?;
        let mut document = io::load(&self.paths).await?;
        let previous_revision = document.revision();
        let source = self.prepare_source(input, true).await?;

        if let Some(existing) = document.sources.get(&source.name) {
            if existing == &source {
                return Ok(mutation("add", false, previous_revision, document));
            }
            return Err(source_error(
                "use.extension.registry_source_exists",
                format!(
                    "Registry source '{}' already exists with different trust or policy configuration.",
                    source.name
                ),
            )
            .with_suggestion(
                "Review the current source revision, then use 'registry source replace'.",
            ));
        }
        if document.sources.len() >= MAX_CONFIGURED_REGISTRY_SOURCES {
            return Err(source_error(
                "use.extension.registry_sources_limit_exceeded",
                format!(
                    "Registry source configuration cannot exceed {MAX_CONFIGURED_REGISTRY_SOURCES} entries."
                ),
            ));
        }
        let name = source.name.clone();
        document.sources.insert(name.clone(), source);
        if document.default_registry.is_none() {
            document.default_registry = Some(name);
        }
        io::write(&self.paths, &document).await?;
        Ok(mutation("add", true, previous_revision, document))
    }

    pub async fn replace(
        &self,
        expected_revision: &str,
        input: RegistrySourceInput,
    ) -> UseResult<RegistrySourceMutation> {
        let _lock = io::RegistrySourcesLock::acquire(&self.paths)?;
        let mut document = io::load(&self.paths).await?;
        let previous_revision = require_revision(&document, expected_revision)?;
        if !document.sources.contains_key(&input.name) {
            return Err(source_not_found(&input.name));
        }
        let enabled = document.sources[&input.name].enabled;
        let source = self.prepare_source(input, enabled).await?;
        let changed = document.sources.get(&source.name) != Some(&source);
        if changed {
            document.sources.insert(source.name.clone(), source);
            io::write(&self.paths, &document).await?;
        }
        Ok(mutation("replace", changed, previous_revision, document))
    }

    pub async fn remove(
        &self,
        name: &str,
        expected_revision: &str,
    ) -> UseResult<RegistrySourceMutation> {
        let _lock = io::RegistrySourcesLock::acquire(&self.paths)?;
        let mut document = io::load(&self.paths).await?;
        let previous_revision = require_revision(&document, expected_revision)?;
        if !document.sources.contains_key(name) {
            return Err(source_not_found(name));
        }
        let another_enabled_source_exists = document
            .sources
            .iter()
            .any(|(source_name, source)| source_name != name && source.enabled);
        if document.default_registry.as_deref() == Some(name) && another_enabled_source_exists {
            return Err(source_error(
                "use.extension.registry_source_default_conflict",
                format!("Registry source '{name}' is the current default and cannot be removed."),
            )
            .with_suggestion(
                "Select another default source with 'registry source default' before removal.",
            ));
        }
        document.sources.remove(name);
        if document.default_registry.as_deref() == Some(name) {
            document.default_registry = None;
        }
        io::write(&self.paths, &document).await?;
        Ok(mutation("remove", true, previous_revision, document))
    }

    pub async fn set_default(
        &self,
        name: &str,
        expected_revision: &str,
    ) -> UseResult<RegistrySourceMutation> {
        let _lock = io::RegistrySourcesLock::acquire(&self.paths)?;
        let mut document = io::load(&self.paths).await?;
        let previous_revision = require_revision(&document, expected_revision)?;
        let source = document
            .sources
            .get(name)
            .ok_or_else(|| source_not_found(name))?;
        if !source.enabled {
            return Err(source_error(
                "use.extension.registry_source_disabled",
                format!("Registry source '{name}' is disabled and cannot become the default."),
            )
            .with_suggestion(
                "Enable the source under the reviewed configuration revision first.",
            ));
        }
        let changed = document.default_registry.as_deref() != Some(name);
        if changed {
            document.default_registry = Some(name.to_owned());
            io::write(&self.paths, &document).await?;
        }
        Ok(mutation("default", changed, previous_revision, document))
    }

    pub async fn disable(
        &self,
        name: &str,
        expected_revision: &str,
    ) -> UseResult<RegistrySourceMutation> {
        let _lock = io::RegistrySourcesLock::acquire(&self.paths)?;
        let mut document = io::load(&self.paths).await?;
        let previous_revision = require_revision(&document, expected_revision)?;
        let source = document
            .sources
            .get(name)
            .ok_or_else(|| source_not_found(name))?;
        if !source.enabled {
            return Ok(mutation("disable", false, previous_revision, document));
        }
        let enabled_count = document
            .sources
            .values()
            .filter(|source| source.enabled)
            .count();
        if document.default_registry.as_deref() == Some(name) && enabled_count > 1 {
            return Err(source_error(
                "use.extension.registry_source_default_conflict",
                format!("Registry source '{name}' is the current default and cannot be disabled."),
            )
            .with_suggestion(
                "Select another default source with 'registry source default' before disabling it.",
            ));
        }
        document
            .sources
            .get_mut(name)
            .ok_or_else(|| source_not_found(name))?
            .enabled = false;
        if document.default_registry.as_deref() == Some(name) {
            document.default_registry = None;
        }
        io::write(&self.paths, &document).await?;
        Ok(mutation("disable", true, previous_revision, document))
    }

    pub async fn enable(
        &self,
        name: &str,
        expected_revision: &str,
    ) -> UseResult<RegistrySourceMutation> {
        let _lock = io::RegistrySourcesLock::acquire(&self.paths)?;
        let mut document = io::load(&self.paths).await?;
        let previous_revision = require_revision(&document, expected_revision)?;
        let source = document
            .sources
            .get_mut(name)
            .ok_or_else(|| source_not_found(name))?;
        if source.enabled {
            return Ok(mutation("enable", false, previous_revision, document));
        }
        source.enabled = true;
        if document.default_registry.is_none() {
            document.default_registry = Some(name.to_owned());
        }
        io::write(&self.paths, &document).await?;
        Ok(mutation("enable", true, previous_revision, document))
    }

    pub async fn resolve(&self, selected: Option<&str>) -> UseResult<ResolvedRegistrySources> {
        let document = io::load(&self.paths).await?;
        let selected = match selected {
            Some(name) => name,
            None => document.default_registry.as_deref().ok_or_else(|| {
                source_error(
                    "use.extension.registry_source_default_missing",
                    "No default Registry source is configured.",
                )
                .with_suggestion(
                    "Add a source with 'a3s-use registry source add' or select one with --registry-name.",
                )
            })?,
        };
        let root_source = document
            .sources
            .get(selected)
            .ok_or_else(|| source_not_found(selected))?;
        if !root_source.enabled {
            return Err(source_error(
                "use.extension.registry_source_disabled",
                format!("Registry source '{selected}' is disabled."),
            ));
        }
        let root = self.trusted_registry(root_source).await?;
        let mut dependencies = Vec::with_capacity(document.sources.len().saturating_sub(1));
        for (name, source) in &document.sources {
            if name != selected && source.enabled {
                dependencies.push(self.trusted_registry(source).await?);
            }
        }
        Ok(ResolvedRegistrySources {
            source_revision: document.revision(),
            root,
            dependencies,
        })
    }

    async fn prepare_source(
        &self,
        input: RegistrySourceInput,
        enabled: bool,
    ) -> UseResult<RegistrySource> {
        let normalized_url = normalize_registry_url(&input.registry_url)?.to_string();
        let normalized_root = normalize_sha256(&input.root_sha256, "registry trust root")?;
        crate::remote::validate_registry_name(&input.name)?;
        let imported_trusted_root = if let Some(path) = input.trusted_root_path.as_deref() {
            io::import_trusted_root(&self.paths, path, &normalized_root).await?;
            true
        } else {
            false
        };
        Ok(RegistrySource::from_normalized(
            input.name,
            normalized_url,
            normalized_root,
            enabled,
            imported_trusted_root,
            input.cache_policy,
        ))
    }

    async fn trusted_registry(&self, source: &RegistrySource) -> UseResult<TrustedRegistry> {
        let trusted_root_path = if source.imported_trusted_root {
            let path = self.paths.registry_trusted_root_path(&source.root_sha256)?;
            io::validate_managed_trusted_root(&path, &source.root_sha256).await?;
            Some(path)
        } else {
            None
        };
        Ok(TrustedRegistry::new(
            source.name.clone(),
            &source.registry_url,
            &source.root_sha256,
            trusted_root_path,
            self.paths
                .registry_source_datastore(&source.name, &source.source_identity)?,
            self.paths.artifact_store(),
        )?
        .with_target_cache_policy(source.cache_policy)
        .with_network_policy(self.network_policy))
    }
}

impl RegistrySource {
    fn from_persisted(
        paths: &UsePaths,
        name: String,
        registry_url: String,
        root_sha256: String,
        enabled: bool,
        imported_trusted_root: bool,
        cache_policy: VerifiedTargetCachePolicy,
    ) -> UseResult<Self> {
        crate::remote::validate_registry_name(&name)?;
        let normalized_url = normalize_registry_url(&registry_url)?.to_string();
        let normalized_root = normalize_sha256(&root_sha256, "registry trust root")?;
        if normalized_url != registry_url || normalized_root != root_sha256 {
            return Err(source_error(
                "use.extension.registry_sources_invalid",
                "Registry source configuration is not canonical.",
            ));
        }
        let source = Self::from_normalized(
            name,
            normalized_url,
            normalized_root,
            enabled,
            imported_trusted_root,
            cache_policy,
        );
        paths.registry_source_datastore(&source.name, &source.source_identity)?;
        paths.registry_trusted_root_path(&source.root_sha256)?;
        Ok(source)
    }

    fn from_normalized(
        name: String,
        registry_url: String,
        root_sha256: String,
        enabled: bool,
        imported_trusted_root: bool,
        cache_policy: VerifiedTargetCachePolicy,
    ) -> Self {
        let source_identity = source_identity(&name, &registry_url, &root_sha256);
        Self {
            name,
            registry_url,
            root_sha256,
            enabled,
            imported_trusted_root,
            cache_policy,
            source_identity,
        }
    }
}

fn source_identity(name: &str, registry_url: &str, root_sha256: &str) -> String {
    crate::remote::registry_source_identity(name, registry_url, root_sha256)
}

fn mutation(
    action: &str,
    changed: bool,
    previous_revision: String,
    document: acl::RegistrySourcesDocument,
) -> RegistrySourceMutation {
    RegistrySourceMutation {
        schema_version: REGISTRY_SOURCE_CONFIG_SCHEMA_VERSION,
        action: action.to_owned(),
        changed,
        previous_revision,
        snapshot: document.snapshot(),
    }
}

fn require_revision(
    document: &acl::RegistrySourcesDocument,
    expected_revision: &str,
) -> UseResult<String> {
    let expected = normalize_sha256(expected_revision, "Registry source configuration revision")?;
    let actual = document.revision();
    if expected == actual {
        Ok(actual)
    } else {
        Err(source_error(
            "use.extension.registry_sources_revision_mismatch",
            "Registry source configuration changed after it was reviewed.",
        )
        .with_detail("expected", expected)
        .with_detail("actual", actual)
        .with_suggestion("List the sources again and review the new revision before retrying."))
    }
}

fn source_not_found(name: &str) -> UseError {
    source_error(
        "use.extension.registry_source_not_found",
        format!("Registry source '{name}' is not configured."),
    )
}

fn source_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests;
