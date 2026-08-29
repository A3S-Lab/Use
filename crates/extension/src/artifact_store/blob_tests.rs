use super::*;

#[test]
fn artifact_blob_handle_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ArtifactBlob>();
}

#[test]
fn blob_paths_are_global_typed_and_sharded() {
    let store = ArtifactStore::from_data_root(Path::new("/data/use"));
    let sha256 = "cd".repeat(32);
    assert_eq!(
        store.blob_path(&format!("sha256:{sha256}")).unwrap(),
        PathBuf::from(format!(
            "/data/use/artifacts/blobs/sha256/cd/{sha256}/content"
        ))
    );
    assert_eq!(
        store.blob_path(&sha256).unwrap_err().code,
        "use.artifact_store.digest_invalid"
    );
}

#[tokio::test]
async fn identical_commits_converge_on_one_global_blob() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let body = b"one globally shared verified target";
    let sha256 = format!("{:x}", Sha256::digest(body));
    let first_path = temporary.path().join("first.part");
    let second_path = temporary.path().join("second.part");
    fs::write(&first_path, body).await.unwrap();
    fs::write(&second_path, body).await.unwrap();
    let mut first = fs::File::open(&first_path).await.unwrap();
    let mut second = fs::File::open(&second_path).await.unwrap();
    let first_admission = store.acquire_reference_admission().await.unwrap();
    let second_admission = store.acquire_reference_admission().await.unwrap();

    let (first, second) = tokio::join!(
        store.commit_blob(&first_admission, &mut first, body.len() as u64, &sha256),
        store.commit_blob(&second_admission, &mut second, body.len() as u64, &sha256)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.path(), second.path());
    assert_eq!(fs::read(first.path()).await.unwrap(), body);
    let mut entries = fs::read_dir(first.path().parent().unwrap()).await.unwrap();
    let mut content = 0;
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        assert!(!name.starts_with(ARTIFACT_STAGING_PREFIX));
        content += usize::from(name == CONTENT_DIRECTORY);
    }
    assert_eq!(content, 1);
}

#[tokio::test]
async fn corrupted_global_blob_is_never_replaced_during_commit() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let body = b"verified target";
    let sha256 = format!("{:x}", Sha256::digest(body));
    let source_path = temporary.path().join("source.part");
    fs::write(&source_path, body).await.unwrap();
    let mut source = fs::File::open(&source_path).await.unwrap();
    let admission = store.acquire_reference_admission().await.unwrap();
    let blob = store
        .commit_blob(&admission, &mut source, body.len() as u64, &sha256)
        .await
        .unwrap();
    let blob_path = blob.path().to_path_buf();
    drop(blob);
    fs::write(&blob_path, b"corrupt target").await.unwrap();
    let mut source = fs::File::open(&source_path).await.unwrap();

    let error = store
        .commit_blob(&admission, &mut source, body.len() as u64, &sha256)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_store.blob_invalid");
    assert_eq!(fs::read(&blob_path).await.unwrap(), b"corrupt target");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn linked_blob_ancestor_is_rejected_before_external_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    fs::create_dir_all(store.root()).await.unwrap();
    let outside = temporary.path().join("outside");
    fs::create_dir_all(&outside).await.unwrap();
    crate::test_filesystem::create_directory_link(&outside, &store.root().join(BLOBS_DIRECTORY));
    let body = b"verified target";
    let sha256 = format!("{:x}", Sha256::digest(body));
    let source_path = temporary.path().join("source.part");
    fs::write(&source_path, body).await.unwrap();
    let mut source = fs::File::open(&source_path).await.unwrap();
    let admission = store.acquire_reference_admission().await.unwrap();

    let error = store
        .commit_blob(&admission, &mut source, body.len() as u64, &sha256)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.artifact_store.ownership_invalid");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}
