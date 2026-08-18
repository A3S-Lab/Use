use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const VERSION: &str = "9.8.7";
const SIGSTORE_BUNDLE: &[u8] = b"{\"fixture\":true}\n";

#[test]
fn release_workflow_publishes_both_verified_installers() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let ci = include_str!("../.github/workflows/ci.yml");
    let unix = include_str!("../install.sh");
    let windows = include_str!("../install.ps1");

    for installer in ["install.sh", "install.ps1"] {
        assert!(
            workflow.contains(installer),
            "release workflow must publish {installer}"
        );
        assert!(
            workflow.contains(&format!("artifacts/{installer}")),
            "release assets must include {installer}"
        );
    }
    assert!(workflow.contains("checksums.txt.sigstore.json"));
    assert!(workflow.contains("export A3S_USE_OCR_HOME=\"${install_root}/ocr-models\""));
    assert!(workflow.contains("$env:A3S_USE_OCR_HOME = \"$root/ocr-models\""));
    assert!(unix.contains("checksums.txt"));
    assert!(windows.contains("checksums.txt"));
    for installer in [unix, windows] {
        assert!(installer.contains("checksums.txt.sigstore.json"));
        assert!(installer.contains("verify-blob"));
        assert!(installer.contains("https://token.actions.githubusercontent.com"));
        assert!(installer.contains(".github/workflows/release.yml@refs/tags/"));
    }
    assert!(ci.contains("cargo test -p a3s-use --test release_installers --locked"));
    assert!(!unix.to_ascii_lowercase().contains("science"));
    assert!(!windows.to_ascii_lowercase().contains("science"));
}

#[cfg(unix)]
#[test]
fn unix_installer_verifies_and_atomically_activates_the_release() {
    let Some(archive_name) = unix_archive_name() else {
        return;
    };
    let archive = unix_fixture_archive();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive.clone(), &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use root");
    let bin_dir = temp.path().join("bin");

    let output = run_unix_installer(&server, &install_root, &bin_dir);
    assert_success(&output);

    let release_root = install_root.join("releases").join(VERSION);
    let executable = release_root.join("a3s-use");
    let launcher = release_root.join("a3s-use-launcher");
    let shim = bin_dir.join("a3s-use");
    assert!(executable.is_file());
    assert!(launcher.is_file());
    assert!(release_root.join("a3s-use-browser-driver").is_file());
    assert_eq!(
        fs::read_to_string(release_root.join(".a3s-use-archive.sha256")).unwrap(),
        format!("{digest}\n")
    );
    assert_eq!(
        fs::read(release_root.join(".a3s-use-checksums.sigstore.json")).unwrap(),
        SIGSTORE_BUNDLE
    );
    assert_eq!(
        fs::read_to_string(release_root.join(".a3s-use-checksums.txt")).unwrap(),
        format!("{digest}  {archive_name}\n")
    );
    assert!(fs::symlink_metadata(&shim)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::canonicalize(&shim).unwrap(),
        fs::canonicalize(launcher).unwrap()
    );
    let launched = Command::new(&shim).output().unwrap();
    assert_success(&launched);
    let environment = String::from_utf8(launched.stdout).unwrap();
    let canonical_release_root = fs::canonicalize(&release_root).unwrap();
    assert_eq!(
        environment.lines().collect::<Vec<_>>(),
        [
            canonical_release_root.join("ocr-models").to_str().unwrap(),
            canonical_release_root.join("ocr-skills").to_str().unwrap(),
            canonical_release_root.join("skill-data").to_str().unwrap(),
        ]
    );
    let overridden = Command::new(&shim)
        .env("A3S_USE_OCR_HOME", "/operator/ocr")
        .output()
        .unwrap();
    assert_success(&overridden);
    assert_eq!(
        String::from_utf8(overridden.stdout).unwrap().lines().next(),
        Some("/operator/ocr")
    );

    let retry_server = release_server(&archive_name, archive.clone(), &digest);
    assert_success(&run_unix_installer(&retry_server, &install_root, &bin_dir));

    fs::write(release_root.join("README.md"), b"tampered\n").unwrap();
    let tamper_server = release_server(&archive_name, archive, &digest);
    let output = run_unix_installer(&tamper_server, &install_root, &bin_dir);
    assert!(
        !output.status.success(),
        "tampered installation was accepted"
    );
}

#[cfg(unix)]
#[test]
fn unix_installer_rejects_a_checksum_mismatch_without_activation() {
    let Some(archive_name) = unix_archive_name() else {
        return;
    };
    let archive = unix_fixture_archive();
    let server = release_server(&archive_name, archive, &"0".repeat(64));
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");

    let output = run_unix_installer(&server, &install_root, &bin_dir);
    assert!(!output.status.success(), "installer unexpectedly succeeded");
    assert!(!install_root.join("releases").join(VERSION).exists());
    assert!(!bin_dir.join("a3s-use").exists());
}

#[cfg(unix)]
#[test]
fn unix_installer_rejects_invalid_sigstore_evidence_without_activation() {
    let Some(archive_name) = unix_archive_name() else {
        return;
    };
    let archive = unix_fixture_archive();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive, &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");
    let cosign = write_fake_cosign(temp.path(), false);

    let output = run_unix_installer_with_cosign(&server, &install_root, &bin_dir, &cosign);
    assert!(
        !output.status.success(),
        "installer accepted invalid evidence"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Sigstore"));
    assert!(!install_root.join("releases").join(VERSION).exists());
    assert!(!bin_dir.join("a3s-use").exists());
}

#[cfg(unix)]
#[test]
fn unix_installer_requires_a_cosign_verifier() {
    let Some(archive_name) = unix_archive_name() else {
        return;
    };
    let archive = unix_fixture_archive();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive, &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");
    let missing_cosign = temp.path().join("missing-cosign");

    let output = run_unix_installer_with_cosign(&server, &install_root, &bin_dir, &missing_cosign);
    assert!(!output.status.success(), "installer ran without Cosign");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cosign is required"));
    assert!(!install_root.join("releases").join(VERSION).exists());
}

#[cfg(unix)]
#[test]
fn unix_installer_refuses_to_replace_an_unmanaged_command() {
    let Some(archive_name) = unix_archive_name() else {
        return;
    };
    let archive = unix_fixture_archive();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive, &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    fs::write(bin_dir.join("a3s-use"), b"user-owned\n").unwrap();

    let output = run_unix_installer(&server, &install_root, &bin_dir);
    assert!(
        !output.status.success(),
        "installer replaced an unmanaged file"
    );
    assert_eq!(fs::read(bin_dir.join("a3s-use")).unwrap(), b"user-owned\n");
    assert!(!install_root.join("releases").join(VERSION).exists());
}

#[cfg(unix)]
#[test]
fn unix_installer_rejects_link_entries_before_extraction() {
    let Some(archive_name) = unix_archive_name() else {
        return;
    };
    let archive = unix_fixture_archive_with_link();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive, &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");

    let output = run_unix_installer(&server, &install_root, &bin_dir);
    assert!(!output.status.success(), "installer accepted a link entry");
    assert!(!install_root.join("releases").join(VERSION).exists());
    assert!(!bin_dir.join("a3s-use").exists());
}

#[cfg(windows)]
#[test]
fn windows_installer_verifies_and_atomically_activates_the_release() {
    let archive_name = format!("a3s-use-{VERSION}-windows-x86_64.zip");
    let archive = windows_fixture_archive();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive.clone(), &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use root");
    let bin_dir = temp.path().join("bin");

    let output = run_windows_installer(&server, &install_root, &bin_dir);
    assert_success(&output);

    let release_root = install_root.join("releases").join(VERSION);
    assert!(release_root.join("a3s-use.exe").is_file());
    assert!(release_root.join("a3s-use-browser-driver.exe").is_file());
    assert_eq!(
        fs::read_to_string(release_root.join(".a3s-use-archive.sha256")).unwrap(),
        format!("{digest}\r\n")
    );
    assert_eq!(
        fs::read(release_root.join(".a3s-use-checksums.sigstore.json")).unwrap(),
        SIGSTORE_BUNDLE
    );
    assert_eq!(
        fs::read_to_string(release_root.join(".a3s-use-checksums.txt")).unwrap(),
        format!("{digest}  {archive_name}\n")
    );
    let shim = fs::read_to_string(bin_dir.join("a3s-use.cmd")).unwrap();
    assert!(shim.contains("A3S_USE_MANAGED_SHIM=1"));
    assert_windows_shim_target(&shim, &release_root.join("a3s-use.exe"));
    assert_windows_shim_environment_path(
        &shim,
        "A3S_USE_OCR_HOME",
        &release_root.join("ocr-models"),
    );
    assert_windows_shim_environment_path(
        &shim,
        "A3S_USE_OCR_SKILLS_DIR",
        &release_root.join("ocr-skills"),
    );
    assert_windows_shim_environment_path(
        &shim,
        "A3S_USE_BROWSER_SKILLS_DIR",
        &release_root.join("skill-data"),
    );

    let retry_server = release_server(&archive_name, archive.clone(), &digest);
    assert_success(&run_windows_installer(
        &retry_server,
        &install_root,
        &bin_dir,
    ));
    assert_eq!(
        fs::read_dir(&bin_dir).unwrap().count(),
        1,
        "managed shim replacement left a temporary or backup file"
    );

    fs::write(release_root.join("README.md"), b"tampered\r\n").unwrap();
    let tamper_server = release_server(&archive_name, archive, &digest);
    let output = run_windows_installer(&tamper_server, &install_root, &bin_dir);
    assert!(
        !output.status.success(),
        "tampered installation was accepted"
    );
}

#[cfg(windows)]
#[test]
fn windows_installer_rejects_a_checksum_mismatch_without_activation() {
    let archive_name = format!("a3s-use-{VERSION}-windows-x86_64.zip");
    let archive = windows_fixture_archive();
    let server = release_server(&archive_name, archive, &"0".repeat(64));
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");

    let output = run_windows_installer(&server, &install_root, &bin_dir);
    assert!(!output.status.success(), "installer unexpectedly succeeded");
    assert!(!install_root.join("releases").join(VERSION).exists());
    assert!(!bin_dir.join("a3s-use.cmd").exists());
}

#[cfg(windows)]
#[test]
fn windows_installer_rejects_invalid_sigstore_evidence_without_activation() {
    let archive_name = format!("a3s-use-{VERSION}-windows-x86_64.zip");
    let archive = windows_fixture_archive();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive, &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");
    let cosign = write_fake_cosign(temp.path(), false);

    let output = run_windows_installer_with_cosign(&server, &install_root, &bin_dir, &cosign);
    assert!(
        !output.status.success(),
        "installer accepted invalid evidence"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Sigstore"),
        "installer returned an unexpected failure\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!install_root.join("releases").join(VERSION).exists());
    assert!(!bin_dir.join("a3s-use.cmd").exists());
}

#[cfg(windows)]
#[test]
fn windows_installer_requires_a_cosign_verifier() {
    let archive_name = format!("a3s-use-{VERSION}-windows-x86_64.zip");
    let archive = windows_fixture_archive();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive, &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");
    let missing_cosign = temp.path().join("missing-cosign.exe");

    let output =
        run_windows_installer_with_cosign(&server, &install_root, &bin_dir, &missing_cosign);
    assert!(!output.status.success(), "installer ran without Cosign");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cosign is required"));
    assert!(!install_root.join("releases").join(VERSION).exists());
}

#[cfg(windows)]
#[test]
fn windows_installer_refuses_to_replace_an_unmanaged_command() {
    let archive_name = format!("a3s-use-{VERSION}-windows-x86_64.zip");
    let archive = windows_fixture_archive();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive, &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    fs::write(bin_dir.join("a3s-use.cmd"), b"user-owned\r\n").unwrap();

    let output = run_windows_installer(&server, &install_root, &bin_dir);
    assert!(
        !output.status.success(),
        "installer replaced an unmanaged file"
    );
    assert_eq!(
        fs::read(bin_dir.join("a3s-use.cmd")).unwrap(),
        b"user-owned\r\n"
    );
    assert!(!install_root.join("releases").join(VERSION).exists());
}

#[cfg(windows)]
#[test]
fn windows_installer_rejects_parent_traversal_before_extraction() {
    let archive_name = format!("a3s-use-{VERSION}-windows-x86_64.zip");
    let archive = windows_fixture_archive_with_parent_traversal();
    let digest = sha256_hex(&archive);
    let server = release_server(&archive_name, archive, &digest);
    let temp = tempfile::tempdir().unwrap();
    let install_root = temp.path().join("use");
    let bin_dir = temp.path().join("bin");

    let output = run_windows_installer(&server, &install_root, &bin_dir);
    assert!(
        !output.status.success(),
        "installer accepted a parent-traversal entry"
    );
    assert!(!install_root.join("releases").join(VERSION).exists());
    assert!(!bin_dir.join("a3s-use.cmd").exists());
}

#[cfg(unix)]
fn run_unix_installer(server: &TestServer, install_root: &Path, bin_dir: &Path) -> Output {
    let cosign = write_fake_cosign(install_root.parent().unwrap(), true);
    run_unix_installer_with_cosign(server, install_root, bin_dir, &cosign)
}

#[cfg(unix)]
fn run_unix_installer_with_cosign(
    server: &TestServer,
    install_root: &Path,
    bin_dir: &Path,
    cosign: &Path,
) -> Output {
    Command::new("sh")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"))
        .arg("--version")
        .arg(VERSION)
        .arg("--base-url")
        .arg(server.base_url())
        .arg("--install-root")
        .arg(install_root)
        .arg("--bin-dir")
        .arg(bin_dir)
        .arg("--cosign")
        .arg(cosign)
        .output()
        .unwrap()
}

#[cfg(windows)]
fn run_windows_installer(server: &TestServer, install_root: &Path, bin_dir: &Path) -> Output {
    let cosign = write_fake_cosign(install_root.parent().unwrap(), true);
    run_windows_installer_with_cosign(server, install_root, bin_dir, &cosign)
}

#[cfg(windows)]
fn run_windows_installer_with_cosign(
    server: &TestServer,
    install_root: &Path,
    bin_dir: &Path,
    cosign: &Path,
) -> Output {
    Command::new("pwsh")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.ps1"))
        .arg("-Version")
        .arg(VERSION)
        .arg("-BaseUrl")
        .arg(server.base_url())
        .arg("-InstallRoot")
        .arg(install_root)
        .arg("-BinDir")
        .arg(bin_dir)
        .arg("-CosignPath")
        .arg(cosign)
        .arg("-NoPathUpdate")
        .output()
        .unwrap()
}

#[cfg(unix)]
fn write_fake_cosign(root: &Path, accepts_evidence: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = root.join(if accepts_evidence {
        "cosign-success"
    } else {
        "cosign-failure"
    });
    let status = if accepts_evidence { 0 } else { 1 };
    let script = format!(
        "#!/bin/sh\n\
         [ \"$#\" -eq 8 ] || exit 64\n\
         [ \"$1\" = verify-blob ] || exit 64\n\
         [ \"$2\" = --bundle ] || exit 64\n\
         [ -s \"$3\" ] || exit 64\n\
         [ \"$4\" = --certificate-identity ] || exit 64\n\
         [ \"$5\" = \"https://github.com/A3S-Lab/Use/.github/workflows/release.yml@refs/tags/v{VERSION}\" ] || exit 64\n\
         [ \"$6\" = --certificate-oidc-issuer ] || exit 64\n\
         [ \"$7\" = \"https://token.actions.githubusercontent.com\" ] || exit 64\n\
         [ -s \"$8\" ] || exit 64\n\
         exit {status}\n"
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(windows)]
fn write_fake_cosign(root: &Path, accepts_evidence: bool) -> PathBuf {
    let path = root.join(if accepts_evidence {
        "cosign-success.cmd"
    } else {
        "cosign-failure.cmd"
    });
    let status = if accepts_evidence { 0 } else { 1 };
    let script = format!(
        "@echo off\r\n\
         if not \"%~1\"==\"verify-blob\" exit /b 64\r\n\
         if not \"%~2\"==\"--bundle\" exit /b 64\r\n\
         if not exist \"%~3\" exit /b 64\r\n\
         if not \"%~4\"==\"--certificate-identity\" exit /b 64\r\n\
         if not \"%~5\"==\"https://github.com/A3S-Lab/Use/.github/workflows/release.yml@refs/tags/v{VERSION}\" exit /b 64\r\n\
         if not \"%~6\"==\"--certificate-oidc-issuer\" exit /b 64\r\n\
         if not \"%~7\"==\"https://token.actions.githubusercontent.com\" exit /b 64\r\n\
         if not exist \"%~8\" exit /b 64\r\n\
         exit /b {status}\r\n"
    );
    fs::write(&path, script).unwrap();
    path
}

#[cfg(windows)]
fn assert_windows_shim_target(shim: &str, expected: &Path) {
    let line = shim
        .lines()
        .find(|line| line.starts_with('"') && line.ends_with("\" %*"))
        .expect("managed shim must invoke the installed executable");
    let actual = line
        .strip_suffix(" %*")
        .and_then(|value| value.strip_prefix('"'))
        .and_then(|value| value.strip_suffix('"'))
        .expect("managed shim executable must be quoted");
    assert_same_windows_path(actual, expected);
}

#[cfg(windows)]
fn assert_windows_shim_environment_path(shim: &str, variable: &str, expected: &Path) {
    let prefix = format!("if not defined {variable} set \"{variable}=");
    let actual = shim
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or_else(|| panic!("managed shim must initialize {variable}"));
    assert_same_windows_path(actual, expected);
}

#[cfg(windows)]
fn assert_same_windows_path(actual: &str, expected: &Path) {
    let actual = fs::canonicalize(actual)
        .unwrap_or_else(|error| panic!("shim path {actual:?} cannot be resolved: {error}"));
    let expected = fs::canonicalize(expected)
        .unwrap_or_else(|error| panic!("expected path {expected:?} cannot be resolved: {error}"));
    assert_eq!(actual, expected);
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "installer failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
fn unix_archive_name() -> Option<String> {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "x86_64") => "darwin-x86_64",
        ("macos", "aarch64") => "darwin-arm64",
        _ => return None,
    };
    Some(format!("a3s-use-{VERSION}-{platform}.tar.gz"))
}

#[cfg(unix)]
fn unix_fixture_archive() -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    append_tar_file(
        &mut archive,
        "a3s-use",
        0o755,
        b"#!/bin/sh\nprintf '%s\\n' \"$A3S_USE_OCR_HOME\" \"$A3S_USE_OCR_SKILLS_DIR\" \"$A3S_USE_BROWSER_SKILLS_DIR\"\n",
    );
    append_tar_file(
        &mut archive,
        "a3s-use-browser-driver",
        0o755,
        b"#!/bin/sh\nexit 0\n",
    );
    append_tar_file(&mut archive, "README.md", 0o644, b"fixture\n");
    for path in required_release_files() {
        append_tar_file(&mut archive, path, 0o644, b"fixture\n");
    }
    archive.finish().unwrap();
    archive.into_inner().unwrap().finish().unwrap()
}

#[cfg(unix)]
fn unix_fixture_archive_with_link() -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    append_tar_file(&mut archive, "a3s-use", 0o755, b"#!/bin/sh\nexit 0\n");
    append_tar_file(
        &mut archive,
        "a3s-use-browser-driver",
        0o755,
        b"#!/bin/sh\nexit 0\n",
    );
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_path("unsafe-link").unwrap();
    link.set_link_name("../outside").unwrap();
    link.set_size(0);
    link.set_mtime(0);
    link.set_cksum();
    archive.append(&link, std::io::empty()).unwrap();
    archive.finish().unwrap();
    archive.into_inner().unwrap().finish().unwrap()
}

#[cfg(unix)]
fn append_tar_file<W: Write>(archive: &mut tar::Builder<W>, path: &str, mode: u32, body: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_path(path).unwrap();
    header.set_size(body.len() as u64);
    header.set_mode(mode);
    header.set_mtime(0);
    header.set_cksum();
    archive.append(&header, body).unwrap();
}

#[cfg(windows)]
fn windows_fixture_archive() -> Vec<u8> {
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    let output = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(output);
    let options = SimpleFileOptions::default();
    archive.start_file("a3s-use.exe", options).unwrap();
    archive.write_all(b"fixture executable").unwrap();
    archive
        .start_file("a3s-use-browser-driver.exe", options)
        .unwrap();
    archive.write_all(b"fixture driver").unwrap();
    archive.start_file("README.md", options).unwrap();
    archive.write_all(b"fixture\n").unwrap();
    for path in required_release_files() {
        archive.start_file(path, options).unwrap();
        archive.write_all(b"fixture\n").unwrap();
    }
    archive.finish().unwrap().into_inner()
}

fn required_release_files() -> &'static [&'static str] {
    &[
        "skills/a3s-use-browser/SKILL.md",
        "skill-data/core/SKILL.md",
        "ocr-skills/a3s-use-ocr/SKILL.md",
        "ocr-models/PP-OCRv6_small/det/inference.onnx",
        "ocr-models/PP-OCRv6_small/det/inference.yml",
        "ocr-models/PP-OCRv6_small/rec/inference.onnx",
        "ocr-models/PP-OCRv6_small/rec/inference.yml",
        "dashboard/index.html",
    ]
}

#[cfg(windows)]
fn windows_fixture_archive_with_parent_traversal() -> Vec<u8> {
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;

    let output = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(output);
    let options = SimpleFileOptions::default();
    archive.start_file("a3s-use.exe", options).unwrap();
    archive.write_all(b"fixture executable").unwrap();
    archive
        .start_file("a3s-use-browser-driver.exe", options)
        .unwrap();
    archive.write_all(b"fixture driver").unwrap();
    archive.start_file("../escape.txt", options).unwrap();
    archive.write_all(b"escape").unwrap();
    archive.finish().unwrap().into_inner()
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn release_server(archive_name: &str, archive: Vec<u8>, digest: &str) -> TestServer {
    let mut files = HashMap::new();
    let prefix = format!("/v{VERSION}");
    files.insert(
        format!("{prefix}/checksums.txt"),
        format!("{digest}  {archive_name}\n").into_bytes(),
    );
    files.insert(
        format!("{prefix}/checksums.txt.sigstore.json"),
        SIGSTORE_BUNDLE.to_vec(),
    );
    files.insert(format!("{prefix}/{archive_name}"), archive);
    TestServer::start(files)
}

struct TestServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    fn start(files: HashMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => serve_request(&mut stream, &files),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture server failed: {error}"),
                }
            }
        });
        Self {
            base_url: format!("http://{address}"),
            stop,
            thread: Some(thread),
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.base_url.trim_start_matches("http://"));
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn serve_request(stream: &mut TcpStream, files: &HashMap<String, Vec<u8>>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while request.len() < 16 * 1024 && !request.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let request = String::from_utf8_lossy(&request);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let (status, body): (&str, &[u8]) = match files.get(path) {
        Some(body) => ("200 OK", body),
        None => ("404 Not Found", b"not found"),
    };
    if write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .is_err()
    {
        return;
    }
    let _ = stream.write_all(body);
}
