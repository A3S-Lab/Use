use std::io;
use std::path::PathBuf;

use a3s_use_core::{UseError, UseResult};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::{PluginGraphCapabilityPublication, RecordingHost};

pub(super) const GRAPH_CUTOVER_CRASH_EXIT_CODE: i32 = 87;

impl RecordingHost {
    pub(super) fn with_durable_graph_root(root: PathBuf) -> Self {
        Self {
            durable_graph_root: Some(root),
            ..Self::default()
        }
    }

    pub(super) async fn crash_after_graph_effect(&self, key: impl Into<String>) {
        *self.crash_after_graph_effect.lock().await = Some(key.into());
    }
}

pub(super) async fn record_graph_effect(
    host: &RecordingHost,
    kind: &str,
    key: &str,
    publication: &PluginGraphCapabilityPublication,
) -> UseResult<()> {
    let Some(root) = host.durable_graph_root.as_ref() else {
        return Ok(());
    };
    let effects = root.join("effects");
    tokio::fs::create_dir_all(&effects)
        .await
        .map_err(|error| graph_host_io("create graph effect directory", error))?;

    let attempt = format!("{kind}\t{key}\n");
    let mut attempts = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("attempts.log"))
        .await
        .map_err(|error| graph_host_io("open graph attempt log", error))?;
    attempts
        .write_all(attempt.as_bytes())
        .await
        .map_err(|error| graph_host_io("append graph attempt log", error))?;
    attempts
        .sync_all()
        .await
        .map_err(|error| graph_host_io("sync graph attempt log", error))?;
    drop(attempts);

    let identity = format!("{:x}", Sha256::digest(format!("{kind}\n{key}").as_bytes()));
    let path = effects.join(format!("{identity}.effect"));
    let effect = encode_effect(kind, key, publication);
    match tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await
    {
        Ok(mut file) => {
            file.write_all(effect.as_bytes())
                .await
                .map_err(|error| graph_host_io("write graph effect", error))?;
            file.sync_all()
                .await
                .map_err(|error| graph_host_io("sync graph effect", error))?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing = tokio::fs::read(&path)
                .await
                .map_err(|error| graph_host_io("read replayed graph effect", error))?;
            if existing != effect.as_bytes() {
                return Err(UseError::new(
                    "use.plugin.test_durable_graph_effect_conflict",
                    "A replay reused a graph cutover key with different publication evidence.",
                ));
            }
        }
        Err(error) => return Err(graph_host_io("create graph effect", error)),
    }

    if host.crash_after_graph_effect.lock().await.as_deref() == Some(key) {
        std::process::exit(GRAPH_CUTOVER_CRASH_EXIT_CODE);
    }
    Ok(())
}

fn encode_effect(kind: &str, key: &str, publication: &PluginGraphCapabilityPublication) -> String {
    let mut effect = format!(
        "{kind}\n{key}\n{}\n{}\n{}\n",
        publication.cutover().capability_generation_before(),
        publication.cutover().capability_generation_after(),
        publication.cutover().capability_snapshot_digest(),
    );
    for package in publication.packages() {
        effect.push_str(package.package_id());
        effect.push('\t');
        effect.push_str(package.evidence().digest());
        effect.push('\n');
    }
    effect
}

fn graph_host_io(action: &str, error: io::Error) -> UseError {
    UseError::new(
        "use.plugin.test_durable_graph_host_io",
        format!("Failed to {action}: {error}"),
    )
}
