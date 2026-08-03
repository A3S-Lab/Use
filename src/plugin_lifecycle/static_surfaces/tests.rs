use std::path::PathBuf;

use a3s_use_core::PluginSurfaceKind;
use a3s_use_extension::ExtensionManifest;
use sha2::{Digest, Sha256};

use crate::plugin_lifecycle::{
    PluginLifecycleAction, PluginLifecycleIntent, PluginLifecycleIntentSpec,
};

use super::*;

const MANIFEST: &str = include_str!(
    "../../../crates/extension/fixtures/packages/plugin-v3/package/a3s-use-extension.acl"
);
const PACKAGE_DIGEST: &str =
    include_str!("../../../crates/extension/fixtures/packages/plugin-v3/package.sha256");

#[tokio::test]
async fn prepares_real_skill_and_ui_files_and_keeps_deletion_package_owned() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent(&manifest);
    let host = StaticPluginSurfaceLifecycleHost::new(package_root());
    let skill = &manifest.skills[0];
    let ui = &manifest.ui[0];
    let skill_key = key(&intent, PluginSurfaceKind::Skill, &skill.id);
    let ui_key = key(&intent, PluginSurfaceKind::Ui, &ui.id);

    let skill_first = host.prepare_skill(&intent, skill, skill_key).await.unwrap();
    let skill_replay = host.prepare_skill(&intent, skill, skill_key).await.unwrap();
    let ui_first = host.prepare_ui(&intent, ui, ui_key).await.unwrap();
    let ui_replay = host.prepare_ui(&intent, ui, ui_key).await.unwrap();
    assert_eq!(skill_first, skill_replay);
    assert_eq!(ui_first, ui_replay);
    assert_ne!(skill_first, ui_first);

    host.stop_ui(&intent, ui, ui_key).await.unwrap();
    host.remove_ui(&intent, ui, ui_key).await.unwrap();
    host.stop_skill(&intent, skill, skill_key).await.unwrap();
    host.remove_skill(&intent, skill, skill_key).await.unwrap();
    assert!(package_root().is_dir());
}

#[tokio::test]
async fn preparation_fails_closed_when_a_declared_static_file_is_missing() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent(&manifest);
    let empty = tempfile::tempdir().unwrap();
    let host = StaticPluginSurfaceLifecycleHost::new(empty.path());
    let error = host
        .prepare_skill(
            &intent,
            &manifest.skills[0],
            key(&intent, PluginSurfaceKind::Skill, &manifest.skills[0].id),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.io");
}

fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3/package")
}

fn intent(manifest: &ExtensionManifest) -> PluginLifecycleIntent {
    PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: "static-install".to_string(),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope_id: "workspace:research".to_string(),
            package_id: manifest.package_id.clone(),
            package_digest: PACKAGE_DIGEST.trim().to_string(),
            manifest_digest: format!("sha256:{:x}", Sha256::digest(MANIFEST.as_bytes())),
            generation: 9,
            action: PluginLifecycleAction::Install,
        },
        manifest,
    )
    .unwrap()
}

fn key<'a>(intent: &'a PluginLifecycleIntent, kind: PluginSurfaceKind, id: &str) -> &'a str {
    &intent
        .checkpoints
        .iter()
        .find(|checkpoint| {
            checkpoint
                .surface
                .as_ref()
                .is_some_and(|surface| surface.kind == kind && surface.id == id)
        })
        .unwrap()
        .idempotency_key
}
