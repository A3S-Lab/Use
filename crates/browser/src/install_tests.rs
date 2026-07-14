use super::*;

#[test]
fn only_approved_https_sources_are_accepted() {
    assert!(trusted_https_url("https://example.com/runtime", &["example.com"]).is_ok());
    assert!(trusted_https_url("http://example.com/runtime", &["example.com"]).is_err());
    assert!(trusted_https_url("https://evil.example/runtime", &["example.com"]).is_err());
}

#[test]
fn version_identifiers_cannot_escape_the_install_root() {
    assert_eq!(validate_version_segment("v1.2.3").unwrap(), "v1.2.3");
    assert!(validate_version_segment("../../escape").is_err());
    assert!(validate_version_segment("version/name").is_err());
}

#[test]
fn published_digest_must_be_a_well_formed_sha256() {
    let digest = format!("sha256:{}", "A1".repeat(32));
    assert_eq!(parse_published_sha256(&digest).unwrap(), "a1".repeat(32));
    assert!(parse_published_sha256("md5:fixture").is_err());
    assert!(parse_published_sha256("sha256:not-hex").is_err());
}

#[tokio::test]
async fn activation_replaces_a_complete_directory_without_partial_visibility() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("v1");
    let stage = create_stage(temp.path(), "fixture").await.unwrap();
    tokio::fs::create_dir(&target).await.unwrap();
    tokio::fs::write(target.join("value"), b"old")
        .await
        .unwrap();
    tokio::fs::write(stage.join("value"), b"new").await.unwrap();

    activate_directory(&stage, &target).await.unwrap();

    assert_eq!(tokio::fs::read(target.join("value")).await.unwrap(), b"new");
    assert!(!tokio::fs::try_exists(&stage).await.unwrap());
}

#[tokio::test]
async fn interrupted_activation_recovers_the_previous_complete_install() {
    let temp = tempfile::tempdir().unwrap();
    let backup = temp.path().join(".a3s-backup-v1-fixture");
    tokio::fs::create_dir(&backup).await.unwrap();
    tokio::fs::write(backup.join("runtime"), b"previous")
        .await
        .unwrap();
    write_receipt(
        &backup,
        &ManagedInstallReceipt {
            schema_version: 1,
            provider: "fixture".to_string(),
            version: "v1".to_string(),
            source_url: "https://example.com/runtime".to_string(),
            artifact_sha256: "a".repeat(64),
            artifact_bytes: 8,
            executable_sha256: "b".repeat(64),
            integrity_policy: "fixture".to_string(),
        },
    )
    .await
    .unwrap();

    cleanup_stale_stages(temp.path()).await.unwrap();

    assert_eq!(
        tokio::fs::read(temp.path().join("v1/runtime"))
            .await
            .unwrap(),
        b"previous"
    );
    assert!(!tokio::fs::try_exists(&backup).await.unwrap());
}
