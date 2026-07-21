//! Unified projection of built-in and externally installed Use capabilities.
//!
//! This is a versioned JSON CLI contract for long-running consumers. It is
//! not a private RPC protocol: invocation still happens through native CLI,
//! standard MCP, and `SKILL.md` surfaces.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use a3s_use_core::{Readiness, UseError, UseResult};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

const SCHEMA_VERSION: u32 = 1;
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_STABLE_SNAPSHOT_ATTEMPTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityRegistrySnapshot {
    pub schema_version: u32,
    pub generation: u64,
    pub revision: String,
    pub capabilities: Vec<CapabilityBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CapabilityOrigin {
    BuiltIn,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum McpTransport {
    Stdio,
    StreamableHttp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpSurface {
    target: String,
    transport: McpTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillSurface {
    path: PathBuf,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedAsset {
    path: PathBuf,
    sha256: String,
    media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityBarContribution {
    id: String,
    title: String,
    description: String,
    icon: String,
    entry: ManagedAsset,
    skill: String,
    order: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityBinding {
    id: String,
    route: String,
    version: String,
    origin: CapabilityOrigin,
    enabled: bool,
    readiness: Readiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_root: Option<PathBuf>,
    surfaces: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp: Option<McpSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skills: Vec<SkillSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    activity_bar: Vec<ActivityBarContribution>,
}

pub(crate) async fn snapshot() -> UseResult<CapabilityRegistrySnapshot> {
    let (generation, extensions) = stable_extensions().await?;
    let mut capabilities = vec![
        browser_capability().await?,
        office_capability().await?,
        office_compatibility_capability(),
        ocr_capability().await?,
        box_capability(),
    ];
    capabilities.extend(extensions);
    capabilities.sort_by(|left, right| left.id.cmp(&right.id));

    let revision = revision(&capabilities)?;
    Ok(CapabilityRegistrySnapshot {
        schema_version: SCHEMA_VERSION,
        generation,
        revision,
        capabilities,
    })
}

pub(crate) async fn wait_for_change(
    after_generation: u64,
    after_revision: Option<&str>,
    timeout: Duration,
) -> UseResult<Option<CapabilityRegistrySnapshot>> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        UseError::new(
            "use.capability.timeout_invalid",
            "The capability watch timeout is too large.",
        )
    })?;

    loop {
        let current = snapshot().await?;
        let changed = match after_revision {
            Some(revision) => {
                current.generation != after_generation || current.revision != revision
            }
            None => current.generation > after_generation,
        };
        if changed {
            return Ok(Some(current));
        }

        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(WATCH_INTERVAL.min(deadline.saturating_duration_since(now))).await;
    }
}

async fn browser_capability() -> UseResult<CapabilityBinding> {
    #[cfg(feature = "browser")]
    {
        let diagnostic = a3s_use_browser::doctor();
        let skill = crate::browser_driver::primary_skill_surface().await;
        let (package_root, skills) = match skill {
            Some((root, path)) => (Some(root), vec![skill_surface(path).await?]),
            None => (None, Vec::new()),
        };
        Ok(CapabilityBinding {
            id: "use/browser".to_string(),
            route: "browser".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: true,
            readiness: diagnostic.readiness,
            package_root,
            surfaces: vec!["cli".to_string(), "mcp".to_string(), "skill".to_string()],
            mcp: crate::browser_driver::is_available().then(|| McpSurface {
                target: "browser".to_string(),
                transport: McpTransport::Stdio,
            }),
            skills,
            activity_bar: Vec::new(),
        })
    }
    #[cfg(not(feature = "browser"))]
    {
        Ok(CapabilityBinding {
            id: "use/browser".to_string(),
            route: "browser".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: false,
            readiness: Readiness::Missing,
            package_root: None,
            surfaces: Vec::new(),
            mcp: None,
            skills: Vec::new(),
            activity_bar: Vec::new(),
        })
    }
}

async fn office_capability() -> UseResult<CapabilityBinding> {
    #[cfg(feature = "office")]
    {
        let skill = crate::office_skills::primary_skill_surface().await;
        let (package_root, skills) = match skill {
            Some((root, path)) => (Some(root), vec![skill_surface(path).await?]),
            None => (None, Vec::new()),
        };
        let mut surfaces = vec!["cli".to_string(), "skill".to_string()];
        #[cfg(feature = "mcp")]
        surfaces.push("mcp".to_string());
        Ok(CapabilityBinding {
            id: "use/office".to_string(),
            route: "office".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: true,
            readiness: Readiness::Ready,
            package_root,
            surfaces,
            #[cfg(feature = "mcp")]
            mcp: Some(McpSurface {
                target: "office-native".to_string(),
                transport: McpTransport::Stdio,
            }),
            #[cfg(not(feature = "mcp"))]
            mcp: None,
            skills,
            activity_bar: Vec::new(),
        })
    }
    #[cfg(not(feature = "office"))]
    {
        Ok(CapabilityBinding {
            id: "use/office".to_string(),
            route: "office".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: false,
            readiness: Readiness::Missing,
            package_root: None,
            surfaces: Vec::new(),
            mcp: None,
            skills: Vec::new(),
            activity_bar: Vec::new(),
        })
    }
}

fn office_compatibility_capability() -> CapabilityBinding {
    #[cfg(feature = "office")]
    {
        let diagnostic = a3s_use_office::doctor();
        let ready = diagnostic.readiness == Readiness::Ready;
        CapabilityBinding {
            id: "use/office-compat".to_string(),
            route: "office-compat".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: true,
            readiness: diagnostic.readiness,
            package_root: None,
            surfaces: vec!["mcp".to_string()],
            mcp: ready.then(|| McpSurface {
                target: "office-compat".to_string(),
                transport: McpTransport::Stdio,
            }),
            skills: Vec::new(),
            activity_bar: Vec::new(),
        }
    }
    #[cfg(not(feature = "office"))]
    {
        CapabilityBinding {
            id: "use/office-compat".to_string(),
            route: "office-compat".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: false,
            readiness: Readiness::Missing,
            package_root: None,
            surfaces: Vec::new(),
            mcp: None,
            skills: Vec::new(),
            activity_bar: Vec::new(),
        }
    }
}

async fn ocr_capability() -> UseResult<CapabilityBinding> {
    #[cfg(feature = "ocr")]
    {
        let diagnostic = crate::ocr_builtin::diagnostic();
        let skill = crate::ocr_builtin::primary_skill_surface().await;
        let (package_root, skills) = match skill {
            Some((root, path)) => (Some(root), vec![skill_surface(path).await?]),
            None => (None, Vec::new()),
        };
        let mut surfaces = vec!["cli".to_string()];
        if !skills.is_empty() {
            surfaces.push("skill".to_string());
        }
        #[cfg(feature = "mcp")]
        surfaces.push("mcp".to_string());
        Ok(CapabilityBinding {
            id: "use/ocr".to_string(),
            route: "ocr".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: true,
            readiness: diagnostic.readiness,
            package_root,
            surfaces,
            #[cfg(feature = "mcp")]
            mcp: Some(McpSurface {
                target: "ocr-native".to_string(),
                transport: McpTransport::Stdio,
            }),
            #[cfg(not(feature = "mcp"))]
            mcp: None,
            skills,
            activity_bar: Vec::new(),
        })
    }
    #[cfg(not(feature = "ocr"))]
    {
        Ok(CapabilityBinding {
            id: "use/ocr".to_string(),
            route: "ocr".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: false,
            readiness: Readiness::Missing,
            package_root: None,
            surfaces: Vec::new(),
            mcp: None,
            skills: Vec::new(),
            activity_bar: Vec::new(),
        })
    }
}

fn box_capability() -> CapabilityBinding {
    let diagnostic = crate::component_route::box_diagnostic();
    CapabilityBinding {
        id: "use/box".to_string(),
        route: "box".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        origin: CapabilityOrigin::BuiltIn,
        enabled: diagnostic.readiness == Readiness::Ready,
        readiness: diagnostic.readiness,
        package_root: None,
        surfaces: vec!["cli".to_string()],
        mcp: None,
        skills: Vec::new(),
        activity_bar: Vec::new(),
    }
}

fn revision(capabilities: &[CapabilityBinding]) -> UseResult<String> {
    let bytes = serde_json::to_vec(capabilities).map_err(|error| {
        UseError::new(
            "use.capability.snapshot_invalid",
            format!("Failed to encode the capability snapshot: {error}"),
        )
    })?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

async fn skill_surface(path: PathBuf) -> UseResult<SkillSurface> {
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|error| skill_io_error("inspect", &path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(UseError::new(
            "use.capability.skill_invalid",
            format!(
                "Projected Skill '{}' must be a regular package file.",
                path.display()
            ),
        ));
    }

    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| skill_io_error("open", &path, error))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| skill_io_error("read", &path, error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }

    Ok(SkillSurface {
        path,
        sha256: format!("{:x}", digest.finalize()),
    })
}

async fn activity_asset(path: PathBuf) -> UseResult<ManagedAsset> {
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|error| activity_io_error("inspect", &path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(UseError::new(
            "use.capability.activity_asset_invalid",
            format!(
                "Projected Activity Bar asset '{}' must be a regular package file.",
                path.display()
            ),
        ));
    }
    if metadata.len() == 0 || metadata.len() > 2 * 1024 * 1024 {
        return Err(UseError::new(
            "use.capability.activity_asset_invalid",
            format!(
                "Projected Activity Bar asset '{}' exceeds the supported size.",
                path.display()
            ),
        ));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| activity_io_error("read", &path, error))?;
    std::str::from_utf8(&bytes).map_err(|error| {
        UseError::new(
            "use.capability.activity_asset_invalid",
            format!(
                "Projected Activity Bar asset '{}' must be UTF-8 HTML: {error}",
                path.display()
            ),
        )
    })?;
    Ok(ManagedAsset {
        path,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        media_type: "text/html".to_string(),
    })
}

fn activity_io_error(action: &str, path: &Path, error: std::io::Error) -> UseError {
    UseError::new(
        "use.capability.activity_asset_unreadable",
        format!(
            "Failed to {action} projected Activity Bar asset '{}': {error}",
            path.display()
        ),
    )
}

fn skill_io_error(action: &str, path: &Path, error: std::io::Error) -> UseError {
    UseError::new(
        "use.capability.skill_unreadable",
        format!(
            "Failed to {action} projected Skill '{}': {error}",
            path.display()
        ),
    )
}

#[cfg(feature = "extensions")]
async fn stable_extensions() -> UseResult<(u64, Vec<CapabilityBinding>)> {
    for _ in 0..MAX_STABLE_SNAPSHOT_ATTEMPTS {
        let before = crate::extension_host::snapshot().await?;
        let Some(capabilities) = project_extensions(&before).await? else {
            continue;
        };
        let after = crate::extension_host::snapshot().await?;
        if before == after {
            return Ok((before.generation, capabilities));
        }
    }
    Err(UseError::new(
        "use.capability.registry_busy",
        "The extension registry changed repeatedly while capabilities were projected.",
    )
    .with_suggestion("Retry the capability snapshot after the current component operation."))
}

#[cfg(not(feature = "extensions"))]
async fn stable_extensions() -> UseResult<(u64, Vec<CapabilityBinding>)> {
    Ok((0, Vec::new()))
}

#[cfg(feature = "extensions")]
async fn project_extensions(
    snapshot: &a3s_use_extension::ExtensionRegistrySnapshot,
) -> UseResult<Option<Vec<CapabilityBinding>>> {
    let mut capabilities = Vec::with_capacity(snapshot.routes.len());
    for route in &snapshot.routes {
        #[cfg(feature = "ocr")]
        if route.route == "ocr" {
            // OCR became a first-party built-in route. Ignore a legacy OCR
            // extension receipt so an older installation cannot shadow or
            // duplicate the release-matched built-in MCP/Skill projection.
            continue;
        }
        let Some(extension) = crate::extension_host::get(&route.package_id).await? else {
            return Ok(None);
        };
        let receipt = &extension.receipt;
        let surfaces = extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if receipt.package_id != route.package_id
            || receipt.component_id != route.component_id
            || receipt.route != route.route
            || receipt.version != route.version
            || receipt.package_root != route.package_root
            || receipt.manifest_sha256 != route.manifest_sha256
            || receipt.enabled != route.enabled
            || surfaces != route.surfaces
        {
            return Ok(None);
        }

        let mcp = extension.manifest.mcp.as_ref().map(|surface| McpSurface {
            target: receipt.package_id.clone(),
            transport: match surface.transport {
                a3s_use_extension::McpTransport::Stdio => McpTransport::Stdio,
                a3s_use_extension::McpTransport::StreamableHttp => McpTransport::StreamableHttp,
            },
        });
        let mut skills = Vec::new();
        if let Some(path) = extension.skill_path() {
            skills.push(skill_surface(path).await?);
        }
        let mut activity_bar = Vec::new();
        for contribution in &extension.manifest.contributes.activity_bar {
            activity_bar.push(ActivityBarContribution {
                id: contribution.id.clone(),
                title: contribution.title.clone(),
                description: contribution.description.clone(),
                icon: contribution.icon.clone(),
                entry: activity_asset(receipt.package_root.join(&contribution.entry)).await?,
                skill: contribution.skill.clone(),
                order: contribution.order,
            });
        }
        capabilities.push(CapabilityBinding {
            id: receipt.component_id.clone(),
            route: receipt.route.clone(),
            version: receipt.version.clone(),
            origin: CapabilityOrigin::Extension,
            enabled: receipt.enabled,
            readiness: if receipt.enabled {
                Readiness::Ready
            } else {
                Readiness::Unknown
            },
            package_root: Some(receipt.package_root.clone()),
            surfaces,
            mcp,
            skills,
            activity_bar,
        });
    }
    Ok(Some(capabilities))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn built_ins_are_projected_without_extension_identity() {
        let snapshot = snapshot().await.unwrap();
        let browser = snapshot
            .capabilities
            .iter()
            .find(|capability| capability.id == "use/browser")
            .unwrap();
        let office = snapshot
            .capabilities
            .iter()
            .find(|capability| capability.id == "use/office")
            .unwrap();
        let office_compat = snapshot
            .capabilities
            .iter()
            .find(|capability| capability.id == "use/office-compat")
            .unwrap();
        let ocr = snapshot
            .capabilities
            .iter()
            .find(|capability| capability.id == "use/ocr")
            .unwrap();

        assert_eq!(browser.origin, CapabilityOrigin::BuiltIn);
        assert_eq!(office.origin, CapabilityOrigin::BuiltIn);
        assert_eq!(office_compat.origin, CapabilityOrigin::BuiltIn);
        assert_eq!(ocr.origin, CapabilityOrigin::BuiltIn);
        #[cfg(feature = "browser")]
        {
            assert!(browser.surfaces.iter().any(|surface| surface == "skill"));
            assert!(browser
                .skills
                .iter()
                .any(|skill| skill.path.ends_with("a3s-use-browser/SKILL.md")));
            assert!(browser.skills.iter().all(|skill| skill.sha256.len() == 64));
        }
        #[cfg(not(feature = "browser"))]
        {
            assert!(!browser.enabled);
            assert!(browser.surfaces.is_empty());
            assert!(browser.skills.is_empty());
        }
        #[cfg(feature = "office")]
        {
            assert!(office.surfaces.iter().any(|surface| surface == "skill"));
            assert!(office
                .skills
                .iter()
                .any(|skill| skill.path.ends_with("a3s-use-office/SKILL.md")));
            assert!(office.skills.iter().all(|skill| skill.sha256.len() == 64));
            #[cfg(feature = "mcp")]
            assert_eq!(
                office.mcp.as_ref().map(|surface| surface.target.as_str()),
                Some("office-native")
            );
            assert!(office_compat.skills.is_empty());
            assert_eq!(office_compat.route, "office-compat");
        }
        #[cfg(not(feature = "office"))]
        {
            assert!(!office.enabled);
            assert!(office.surfaces.is_empty());
            assert!(office.skills.is_empty());
        }
        #[cfg(feature = "ocr")]
        {
            assert!(ocr.enabled);
            assert!(ocr.surfaces.iter().any(|surface| surface == "skill"));
            assert!(ocr
                .skills
                .iter()
                .any(|skill| skill.path.ends_with("a3s-use-ocr/SKILL.md")));
            assert!(ocr.skills.iter().all(|skill| skill.sha256.len() == 64));
            #[cfg(feature = "mcp")]
            assert_eq!(
                ocr.mcp.as_ref().map(|surface| surface.target.as_str()),
                Some("ocr-native")
            );
        }
        #[cfg(not(feature = "ocr"))]
        {
            assert!(!ocr.enabled);
            assert!(ocr.surfaces.is_empty());
            assert!(ocr.skills.is_empty());
        }
        assert_eq!(snapshot.revision.len(), 64);
    }

    #[tokio::test]
    async fn skill_content_changes_revision_without_changing_its_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        tokio::fs::write(&path, b"first").await.unwrap();
        let first = skill_surface(path.clone()).await.unwrap();
        tokio::fs::write(&path, b"second").await.unwrap();
        let second = skill_surface(path).await.unwrap();
        assert_ne!(first.sha256, second.sha256);

        let mut capability = box_capability();
        capability.skills = vec![first];
        let first_revision = revision(&[capability.clone()]).unwrap();
        capability.skills = vec![second];
        let second_revision = revision(&[capability]).unwrap();
        assert_ne!(first_revision, second_revision);
    }

    #[tokio::test]
    async fn activity_asset_content_is_integrity_bound_to_the_registry_revision() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("activity.html");
        tokio::fs::write(&path, b"<main>first</main>")
            .await
            .unwrap();
        let first = activity_asset(path.clone()).await.unwrap();
        tokio::fs::write(&path, b"<main>second</main>")
            .await
            .unwrap();
        let second = activity_asset(path).await.unwrap();
        assert_ne!(first.sha256, second.sha256);

        let mut capability = box_capability();
        capability.activity_bar = vec![ActivityBarContribution {
            id: "science".to_string(),
            title: "Science".to_string(),
            description: "Scientific workspace".to_string(),
            icon: "flask-conical".to_string(),
            entry: first,
            skill: "science".to_string(),
            order: 120,
        }];
        let first_revision = revision(&[capability.clone()]).unwrap();
        capability.activity_bar[0].entry = second;
        let second_revision = revision(&[capability]).unwrap();
        assert_ne!(first_revision, second_revision);
    }

    #[tokio::test]
    async fn matching_revision_times_out_without_reporting_a_change() {
        let current = snapshot().await.unwrap();
        let changed = wait_for_change(
            current.generation,
            Some(&current.revision),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert!(changed.is_none());
    }
}
