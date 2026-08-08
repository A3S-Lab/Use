use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionPaths {
    data_root: PathBuf,
    state_root: PathBuf,
}

impl ExtensionPaths {
    pub fn from_env() -> UseResult<Self> {
        if let Some(root) = std::env::var_os("A3S_USE_HOME") {
            let root = absolute(PathBuf::from(root))?;
            return Ok(Self {
                data_root: root.join("data"),
                state_root: root.join("state"),
            });
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
        Ok(Self {
            data_root,
            state_root,
        })
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

    pub(crate) fn extensions_root(&self) -> PathBuf {
        self.data_root.join("extensions")
    }

    pub(crate) fn receipts_root(&self) -> PathBuf {
        self.state_root.join("extensions")
    }

    pub(crate) fn retained_lifecycle_receipts_root(&self) -> PathBuf {
        self.state_root.join("extension-generations")
    }

    pub(crate) fn package_parent(&self, package_id: &str) -> PathBuf {
        append_package_id(self.extensions_root(), package_id)
    }

    pub(crate) fn lifecycle_package_root(
        &self,
        package_id: &str,
        generation: u64,
        package_sha256: &str,
    ) -> PathBuf {
        self.package_parent(package_id)
            .join(format!("lifecycle-{generation}-{package_sha256}"))
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
        self.state_root.join("registry.json")
    }

    pub(crate) fn lifecycle_package_lock_path(&self, package_id: &str, generation: u64) -> PathBuf {
        append_package_id(self.state_root.join("route-locks"), package_id)
            .join(format!("{generation:020}.lock"))
    }

    pub fn tuf_datastore(&self, registry_name: &str) -> UseResult<PathBuf> {
        super::remote::validate_registry_name(registry_name)?;
        Ok(self
            .state_root
            .join("remote-registries")
            .join(registry_name))
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
        let paths = ExtensionPaths::new("/data/use", "/state/use");
        assert_eq!(
            paths.lifecycle_package_root("acme/slack", 7, &"a".repeat(64)),
            PathBuf::from(format!(
                "/data/use/extensions/acme/slack/lifecycle-7-{}",
                "a".repeat(64)
            ))
        );
        assert_eq!(
            paths.receipt_path("acme/slack"),
            PathBuf::from("/state/use/extensions/acme/slack.json")
        );
        assert_eq!(
            paths.retained_lifecycle_receipt_path("acme/slack", 7, &"a".repeat(64)),
            PathBuf::from(format!(
                "/state/use/extension-generations/acme/slack/{:020}-{}.json",
                7,
                "a".repeat(64)
            ))
        );
        assert_eq!(
            paths.lifecycle_package_lock_path("acme/slack", 7),
            PathBuf::from("/state/use/route-locks/acme/slack/00000000000000000007.lock")
        );
        assert_eq!(
            paths.registry_snapshot_path(),
            PathBuf::from("/state/use/registry.json")
        );
        assert_eq!(
            paths.tuf_datastore("a3s").unwrap(),
            PathBuf::from("/state/use/remote-registries/a3s")
        );
        assert_eq!(
            paths
                .registry_source_datastore("a3s", &"b".repeat(64))
                .unwrap(),
            PathBuf::from(format!(
                "/state/use/remote-registries/a3s/sources/{}",
                "b".repeat(64)
            ))
        );
        assert_eq!(
            paths.registry_trusted_root_path(&"c".repeat(64)).unwrap(),
            PathBuf::from(format!(
                "/state/use/registry-trust-roots/sha256/{}.json",
                "c".repeat(64)
            ))
        );
        assert_eq!(
            paths.tuf_datastore("../escape").unwrap_err().code,
            "use.extension.registry_name_invalid"
        );
    }
}
