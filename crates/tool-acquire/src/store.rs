use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::metadata_is_link_or_reparse_point;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::ToolAcquireError;
use crate::lock::{join_under, InstallLock};
use crate::receipt::{ToolFailureRecord, ToolReceipt, RECEIPT_SCHEMA};
use crate::source::CompanionFile;
use crate::spec::{parse_tool_name, parse_tool_spec, parse_tool_version, ToolSpec};

/// Copy one regular file into the store as a versioned executable.
#[derive(Debug, Clone)]
pub struct LocalApply {
    pub name: String,
    pub spec: String,
    pub version: String,
    pub executable_name: String,
    pub payload: PathBuf,
    pub sha256: Option<String>,
    pub require_checksum: bool,
    /// Sibling files written into the version directory next to the executable.
    pub companions: Vec<CompanionFile>,
}

/// Host-supplied root for installs, shims, and receipts.
#[derive(Debug, Clone)]
pub struct ToolStore {
    root: PathBuf,
}

pub(crate) fn root_of(store: &ToolStore) -> &Path {
    &store.root
}

impl ToolStore {
    /// Create the root when it is missing. A symlink root is refused.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ToolAcquireError> {
        let root = root.into();
        fs::create_dir_all(&root)
            .map_err(|error| ToolAcquireError::io("create tool root", &root, error))?;
        ensure_real_directory(&root)?;
        let root = root
            .canonicalize()
            .map_err(|error| ToolAcquireError::io("canonicalize tool root", &root, error))?;
        Ok(Self { root })
    }

    pub async fn apply_local(&self, request: LocalApply) -> Result<ToolReceipt, ToolAcquireError> {
        let root = self.root.clone();
        spawn_blocking(move || apply_local(&root, request)).await
    }

    pub async fn list(&self) -> Result<Vec<ToolReceipt>, ToolAcquireError> {
        let root = self.root.clone();
        spawn_blocking(move || list(&root)).await
    }

    pub fn list_sync(&self) -> Result<Vec<ToolReceipt>, ToolAcquireError> {
        list(&self.root)
    }

    pub async fn remove(&self, name: &str) -> Result<(), ToolAcquireError> {
        let root = self.root.clone();
        let name = name.to_string();
        spawn_blocking(move || remove(&root, &name)).await
    }

    pub fn remove_sync(&self, name: &str) -> Result<(), ToolAcquireError> {
        remove(&self.root, name)
    }
}

pub(crate) fn apply_local(
    root: &Path,
    request: LocalApply,
) -> Result<ToolReceipt, ToolAcquireError> {
    parse_tool_name(&request.name)?;
    parse_tool_name(&request.executable_name)?;
    let spec = parse_tool_spec(&request.spec)?;
    parse_tool_version(&request.version)?;
    let expected = match &request.sha256 {
        Some(value) => Some(normalize_sha256(value)?),
        None if request.require_checksum => return Err(ToolAcquireError::ChecksumRequired),
        None => None,
    };

    let _lock = InstallLock::acquire(root)?;
    if let Some(current) = read_pointer(root, &request.name)? {
        if current == request.version {
            return same_version(root, &request, &spec, expected.as_deref());
        }
    }

    let staging_name = format!(".staging-{}", request.version);
    let staging = join_under(root, &["installs", &request.name, &staging_name])?;
    ensure_parent_real(root, &["installs", &request.name])?;
    remove_owned_tree(&staging)?;
    fs::create_dir_all(&staging)
        .map_err(|error| ToolAcquireError::io("create tool staging directory", &staging, error))?;

    let staged_executable = join_under(
        root,
        &[
            "installs",
            &request.name,
            &staging_name,
            &request.executable_name,
        ],
    )?;
    let hashed = match stage_payload(&request.payload, &staged_executable) {
        Ok(hashed) => hashed,
        Err(error) => {
            remove_owned_tree(&staging)?;
            return Err(error);
        }
    };
    if let Some(expected) = expected.as_deref() {
        if hashed != expected {
            remove_owned_tree(&staging)?;
            record_failure(root, &request, &ToolAcquireError::ChecksumMismatch)?;
            return Err(ToolAcquireError::ChecksumMismatch);
        }
    }
    if let Err(error) = mark_executable(&staged_executable) {
        remove_owned_tree(&staging)?;
        record_failure(root, &request, &error)?;
        return Err(error);
    }
    if let Err(error) = stage_companions(&staging, &request.executable_name, &request.companions) {
        remove_owned_tree(&staging)?;
        record_failure(root, &request, &error)?;
        return Err(error);
    }

    let version_dir = join_under(root, &["installs", &request.name, &request.version])?;
    if version_dir.exists() {
        let pointer = read_pointer(root, &request.name)?;
        if pointer.as_deref() == Some(request.version.as_str()) {
            remove_owned_tree(&staging)?;
            return Err(ToolAcquireError::VersionConflict {
                version: request.version,
            });
        }
        remove_owned_tree(&version_dir)?;
    }
    fs::rename(&staging, &version_dir).map_err(|error| {
        ToolAcquireError::io("publish tool version directory", &version_dir, error)
    })?;

    let receipt = ToolReceipt::new(
        &request.name,
        spec.raw(),
        &request.version,
        &request.executable_name,
        expected.as_deref().unwrap_or(&hashed),
    )?;
    publish_current(root, &receipt)?;
    clear_failure(root, &request.name)?;
    Ok(receipt)
}

fn same_version(
    root: &Path,
    request: &LocalApply,
    spec: &ToolSpec,
    expected: Option<&str>,
) -> Result<ToolReceipt, ToolAcquireError> {
    let receipt = read_receipt(root, &request.name)?;
    let matches = receipt.spec == spec.raw()
        && receipt.version == request.version
        && receipt.executable == request.executable_name
        && expected.is_none_or(|expected| expected == receipt.sha256);
    if !matches {
        return Err(ToolAcquireError::VersionConflict {
            version: request.version.clone(),
        });
    }
    let executable = join_under(
        root,
        &[
            "installs",
            &request.name,
            &request.version,
            &request.executable_name,
        ],
    )?;
    ensure_real_file(&executable)?;
    Ok(receipt)
}

fn publish_current(root: &Path, receipt: &ToolReceipt) -> Result<(), ToolAcquireError> {
    ensure_real_directory(&join_under(root, &["shims"])?)?;
    ensure_real_directory(&join_under(root, &["current"])?)?;
    ensure_real_directory(&join_under(root, &["receipts"])?)?;
    let shim = join_under(root, &["shims", &receipt.executable])?;
    write_atomic(&shim, &shim_bytes(&receipt.name, &receipt.executable))?;
    set_mode(&shim, 0o755)?;
    let pointer = join_under(root, &["current", &receipt.name])?;
    let previous =
        if pointer.exists() {
            ensure_real_file(&pointer)?;
            Some(fs::read(&pointer).map_err(|error| {
                ToolAcquireError::io("read current tool version", &pointer, error)
            })?)
        } else {
            None
        };
    write_atomic(&pointer, format!("{}\n", receipt.version).as_bytes())?;
    let receipt_path = join_under(root, &["receipts", &format!("{}.json", receipt.name)])?;
    if let Err(error) = write_atomic(&receipt_path, &json_bytes(receipt)?) {
        restore_pointer(&pointer, previous)?;
        return Err(error);
    }
    Ok(())
}

fn restore_pointer(pointer: &Path, previous: Option<Vec<u8>>) -> Result<(), ToolAcquireError> {
    match previous {
        Some(bytes) => write_atomic(pointer, &bytes),
        None => remove_owned_file(pointer),
    }
}

fn list(root: &Path) -> Result<Vec<ToolReceipt>, ToolAcquireError> {
    let _lock = InstallLock::acquire(root)?;
    let receipts = join_under(root, &["receipts"])?;
    if !receipts.exists() {
        return Ok(Vec::new());
    }
    ensure_real_directory(&receipts)?;
    let mut names = Vec::new();
    for entry in fs::read_dir(&receipts)
        .map_err(|error| ToolAcquireError::io("read tool receipts", &receipts, error))?
    {
        let entry = entry
            .map_err(|error| ToolAcquireError::io("read tool receipt entry", &receipts, error))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.ends_with(".failure.json") || !name.ends_with(".json") || name.starts_with('.') {
            continue;
        }
        names.push(name.trim_end_matches(".json").to_string());
    }
    names.sort();
    let mut listed = Vec::with_capacity(names.len());
    for name in names {
        let receipt = read_receipt(root, &name)?;
        let pointer = read_pointer(root, &name)?;
        if pointer.as_deref() != Some(receipt.version.as_str()) {
            return Err(ToolAcquireError::ReceiptDisagrees { name });
        }
        listed.push(receipt);
    }
    Ok(listed)
}

fn remove(root: &Path, name: &str) -> Result<(), ToolAcquireError> {
    parse_tool_name(name)?;
    let _lock = InstallLock::acquire(root)?;
    let receipt = join_under(root, &["receipts", &format!("{name}.json")])?;
    if receipt.exists() {
        let parsed: ToolReceipt = read_json(&receipt)?;
        let shim = join_under(root, &["shims", &parsed.executable])?;
        remove_owned_file(&shim)?;
    }
    remove_owned_file(&receipt)?;
    remove_owned_file(&join_under(
        root,
        &["receipts", &format!("{name}.failure.json")],
    )?)?;
    remove_owned_file(&join_under(root, &["current", name])?)?;
    remove_owned_tree(&join_under(root, &["installs", name])?)?;
    Ok(())
}

fn stage_payload(source: &Path, destination: &Path) -> Result<String, ToolAcquireError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| ToolAcquireError::io("inspect tool payload", source, error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(ToolAcquireError::PayloadInvalid {
            path: source.to_path_buf(),
        });
    }
    if metadata.len() == 0 {
        return Err(ToolAcquireError::NotExecutable);
    }
    let mut input = File::open(source)
        .map_err(|error| ToolAcquireError::io("open tool payload", source, error))?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| ToolAcquireError::io("create staged tool", destination, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| ToolAcquireError::io("read tool payload", source, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|error| ToolAcquireError::io("write staged tool", destination, error))?;
    }
    output
        .sync_all()
        .map_err(|error| ToolAcquireError::io("sync staged tool", destination, error))?;
    Ok(hex_encode(&hasher.finalize()))
}

fn stage_companions(
    staging: &Path,
    executable_name: &str,
    companions: &[CompanionFile],
) -> Result<(), ToolAcquireError> {
    for companion in companions {
        let destination =
            companion_destination(staging, executable_name, &companion.relative_path)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ToolAcquireError::io("create companion directory", parent, error)
            })?;
        }
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&destination)
            .map_err(|error| ToolAcquireError::io("create companion file", &destination, error))?;
        output
            .write_all(&companion.bytes)
            .map_err(|error| ToolAcquireError::io("write companion file", &destination, error))?;
        output
            .sync_all()
            .map_err(|error| ToolAcquireError::io("sync companion file", &destination, error))?;
        let mode = if companion.executable { 0o755 } else { 0o644 };
        set_mode(&destination, mode)?;
    }
    Ok(())
}

fn companion_destination(
    staging: &Path,
    executable_name: &str,
    relative_path: &str,
) -> Result<PathBuf, ToolAcquireError> {
    if relative_path.starts_with('/') || relative_path.contains('\0') {
        return Err(ToolAcquireError::PayloadInvalid {
            path: PathBuf::from(relative_path),
        });
    }
    let parts: Vec<&str> = relative_path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    let invalid = parts.is_empty()
        || parts.iter().any(|part| {
            *part == "."
                || *part == ".."
                || part.contains('/')
                || part.contains('\\')
                || part.contains('\0')
        });
    if invalid {
        return Err(ToolAcquireError::PayloadInvalid {
            path: PathBuf::from(relative_path),
        });
    }
    let replaces_executable = parts.len() == 1
        && (parts[0] == executable_name
            || parts[0].eq_ignore_ascii_case(&format!("{executable_name}.exe")));
    if replaces_executable {
        return Err(ToolAcquireError::PayloadInvalid {
            path: PathBuf::from(relative_path),
        });
    }
    let mut destination = staging.to_path_buf();
    for part in parts {
        destination.push(part);
    }
    Ok(destination)
}

fn mark_executable(path: &Path) -> Result<(), ToolAcquireError> {
    ensure_real_file(path)?;
    set_mode(path, 0o755)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ToolAcquireError::io("inspect staged tool", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() || metadata.len() == 0 {
        return Err(ToolAcquireError::NotExecutable);
    }
    Ok(())
}

fn record_failure(
    root: &Path,
    request: &LocalApply,
    error: &ToolAcquireError,
) -> Result<(), ToolAcquireError> {
    let record = ToolFailureRecord::new(&request.name, &request.spec, &request.version, error)?;
    ensure_real_directory(&join_under(root, &["receipts"])?)?;
    let path = join_under(
        root,
        &["receipts", &format!("{}.failure.json", request.name)],
    )?;
    write_atomic(&path, &json_bytes(&record)?)
}

fn clear_failure(root: &Path, name: &str) -> Result<(), ToolAcquireError> {
    remove_owned_file(&join_under(
        root,
        &["receipts", &format!("{name}.failure.json")],
    )?)
}

pub(crate) fn read_receipt(root: &Path, name: &str) -> Result<ToolReceipt, ToolAcquireError> {
    let path = join_under(root, &["receipts", &format!("{name}.json")])?;
    let receipt: ToolReceipt = read_json(&path)?;
    if receipt.schema != RECEIPT_SCHEMA {
        return Err(ToolAcquireError::ReceiptDisagrees {
            name: name.to_string(),
        });
    }
    Ok(receipt)
}

pub(crate) fn read_pointer(root: &Path, name: &str) -> Result<Option<String>, ToolAcquireError> {
    let path = join_under(root, &["current", name])?;
    if !path.exists() {
        return Ok(None);
    }
    ensure_real_file(&path)?;
    let text = fs::read_to_string(&path)
        .map_err(|error| ToolAcquireError::io("read current tool version", &path, error))?;
    let version = text.trim();
    parse_tool_version(version)?;
    Ok(Some(version.to_string()))
}

fn ensure_parent_real(root: &Path, parts: &[&str]) -> Result<(), ToolAcquireError> {
    let mut current = root.to_path_buf();
    for part in parts {
        current = join_under(&current, &[part])?;
        if current.exists() {
            ensure_real_directory(&current)?;
        } else {
            fs::create_dir(&current)
                .map_err(|error| ToolAcquireError::io("create tool directory", &current, error))?;
        }
    }
    Ok(())
}

pub(crate) fn ensure_real_directory(path: &Path) -> Result<(), ToolAcquireError> {
    if !path.exists() {
        fs::create_dir_all(path)
            .map_err(|error| ToolAcquireError::io("create tool directory", path, error))?;
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ToolAcquireError::io("inspect tool directory", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(ToolAcquireError::LinkRefused {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

pub(crate) fn ensure_real_file(path: &Path) -> Result<(), ToolAcquireError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ToolAcquireError::io("inspect tool file", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(ToolAcquireError::LinkRefused {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn remove_owned_file(path: &Path) -> Result<(), ToolAcquireError> {
    if !path.exists() && fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    if path.exists() || fs::symlink_metadata(path).is_ok() {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| ToolAcquireError::io("inspect tool file", path, error))?;
        if metadata_is_link_or_reparse_point(&metadata) {
            return Err(ToolAcquireError::LinkRefused {
                path: path.to_path_buf(),
            });
        }
        if metadata.is_file() {
            fs::remove_file(path)
                .map_err(|error| ToolAcquireError::io("remove tool file", path, error))?;
        }
    }
    Ok(())
}

pub(crate) fn remove_owned_tree(path: &Path) -> Result<(), ToolAcquireError> {
    if fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ToolAcquireError::io("inspect tool directory", path, error))?;
    if metadata_is_link_or_reparse_point(&metadata) {
        return Err(ToolAcquireError::LinkRefused {
            path: path.to_path_buf(),
        });
    }
    if metadata.is_dir() {
        fs::remove_dir_all(path)
            .map_err(|error| ToolAcquireError::io("remove tool directory", path, error))?;
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ToolAcquireError> {
    let parent = path.parent().ok_or_else(|| ToolAcquireError::Io {
        action: "locate tool file parent",
        path: path.to_path_buf(),
        source: std::io::Error::other("missing parent"),
    })?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file"),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| {
                ToolAcquireError::io("create temporary tool file", &temporary, error)
            })?;
        file.write_all(bytes).map_err(|error| {
            ToolAcquireError::io("write temporary tool file", &temporary, error)
        })?;
        file.sync_all()
            .map_err(|error| ToolAcquireError::io("sync temporary tool file", &temporary, error))?;
    }
    fs::rename(&temporary, path)
        .map_err(|error| ToolAcquireError::io("publish tool file", path, error))?;
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, ToolAcquireError> {
    ensure_real_file(path)?;
    let bytes =
        fs::read(path).map_err(|error| ToolAcquireError::io("read tool record", path, error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        ToolAcquireError::io(
            "parse tool record",
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidData, error),
        )
    })
}

fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>, ToolAcquireError> {
    serde_json::to_vec_pretty(value).map_err(|error| {
        ToolAcquireError::io(
            "encode tool record",
            PathBuf::from("."),
            std::io::Error::other(error),
        )
    })
}

fn normalize_sha256(value: &str) -> Result<String, ToolAcquireError> {
    let value = value.trim().to_ascii_lowercase();
    if value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit()) {
        Ok(value)
    } else {
        Err(ToolAcquireError::ChecksumMismatch)
    }
}

pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn shim_bytes(name: &str, executable: &str) -> Vec<u8> {
    #[cfg(windows)]
    {
        format!(
            "@echo off\r\nset \"root=%~dp0..\"\r\nset /p version=<\"%root%\\current\\{name}\"\r\n\"%root%\\installs\\{name}\\%version%\\{executable}\" %*\r\n"
        )
        .into_bytes()
    }
    #[cfg(not(windows))]
    {
        format!(
            "#!/bin/sh\nset -eu\nhere=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nroot=$(CDPATH= cd -- \"$here/..\" && pwd)\nversion=$(cat -- \"$root/current/{name}\")\nexec \"$root/installs/{name}/$version/{executable}\" \"$@\"\n"
        )
        .into_bytes()
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), ToolAcquireError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| ToolAcquireError::io("set tool file mode", path, error))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

async fn spawn_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ToolAcquireError> + Send + 'static,
) -> Result<T, ToolAcquireError> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => Err(ToolAcquireError::Task(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(directory: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn sha256(bytes: &[u8]) -> String {
        hex_encode(&Sha256::digest(bytes))
    }

    #[tokio::test]
    async fn checksum_failure_keeps_the_previous_current_version() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ToolStore::open(temporary.path()).unwrap();
        let good = b"#!/bin/sh\necho uv\n";
        let good_path = payload(temporary.path(), "uv-good", good);
        store
            .apply_local(LocalApply {
                name: "uv".to_string(),
                spec: "uv".to_string(),
                version: "1.0.0".to_string(),
                executable_name: "uv".to_string(),
                payload: good_path,
                sha256: Some(sha256(good)),
                require_checksum: true,
                companions: Vec::new(),
            })
            .await
            .unwrap();

        let bad = b"not-the-published-bytes";
        let bad_path = payload(temporary.path(), "uv-bad", bad);
        let error = store
            .apply_local(LocalApply {
                name: "uv".to_string(),
                spec: "uv".to_string(),
                version: "2.0.0".to_string(),
                executable_name: "uv".to_string(),
                payload: bad_path,
                sha256: Some(sha256(good)),
                require_checksum: true,
                companions: Vec::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.checksum_mismatch");
        assert_eq!(
            fs::read_to_string(temporary.path().join("current/uv")).unwrap(),
            "1.0.0\n"
        );
        assert!(!temporary.path().join("installs/uv/2.0.0").exists());
        assert!(temporary.path().join("installs/uv/1.0.0/uv").is_file());
        let failure: ToolFailureRecord = serde_json::from_slice(
            &fs::read(temporary.path().join("receipts/uv.failure.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(failure.schema, crate::FAILURE_SCHEMA);
        assert_eq!(failure.version, "2.0.0");
        let listed = store.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].version, "1.0.0");
    }

    #[tokio::test]
    async fn first_checksum_failure_leaves_no_current_version() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ToolStore::open(temporary.path()).unwrap();
        let bytes = b"tool";
        let error = store
            .apply_local(LocalApply {
                name: "rg".to_string(),
                spec: "rg".to_string(),
                version: "14.1.1".to_string(),
                executable_name: "rg".to_string(),
                payload: payload(temporary.path(), "rg-payload", bytes),
                sha256: Some("ab".repeat(32)),
                require_checksum: true,
                companions: Vec::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.checksum_mismatch");
        assert!(!temporary.path().join("current/rg").exists());
        assert!(store.list().await.unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remove_refuses_a_symlinked_install_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let store = ToolStore::open(temporary.path()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let kept = outside.path().join("kept.txt");
        fs::write(&kept, b"keep").unwrap();
        fs::create_dir_all(temporary.path().join("installs")).unwrap();
        std::os::unix::fs::symlink(outside.path(), temporary.path().join("installs/uv")).unwrap();

        let error = store.remove("uv").await.unwrap_err();
        assert_eq!(error.code(), "use.tool_acquire.link_refused");
        assert_eq!(fs::read(&kept).unwrap(), b"keep");
        assert!(temporary
            .path()
            .join("installs/uv")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
