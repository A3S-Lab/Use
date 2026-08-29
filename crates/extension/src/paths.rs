use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseError, UseResult};

use crate::ArtifactStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsePaths {
    data_root: PathBuf,
    state_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionPaths {
    roots: UsePaths,
    installation: InstallationId,
    data_root: PathBuf,
    state_root: PathBuf,
}

impl UsePaths {
    pub fn from_env() -> UseResult<Self> {
        if let Some(root) = std::env::var_os("A3S_USE_HOME") {
            let root = absolute(PathBuf::from(root))?;
            return Ok(Self::new(root.join("data"), root.join("state")));
        }

        let home = std::env::var_os("HOME").map(PathBuf::from);
        let data_root = configured_root(
            "A3S_DATA_HOME",
            "XDG_DATA_HOME",
            home.as_deref().map(|path| path.join(".local/share")),
        )?
        .join("use");
        let state_root = configured_root(
            "A3S_STATE_HOME",
            "XDG_STATE_HOME",
            home.as_deref().map(|path| path.join(".local/state")),
        )?
        .join("use");
        Ok(Self::new(data_root, state_root))
    }

    pub fn new(data_root: impl Into<PathBuf>, state_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            state_root: state_root.into(),
        }
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub fn artifact_store(&self) -> ArtifactStore {
        ArtifactStore::from_data_root(&self.data_root)
    }

    pub fn for_installation(&self, installation: InstallationId) -> UseResult<ExtensionPaths> {
        ExtensionPaths::from_roots(self.clone(), installation)
    }

    pub(crate) fn registry_sources_path(&self) -> PathBuf {
        self.state_root.join("registries.acl")
    }

    pub(crate) fn registry_sources_lock_path(&self) -> PathBuf {
        self.state_root.join(".registries.lock")
    }

    pub(crate) fn registry_source_datastore(
        &self,
        registry_name: &str,
        source_identity: &str,
    ) -> UseResult<PathBuf> {
        super::remote::validate_registry_name(registry_name)?;
        validate_sha256_path_segment(source_identity, "Registry source identity")?;
        Ok(self
            .state_root
            .join("remote-registries")
            .join(registry_name)
            .join("sources")
            .join(source_identity))
    }

    pub(crate) fn registry_trusted_root_path(&self, root_sha256: &str) -> UseResult<PathBuf> {
        validate_sha256_path_segment(root_sha256, "Registry trust-root digest")?;
        Ok(self
            .state_root
            .join("registry-trust-roots")
            .join("sha256")
            .join(format!("{root_sha256}.json")))
    }
}

impl ExtensionPaths {
    pub fn from_env(installation: InstallationId) -> UseResult<Self> {
        UsePaths::from_env()?.for_installation(installation)
    }

    pub fn new(
        data_root: impl Into<PathBuf>,
        state_root: impl Into<PathBuf>,
        installation: InstallationId,
    ) -> UseResult<Self> {
        Self::from_roots(UsePaths::new(data_root, state_root), installation)
    }

    pub fn from_roots(roots: UsePaths, installation: InstallationId) -> UseResult<Self> {
        let installation_key = installation.storage_key()?;
        reject_unscoped_installation_state(&roots)?;
        let data_root = installation_root(roots.data_root(), &installation, &installation_key);
        let state_root = installation_root(roots.state_root(), &installation, &installation_key);
        reject_legacy_installation_artifacts(&data_root)?;
        Ok(Self {
            roots,
            installation,
            data_root,
            state_root,
        })
    }

    pub fn use_paths(&self) -> &UsePaths {
        &self.roots
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub fn installation(&self) -> &InstallationId {
        &self.installation
    }

    pub fn artifact_store(&self) -> ArtifactStore {
        self.roots.artifact_store()
    }

    pub fn installation_state_root(&self) -> PathBuf {
        self.state_root.clone()
    }

    pub(crate) fn receipts_root(&self) -> PathBuf {
        self.installation_state_root().join("extensions")
    }

    pub(crate) fn retained_lifecycle_receipts_root(&self) -> PathBuf {
        self.installation_state_root().join("extension-generations")
    }

    pub(crate) fn receipt_path(&self, package_id: &str) -> PathBuf {
        let mut path = append_package_id(self.receipts_root(), package_id);
        path.set_extension("json");
        path
    }

    pub(crate) fn retained_lifecycle_receipt_directory(&self, package_id: &str) -> PathBuf {
        append_package_id(self.retained_lifecycle_receipts_root(), package_id)
    }

    pub(crate) fn retained_lifecycle_receipt_path(
        &self,
        package_id: &str,
        generation: u64,
        package_sha256: &str,
    ) -> PathBuf {
        self.retained_lifecycle_receipt_directory(package_id)
            .join(format!("{generation:020}-{package_sha256}.json"))
    }

    pub(crate) fn registry_lock_path(&self) -> PathBuf {
        self.receipts_root().join(".registry.lock")
    }

    pub(crate) fn registry_snapshot_path(&self) -> PathBuf {
        self.installation_state_root().join("registry.json")
    }

    pub(crate) fn lifecycle_package_lock_path(&self, package_id: &str, generation: u64) -> PathBuf {
        append_package_id(
            self.installation_state_root().join("generation-leases"),
            package_id,
        )
        .join(format!("{generation:020}.lock"))
    }
}

fn reject_legacy_installation_artifacts(data_root: &Path) -> UseResult<()> {
    let legacy = data_root.join("extensions");
    match std::fs::symlink_metadata(&legacy) {
        Ok(_) => Err(UseError::new(
            "use.artifact_store.legacy_state_unsupported",
            format!(
                "Installation-scoped package bytes '{}' are unsupported.",
                legacy.display()
            ),
        )
        .with_suggestion(
            "Preserve the old installation for incident review, remove it with an approved cleanup procedure, then reinstall into the global Artifact Store.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(UseError::new(
            "use.artifact_store.state_inspection_failed",
            format!(
                "Installation-scoped package bytes '{}' cannot be inspected: {error}",
                legacy.display()
            ),
        )),
    }
}

fn installation_root(root: &Path, installation: &InstallationId, key: &str) -> PathBuf {
    root.join("installations")
        .join(installation.kind.as_str())
        .join(key)
}

const LEGACY_DATA_ENTRIES: &[&str] = &["extensions"];
const LEGACY_STATE_ENTRIES: &[&str] = &[
    ".installation-mutation.lock",
    ".maintenance.lock",
    ".package-graph.lock",
    "bindings",
    "extension-generations",
    "extensions",
    "flow-runtime",
    "grants",
    "knowledge",
    "operations",
    "package-enablement",
    "package-graphs",
    "plugin-host-manager",
    "registry.json",
    "generation-leases",
    "route-locks",
];

fn reject_unscoped_installation_state(roots: &UsePaths) -> UseResult<()> {
    for path in LEGACY_DATA_ENTRIES
        .iter()
        .map(|entry| roots.data_root().join(entry))
        .chain(
            LEGACY_STATE_ENTRIES
                .iter()
                .map(|entry| roots.state_root().join(entry)),
        )
    {
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                return Err(UseError::new(
                    "use.installation.legacy_state_unsupported",
                    format!(
                        "Unscoped pre-release installation state '{}' is unsupported.",
                        path.display()
                    ),
                )
                .with_suggestion(
                    "Preserve the old state for incident review, remove it with an approved cleanup procedure, then reinstall into an explicit User or Workspace installation.",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(UseError::new(
                    "use.installation.state_inspection_failed",
                    format!(
                        "Unscoped pre-release installation state '{}' cannot be inspected: {error}",
                        path.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_sha256_path_segment(value: &str, label: &str) -> UseResult<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(UseError::new(
            "use.extension.registry_sources_invalid",
            format!("{label} must be exactly 64 lowercase hexadecimal characters."),
        ))
    }
}

fn configured_root(
    a3s_variable: &str,
    xdg_variable: &str,
    fallback_parent: Option<PathBuf>,
) -> UseResult<PathBuf> {
    if let Some(value) = std::env::var_os(a3s_variable) {
        return absolute(PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os(xdg_variable) {
        return Ok(absolute(PathBuf::from(value))?.join("a3s"));
    }
    if let Some(parent) = fallback_parent {
        return Ok(absolute(parent)?.join("a3s"));
    }
    #[cfg(windows)]
    if let Some(value) = std::env::var_os("LOCALAPPDATA") {
        return Ok(absolute(PathBuf::from(value))?.join("a3s"));
    }
    Err(UseError::new(
        "use.paths.unavailable",
        format!("{a3s_variable} is not set and no home directory is available."),
    ))
}

fn absolute(path: PathBuf) -> UseResult<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .map_err(|error| {
            UseError::new(
                "use.paths.unavailable",
                format!("Failed to resolve a relative A3S path: {error}"),
            )
        })
}

fn append_package_id(mut root: PathBuf, package_id: &str) -> PathBuf {
    for segment in package_id.split('/') {
        root.push(segment);
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_paths_preserve_publisher_namespace() {
        let installation =
            InstallationId::new(a3s_use_core::InstallationKind::Workspace, "same/identity")
                .unwrap();
        let paths = ExtensionPaths::new("/data/use", "/state/use", installation.clone()).unwrap();
        let key = installation.storage_key().unwrap();
        let data = PathBuf::from(format!("/data/use/installations/workspace/{key}"));
        let state = PathBuf::from(format!("/state/use/installations/workspace/{key}"));
        assert_eq!(paths.installation(), &installation);
        assert_eq!(paths.data_root(), data);
        assert_eq!(paths.installation_state_root(), state);
        assert_eq!(
            paths
                .artifact_store()
                .expanded_package_path(&format!("sha256:{}", "a".repeat(64)))
                .unwrap(),
            PathBuf::from(format!(
                "/data/use/artifacts/expanded-packages/sha256/aa/{}/content",
                "a".repeat(64)
            ))
        );
        assert_eq!(
            paths.receipt_path("acme/slack"),
            state.join("extensions/acme/slack.json")
        );
        assert_eq!(
            paths.retained_lifecycle_receipt_path("acme/slack", 7, &"a".repeat(64)),
            state.join(format!(
                "extension-generations/acme/slack/{:020}-{}.json",
                7,
                "a".repeat(64)
            ))
        );
        assert_eq!(
            paths.lifecycle_package_lock_path("acme/slack", 7),
            state.join("generation-leases/acme/slack/00000000000000000007.lock")
        );
        assert_eq!(paths.registry_snapshot_path(), state.join("registry.json"));
        assert_eq!(
            paths
                .use_paths()
                .registry_source_datastore("a3s", &"b".repeat(64))
                .unwrap(),
            PathBuf::from(format!(
                "/state/use/remote-registries/a3s/sources/{}",
                "b".repeat(64)
            ))
        );
        assert_eq!(
            paths
                .use_paths()
                .registry_trusted_root_path(&"c".repeat(64))
                .unwrap(),
            PathBuf::from(format!(
                "/state/use/registry-trust-roots/sha256/{}.json",
                "c".repeat(64)
            ))
        );
    }

    #[test]
    fn same_textual_id_has_distinct_user_and_workspace_roots() {
        let user = ExtensionPaths::new(
            "/data/use",
            "/state/use",
            InstallationId::new(a3s_use_core::InstallationKind::User, "same").unwrap(),
        )
        .unwrap();
        let workspace = ExtensionPaths::new(
            "/data/use",
            "/state/use",
            InstallationId::new(a3s_use_core::InstallationKind::Workspace, "same").unwrap(),
        )
        .unwrap();

        assert_ne!(user.data_root(), workspace.data_root());
        assert_ne!(
            user.installation_state_root(),
            workspace.installation_state_root()
        );
        assert_eq!(user.artifact_store(), workspace.artifact_store());
    }

    #[test]
    fn unscoped_installation_state_is_rejected_but_global_registry_state_is_allowed() {
        let temporary = tempfile::tempdir().unwrap();
        let roots = UsePaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
        );
        std::fs::create_dir_all(roots.state_root().join("remote-registries/fixture")).unwrap();
        std::fs::create_dir_all(roots.state_root().join("registry-trust-roots/sha256")).unwrap();
        std::fs::write(
            roots.state_root().join("registries.acl"),
            b"schema_version = 1\n",
        )
        .unwrap();
        let installation = InstallationId::new(
            a3s_use_core::InstallationKind::Workspace,
            "workspace/acme-project",
        )
        .unwrap();
        roots.for_installation(installation.clone()).unwrap();

        std::fs::create_dir_all(roots.state_root().join("bindings/runtime")).unwrap();
        let error = roots.for_installation(installation).unwrap_err();
        assert_eq!(error.code, "use.installation.legacy_state_unsupported");
    }

    #[test]
    fn installation_scoped_package_bytes_are_rejected_after_artifact_store_cutover() {
        let temporary = tempfile::tempdir().unwrap();
        let roots = UsePaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
        );
        let installation = InstallationId::new(
            a3s_use_core::InstallationKind::Workspace,
            "workspace/legacy-bytes",
        )
        .unwrap();
        let paths = roots.for_installation(installation.clone()).unwrap();
        std::fs::create_dir_all(paths.data_root().join("extensions/acme/legacy")).unwrap();

        let error = roots.for_installation(installation).unwrap_err();
        assert_eq!(error.code, "use.artifact_store.legacy_state_unsupported");
    }
}
