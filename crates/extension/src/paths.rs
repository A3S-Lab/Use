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

    pub(crate) fn package_parent(&self, package_id: &str) -> PathBuf {
        append_package_id(self.extensions_root(), package_id)
    }

    pub(crate) fn package_root(&self, package_id: &str, version: &str) -> PathBuf {
        self.package_parent(package_id).join(version)
    }

    pub(crate) fn receipt_path(&self, package_id: &str) -> PathBuf {
        let mut path = append_package_id(self.receipts_root(), package_id);
        path.set_extension("json");
        path
    }

    pub(crate) fn registry_lock_path(&self) -> PathBuf {
        self.receipts_root().join(".registry.lock")
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
            paths.package_root("acme/slack", "1.2.0"),
            PathBuf::from("/data/use/extensions/acme/slack/1.2.0")
        );
        assert_eq!(
            paths.receipt_path("acme/slack"),
            PathBuf::from("/state/use/extensions/acme/slack.json")
        );
    }
}
