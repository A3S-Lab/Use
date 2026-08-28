use a3s_use_core::{UseError, UseResult};
use serde::Serialize;
use url::Url;

pub const DEFAULT_GITHUB_REGISTRY_REF: &str = "main";
pub const DEFAULT_GITHUB_REGISTRY_PATH: &str = "registry";

/// A GitHub repository location that publishes an A3S Use Registry tree.
///
/// GitHub is an authoring and static-distribution convenience only. The
/// resulting URL is still consumed through the ordinary caller-pinned TUF
/// trust root; Git history is not treated as package authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubRegistryRepository {
    owner: String,
    repository: String,
    git_ref: String,
    registry_path: String,
}

impl GitHubRegistryRepository {
    pub fn parse(slug: &str) -> UseResult<Self> {
        let mut parts = slug.split('/');
        let owner = parts.next().unwrap_or_default();
        let repository = parts.next().unwrap_or_default();
        if parts.next().is_some() || !valid_owner(owner) || !valid_repository_name(repository) {
            return Err(github_registry_error(
                "GitHub Registry repositories must use an exact 'owner/repository' slug.",
            ));
        }
        Ok(Self {
            owner: owner.to_owned(),
            repository: repository.to_owned(),
            git_ref: DEFAULT_GITHUB_REGISTRY_REF.to_owned(),
            registry_path: DEFAULT_GITHUB_REGISTRY_PATH.to_owned(),
        })
    }

    pub fn with_git_ref(mut self, git_ref: &str) -> UseResult<Self> {
        if !valid_component(git_ref, 255) || matches!(git_ref, "." | "..") {
            return Err(github_registry_error(
                "GitHub Registry refs may contain only letters, digits, '.', '_', and '-'.",
            ));
        }
        self.git_ref = git_ref.to_owned();
        Ok(self)
    }

    pub fn with_registry_path(mut self, registry_path: &str) -> UseResult<Self> {
        if registry_path.is_empty()
            || registry_path.len() > 512
            || registry_path.starts_with('/')
            || registry_path.ends_with('/')
            || registry_path
                .split('/')
                .any(|segment| !valid_component(segment, 100) || matches!(segment, "." | ".."))
        {
            return Err(github_registry_error(
                "GitHub Registry paths must be canonical relative paths without empty, '.', or '..' segments.",
            ));
        }
        self.registry_path = registry_path.to_owned();
        Ok(self)
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn repository(&self) -> &str {
        &self.repository
    }

    pub fn git_ref(&self) -> &str {
        &self.git_ref
    }

    pub fn registry_path(&self) -> &str {
        &self.registry_path
    }

    pub fn registry_url(&self) -> UseResult<String> {
        let mut url = Url::parse("https://raw.githubusercontent.com/").map_err(|error| {
            github_registry_error(format!(
                "Failed to initialize the GitHub Registry URL: {error}"
            ))
        })?;
        {
            let mut segments = url.path_segments_mut().map_err(|_| {
                github_registry_error("The GitHub Registry base URL cannot contain path segments.")
            })?;
            segments
                .push(&self.owner)
                .push(&self.repository)
                .push(&self.git_ref);
            for segment in self.registry_path.split('/') {
                segments.push(segment);
            }
            segments.push("");
        }
        Ok(url.to_string())
    }
}

fn valid_owner(value: &str) -> bool {
    valid_component(value, 100)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

fn valid_repository_name(value: &str) -> bool {
    valid_component(value, 100)
        && !matches!(value, "." | "..")
        && !value.ends_with(".git")
        && !value.starts_with('.')
        && !value.ends_with('.')
}

fn valid_component(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn github_registry_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.github_registry_invalid", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_slug_maps_to_the_default_registry_tree() {
        let repository = GitHubRegistryRepository::parse("A3S-Lab/Use-Registry").unwrap();

        assert_eq!(repository.git_ref(), "main");
        assert_eq!(repository.registry_path(), "registry");
        assert_eq!(
            repository.registry_url().unwrap(),
            "https://raw.githubusercontent.com/A3S-Lab/Use-Registry/main/registry/"
        );
    }

    #[test]
    fn explicit_ref_and_nested_registry_path_remain_canonical() {
        let repository = GitHubRegistryRepository::parse("a3s-lab/packages")
            .unwrap()
            .with_git_ref("v1.2.3")
            .unwrap()
            .with_registry_path("public/stable")
            .unwrap();

        assert_eq!(
            repository.registry_url().unwrap(),
            "https://raw.githubusercontent.com/a3s-lab/packages/v1.2.3/public/stable/"
        );
    }

    #[test]
    fn ambiguous_slugs_refs_and_paths_fail_closed() {
        for slug in [
            "packages",
            "a3s/packages/extra",
            "a3s/../packages",
            "a3s/packages.git",
        ] {
            assert_eq!(
                GitHubRegistryRepository::parse(slug).unwrap_err().code,
                "use.extension.github_registry_invalid"
            );
        }

        let repository = GitHubRegistryRepository::parse("a3s/packages").unwrap();
        assert_eq!(
            repository
                .clone()
                .with_git_ref("feature/unsafe")
                .unwrap_err()
                .code,
            "use.extension.github_registry_invalid"
        );
        for path in ["/registry", "registry/", "registry//stable", "../registry"] {
            assert_eq!(
                repository
                    .clone()
                    .with_registry_path(path)
                    .unwrap_err()
                    .code,
                "use.extension.github_registry_invalid"
            );
        }
    }
}
