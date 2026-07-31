use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use a3s_use_core::UseResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tough::{ExpirationEnforcement, FilesystemTransport, Limits, Repository, RepositoryLoader};
use url::Url;

use crate::package::{activate_temporary_file, io_error, sync_parent_directory, unique_suffix};

use super::super::{
    acquire_metadata_lock, validate_download_url, TrustedRegistry, MAX_BOOTSTRAP_ROOT_BYTES,
    MAX_ROOT_UPDATES,
};
use super::{catalog_cache_error, VerifiedRegistryMetadata};

const CATALOG_CACHE_SCHEMA: &str = "a3s.use.catalog-cache.v1";
const CATALOG_CACHE_DIRECTORY: &str = "catalog-metadata";
const CATALOG_CACHE_NAME: &str = "catalog-cache.json";
const MAX_CATALOG_CACHE_BYTES: u64 = 16 * 1024;
const MAX_TARGETS_METADATA_BYTES: u64 = 10 * 1024 * 1024;
const MAX_TIMESTAMP_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_SNAPSHOT_METADATA_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CatalogCacheStamp {
    schema: String,
    registry_name: String,
    registry_url: String,
    root_sha256: String,
    root_version: u64,
    timestamp_version: u64,
    snapshot_version: u64,
    targets_version: u64,
    root_metadata_sha256: String,
    timestamp_metadata_sha256: String,
    snapshot_metadata_sha256: String,
    targets_metadata_sha256: String,
    pub(super) verified_at_unix_seconds: u64,
}

pub(super) async fn record_catalog_refresh(
    registry: &TrustedRegistry,
    repository: &Repository,
    metadata: &VerifiedRegistryMetadata,
) -> UseResult<CatalogCacheStamp> {
    let _lock = acquire_metadata_lock(registry.datastore())?;
    let cache_directory = ensure_cache_directory(registry.datastore()).await?;
    let root = read_metadata_file(
        &registry.datastore().join("root.json"),
        MAX_BOOTSTRAP_ROOT_BYTES,
    )
    .await?;
    let metadata_url = registry.metadata_url()?;
    let timestamp = download_metadata_role(
        &metadata_url.join("timestamp.json").map_err(|error| {
            catalog_cache_error(
                "use.extension.registry_url_invalid",
                format!("Failed to resolve timestamp metadata URL: {error}"),
            )
        })?,
        MAX_TIMESTAMP_METADATA_BYTES,
        "timestamp",
    )
    .await?;
    let snapshot_name = if repository.root().signed.consistent_snapshot {
        format!(
            "{}.snapshot.json",
            repository.snapshot().signed.version.get()
        )
    } else {
        "snapshot.json".to_owned()
    };
    let snapshot = download_metadata_role(
        &metadata_url.join(&snapshot_name).map_err(|error| {
            catalog_cache_error(
                "use.extension.registry_url_invalid",
                format!("Failed to resolve snapshot metadata URL: {error}"),
            )
        })?,
        MAX_SNAPSHOT_METADATA_BYTES,
        "snapshot",
    )
    .await?;
    let targets_name = if repository.root().signed.consistent_snapshot {
        format!("{}.targets.json", repository.targets().signed.version.get())
    } else {
        "targets.json".to_owned()
    };
    let targets = download_metadata_role(
        &metadata_url.join(&targets_name).map_err(|error| {
            catalog_cache_error(
                "use.extension.registry_url_invalid",
                format!("Failed to resolve targets metadata URL: {error}"),
            )
        })?,
        MAX_TARGETS_METADATA_BYTES,
        "targets",
    )
    .await?;
    verify_role_matches(&root, repository.root(), "root")?;
    verify_role_matches(&timestamp, repository.timestamp(), "timestamp")?;
    verify_role_matches(&snapshot, repository.snapshot(), "snapshot")?;
    verify_role_matches(&targets, repository.targets(), "targets")?;

    let stamp = CatalogCacheStamp {
        schema: CATALOG_CACHE_SCHEMA.to_owned(),
        registry_name: registry.name().to_owned(),
        registry_url: registry.base_url().to_string(),
        root_sha256: registry.root_sha256().to_owned(),
        root_version: signed_role_version(&root, "root")?,
        timestamp_version: signed_role_version(&timestamp, "timestamp")?,
        snapshot_version: signed_role_version(&snapshot, "snapshot")?,
        targets_version: signed_role_version(&targets, "targets")?,
        root_metadata_sha256: raw_sha256(&root),
        timestamp_metadata_sha256: raw_sha256(&timestamp),
        snapshot_metadata_sha256: raw_sha256(&snapshot),
        targets_metadata_sha256: raw_sha256(&targets),
        verified_at_unix_seconds: unix_time_seconds()?,
    };
    verify_stamp_versions(&stamp, repository)?;
    verify_stamp_metadata(&stamp, metadata)?;
    write_cache_file(&cache_directory.join("root.json"), &root, "root metadata").await?;
    write_cache_file(
        &cache_directory.join("timestamp.json"),
        &timestamp,
        "timestamp metadata",
    )
    .await?;
    write_cache_file(
        &cache_directory.join("snapshot.json"),
        &snapshot,
        "snapshot metadata",
    )
    .await?;
    write_cache_file(
        &cache_directory.join("targets.json"),
        &targets,
        "targets metadata",
    )
    .await?;
    write_cache_stamp(&cache_directory, &stamp).await?;
    Ok(stamp)
}

pub(super) async fn load_cached_repository(
    registry: &TrustedRegistry,
) -> UseResult<(Repository, CatalogCacheStamp)> {
    let datastore_metadata = fs::symlink_metadata(registry.datastore())
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                catalog_cache_error(
                    "use.extension.catalog_cache_missing",
                    "No verified catalog cache exists for this registry.",
                )
            } else {
                io_error(
                    "inspect cached TUF metadata directory",
                    registry.datastore(),
                    error,
                )
            }
        })?;
    if datastore_metadata.file_type().is_symlink() || !datastore_metadata.is_dir() {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            "The cached TUF metadata path must be a real directory.",
        ));
    }

    let cache_directory = registry.datastore().join(CATALOG_CACHE_DIRECTORY);
    let cache_metadata = fs::symlink_metadata(&cache_directory)
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                catalog_cache_error(
                    "use.extension.catalog_cache_missing",
                    "No verified catalog snapshot exists for this registry.",
                )
            } else {
                io_error("inspect verified catalog snapshot", &cache_directory, error)
            }
        })?;
    if cache_metadata.file_type().is_symlink() || !cache_metadata.is_dir() {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            "The verified catalog snapshot path must be a real directory.",
        ));
    }

    let _lock = acquire_metadata_lock(registry.datastore())?;
    let stamp_path = cache_directory.join(CATALOG_CACHE_NAME);
    let stamp_bytes = read_metadata_file(&stamp_path, MAX_CATALOG_CACHE_BYTES).await?;
    let stamp: CatalogCacheStamp = serde_json::from_slice(&stamp_bytes).map_err(|error| {
        catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            format!("Failed to decode the verified catalog cache stamp: {error}"),
        )
    })?;
    stamp.validate(registry)?;
    let root = read_and_verify_stamped_role(
        &cache_directory,
        "root.json",
        MAX_BOOTSTRAP_ROOT_BYTES,
        &stamp.root_metadata_sha256,
        stamp.root_version,
    )
    .await?;
    read_and_verify_stamped_role(
        &cache_directory,
        "timestamp.json",
        MAX_TIMESTAMP_METADATA_BYTES,
        &stamp.timestamp_metadata_sha256,
        stamp.timestamp_version,
    )
    .await?;
    read_and_verify_stamped_role(
        &cache_directory,
        "snapshot.json",
        MAX_SNAPSHOT_METADATA_BYTES,
        &stamp.snapshot_metadata_sha256,
        stamp.snapshot_version,
    )
    .await?;
    read_and_verify_stamped_role(
        &cache_directory,
        "targets.json",
        MAX_TARGETS_METADATA_BYTES,
        &stamp.targets_metadata_sha256,
        stamp.targets_version,
    )
    .await?;

    let metadata_url = Url::from_directory_path(&cache_directory).map_err(|()| {
        catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            "The registry metadata cache path cannot be represented as a file URL.",
        )
    })?;
    let repository = RepositoryLoader::new(&root, metadata_url.clone(), metadata_url)
        .transport(FilesystemTransport)
        .limits(repository_limits())
        .expiration_enforcement(ExpirationEnforcement::Safe)
        .load()
        .await
        .map_err(|error| {
            catalog_cache_error(
                "use.extension.catalog_cache_untrusted",
                format!(
                    "Cached TUF verification failed for registry '{}': {error}",
                    registry.name()
                ),
            )
        })?;
    verify_stamp_versions(&stamp, &repository)?;
    Ok((repository, stamp))
}

async fn read_and_verify_stamped_role(
    datastore: &Path,
    name: &str,
    max_bytes: u64,
    expected_sha256: &str,
    expected_version: u64,
) -> UseResult<Vec<u8>> {
    let path = datastore.join(name);
    let bytes = read_metadata_file(&path, max_bytes).await?;
    if raw_sha256(&bytes) != expected_sha256
        || signed_role_version(&bytes, name)? != expected_version
    {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_changed",
            format!("Cached TUF role '{name}' changed after its last verified refresh."),
        ));
    }
    Ok(bytes)
}

async fn ensure_cache_directory(datastore: &Path) -> UseResult<std::path::PathBuf> {
    let path = datastore.join(CATALOG_CACHE_DIRECTORY);
    fs::create_dir_all(&path)
        .await
        .map_err(|error| io_error("create verified catalog snapshot directory", &path, error))?;
    let metadata = fs::symlink_metadata(&path)
        .await
        .map_err(|error| io_error("inspect verified catalog snapshot directory", &path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            "The verified catalog snapshot path must be a real directory.",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|error| {
                io_error("secure verified catalog snapshot directory", &path, error)
            })?;
    }
    Ok(path)
}

async fn download_metadata_role(url: &Url, max_bytes: u64, role: &str) -> UseResult<Vec<u8>> {
    validate_download_url(url)?;
    let client = reqwest::Client::builder()
        .user_agent("a3s-use-extension/0.2")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|error| {
            catalog_cache_error(
                "use.extension.registry_download_failed",
                format!("Failed to build the catalog metadata client: {error}"),
            )
        })?;
    let mut response = client.get(url.clone()).send().await.map_err(|error| {
        catalog_cache_error(
            "use.extension.registry_download_failed",
            format!("Failed to download {role} metadata: {error}"),
        )
    })?;
    validate_download_url(response.url())?;
    if !response.status().is_success() {
        return Err(catalog_cache_error(
            "use.extension.registry_download_failed",
            format!(
                "Catalog {role} metadata download returned HTTP {}.",
                response.status()
            ),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length == 0 || length > max_bytes)
    {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            format!("Catalog {role} metadata exceeds its download bound."),
        ));
    }
    let mut bytes =
        Vec::with_capacity(response.content_length().unwrap_or_default().min(max_bytes) as usize);
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        catalog_cache_error(
            "use.extension.registry_download_failed",
            format!("Failed to read {role} metadata: {error}"),
        )
    })? {
        if bytes.len().saturating_add(chunk.len()) as u64 > max_bytes {
            return Err(catalog_cache_error(
                "use.extension.catalog_cache_invalid",
                format!("Catalog {role} metadata exceeds its download bound."),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            format!("Catalog {role} metadata is empty."),
        ));
    }
    Ok(bytes)
}

fn verify_role_matches<T: Serialize>(bytes: &[u8], expected: &T, role: &str) -> UseResult<()> {
    let actual: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            format!("Failed to decode refreshed {role} metadata: {error}"),
        )
    })?;
    let expected = serde_json::to_value(expected).map_err(|error| {
        catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            format!("Failed to encode verified {role} metadata: {error}"),
        )
    })?;
    if actual != expected {
        return Err(catalog_cache_error(
            "use.extension.catalog_refresh_changed",
            format!("Registry {role} metadata changed during refresh; retry against one snapshot."),
        ));
    }
    Ok(())
}

async fn read_metadata_file(path: &Path, max_bytes: u64) -> UseResult<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| io_error("inspect cached TUF metadata", path, error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > max_bytes
    {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            format!(
                "Cached TUF metadata '{}' is not a bounded regular file.",
                path.display()
            ),
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| io_error("read cached TUF metadata", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > max_bytes {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            format!(
                "Cached TUF metadata '{}' exceeds its read bound.",
                path.display()
            ),
        ));
    }
    Ok(bytes)
}

async fn write_cache_stamp(datastore: &Path, stamp: &CatalogCacheStamp) -> UseResult<()> {
    let bytes = serde_json::to_vec(stamp).map_err(|error| {
        catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            format!("Failed to encode the verified catalog cache stamp: {error}"),
        )
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_CATALOG_CACHE_BYTES {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            "The verified catalog cache stamp exceeds its size bound.",
        ));
    }
    write_cache_file(
        &datastore.join(CATALOG_CACHE_NAME),
        &bytes,
        "catalog cache stamp",
    )
    .await
}

async fn write_cache_file(path: &Path, bytes: &[u8], label: &str) -> UseResult<()> {
    let datastore = path.parent().ok_or_else(|| {
        catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            "The catalog cache file has no parent directory.",
        )
    })?;
    let temporary = datastore.join(format!(".catalog-cache-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| io_error(&format!("create {label}"), &temporary, error))?;
    if let Err(error) = file.write_all(bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(&format!("write {label}"), &temporary, error));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(io_error(&format!("sync {label}"), &temporary, error));
    }
    drop(file);
    if let Err(error) = activate_temporary_file(
        temporary.clone(),
        path.to_path_buf(),
        "activate catalog cache file",
    )
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent_directory(datastore, "catalog cache").await
}

impl CatalogCacheStamp {
    fn validate(&self, registry: &TrustedRegistry) -> UseResult<()> {
        if self.schema != CATALOG_CACHE_SCHEMA
            || self.registry_name != registry.name()
            || self.registry_url != registry.base_url().as_str()
            || self.root_sha256 != registry.root_sha256()
            || self.root_version == 0
            || self.timestamp_version == 0
            || self.snapshot_version == 0
            || self.targets_version == 0
            || !valid_raw_sha256(&self.root_metadata_sha256)
            || !valid_raw_sha256(&self.timestamp_metadata_sha256)
            || !valid_raw_sha256(&self.snapshot_metadata_sha256)
            || !valid_raw_sha256(&self.targets_metadata_sha256)
            || self.verified_at_unix_seconds == 0
        {
            return Err(catalog_cache_error(
                "use.extension.catalog_cache_invalid",
                "The verified catalog cache stamp does not match this registry.",
            ));
        }
        Ok(())
    }

    pub(super) fn snapshot_digest(&self) -> String {
        let mut hasher = Sha256::new();
        update_digest_field(&mut hasher, &self.registry_name);
        update_digest_field(&mut hasher, &self.registry_url);
        update_digest_field(&mut hasher, &self.root_sha256);
        hasher.update(self.root_version.to_be_bytes());
        hasher.update(self.timestamp_version.to_be_bytes());
        hasher.update(self.snapshot_version.to_be_bytes());
        hasher.update(self.targets_version.to_be_bytes());
        update_digest_field(&mut hasher, &self.timestamp_metadata_sha256);
        update_digest_field(&mut hasher, &self.snapshot_metadata_sha256);
        update_digest_field(&mut hasher, &self.targets_metadata_sha256);
        format!("sha256:{:x}", hasher.finalize())
    }
}

pub(super) fn verify_stamp_metadata(
    stamp: &CatalogCacheStamp,
    metadata: &VerifiedRegistryMetadata,
) -> UseResult<()> {
    if stamp.registry_name != metadata.registry_name
        || stamp.registry_url != metadata.registry_url
        || stamp.root_sha256 != metadata.root_sha256
        || stamp.root_version != metadata.root_version
        || stamp.timestamp_version != metadata.timestamp_version
        || stamp.snapshot_version != metadata.snapshot_version
        || stamp.targets_version != metadata.targets_version
    {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_changed",
            "The cached catalog provenance changed after its last verified refresh.",
        ));
    }
    Ok(())
}

fn verify_stamp_versions(stamp: &CatalogCacheStamp, repository: &Repository) -> UseResult<()> {
    if stamp.root_version != repository.root().signed.version.get()
        || stamp.timestamp_version != repository.timestamp().signed.version.get()
        || stamp.snapshot_version != repository.snapshot().signed.version.get()
        || stamp.targets_version != repository.targets().signed.version.get()
    {
        return Err(catalog_cache_error(
            "use.extension.catalog_cache_changed",
            "The cached TUF role versions changed after catalog verification.",
        ));
    }
    Ok(())
}

fn signed_role_version(bytes: &[u8], role: &str) -> UseResult<u64> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        catalog_cache_error(
            "use.extension.catalog_cache_invalid",
            format!("Failed to decode cached TUF role '{role}': {error}"),
        )
    })?;
    value
        .get("signed")
        .and_then(|signed| signed.get("version"))
        .and_then(serde_json::Value::as_u64)
        .filter(|version| *version > 0)
        .ok_or_else(|| {
            catalog_cache_error(
                "use.extension.catalog_cache_invalid",
                format!("Cached TUF role '{role}' has no valid signed version."),
            )
        })
}

fn repository_limits() -> Limits {
    Limits {
        max_root_size: MAX_BOOTSTRAP_ROOT_BYTES,
        max_targets_size: MAX_TARGETS_METADATA_BYTES,
        max_timestamp_size: MAX_TIMESTAMP_METADATA_BYTES,
        max_snapshot_size: MAX_SNAPSHOT_METADATA_BYTES,
        max_root_updates: MAX_ROOT_UPDATES,
    }
}

fn raw_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn update_digest_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

pub(super) fn unix_time_seconds() -> UseResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| {
            catalog_cache_error(
                "use.extension.catalog_cache_invalid",
                format!("The system clock is before the Unix epoch: {error}"),
            )
        })
}

fn valid_raw_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
