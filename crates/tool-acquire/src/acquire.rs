use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::ToolAcquireError;
use crate::lock::{join_under, InstallLock};
use crate::receipt::ToolReceipt;
use crate::source::{ToolQuery, ToolSource};
use crate::spec::{parse_tool_name, parse_tool_spec, parse_tool_version};
use crate::store::{
    self, apply_local, ensure_real_directory, ensure_real_file, read_pointer, read_receipt,
    remove_owned_tree, LocalApply,
};

/// Install one resolved spec through a source, then publish it with the store.
#[derive(Debug, Clone)]
pub struct AcquireRequest {
    pub name: String,
    pub spec: String,
    pub version: Option<String>,
    pub executable_name: String,
}

impl crate::store::ToolStore {
    pub fn acquire_with(
        &self,
        request: AcquireRequest,
        source: &dyn ToolSource,
    ) -> Result<ToolReceipt, ToolAcquireError> {
        parse_tool_name(&request.name)?;
        parse_tool_name(&request.executable_name)?;
        let spec = parse_tool_spec(&request.spec)?;
        if let Some(version) = &request.version {
            parse_tool_version(version)?;
        }
        let payload = source.fetch(&ToolQuery {
            spec: spec.clone(),
            version: request.version.clone(),
            executable: request.executable_name.clone(),
        })?;
        parse_tool_version(&payload.version)?;
        if let Some(wanted) = &request.version {
            if wanted != &payload.version {
                return Err(ToolAcquireError::VersionConflict {
                    version: payload.version,
                });
            }
        }
        let incoming =
            write_incoming(self.root(), &request.name, &payload.version, &payload.bytes)?;
        let applied = apply_local(
            self.root(),
            LocalApply {
                name: request.name,
                spec: spec.raw().to_string(),
                version: payload.version,
                executable_name: request.executable_name,
                payload: incoming.clone(),
                sha256: Some(payload.sha256),
                require_checksum: true,
                companions: payload.companions,
            },
        );
        let _ = fs::remove_file(&incoming);
        applied
    }

    pub fn which(&self, name: &str) -> Result<Option<PathBuf>, ToolAcquireError> {
        parse_tool_name(name)?;
        let receipt_path = join_under(self.root(), &["receipts", &format!("{name}.json")])?;
        if !receipt_path.exists() {
            return Ok(None);
        }
        let receipt = read_receipt(self.root(), name)?;
        let shim = join_under(self.root(), &["shims", &receipt.executable])?;
        if !shim.exists() {
            return Ok(None);
        }
        ensure_real_file(&shim)?;
        Ok(Some(shim))
    }

    pub fn latest_with(
        &self,
        spec: &str,
        source: &dyn ToolSource,
    ) -> Result<String, ToolAcquireError> {
        let spec = parse_tool_spec(spec)?;
        source.latest(&spec)
    }

    /// Delete version directories other than the current pointer.
    pub fn prune(&self, name: &str) -> Result<(), ToolAcquireError> {
        parse_tool_name(name)?;
        let _lock = InstallLock::acquire(self.root())?;
        let Some(current) = read_pointer(self.root(), name)? else {
            return Ok(());
        };
        let directory = join_under(self.root(), &["installs", name])?;
        if !directory.exists() {
            return Ok(());
        }
        ensure_real_directory(&directory)?;
        let entries = fs::read_dir(&directory)
            .map_err(|error| ToolAcquireError::io("read tool versions", &directory, error))?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                ToolAcquireError::io("read tool version entry", &directory, error)
            })?;
            let file_name = entry.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            if file_name == current || file_name.starts_with('.') {
                continue;
            }
            remove_owned_tree(&entry.path())?;
        }
        Ok(())
    }
}

impl crate::store::ToolStore {
    pub(crate) fn root(&self) -> &Path {
        store::root_of(self)
    }
}

fn write_incoming(
    root: &Path,
    name: &str,
    version: &str,
    bytes: &[u8],
) -> Result<PathBuf, ToolAcquireError> {
    if bytes.is_empty() {
        return Err(ToolAcquireError::NotExecutable);
    }
    let directory = join_under(root, &[".incoming"])?;
    ensure_real_directory(&directory)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let path = directory.join(format!("{name}-{version}-{nanos}"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|error| ToolAcquireError::io("create incoming tool", &path, error))?;
    file.write_all(bytes)
        .map_err(|error| ToolAcquireError::io("write incoming tool", &path, error))?;
    file.sync_all()
        .map_err(|error| ToolAcquireError::io("sync incoming tool", &path, error))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{StaticSource, ToolPayload};
    use crate::store::ToolStore;
    use sha2::{Digest, Sha256};

    fn sha256(bytes: &[u8]) -> String {
        store::hex_encode(&Sha256::digest(bytes))
    }

    fn payload(version: &str, bytes: &[u8]) -> ToolPayload {
        ToolPayload::new(version, sha256(bytes), bytes.to_vec())
    }

    #[test]
    fn github_checksum_mismatch_leaves_current_unchanged() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ToolStore::open(temporary.path()).unwrap();
        let good = b"#!/bin/sh\necho uv\n";
        let mut source = StaticSource::new();
        source.insert("github:astral-sh/uv", payload("1.0.0", good));
        store
            .acquire_with(
                AcquireRequest {
                    name: "uv".into(),
                    spec: "github:astral-sh/uv".into(),
                    version: None,
                    executable_name: "uv".into(),
                },
                &source,
            )
            .unwrap();

        let mut bad = payload("2.0.0", b"not-the-published-bytes");
        bad.sha256 = sha256(good);
        source.insert("github:astral-sh/uv", bad);
        let error = store
            .acquire_with(
                AcquireRequest {
                    name: "uv".into(),
                    spec: "github:astral-sh/uv".into(),
                    version: Some("2.0.0".into()),
                    executable_name: "uv".into(),
                },
                &source,
            )
            .unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.checksum_mismatch");
        assert_eq!(
            fs::read_to_string(temporary.path().join("current/uv")).unwrap(),
            "1.0.0\n"
        );
        assert!(!temporary.path().join("installs/uv/2.0.0").exists());
    }

    #[test]
    fn npm_and_pipx_fixtures_roll_back() {
        for spec in ["npm:@openai/codex", "pipx:babeldoc-stream"] {
            let temporary = tempfile::tempdir().unwrap();
            let store = ToolStore::open(temporary.path()).unwrap();
            let name = if spec.starts_with("npm:") {
                "codex"
            } else {
                "babeldoc-stream"
            };
            let first = b"#!/bin/sh\necho one\n";
            let mut source = StaticSource::new();
            source.insert(spec, payload("1.0.0", first));
            store
                .acquire_with(
                    AcquireRequest {
                        name: name.into(),
                        spec: spec.into(),
                        version: None,
                        executable_name: name.into(),
                    },
                    &source,
                )
                .unwrap();
            let mut bad = payload("2.0.0", b"broken");
            bad.sha256 = sha256(first);
            source.insert(spec, bad);
            let error = store
                .acquire_with(
                    AcquireRequest {
                        name: name.into(),
                        spec: spec.into(),
                        version: None,
                        executable_name: name.into(),
                    },
                    &source,
                )
                .unwrap_err();
            assert_eq!(error.code(), "use.tool_acquire.checksum_mismatch");
            assert_eq!(
                fs::read_to_string(temporary.path().join(format!("current/{name}"))).unwrap(),
                "1.0.0\n"
            );
        }
    }

    #[test]
    fn aqua_which_prune_and_latest_failure_keep_the_install() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ToolStore::open(temporary.path()).unwrap();
        let spec = "aqua:google-antigravity/antigravity-cli";
        let mut source = StaticSource::new();
        source.insert(spec, payload("1.0.0", b"#!/bin/sh\necho agy\n"));
        source.insert_latest(spec, "9.9.9");
        store
            .acquire_with(
                AcquireRequest {
                    name: "agy".into(),
                    spec: spec.into(),
                    version: None,
                    executable_name: "agy".into(),
                },
                &source,
            )
            .unwrap();
        let shim = store.which("agy").unwrap().unwrap();
        assert!(shim.ends_with("shims/agy"));
        assert!(shim.is_file());

        source.insert(spec, payload("1.1.0", b"#!/bin/sh\necho next\n"));
        store
            .acquire_with(
                AcquireRequest {
                    name: "agy".into(),
                    spec: spec.into(),
                    version: None,
                    executable_name: "agy".into(),
                },
                &source,
            )
            .unwrap();
        assert!(temporary.path().join("installs/agy/1.0.0").is_dir());
        store.prune("agy").unwrap();
        assert!(!temporary.path().join("installs/agy/1.0.0").exists());
        assert!(temporary.path().join("installs/agy/1.1.0/agy").is_file());

        let before = fs::read(temporary.path().join("current/agy")).unwrap();
        source.fail_latest(spec);
        let error = store.latest_with(spec, &source).unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.latest_unavailable");
        assert_eq!(
            fs::read(temporary.path().join("current/agy")).unwrap(),
            before
        );
        assert_eq!(
            fs::read_to_string(temporary.path().join("installs/agy/1.1.0/agy")).unwrap(),
            "#!/bin/sh\necho next\n"
        );
    }
}
