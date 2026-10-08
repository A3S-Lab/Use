//! HTTPS fetch for `github:`, `aqua:`, and `npm:` specs.
//!
//! `pipx:` is not downloaded here. The host installs it with `uv`.
//! A missing publisher digest refuses the payload.

use std::io::{Cursor, Read};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256, Sha512};

use crate::catalog::catalog;
use crate::error::ToolAcquireError;
use crate::source::{CompanionFile, ToolPayload, ToolQuery, ToolSource};
use crate::spec::{parse_tool_version, ToolBackend, ToolSpec};
use crate::store::hex_encode;

const MAX_BYTES: usize = 200 * 1024 * 1024;
const GITHUB_API: &str = "https://api.github.com";
const NPM_REGISTRY: &str = "https://registry.npmjs.org";

/// One GitHub or aqua release file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub download_url: String,
    pub digest_sha256: Option<String>,
}

/// Closed download policy. Empty strings keep the official source.
/// The token is sent only to `api.github.com`.
#[derive(Clone, Default)]
pub struct FetchPolicy {
    pub github_mirror: String,
    pub npm_registry: String,
    pub github_token: String,
}

impl std::fmt::Debug for FetchPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FetchPolicy")
            .field("github_mirror", &self.github_mirror)
            .field("npm_registry", &self.npm_registry)
            .field(
                "github_token",
                &if self.github_token.is_empty() {
                    "empty"
                } else {
                    "set"
                },
            )
            .finish()
    }
}

#[derive(Clone)]
pub struct HttpSource {
    client: reqwest::blocking::Client,
    github_api: String,
    npm_registry: String,
    loopback: bool,
    tokens: &'static [&'static str],
    github_mirror: Option<String>,
    mirror_host: Option<String>,
    github_token: Option<String>,
    npm_hosts: &'static [&'static str],
}

impl std::fmt::Debug for HttpSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HttpSource")
            .field("github_api", &self.github_api)
            .field("npm_registry", &self.npm_registry)
            .field("loopback", &self.loopback)
            .field("github_mirror", &self.github_mirror)
            .field("github_token", &self.github_token.as_ref().map(|_| "set"))
            .finish()
    }
}

impl HttpSource {
    pub fn new() -> Result<Self, ToolAcquireError> {
        Self::with_policy(FetchPolicy::default())
    }

    /// Official hosts stay allowlisted. A mirror or registry outside the closed
    /// list is refused. An empty field keeps the official source.
    pub fn with_policy(policy: FetchPolicy) -> Result<Self, ToolAcquireError> {
        let mirror = normalize_mirror(&policy.github_mirror)?;
        let registry = normalize_npm(&policy.npm_registry)?;
        let token = normalize_token(&policy.github_token)?;
        Self::build(
            GITHUB_API.to_string(),
            registry,
            false,
            host_tokens(),
            mirror,
            token,
        )
    }

    pub fn for_test(
        github_api: impl Into<String>,
        npm_registry: impl Into<String>,
        tokens: &'static [&'static str],
    ) -> Result<Self, ToolAcquireError> {
        Self::build(
            github_api.into(),
            npm_registry.into(),
            true,
            tokens,
            None,
            None,
        )
    }

    pub fn npm_registry(&self) -> &str {
        &self.npm_registry
    }

    pub fn github_mirror(&self) -> Option<&str> {
        self.github_mirror.as_deref()
    }

    fn build(
        github_api: String,
        npm_registry: String,
        loopback: bool,
        tokens: &'static [&'static str],
        github_mirror: Option<String>,
        github_token: Option<String>,
    ) -> Result<Self, ToolAcquireError> {
        let mirror_host = github_mirror.as_deref().and_then(host_of);
        let npm_hosts = npm_hosts_for(&npm_registry);
        let redirect_mirror = mirror_host.clone();
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.error("too many redirects");
                }
                if url_allowed(
                    attempt.url(),
                    loopback,
                    redirect_mirror.as_deref(),
                    npm_hosts,
                ) {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .timeout(Duration::from_secs(60))
            .user_agent("a3s-use-tool-acquire")
            .build()
            .map_err(|_| ToolAcquireError::DownloadRejected)?;
        Ok(Self {
            client,
            github_api,
            npm_registry,
            loopback,
            tokens,
            github_mirror,
            mirror_host,
            github_token,
            npm_hosts,
        })
    }
}

impl ToolSource for HttpSource {
    fn fetch(&self, query: &ToolQuery) -> Result<ToolPayload, ToolAcquireError> {
        match query.spec.backend() {
            ToolBackend::Github { owner, repo } | ToolBackend::Aqua { owner, repo } => {
                self.fetch_github(owner, repo, query.version.as_deref(), &query.executable)
            }
            ToolBackend::Npm { package } => {
                self.fetch_npm(package, query.version.as_deref(), &query.executable)
            }
            ToolBackend::Pipx { .. } => Err(ToolAcquireError::RuntimeMissing("uv".to_string())),
            ToolBackend::Catalog => {
                Err(ToolAcquireError::InvalidSpec(query.spec.raw().to_string()))
            }
        }
    }

    fn latest(&self, spec: &ToolSpec) -> Result<String, ToolAcquireError> {
        match spec.backend() {
            ToolBackend::Github { owner, repo } | ToolBackend::Aqua { owner, repo } => {
                if let Some(executable) = catalog_executable(spec.raw()) {
                    let chosen = self.resolve_github(owner, repo, None, executable)?;
                    Ok(chosen.version)
                } else {
                    let release = self.release(owner, repo, None)?;
                    Ok(release.version)
                }
            }
            ToolBackend::Npm { package } => {
                let release = self.npm_release(package, None)?;
                Ok(release.version)
            }
            ToolBackend::Pipx { .. } | ToolBackend::Catalog => {
                Err(ToolAcquireError::LatestUnavailable)
            }
        }
    }
}

#[derive(Clone)]
struct GithubRelease {
    version: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug)]
struct NpmRelease {
    version: String,
    tarball: String,
    integrity: String,
}

impl HttpSource {
    fn fetch_github(
        &self,
        owner: &str,
        repo: &str,
        version: Option<&str>,
        executable: &str,
    ) -> Result<ToolPayload, ToolAcquireError> {
        let chosen = self.resolve_github(owner, repo, version, executable)?;
        let bytes = self.download(&chosen.asset.download_url)?;
        let digest = match chosen.asset.digest_sha256 {
            Some(digest) => digest,
            None => self.sibling_digest(&chosen.assets, &chosen.asset.name)?,
        };
        if hex_encode(&Sha256::digest(&bytes)) != digest {
            return Err(ToolAcquireError::ChecksumMismatch);
        }
        let extracted = extract_tree(&chosen.asset.name, &bytes, executable)?;
        if extracted.executable.is_empty() {
            return Err(ToolAcquireError::NotExecutable);
        }
        Ok(ToolPayload {
            version: chosen.version,
            sha256: hex_encode(&Sha256::digest(&extracted.executable)),
            bytes: extracted.executable,
            companions: extracted.companions,
        })
    }

    /// Latest is enough when it already has a host archive for `executable`.
    /// A miss walks the release list. A pinned version stays on that tag.
    fn resolve_github(
        &self,
        owner: &str,
        repo: &str,
        version: Option<&str>,
        executable: &str,
    ) -> Result<ChosenRelease, ToolAcquireError> {
        if let Some(version) = version {
            let release = self.release(owner, repo, Some(version))?;
            if version != release.version {
                return Err(ToolAcquireError::VersionConflict {
                    version: release.version,
                });
            }
            return chosen_release(std::slice::from_ref(&release), self.tokens, executable);
        }
        let latest = self.release(owner, repo, None)?;
        match chosen_release(std::slice::from_ref(&latest), self.tokens, executable) {
            Ok(chosen) => Ok(chosen),
            Err(ToolAcquireError::AssetMissing) => {
                let listed = self.list_releases(owner, repo)?;
                chosen_release(&listed, self.tokens, executable)
            }
            Err(error) => Err(error),
        }
    }

    fn release(
        &self,
        owner: &str,
        repo: &str,
        version: Option<&str>,
    ) -> Result<GithubRelease, ToolAcquireError> {
        let url = match version {
            Some(version) => format!(
                "{}/repos/{owner}/{repo}/releases/tags/v{version}",
                self.github_api.trim_end_matches('/')
            ),
            None => format!(
                "{}/repos/{owner}/{repo}/releases/latest",
                self.github_api.trim_end_matches('/')
            ),
        };
        let value = self.get_json(&url).or_else(|error| {
            let Some(version) = version else {
                return Err(error);
            };
            let raw = format!(
                "{}/repos/{owner}/{repo}/releases/tags/{version}",
                self.github_api.trim_end_matches('/')
            );
            self.get_json(&raw)
        })?;
        self.parse_release_object(&value)
    }

    fn list_releases(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<GithubRelease>, ToolAcquireError> {
        let url = format!(
            "{}/repos/{owner}/{repo}/releases?per_page=30",
            self.github_api.trim_end_matches('/')
        );
        let value = self.get_json(&url)?;
        let items = value.as_array().ok_or(ToolAcquireError::AssetMissing)?;
        let mut releases = Vec::new();
        for item in items {
            if let Ok(release) = self.parse_release_object(item) {
                releases.push(release);
            }
        }
        if releases.is_empty() {
            return Err(ToolAcquireError::AssetMissing);
        }
        Ok(releases)
    }

    fn parse_release_object(
        &self,
        value: &serde_json::Value,
    ) -> Result<GithubRelease, ToolAcquireError> {
        let tag = value
            .get("tag_name")
            .and_then(|item| item.as_str())
            .ok_or(ToolAcquireError::LatestUnavailable)?;
        let version = tag.strip_prefix('v').unwrap_or(tag).to_string();
        parse_tool_version(&version)?;
        let assets = value
            .get("assets")
            .and_then(|item| item.as_array())
            .ok_or(ToolAcquireError::AssetMissing)?;
        let mut parsed = Vec::new();
        for asset in assets {
            let Some(name) = asset.get("name").and_then(|item| item.as_str()) else {
                continue;
            };
            let Some(download_url) = asset
                .get("browser_download_url")
                .and_then(|item| item.as_str())
            else {
                continue;
            };
            let download_url = self.prepare_download(download_url)?;
            parsed.push(ReleaseAsset {
                name: name.to_string(),
                download_url,
                digest_sha256: asset
                    .get("digest")
                    .and_then(|item| item.as_str())
                    .and_then(parse_sha256_digest),
            });
        }
        Ok(GithubRelease {
            version,
            assets: parsed,
        })
    }

    fn sibling_digest(
        &self,
        assets: &[ReleaseAsset],
        asset_name: &str,
    ) -> Result<String, ToolAcquireError> {
        let wanted = [
            format!("{asset_name}.sha256"),
            format!("{asset_name}.sha256sum"),
        ];
        let Some(sibling) = assets
            .iter()
            .find(|asset| wanted.iter().any(|name| name == &asset.name))
        else {
            return Err(ToolAcquireError::ChecksumRequired);
        };
        let bytes = self.download(&sibling.download_url)?;
        let text = String::from_utf8(bytes).map_err(|_| ToolAcquireError::ChecksumMismatch)?;
        let token = text
            .split_whitespace()
            .next()
            .ok_or(ToolAcquireError::ChecksumRequired)?;
        parse_sha256_digest(token).ok_or(ToolAcquireError::ChecksumRequired)
    }

    fn fetch_npm(
        &self,
        package: &str,
        version: Option<&str>,
        executable: &str,
    ) -> Result<ToolPayload, ToolAcquireError> {
        let release = self.npm_release(package, version)?;
        if let Some(wanted) = version {
            if wanted != release.version {
                return Err(ToolAcquireError::VersionConflict {
                    version: release.version,
                });
            }
        }
        let bytes = self.download(&release.tarball)?;
        verify_sha512(&release.integrity, &bytes)?;
        let bin = npm_bin_path(&bytes, executable)?;
        let mut executable_bytes = extract_named(&bytes, &bin)?;
        if executable_bytes.is_empty() {
            return Err(ToolAcquireError::NotExecutable);
        }
        if (bin.ends_with(".js") || bin.ends_with(".mjs")) && !executable_bytes.starts_with(b"#!") {
            let mut wrapped = b"#!/usr/bin/env node\n".to_vec();
            wrapped.extend_from_slice(&executable_bytes);
            executable_bytes = wrapped;
        }
        Ok(ToolPayload::new(
            release.version,
            hex_encode(&Sha256::digest(&executable_bytes)),
            executable_bytes,
        ))
    }

    fn npm_release(
        &self,
        package: &str,
        version: Option<&str>,
    ) -> Result<NpmRelease, ToolAcquireError> {
        let url = format!(
            "{}/{}",
            self.npm_registry.trim_end_matches('/'),
            npm_path(package)
        );
        let value = self.get_json(&url)?;
        parse_npm_metadata(&value, version)
    }

    fn get_json(&self, url: &str) -> Result<serde_json::Value, ToolAcquireError> {
        if !self.allows(url) {
            return Err(ToolAcquireError::DownloadRejected);
        }
        let mut request = self
            .client
            .get(url)
            .header("accept", "application/vnd.github+json");
        if let Some(header) = self.token_header(url) {
            request = request.header("authorization", header);
        }
        let response = request
            .send()
            .map_err(|_| ToolAcquireError::DownloadRejected)?;
        if !response.status().is_success() {
            return Err(ToolAcquireError::DownloadRejected);
        }
        response
            .json()
            .map_err(|_| ToolAcquireError::DownloadRejected)
    }

    fn allows(&self, url: &str) -> bool {
        url_allowed_str(
            url,
            self.loopback,
            self.mirror_host.as_deref(),
            self.npm_hosts,
        )
    }

    /// Bearer token for `api.github.com` only. Asset and mirror URLs stay anonymous.
    fn token_header(&self, url: &str) -> Option<String> {
        let token = self.github_token.as_ref()?;
        let parsed = reqwest::Url::parse(url).ok()?;
        if parsed.host_str() == Some("api.github.com") {
            Some(format!("Bearer {token}"))
        } else {
            None
        }
    }

    /// Prefix an official asset URL when a mirror is set. The API URL stays direct.
    fn prepare_download(&self, original: &str) -> Result<String, ToolAcquireError> {
        if self.loopback {
            if self.allows(original) {
                return Ok(original.to_string());
            }
            return Err(ToolAcquireError::DownloadRejected);
        }
        if !official_asset_url(original) {
            return Err(ToolAcquireError::DownloadRejected);
        }
        match &self.github_mirror {
            Some(mirror) => Ok(format!("{mirror}/{original}")),
            None => Ok(original.to_string()),
        }
    }

    fn download(&self, url: &str) -> Result<Vec<u8>, ToolAcquireError> {
        if !self.allows(url) {
            return Err(ToolAcquireError::DownloadRejected);
        }
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|_| ToolAcquireError::DownloadRejected)?;
        if !response.status().is_success() {
            return Err(ToolAcquireError::DownloadRejected);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(ToolAcquireError::DownloadRejected);
        }
        let bytes = response
            .bytes()
            .map_err(|_| ToolAcquireError::DownloadRejected)?;
        if bytes.len() > MAX_BYTES {
            return Err(ToolAcquireError::DownloadRejected);
        }
        Ok(bytes.to_vec())
    }
}

/// Host aliases, most specific first. `select_asset` stops at the first alias
/// that matches, so a later generic alias cannot hide a triple.
pub fn host_tokens() -> &'static [&'static str] {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        &[
            "aarch64-apple-darwin",
            "darwin-aarch64",
            "macos-arm64",
            "macos_arm64",
            "darwin-arm64",
            "osx-arm64",
            "mac_arm64",
            "apple-darwin",
        ]
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        &[
            "x86_64-apple-darwin",
            "darwin-x64",
            "darwin-amd64",
            "macos-x64",
            "macos-amd64",
            "macos_amd64",
            "osx-x64",
            "mac_x64",
            "apple-darwin",
        ]
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        &[
            "aarch64-unknown-linux-gnu",
            "linux-arm64",
            "linux_arm64",
            "linux-aarch64",
            "aarch64-unknown-linux-musl",
        ]
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        &[
            "x86_64-unknown-linux-gnu",
            "linux-x64",
            "linux-amd64",
            "linux_amd64",
            "linux_x64",
            "x86_64-unknown-linux-musl",
        ]
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        &["aarch64-pc-windows-msvc", "windows-arm64", "windows_arm64"]
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        &[
            "x86_64-pc-windows-msvc",
            "windows-x64",
            "windows-amd64",
            "windows_amd64",
        ]
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    {
        &["unknown-target"]
    }
}

/// Pick one archive for this host.
///
/// Token order is the preference. Package formats this crate cannot unpack are
/// ignored. A profile, baseline, musl, or android build loses when a plain
/// archive matched the same token. Two plain archives for that token stay an error.
pub fn select_asset<'a>(
    assets: &'a [ReleaseAsset],
    tokens: &[&str],
) -> Result<&'a ReleaseAsset, ToolAcquireError> {
    let matched = host_matches(assets, tokens);
    match matched.as_slice() {
        [] => Err(ToolAcquireError::AssetMissing),
        [one] => Ok(*one),
        _ => Err(ToolAcquireError::AssetAmbiguous),
    }
}

/// One chosen release asset. `assets` is the whole release, so a sidecar
/// checksum can still be found after the archive itself is chosen.
struct ReleaseChoice<'a> {
    version: &'a str,
    asset: &'a ReleaseAsset,
    assets: &'a [ReleaseAsset],
}

struct ChosenRelease {
    version: String,
    asset: ReleaseAsset,
    assets: Vec<ReleaseAsset>,
}

fn chosen_release(
    releases: &[GithubRelease],
    tokens: &[&str],
    executable: &str,
) -> Result<ChosenRelease, ToolAcquireError> {
    let choice = choose_release(releases, tokens, executable)?;
    Ok(ChosenRelease {
        version: choice.version.to_string(),
        asset: choice.asset.clone(),
        assets: choice.assets.to_vec(),
    })
}

/// Pick the release whose host archive is the product for `executable`.
///
/// Releases with no host archive are skipped. A name that contains `-` must
/// match an asset whose basename starts with that executable, so a host token
/// on a libkrun zip or a containerd shim is not a successful install. A name
/// without `-` keeps the first unique host match, including archives that
/// publish a different file name.
fn choose_release<'a>(
    releases: &'a [GithubRelease],
    tokens: &[&str],
    executable: &str,
) -> Result<ReleaseChoice<'a>, ToolAcquireError> {
    for release in releases {
        let matched = host_matches(&release.assets, tokens);
        if matched.is_empty() {
            continue;
        }
        if executable.contains('-') {
            let named: Vec<&ReleaseAsset> = matched
                .into_iter()
                .filter(|asset| asset_names_product(&asset.name, executable))
                .collect();
            match named.as_slice() {
                [one] => {
                    return Ok(ReleaseChoice {
                        version: &release.version,
                        asset: *one,
                        assets: &release.assets,
                    });
                }
                [] => continue,
                _ => return Err(ToolAcquireError::AssetAmbiguous),
            }
        }
        match select_asset(&release.assets, tokens) {
            Ok(asset) => {
                return Ok(ReleaseChoice {
                    version: &release.version,
                    asset,
                    assets: &release.assets,
                });
            }
            Err(ToolAcquireError::AssetMissing) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(ToolAcquireError::AssetMissing)
}

fn host_matches<'a>(assets: &'a [ReleaseAsset], tokens: &[&str]) -> Vec<&'a ReleaseAsset> {
    for token in tokens {
        let matched: Vec<&ReleaseAsset> = assets
            .iter()
            .filter(|asset| {
                !is_checksum_name(&asset.name)
                    && !is_unsupported_package(&asset.name)
                    && contains_token(&asset.name, token)
            })
            .collect();
        if matched.is_empty() {
            continue;
        }
        let plain: Vec<&ReleaseAsset> = matched
            .iter()
            .copied()
            .filter(|asset| !is_variant_build(&asset.name))
            .collect();
        return if plain.is_empty() { matched } else { plain };
    }
    Vec::new()
}

/// Basename starts with the executable, so `a3s-box-v3.3.0-…` matches and
/// `a3s-libkrun-sys-…` or `containerd-shim-a3s-box-…` does not.
fn asset_names_product(name: &str, executable: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let base = base.to_ascii_lowercase();
    let executable = executable.to_ascii_lowercase();
    base == executable
        || base == format!("{executable}.exe")
        || base.starts_with(&format!("{executable}-"))
        || base.starts_with(&format!("{executable}_"))
        || base.starts_with(&format!("{executable}."))
}

fn catalog_executable(spec: &str) -> Option<&'static str> {
    catalog()
        .iter()
        .find(|entry| entry.spec == spec)
        .map(|entry| entry.executable)
}

fn contains_token(name: &str, token: &str) -> bool {
    name.to_ascii_lowercase()
        .contains(&token.to_ascii_lowercase())
}

fn is_checksum_name(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.ends_with(".sha256")
        || lowered.ends_with(".sha256sum")
        || lowered.ends_with(".sig")
        || lowered.ends_with(".pem")
        || lowered.ends_with(".asc")
}

fn is_unsupported_package(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    [".deb", ".rpm", ".pkg", ".msi", ".dmg", ".appimage"]
        .iter()
        .any(|extension| lowered.ends_with(extension))
}

fn is_variant_build(name: &str) -> bool {
    name.to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|segment| matches!(segment, "profile" | "baseline" | "musl" | "android"))
}

pub fn parse_sha256_digest(value: &str) -> Option<String> {
    let value = value.trim().strip_prefix("sha256:").unwrap_or(value.trim());
    let value = value.to_ascii_lowercase();
    if value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit()) {
        Some(value)
    } else {
        None
    }
}

fn npm_path(package: &str) -> String {
    package.replace('/', "%2f")
}

fn parse_npm_metadata(
    value: &serde_json::Value,
    version: Option<&str>,
) -> Result<NpmRelease, ToolAcquireError> {
    let version = match version {
        Some(version) => version.to_string(),
        None => value
            .get("dist-tags")
            .and_then(|item| item.get("latest"))
            .and_then(|item| item.as_str())
            .ok_or(ToolAcquireError::LatestUnavailable)?
            .to_string(),
    };
    parse_tool_version(&version)?;
    let dist = value
        .get("versions")
        .and_then(|item| item.get(&version))
        .and_then(|item| item.get("dist"))
        .ok_or(ToolAcquireError::AssetMissing)?;
    let tarball = dist
        .get("tarball")
        .and_then(|item| item.as_str())
        .ok_or(ToolAcquireError::AssetMissing)?
        .to_string();
    let integrity = dist
        .get("integrity")
        .and_then(|item| item.as_str())
        .ok_or(ToolAcquireError::ChecksumRequired)?
        .to_string();
    Ok(NpmRelease {
        version,
        tarball,
        integrity,
    })
}

fn verify_sha512(integrity: &str, bytes: &[u8]) -> Result<(), ToolAcquireError> {
    let encoded = integrity
        .strip_prefix("sha512-")
        .ok_or(ToolAcquireError::ChecksumRequired)?;
    let expected = STANDARD
        .decode(encoded)
        .map_err(|_| ToolAcquireError::ChecksumMismatch)?;
    let actual = Sha512::digest(bytes);
    if actual.as_slice() == expected.as_slice() {
        Ok(())
    } else {
        Err(ToolAcquireError::ChecksumMismatch)
    }
}

fn npm_bin_path(archive: &[u8], executable: &str) -> Result<String, ToolAcquireError> {
    let manifest = extract_named(archive, "package/package.json")?;
    let value: serde_json::Value =
        serde_json::from_slice(&manifest).map_err(|_| ToolAcquireError::PayloadInvalid {
            path: std::path::PathBuf::from("package/package.json"),
        })?;
    let bin = value.get("bin").ok_or(ToolAcquireError::AssetMissing)?;
    let relative = if let Some(path) = bin.as_str() {
        path.to_string()
    } else if let Some(path) = bin.get(executable).and_then(|item| item.as_str()) {
        path.to_string()
    } else if let Some(object) = bin.as_object() {
        if object.len() == 1 {
            object
                .values()
                .next()
                .and_then(|item| item.as_str())
                .ok_or(ToolAcquireError::AssetMissing)?
                .to_string()
        } else {
            return Err(ToolAcquireError::AssetAmbiguous);
        }
    } else {
        return Err(ToolAcquireError::AssetMissing);
    };
    if relative.is_empty() || relative.contains("..") || relative.starts_with('/') {
        return Err(ToolAcquireError::PayloadInvalid {
            path: std::path::PathBuf::from(relative),
        });
    }
    Ok(format!("package/{relative}"))
}

struct ArchiveFile {
    path: String,
    bytes: Vec<u8>,
    executable: bool,
}

#[derive(Debug)]
struct ExtractedTool {
    executable: Vec<u8>,
    companions: Vec<CompanionFile>,
}

fn extract_tree(
    asset_name: &str,
    bytes: &[u8],
    executable: &str,
) -> Result<ExtractedTool, ToolAcquireError> {
    let lowered = asset_name.to_ascii_lowercase();
    if lowered.ends_with(".tar.gz") || lowered.ends_with(".tgz") {
        return finish_extract(read_tar(bytes, executable)?, executable);
    }
    if lowered.ends_with(".zip") {
        return finish_extract(read_zip(bytes, executable)?, executable);
    }
    let file = lowered.rsplit(['/', '\\']).next().unwrap_or(&lowered);
    let wanted = executable.to_ascii_lowercase();
    if file == wanted || file == format!("{wanted}.exe") {
        return Ok(ExtractedTool {
            executable: bytes.to_vec(),
            companions: Vec::new(),
        });
    }
    Err(ToolAcquireError::AssetMissing)
}

fn read_tar(bytes: &[u8], executable: &str) -> Result<Vec<ArchiveFile>, ToolAcquireError> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|_| ToolAcquireError::PayloadInvalid {
            path: std::path::PathBuf::from(executable),
        })?;
    let mut files = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(|_| ToolAcquireError::PayloadInvalid {
            path: std::path::PathBuf::from(executable),
        })?;
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() || !kind.is_file() {
            continue;
        }
        let path = entry.path().map_err(|_| ToolAcquireError::PayloadInvalid {
            path: std::path::PathBuf::from(executable),
        })?;
        if path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(ToolAcquireError::PayloadInvalid { path: path.into() });
        }
        let mode = entry.header().mode().unwrap_or(0o644);
        let full = path.to_string_lossy().into_owned();
        let mut buffer = Vec::new();
        entry
            .read_to_end(&mut buffer)
            .map_err(|_| ToolAcquireError::PayloadInvalid {
                path: std::path::PathBuf::from(executable),
            })?;
        files.push(ArchiveFile {
            path: full,
            bytes: buffer,
            executable: mode & 0o111 != 0,
        });
    }
    Ok(files)
}

/// Keep sibling files. One common top directory is removed so `lib/` sits
/// beside the executable, which is where `@executable_path/lib` looks.
fn finish_extract(
    mut files: Vec<ArchiveFile>,
    executable: &str,
) -> Result<ExtractedTool, ToolAcquireError> {
    strip_common_directory(&mut files);
    let mut named_index = None;
    for (index, file) in files.iter().enumerate() {
        let name = file
            .path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(file.path.as_str());
        if file_matches_executable(name, &file.path, executable) {
            if named_index.is_some() {
                return Err(ToolAcquireError::AssetAmbiguous);
            }
            named_index = Some(index);
        }
    }
    let index = if let Some(index) = named_index {
        index
    } else if files.len() == 1 {
        // One regular file with a different name is still the executable.
        // `agy` publishes `antigravity` and nothing else.
        0
    } else {
        return Err(ToolAcquireError::AssetMissing);
    };
    let executable_bytes = std::mem::take(&mut files[index].bytes);
    let mut companions = Vec::new();
    for (file_index, file) in files.into_iter().enumerate() {
        if file_index == index {
            continue;
        }
        companions.push(CompanionFile {
            relative_path: normalize_relative(&file.path)?,
            bytes: file.bytes,
            executable: file.executable,
        });
    }
    Ok(ExtractedTool {
        executable: executable_bytes,
        companions,
    })
}

fn strip_common_directory(files: &mut [ArchiveFile]) {
    let Some(first) = files.first() else {
        return;
    };
    let first_parts = path_parts(&first.path);
    let Some(prefix) = first_parts.first().copied() else {
        return;
    };
    if first_parts.len() < 2 {
        return;
    }
    let shared = files.iter().all(|file| {
        let parts = path_parts(&file.path);
        parts.first().copied() == Some(prefix) && parts.len() >= 2
    });
    if !shared {
        return;
    }
    let prefix_owned = prefix.to_string();
    for file in files {
        let parts = path_parts(&file.path);
        if parts.first().map(|part| *part == prefix_owned) != Some(true) {
            continue;
        }
        file.path = parts[1..].join("/");
    }
}

fn path_parts(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

fn normalize_relative(path: &str) -> Result<String, ToolAcquireError> {
    let parts = path_parts(path);
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| *part == "." || *part == ".." || part.contains('\0'))
    {
        return Err(ToolAcquireError::PayloadInvalid {
            path: std::path::PathBuf::from(path),
        });
    }
    Ok(parts.join("/"))
}

fn extract_named(bytes: &[u8], executable: &str) -> Result<Vec<u8>, ToolAcquireError> {
    let files = read_tar(bytes, executable)?;
    let mut found = None;
    let mut lone = None;
    let mut lone_count = 0_usize;
    for file in files {
        let name = file
            .path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(file.path.as_str())
            .to_string();
        if file_matches_executable(&name, &file.path, executable) {
            if found.is_some() {
                return Err(ToolAcquireError::AssetAmbiguous);
            }
            found = Some(file.bytes);
            continue;
        }
        lone_count += 1;
        if lone_count == 1 {
            lone = Some(file.bytes);
        } else {
            lone = None;
        }
    }
    if let Some(found) = found {
        return Ok(found);
    }
    if lone_count == 1 {
        return lone.ok_or(ToolAcquireError::AssetMissing);
    }
    Err(ToolAcquireError::AssetMissing)
}

fn file_matches_executable(name: &str, full: &str, executable: &str) -> bool {
    name == executable
        || full == executable
        || full.ends_with(&format!("/{executable}"))
        || full.ends_with(&format!("\\{executable}"))
        || name.eq_ignore_ascii_case(&format!("{executable}.exe"))
}

fn read_zip(bytes: &[u8], executable: &str) -> Result<Vec<ArchiveFile>, ToolAcquireError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| ToolAcquireError::PayloadInvalid {
            path: std::path::PathBuf::from(executable),
        })?;
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|_| ToolAcquireError::PayloadInvalid {
                path: std::path::PathBuf::from(executable),
            })?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        if name.contains("..") || name.starts_with('/') || name.starts_with('\\') {
            return Err(ToolAcquireError::PayloadInvalid {
                path: std::path::PathBuf::from(name),
            });
        }
        let mode = file.unix_mode().unwrap_or(0o644);
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|_| ToolAcquireError::PayloadInvalid {
                path: std::path::PathBuf::from(executable),
            })?;
        files.push(ArchiveFile {
            path: name,
            bytes: buffer,
            executable: mode & 0o111 != 0,
        });
    }
    Ok(files)
}

fn normalize_mirror(value: &str) -> Result<Option<String>, ToolAcquireError> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed == "https://ghfast.top" || trimmed == "https://ghproxy.net" {
        Ok(Some(trimmed.to_string()))
    } else {
        Err(ToolAcquireError::DownloadRejected)
    }
}

fn normalize_npm(value: &str) -> Result<String, ToolAcquireError> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed == NPM_REGISTRY {
        return Ok(NPM_REGISTRY.to_string());
    }
    if trimmed == "https://registry.npmmirror.com" {
        Ok(trimmed.to_string())
    } else {
        Err(ToolAcquireError::DownloadRejected)
    }
}

fn normalize_token(value: &str) -> Result<Option<String>, ToolAcquireError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > 256
        || !trimmed.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
    {
        return Err(ToolAcquireError::DownloadRejected);
    }
    Ok(Some(trimmed.to_string()))
}

fn npm_hosts_for(registry: &str) -> &'static [&'static str] {
    if registry.trim_end_matches('/') == "https://registry.npmmirror.com" {
        &["registry.npmmirror.com", "cdn.npmmirror.com"]
    } else {
        &["registry.npmjs.org"]
    }
}

fn host_of(origin: &str) -> Option<String> {
    reqwest::Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
}

fn official_asset_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "objects.githubusercontent.com"
                    | "release-assets.githubusercontent.com"
            )
        )
}

fn url_allowed(
    url: &reqwest::Url,
    loopback: bool,
    mirror_host: Option<&str>,
    npm_hosts: &[&str],
) -> bool {
    url_allowed_str(url.as_str(), loopback, mirror_host, npm_hosts)
}

fn url_allowed_str(
    value: &str,
    loopback: bool,
    mirror_host: Option<&str>,
    npm_hosts: &[&str],
) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if loopback {
        return matches!(url.scheme(), "http" | "https")
            && matches!(host, "127.0.0.1" | "localhost");
    }
    if url.scheme() != "https" {
        return false;
    }
    if matches!(
        host,
        "api.github.com"
            | "github.com"
            | "objects.githubusercontent.com"
            | "release-assets.githubusercontent.com"
    ) {
        return true;
    }
    if npm_hosts.iter().any(|item| *item == host) {
        return true;
    }
    if mirror_host == Some(host) {
        return mirrored_path_is_official(url.path());
    }
    false
}

fn mirrored_path_is_official(path: &str) -> bool {
    let Some(embedded) = path.strip_prefix('/') else {
        return false;
    };
    official_asset_url(embedded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            download_url: format!("https://github.com/{name}"),
            digest_sha256: None,
        }
    }

    const MAC_ARM_TOKENS: &[&str] = &[
        "aarch64-apple-darwin",
        "darwin-aarch64",
        "macos-arm64",
        "macos_arm64",
        "darwin-arm64",
        "osx-arm64",
        "mac_arm64",
        "apple-darwin",
    ];

    #[test]
    fn select_asset_keeps_the_first_matching_token() {
        let assets = vec![
            asset("uv-x86_64-apple-darwin.tar.gz"),
            asset("uv-aarch64-apple-darwin.tar.gz"),
            asset("uv-aarch64-apple-darwin.tar.gz.sha256"),
        ];
        let selected = select_asset(&assets, MAC_ARM_TOKENS).unwrap();
        assert_eq!(selected.name, "uv-aarch64-apple-darwin.tar.gz");
    }

    #[test]
    fn select_asset_rejects_two_equal_matches() {
        let assets = vec![
            asset("tool-aarch64-apple-darwin.tar.gz"),
            asset("tool-aarch64-apple-darwin.zip"),
        ];
        let error = select_asset(&assets, &["aarch64-apple-darwin"]).unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.asset_ambiguous");
    }

    #[test]
    fn select_asset_rejects_two_variant_builds() {
        let assets = vec![
            asset("bun-darwin-aarch64-profile.zip"),
            asset("bun-darwin-aarch64-baseline.zip"),
        ];
        let error = select_asset(&assets, MAC_ARM_TOKENS).unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.asset_ambiguous");
    }

    #[test]
    fn select_asset_picks_catalog_archives_for_macos_arm() {
        let cases = [
            (
                "uv-aarch64-apple-darwin.tar.gz",
                vec![
                    "source.tar.gz",
                    "uv-installer.sh",
                    "uv-aarch64-apple-darwin.tar.gz",
                    "uv-aarch64-apple-darwin.tar.gz.sha256",
                    "uv-x86_64-apple-darwin.tar.gz",
                ],
            ),
            (
                "bun-darwin-aarch64.zip",
                vec![
                    "bun-darwin-aarch64-profile.zip",
                    "bun-darwin-aarch64.zip",
                    "bun-darwin-x64-baseline.zip",
                    "bun-darwin-x64.zip",
                ],
            ),
            (
                "fd-v10.5.0-aarch64-apple-darwin.tar.gz",
                vec![
                    "fd_10.5.0_arm64.deb",
                    "fd-musl_10.5.0_arm64.deb",
                    "fd-v10.5.0-aarch64-apple-darwin.tar.gz",
                    "fd-v10.5.0-x86_64-apple-darwin.tar.gz",
                ],
            ),
            (
                "rtk-aarch64-apple-darwin.tar.gz",
                vec![
                    "checksums.txt",
                    "rtk_amd64.deb",
                    "rtk-aarch64-apple-darwin.tar.gz",
                    "rtk-x86_64-apple-darwin.tar.gz",
                ],
            ),
            (
                "lark-cli-1.0.97-darwin-arm64.tar.gz",
                vec![
                    "checksums.txt",
                    "lark-cli-1.0.97-darwin-amd64.tar.gz",
                    "lark-cli-1.0.97-darwin-arm64.tar.gz",
                    "lark-cli-1.0.97-linux-arm64.tar.gz",
                ],
            ),
            (
                "gh_2.102.0_macOS_arm64.zip",
                vec![
                    "gh_2.102.0_checksums.txt",
                    "gh_2.102.0_macOS_amd64.zip",
                    "gh_2.102.0_macOS_arm64.zip",
                    "gh_2.102.0_macOS_universal.pkg",
                ],
            ),
            (
                "agy_cli_mac_arm64.tar.gz",
                vec![
                    "agy_cli_linux_arm64.tar.gz",
                    "agy_cli_mac_arm64.tar.gz",
                    "agy_cli_mac_x64.tar.gz",
                ],
            ),
        ];
        for (expected, names) in cases {
            let assets: Vec<_> = names.into_iter().map(asset).collect();
            let selected = select_asset(&assets, MAC_ARM_TOKENS).unwrap();
            assert_eq!(selected.name, expected);
        }
    }

    #[test]
    fn select_asset_prefers_a_plain_linux_archive_and_keeps_musl_when_alone() {
        let arm = &[
            "aarch64-unknown-linux-gnu",
            "linux-arm64",
            "linux_arm64",
            "linux-aarch64",
            "aarch64-unknown-linux-musl",
        ];
        let bun = vec![
            asset("bun-linux-aarch64-android.zip"),
            asset("bun-linux-aarch64-musl.zip"),
            asset("bun-linux-aarch64-profile.zip"),
            asset("bun-linux-aarch64.zip"),
        ];
        assert_eq!(
            select_asset(&bun, arm).unwrap().name,
            "bun-linux-aarch64.zip"
        );
        let gh = vec![
            asset("gh_2.102.0_linux_arm64.deb"),
            asset("gh_2.102.0_linux_arm64.rpm"),
            asset("gh_2.102.0_linux_arm64.tar.gz"),
        ];
        assert_eq!(
            select_asset(&gh, arm).unwrap().name,
            "gh_2.102.0_linux_arm64.tar.gz"
        );
        let uv = vec![
            asset("uv-aarch64-unknown-linux-gnu.tar.gz"),
            asset("uv-aarch64-unknown-linux-musl.tar.gz"),
        ];
        assert_eq!(
            select_asset(&uv, arm).unwrap().name,
            "uv-aarch64-unknown-linux-gnu.tar.gz"
        );
        let x64 = &[
            "x86_64-unknown-linux-gnu",
            "linux-x64",
            "linux-amd64",
            "linux_amd64",
            "linux_x64",
            "x86_64-unknown-linux-musl",
        ];
        let ripgrep = vec![asset("ripgrep-15.2.0-x86_64-unknown-linux-musl.tar.gz")];
        assert_eq!(
            select_asset(&ripgrep, x64).unwrap().name,
            "ripgrep-15.2.0-x86_64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn select_asset_picks_windows_zip_over_msi_and_profile() {
        let tokens = &[
            "x86_64-pc-windows-msvc",
            "windows-x64",
            "windows-amd64",
            "windows_amd64",
        ];
        let lark = vec![
            asset("lark-cli-1.0.97-windows-amd64.zip"),
            asset("lark-cli-1.0.97-windows-arm64.zip"),
        ];
        assert_eq!(
            select_asset(&lark, tokens).unwrap().name,
            "lark-cli-1.0.97-windows-amd64.zip"
        );
        let gh = vec![
            asset("gh_2.102.0_windows_amd64.msi"),
            asset("gh_2.102.0_windows_amd64.zip"),
        ];
        assert_eq!(
            select_asset(&gh, tokens).unwrap().name,
            "gh_2.102.0_windows_amd64.zip"
        );
        let bun = vec![
            asset("bun-windows-x64-baseline-profile.zip"),
            asset("bun-windows-x64-profile.zip"),
            asset("bun-windows-x64.zip"),
        ];
        assert_eq!(
            select_asset(&bun, tokens).unwrap().name,
            "bun-windows-x64.zip"
        );
        let fd = vec![
            asset("fd-v10.5.0-x86_64-pc-windows-gnu.zip"),
            asset("fd-v10.5.0-x86_64-pc-windows-msvc.zip"),
        ];
        assert_eq!(
            select_asset(&fd, tokens).unwrap().name,
            "fd-v10.5.0-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn host_tokens_put_the_generic_alias_last_on_macos() {
        let tokens = host_tokens();
        if let Some(index) = tokens.iter().position(|token| *token == "apple-darwin") {
            assert_eq!(index, tokens.len() - 1);
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn host_tokens_match_the_macos_arm_aliases() {
        assert_eq!(host_tokens(), MAC_ARM_TOKENS);
    }

    #[test]
    #[ignore = "downloads current GitHub releases"]
    fn live_github_catalog_tools_install_and_run() {
        let source = HttpSource::new().expect("http");
        let root = tempfile::tempdir().expect("temp");
        let store = crate::store::ToolStore::open(root.path()).expect("store");
        let tools = [
            ("fd", "github:sharkdp/fd", "fd"),
            ("rg", "github:BurntSushi/ripgrep", "rg"),
            ("rtk", "github:rtk-ai/rtk", "rtk"),
            ("lark-cli", "github:larksuite/cli", "lark-cli"),
            ("gh", "github:cli/cli", "gh"),
            ("bun", "github:oven-sh/bun", "bun"),
            ("uv", "github:astral-sh/uv", "uv"),
            ("agy", "aqua:google-antigravity/antigravity-cli", "agy"),
        ];
        for (name, spec, executable) in tools {
            let started = std::time::Instant::now();
            let receipt = store
                .acquire_with(
                    crate::acquire::AcquireRequest {
                        name: name.to_string(),
                        spec: spec.to_string(),
                        version: None,
                        executable_name: executable.to_string(),
                    },
                    &source,
                )
                .unwrap_or_else(|error| panic!("{name} install failed: {error}"));
            let shim = store.which(name).expect("which").expect(name);
            let output = std::process::Command::new(&shim)
                .arg("--version")
                .output()
                .unwrap_or_else(|error| panic!("{name} did not start: {error}"));
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.status.success(), "{name} --version failed: {text}");
            assert!(
                text.to_ascii_lowercase().contains(executable)
                    || text.contains(&receipt.version)
                    || text.chars().any(|character| character.is_ascii_digit()),
                "{name} version text was empty: {text}"
            );
            eprintln!(
                "{name} {} in {:.1}s",
                receipt.version,
                started.elapsed().as_secs_f64()
            );
        }
    }

    #[test]
    fn npm_metadata_requires_integrity() {
        let value = serde_json::json!({
            "dist-tags": { "latest": "1.2.3" },
            "versions": {
                "1.2.3": { "dist": { "tarball": "https://registry.npmjs.org/pkg/-/pkg-1.2.3.tgz" } }
            }
        });
        let error = parse_npm_metadata(&value, None).unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.checksum_required");
    }

    const TEST_TOKENS: &[&str] = &["test-host"];

    #[test]
    fn loopback_github_returns_the_verified_executable() {
        let bytes = b"#!/bin/sh\necho uv\n";
        let archive = tar_gz("uv", bytes);
        let digest = hex_encode(&Sha256::digest(&archive));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let archive_for_thread = archive.clone();
        let digest_for_thread = digest.clone();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 4096];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let request = String::from_utf8_lossy(&buffer);
                let body = if request.contains("/releases/latest") {
                    format!(
                        "{{\"tag_name\":\"v1.2.3\",\"assets\":[{{\"name\":\"uv-test-host.tar.gz\",\"browser_download_url\":\"http://127.0.0.1:{port}/uv-test-host.tar.gz\",\"digest\":\"sha256:{digest_for_thread}\"}}]}}"
                    )
                    .into_bytes()
                } else if request.contains("/uv-test-host.tar.gz") {
                    archive_for_thread.clone()
                } else {
                    Vec::new()
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let source = HttpSource::for_test(
            format!("http://127.0.0.1:{port}"),
            format!("http://127.0.0.1:{port}"),
            TEST_TOKENS,
        )
        .unwrap();
        let spec = crate::parse_tool_spec("github:astral-sh/uv").unwrap();
        let payload = source
            .fetch(&ToolQuery {
                spec,
                version: None,
                executable: "uv".to_string(),
            })
            .unwrap();
        assert_eq!(payload.version, "1.2.3");
        assert_eq!(payload.bytes, bytes);
        assert_eq!(payload.sha256, hex_encode(&Sha256::digest(bytes)));
    }

    #[test]
    fn loopback_archive_installs_runs_and_removes() {
        let script = b"#!/bin/sh\necho probe 1.2.3\n";
        let archive = tar_gz("renamed-only", script);
        let digest = hex_encode(&Sha256::digest(&archive));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let archive_for_thread = archive.clone();
        let digest_for_thread = digest.clone();
        std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 4096];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let request = String::from_utf8_lossy(&buffer);
                let body = if request.contains("/releases/latest") {
                    format!(
                        "{{\"tag_name\":\"v1.2.3\",\"assets\":[{{\"name\":\"probe-test-host.tar.gz\",\"browser_download_url\":\"http://127.0.0.1:{port}/probe-test-host.tar.gz\",\"digest\":\"sha256:{digest_for_thread}\"}}]}}"
                    )
                    .into_bytes()
                } else if request.contains("/probe-test-host.tar.gz") {
                    archive_for_thread.clone()
                } else {
                    Vec::new()
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let source = HttpSource::for_test(
            format!("http://127.0.0.1:{port}"),
            format!("http://127.0.0.1:{port}"),
            TEST_TOKENS,
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = crate::store::ToolStore::open(root.path()).unwrap();
        let receipt = store
            .acquire_with(
                crate::acquire::AcquireRequest {
                    name: "probe".to_string(),
                    spec: "github:fixture/probe".to_string(),
                    version: None,
                    executable_name: "probe".to_string(),
                },
                &source,
            )
            .unwrap();
        assert_eq!(receipt.version, "1.2.3");
        assert_eq!(receipt.executable, "probe");
        let shim = store.which("probe").unwrap().expect("shim");
        let output = std::process::Command::new(&shim)
            .arg("--version")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.status.success(), "{text}");
        assert!(text.contains("probe 1.2.3"), "{text}");
        store.remove_sync("probe").unwrap();
        assert!(store.which("probe").unwrap().is_none());
        assert!(!root.path().join("shims/probe").exists());
        assert!(!root.path().join("receipts/probe.json").exists());
        assert!(!root.path().join("current/probe").exists());
        assert!(!root.path().join("installs/probe").exists());
    }

    fn github_release(version: &str, names: &[&str]) -> GithubRelease {
        GithubRelease {
            version: version.to_string(),
            assets: names.iter().copied().map(asset).collect(),
        }
    }

    #[test]
    fn choose_release_skips_libkrun_and_selects_the_product_archive() {
        let libkrun = github_release(
            "libkrun-sys-v3.3.0",
            &[
                "a3s-libkrun-source.tar",
                "a3s-libkrun-sys-3.3.0-windows-x86_64.zip",
                "libkrunfw-x86_64-v5.5.0.tgz",
                "linux-6.12.91.tar.xz",
            ],
        );
        let product = github_release(
            "3.3.0",
            &[
                "a3s-box-v3.3.0-linux-arm64.tar.gz",
                "a3s-box-v3.3.0-linux-arm64.sha256",
                "a3s-box-v3.3.0-linux-x86_64.tar.gz",
                "a3s-box-v3.3.0-macos-arm64.tar.gz",
                "a3s-box-v3.3.0-macos-arm64.sha256",
                "a3s-box-v3.3.0-windows-x86_64.zip",
                "a3s-box-v3.3.0-windows-x86_64.sha256",
                "containerd-shim-a3s-box-v2-linux-arm64",
                "containerd-shim-a3s-box-v2-linux-x86_64",
                "a3s_box-3.3.0-py3-none-any.whl",
            ],
        );
        let both = [libkrun.clone(), product.clone()];
        let mac = choose_release(&both, MAC_ARM_TOKENS, "a3s-box").unwrap();
        assert_eq!(mac.version, "3.3.0");
        assert_eq!(mac.asset.name, "a3s-box-v3.3.0-macos-arm64.tar.gz");
        let libkrun_only = [libkrun.clone()];
        let missing = match choose_release(&libkrun_only, MAC_ARM_TOKENS, "a3s-box") {
            Ok(choice) => panic!("selected {}", choice.asset.name),
            Err(error) => error,
        };
        assert_eq!(missing.code(), "use.tool_acquire.asset_missing");

        let windows = &["windows-x86_64"];
        let windows_releases = [libkrun.clone(), product.clone()];
        let windows_choice = choose_release(&windows_releases, windows, "a3s-box").unwrap();
        assert_eq!(
            windows_choice.asset.name,
            "a3s-box-v3.3.0-windows-x86_64.zip"
        );
        let windows_libkrun_only = [libkrun];
        let windows_libkrun = match choose_release(&windows_libkrun_only, windows, "a3s-box") {
            Ok(choice) => panic!("selected {}", choice.asset.name),
            Err(error) => error,
        };
        assert_eq!(windows_libkrun.code(), "use.tool_acquire.asset_missing");

        let linux = &["linux-arm64"];
        let linux_releases = [product];
        let linux_choice = choose_release(&linux_releases, linux, "a3s-box").unwrap();
        assert_eq!(linux_choice.asset.name, "a3s-box-v3.3.0-linux-arm64.tar.gz");
    }

    #[test]
    fn choose_release_keeps_the_first_host_match_for_a_plain_executable() {
        let agy = github_release(
            "1.0.0",
            &["antigravity-test-host.tar.gz", "antigravity-other.tar.gz"],
        );
        let releases = [agy];
        let choice = choose_release(&releases, TEST_TOKENS, "agy").unwrap();
        assert_eq!(choice.version, "1.0.0");
        assert_eq!(choice.asset.name, "antigravity-test-host.tar.gz");
    }

    #[test]
    fn extract_tree_keeps_a3s_box_sibling_runtime_files() {
        let script = b"#!/bin/sh\nprintf '%s\\n' 'a3s-box version 3.3.0'\n";
        let archive = tar_gz_files(&[
            ("a3s-box-v3.3.0-macos-arm64/a3s-box", script),
            ("a3s-box-v3.3.0-macos-arm64/a3s-box-shim", b"shim"),
            (
                "a3s-box-v3.3.0-macos-arm64/a3s-box-guest-init",
                b"guest-init",
            ),
            ("a3s-box-v3.3.0-macos-arm64/lib/libkrun.dylib", b"libkrun"),
        ]);
        let extracted =
            extract_tree("a3s-box-v3.3.0-macos-arm64.tar.gz", &archive, "a3s-box").unwrap();
        assert_eq!(extracted.executable, script);
        let mut paths: Vec<&str> = extracted
            .companions
            .iter()
            .map(|file| file.relative_path.as_str())
            .collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec!["a3s-box-guest-init", "a3s-box-shim", "lib/libkrun.dylib",]
        );
    }

    #[test]
    fn a3s_box_install_runs_version_and_remove_leaves_the_sibling_receipt() {
        let script = b"#!/bin/sh\nif [ \"${1:-}\" = version ]; then printf '%s\\n' 'a3s-box version 3.3.0'; exit 0; fi\nexit 1\n";
        let archive = tar_gz_files(&[
            ("a3s-box-v3.3.0-macos-arm64/a3s-box", script),
            ("a3s-box-v3.3.0-macos-arm64/a3s-box-shim", b"shim"),
            (
                "a3s-box-v3.3.0-macos-arm64/a3s-box-guest-init",
                b"guest-init",
            ),
            ("a3s-box-v3.3.0-macos-arm64/lib/libkrun.dylib", b"libkrun"),
        ]);
        let digest = hex_encode(&Sha256::digest(&archive));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let product_name = "a3s-box-v3.3.0-test-host.tar.gz";
        let product_url = format!("http://127.0.0.1:{port}/{product_name}");
        let decoy_name = "a3s-libkrun-sys-3.3.0-test-host.zip";
        let latest = serde_json::json!({
            "tag_name": "libkrun-sys-v3.3.0",
            "assets": [{
                "name": decoy_name,
                "browser_download_url": format!("http://127.0.0.1:{port}/{decoy_name}"),
                "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            }]
        });
        let listed = serde_json::json!([
            {
                "tag_name": "libkrun-sys-v3.3.0",
                "assets": [{
                    "name": decoy_name,
                    "browser_download_url": format!("http://127.0.0.1:{port}/{decoy_name}")
                }]
            },
            {
                "tag_name": "v3.3.0",
                "assets": [
                    {
                        "name": "a3s-box-v3.3.0-linux-arm64.tar.gz",
                        "browser_download_url": format!("http://127.0.0.1:{port}/a3s-box-v3.3.0-linux-arm64.tar.gz")
                    },
                    {
                        "name": "containerd-shim-a3s-box-v2-test-host",
                        "browser_download_url": format!("http://127.0.0.1:{port}/containerd-shim-a3s-box-v2-test-host")
                    },
                    {
                        "name": product_name,
                        "browser_download_url": product_url,
                        "digest": format!("sha256:{digest}")
                    },
                    {
                        "name": "a3s-box-v3.3.0-test-host.tar.gz.sha256",
                        "browser_download_url": format!("http://127.0.0.1:{port}/a3s-box-v3.3.0-test-host.tar.gz.sha256")
                    }
                ]
            }
        ]);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_thread = std::sync::Arc::clone(&seen);
        let latest_bytes = serde_json::to_vec(&latest).unwrap();
        let listed_bytes = serde_json::to_vec(&listed).unwrap();
        std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 8192];
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request.lines().next().unwrap_or("").to_string();
                seen_thread.lock().unwrap().push(path.clone());
                let body = if path.contains("/releases/latest") {
                    latest_bytes.clone()
                } else if path.contains("/releases?") {
                    listed_bytes.clone()
                } else if path.contains(product_name) {
                    archive.clone()
                } else {
                    let header =
                        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                    let _ = stream.write_all(header);
                    continue;
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let source = HttpSource::for_test(
            format!("http://127.0.0.1:{port}"),
            format!("http://127.0.0.1:{port}"),
            TEST_TOKENS,
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = crate::store::ToolStore::open(root.path()).unwrap();
        assert!(store.which("a3s-box").unwrap().is_none());
        let fd_bytes = b"#!/bin/sh\necho fd\n";
        let mut sibling = crate::source::StaticSource::new();
        sibling.insert(
            "github:sharkdp/fd",
            ToolPayload::new(
                "10.5.0",
                hex_encode(&Sha256::digest(fd_bytes)),
                fd_bytes.to_vec(),
            ),
        );
        store
            .acquire_with(
                crate::acquire::AcquireRequest {
                    name: "fd".to_string(),
                    spec: "github:sharkdp/fd".to_string(),
                    version: None,
                    executable_name: "fd".to_string(),
                },
                &sibling,
            )
            .unwrap();
        let receipt = store
            .acquire_with(
                crate::acquire::AcquireRequest {
                    name: "a3s-box".to_string(),
                    spec: "github:A3S-Lab/Box".to_string(),
                    version: None,
                    executable_name: "a3s-box".to_string(),
                },
                &source,
            )
            .unwrap();
        assert_eq!(receipt.version, "3.3.0");
        assert_eq!(receipt.spec, "github:A3S-Lab/Box");
        assert_eq!(receipt.schema, crate::RECEIPT_SCHEMA);
        let requests = seen.lock().unwrap().clone();
        assert!(
            requests.iter().any(|path| path.contains(product_name)),
            "{requests:?}"
        );
        assert!(
            requests.iter().all(|path| !path.contains(decoy_name)),
            "{requests:?}"
        );
        let version_dir = root.path().join("installs/a3s-box/3.3.0");
        assert_eq!(
            std::fs::read(version_dir.join("a3s-box-shim")).unwrap(),
            b"shim"
        );
        assert_eq!(
            std::fs::read(version_dir.join("a3s-box-guest-init")).unwrap(),
            b"guest-init"
        );
        assert_eq!(
            std::fs::read(version_dir.join("lib/libkrun.dylib")).unwrap(),
            b"libkrun"
        );
        let shim = store.which("a3s-box").unwrap().expect("shim");
        let output = std::process::Command::new(&shim)
            .arg("version")
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            text,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            text.lines()
                .any(|line| line.starts_with("a3s-box version ")),
            "{text}"
        );
        store.remove_sync("a3s-box").unwrap();
        assert!(store.which("a3s-box").unwrap().is_none());
        assert!(!root.path().join("shims/a3s-box").exists());
        assert!(!root.path().join("receipts/a3s-box.json").exists());
        assert!(!root.path().join("current/a3s-box").exists());
        assert!(!root.path().join("installs/a3s-box").exists());
        let sibling_receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.path().join("receipts/fd.json")).unwrap())
                .unwrap();
        assert_eq!(sibling_receipt["schema"], crate::RECEIPT_SCHEMA);
        assert_eq!(sibling_receipt["version"], "10.5.0");
        assert!(root.path().join("shims/fd").is_file());
        assert!(root.path().join("installs/fd/10.5.0/fd").is_file());
    }

    #[test]
    fn a3s_box_without_a_checksum_is_refused() {
        let archive = tar_gz(
            "a3s-box",
            b"#!/bin/sh\nprintf '%s\\n' 'a3s-box version 3.3.0'\n",
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let listed = serde_json::json!([
            {
                "tag_name": "libkrun-sys-v3.3.0",
                "assets": [{
                    "name": "a3s-libkrun-sys-3.3.0-test-host.zip",
                    "browser_download_url": format!("http://127.0.0.1:{port}/libkrun.zip")
                }]
            },
            {
                "tag_name": "v3.3.0",
                "assets": [{
                    "name": "a3s-box-v3.3.0-test-host.tar.gz",
                    "browser_download_url": format!("http://127.0.0.1:{port}/a3s-box-v3.3.0-test-host.tar.gz")
                }]
            }
        ]);
        let latest = serde_json::json!({
            "tag_name": "libkrun-sys-v3.3.0",
            "assets": [{
                "name": "a3s-libkrun-source.tar",
                "browser_download_url": format!("http://127.0.0.1:{port}/source.tar")
            }]
        });
        let latest_bytes = serde_json::to_vec(&latest).unwrap();
        let listed_bytes = serde_json::to_vec(&listed).unwrap();
        let archive_for_thread = archive.clone();
        std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0_u8; 8192];
                let read = std::io::Read::read(&mut stream, &mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request.lines().next().unwrap_or("");
                let body = if path.contains("/releases/latest") {
                    latest_bytes.clone()
                } else if path.contains("/releases?") {
                    listed_bytes.clone()
                } else if path.contains("a3s-box-v3.3.0-test-host.tar.gz") {
                    archive_for_thread.clone()
                } else {
                    let header =
                        b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
                    let _ = stream.write_all(header);
                    continue;
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        let source = HttpSource::for_test(
            format!("http://127.0.0.1:{port}"),
            format!("http://127.0.0.1:{port}"),
            TEST_TOKENS,
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = crate::store::ToolStore::open(root.path()).unwrap();
        let error = store
            .acquire_with(
                crate::acquire::AcquireRequest {
                    name: "a3s-box".to_string(),
                    spec: "github:A3S-Lab/Box".to_string(),
                    version: None,
                    executable_name: "a3s-box".to_string(),
                },
                &source,
            )
            .unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.checksum_required");
        assert!(!root.path().join("receipts/a3s-box.json").exists());
        assert!(!root.path().join("installs/a3s-box").exists());
    }

    #[test]
    fn empty_policy_keeps_the_official_sources() {
        let source = HttpSource::with_policy(FetchPolicy::default()).unwrap();
        assert_eq!(source.npm_registry(), "https://registry.npmjs.org");
        assert!(source.github_mirror().is_none());
        assert!(source.github_token.is_none());
        assert!(source.allows("https://registry.npmjs.org/pkg"));
        assert!(!source.allows("https://registry.npmmirror.com/pkg"));
        assert!(!source.allows("https://ghfast.top/https://github.com/a/b/file.tar.gz"));
    }

    #[test]
    fn mirror_rewrites_asset_urls_and_keeps_the_token_on_the_api() {
        let source = HttpSource::with_policy(FetchPolicy {
            github_mirror: "https://ghfast.top".to_string(),
            npm_registry: String::new(),
            github_token: "ghp_exampletoken".to_string(),
        })
        .unwrap();
        let asset = "https://github.com/astral-sh/uv/releases/download/v1/uv.tar.gz";
        let rewritten = source.prepare_download(asset).unwrap();
        assert_eq!(
            rewritten,
            "https://ghfast.top/https://github.com/astral-sh/uv/releases/download/v1/uv.tar.gz"
        );
        assert!(source.allows(&rewritten));
        assert!(source
            .token_header("https://api.github.com/repos/astral-sh/uv/releases/latest")
            .unwrap()
            .starts_with("Bearer "));
        assert!(source.token_header(&rewritten).is_none());
        assert!(source.token_header(asset).is_none());
        let debug = format!("{source:?}");
        assert!(!debug.contains("ghp_exampletoken"));
    }

    #[test]
    fn unknown_mirror_registry_and_token_are_refused() {
        let mirror = HttpSource::with_policy(FetchPolicy {
            github_mirror: "https://evil.example".to_string(),
            ..FetchPolicy::default()
        })
        .unwrap_err();
        assert_eq!(mirror.code(), "use.tool_acquire.download_rejected");
        let registry = HttpSource::with_policy(FetchPolicy {
            npm_registry: "https://evil.example".to_string(),
            ..FetchPolicy::default()
        })
        .unwrap_err();
        assert_eq!(registry.code(), "use.tool_acquire.download_rejected");
        let token = HttpSource::with_policy(FetchPolicy {
            github_token: "bad token".to_string(),
            ..FetchPolicy::default()
        })
        .unwrap_err();
        assert_eq!(token.code(), "use.tool_acquire.download_rejected");
        assert!(!token.to_string().contains("bad token"));
    }

    #[test]
    fn npmmirror_allows_its_cdn_and_refuses_the_official_tarball_host() {
        let source = HttpSource::with_policy(FetchPolicy {
            npm_registry: "https://registry.npmmirror.com/".to_string(),
            ..FetchPolicy::default()
        })
        .unwrap();
        assert_eq!(source.npm_registry(), "https://registry.npmmirror.com");
        assert!(source.allows("https://registry.npmmirror.com/pkg"));
        assert!(source.allows("https://cdn.npmmirror.com/binaries/npm/pkg.tgz"));
        assert!(!source.allows("https://registry.npmjs.org/pkg/-/pkg.tgz"));
        assert!(source
            .prepare_download("https://evil.example/tool.tar.gz")
            .is_err());
    }

    fn tar_gz(name: &str, bytes: &[u8]) -> Vec<u8> {
        tar_gz_files(&[(name, bytes)])
    }

    fn tar_gz_files(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut raw = Vec::new();
        {
            let encoder = GzEncoder::new(&mut raw, Compression::default());
            let mut builder = tar::Builder::new(encoder);
            for (name, bytes) in files {
                let mut header = tar::Header::new_gnu();
                header.set_path(name).unwrap();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append(&header, *bytes).unwrap();
            }
            builder.finish().unwrap();
        }
        raw
    }

    #[test]
    fn extract_uses_the_only_archive_file_when_its_name_differs() {
        let archive = tar_gz("antigravity", b"#!/bin/sh\necho agy\n");
        let bytes = extract_tree("agy_cli_mac_arm64.tar.gz", &archive, "agy")
            .unwrap()
            .executable;
        assert_eq!(bytes, b"#!/bin/sh\necho agy\n");
    }

    #[test]
    fn extract_keeps_the_named_file_beside_another_file() {
        let archive = tar_gz_files(&[("autocomplete/_fd", b"complete"), ("fd", b"the-fd-binary")]);
        let bytes = extract_tree("fd.tar.gz", &archive, "fd")
            .unwrap()
            .executable;
        assert_eq!(bytes, b"the-fd-binary");
    }

    #[test]
    fn extract_refuses_two_files_when_neither_matches() {
        let archive = tar_gz_files(&[("one", b"a"), ("two", b"b")]);
        let error = extract_tree("tool.tar.gz", &archive, "agy").unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.asset_missing");
    }
}
