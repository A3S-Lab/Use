use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

pub(super) async fn append_durable_line(path: &Path, value: &str) -> io::Result<()> {
    let parent = parent(path)?;
    tokio::fs::create_dir_all(parent).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(value.as_bytes()).await?;
    file.sync_all().await
}

pub(super) async fn read_optional_json<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) async fn write_new_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let parent = parent(path)?;
    tokio::fs::create_dir_all(parent).await?;
    let bytes = json_bytes(value)?;
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await?;
    file.write_all(&bytes).await?;
    file.sync_all().await?;
    drop(file);
    sync_test_parent(parent).await
}

pub(super) async fn replace_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = json_bytes(value)?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .await?;
    file.write_all(&bytes).await?;
    file.sync_all().await
}

#[cfg(unix)]
pub(super) async fn sync_test_parent(parent: &Path) -> io::Result<()> {
    tokio::fs::File::open(parent).await?.sync_all().await
}

#[cfg(not(unix))]
pub(super) async fn sync_test_parent(_parent: &Path) -> io::Result<()> {
    Ok(())
}

fn parent(path: &Path) -> io::Result<&Path> {
    path.parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent"))
}

fn json_bytes<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
