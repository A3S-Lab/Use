use a3s_use_core::{PlanScope, PluginHostPackageState, VerifiedPluginCatalogRecord};
use a3s_use_extension::PluginCatalogSnapshot;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManagerSearchResult {
    pub source_revision: String,
    pub snapshots: Vec<PluginCatalogSnapshot>,
    pub plugins: Vec<VerifiedPluginCatalogRecord>,
    pub total_matches: u64,
    pub next_cursors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManagerInstalledPackage {
    pub package_id: String,
    pub state: PluginHostPackageState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManagerInstalledPage {
    pub scope: PlanScope,
    pub snapshot_digest: String,
    pub packages: Vec<PluginManagerInstalledPackage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}
