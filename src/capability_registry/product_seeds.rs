//! Standalone product first-party capability seeds.
//!
//! Browser, OCR, and Box are ordinary product-profile projectors injected
//! through [`super::CapabilitySeedProvider`]. The universal engine path never
//! imports this module for its default empty seed set.

use a3s_use_core::{Readiness, UseResult};
use async_trait::async_trait;

use super::{
    skill_surface, CapabilityBinding, CapabilityOrigin, CapabilitySeedProvider, McpSurface,
    McpTransport,
};

/// Standalone product profile: Browser, OCR, and Box as injected BuiltIn seeds.
#[derive(Debug, Default, Clone, Copy)]
pub struct BundledFirstPartyCapabilitySeeds;

#[async_trait]
impl CapabilitySeedProvider for BundledFirstPartyCapabilitySeeds {
    async fn project(&self) -> UseResult<Vec<CapabilityBinding>> {
        Ok(vec![
            browser_capability().await?,
            ocr_capability().await?,
            box_capability(),
        ])
    }
}

async fn browser_capability() -> UseResult<CapabilityBinding> {
    #[cfg(feature = "browser")]
    {
        let diagnostic = a3s_use_browser::doctor();
        let skill = crate::browser_driver::primary_skill_surface().await;
        let (package_root, skills) = match skill {
            Some((root, path)) => (Some(root), vec![skill_surface("browser", path).await?]),
            None => (None, Vec::new()),
        };
        Ok(CapabilityBinding {
            id: "use/browser".to_string(),
            alias: Some("browser".to_string()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: true,
            readiness: diagnostic.readiness,
            #[cfg(feature = "extensions")]
            reconciliation: None,
            planner_evidence: None,
            package_root,
            lifecycle_generation: None,
            requires_use: None,
            repository: None,
            surfaces: vec!["cli".to_string(), "mcp".to_string(), "skill".to_string()],
            mcp: crate::browser_driver::is_available().then(|| McpSurface {
                target: "browser".to_string(),
                transport: McpTransport::Stdio,
            }),
            mcp_servers: Vec::new(),
            skills,
            flows: Vec::new(),
            knowledge: Vec::new(),
            activity_bar: Vec::new(),
            tool_tasks: Vec::new(),
            executable_tools: Vec::new(),
        })
    }
    #[cfg(not(feature = "browser"))]
    {
        Ok(CapabilityBinding {
            id: "use/browser".to_string(),
            alias: Some("browser".to_string()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: false,
            readiness: Readiness::Missing,
            #[cfg(feature = "extensions")]
            reconciliation: None,
            planner_evidence: None,
            package_root: None,
            lifecycle_generation: None,
            requires_use: None,
            repository: None,
            surfaces: Vec::new(),
            mcp: None,
            mcp_servers: Vec::new(),
            skills: Vec::new(),
            flows: Vec::new(),
            knowledge: Vec::new(),
            activity_bar: Vec::new(),
            tool_tasks: Vec::new(),
            executable_tools: Vec::new(),
        })
    }
}

async fn ocr_capability() -> UseResult<CapabilityBinding> {
    #[cfg(feature = "ocr")]
    {
        let diagnostic = crate::ocr_builtin::diagnostic();
        let skill = crate::ocr_builtin::primary_skill_surface().await;
        let (package_root, skills) = match skill {
            Some((root, path)) => (Some(root), vec![skill_surface("ocr", path).await?]),
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
            alias: Some("ocr".to_string()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: true,
            readiness: diagnostic.readiness,
            #[cfg(feature = "extensions")]
            reconciliation: None,
            planner_evidence: None,
            package_root,
            lifecycle_generation: None,
            requires_use: None,
            repository: None,
            surfaces,
            #[cfg(feature = "mcp")]
            mcp: Some(McpSurface {
                target: "ocr-native".to_string(),
                transport: McpTransport::Stdio,
            }),
            #[cfg(not(feature = "mcp"))]
            mcp: None,
            mcp_servers: Vec::new(),
            skills,
            flows: Vec::new(),
            knowledge: Vec::new(),
            activity_bar: Vec::new(),
            tool_tasks: Vec::new(),
            executable_tools: Vec::new(),
        })
    }
    #[cfg(not(feature = "ocr"))]
    {
        Ok(CapabilityBinding {
            id: "use/ocr".to_string(),
            alias: Some("ocr".to_string()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: false,
            readiness: Readiness::Missing,
            #[cfg(feature = "extensions")]
            reconciliation: None,
            planner_evidence: None,
            package_root: None,
            lifecycle_generation: None,
            requires_use: None,
            repository: None,
            surfaces: Vec::new(),
            mcp: None,
            mcp_servers: Vec::new(),
            skills: Vec::new(),
            flows: Vec::new(),
            knowledge: Vec::new(),
            activity_bar: Vec::new(),
            tool_tasks: Vec::new(),
            executable_tools: Vec::new(),
        })
    }
}

pub(super) fn box_capability() -> CapabilityBinding {
    let diagnostic = crate::component_route::box_diagnostic();
    CapabilityBinding {
        id: "use/box".to_string(),
        alias: Some("box".to_string()),
        version: env!("CARGO_PKG_VERSION").to_string(),
        origin: CapabilityOrigin::BuiltIn,
        enabled: diagnostic.readiness == Readiness::Ready,
        readiness: diagnostic.readiness,
        #[cfg(feature = "extensions")]
        reconciliation: None,
        planner_evidence: None,
        package_root: None,
        lifecycle_generation: None,
        requires_use: None,
        repository: None,
        surfaces: vec!["cli".to_string()],
        mcp: None,
        mcp_servers: Vec::new(),
        skills: Vec::new(),
        flows: Vec::new(),
        knowledge: Vec::new(),
        activity_bar: Vec::new(),
        tool_tasks: Vec::new(),
        executable_tools: Vec::new(),
    }
}
