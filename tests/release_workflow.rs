use std::fs;
use std::io::Cursor;
use std::path::Path;
use std::process::{Command, Output};

#[test]
fn release_publishes_only_use_owned_crates_in_dependency_order() {
    let workflow = include_str!("../.github/workflows/release.yml").replace("\r\n", "\n");
    let workflow = workflow.as_str();
    let core = position(workflow, "publish_once a3s-use-core");
    let core_visible = position(workflow, "wait_until_visible a3s-use-core");
    let extension = position(workflow, "publish_once a3s-use-extension");
    let extension_visible = position(workflow, "wait_until_visible a3s-use-extension");
    let facade = position(workflow, "\n          publish_once a3s-use\n");
    let facade_visible = position(workflow, "\n          wait_until_visible a3s-use\n");

    assert!(
        core < core_visible
            && core_visible < extension
            && extension < extension_visible
            && extension_visible < facade
            && facade < facade_visible,
        "release publication order must make every dependency visible before its downstream crate"
    );
    assert!(
        workflow.contains("version=\"$(package_version \"${package}\")\""),
        "each crate must be checked and awaited using its own package version"
    );
    assert!(
        !workflow.contains("publish_once a3s-use-browser"),
        "the independent Browser repository must own Browser crate publication"
    );
    assert!(
        !workflow.contains("publish_once a3s-use-ocr"),
        "the independent OCR repository must own OCR crate publication"
    );
}

#[test]
fn release_waits_for_independent_crates_before_validating_the_facade() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let manifest = include_str!("../Cargo.toml");
    let browser = position(
        workflow,
        "wait_until_visible a3s-use-browser \"$(dependency_version a3s-use-browser)\"",
    );
    let ocr = position(
        workflow,
        "wait_until_visible a3s-use-ocr \"$(dependency_version a3s-use-ocr)\"",
    );
    let validation = position(workflow, "cargo fmt --all -- --check");

    assert!(
        browser < validation && ocr < validation,
        "Browser and OCR must be visible on crates.io before Use validation and packaging"
    );
    assert!(
        manifest.contains("a3s-use-browser = { version = \"=")
            && manifest.contains("a3s-use-ocr = { version = \"="),
        "independent crate dependencies must use exact release versions"
    );
}

#[test]
fn release_assembles_the_same_immutable_browser_revision_as_cargo() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let manifest = include_str!("../Cargo.toml");
    let lock = include_str!("../Cargo.lock");
    let revision = value_after(workflow, "A3S_BROWSER_REVISION: ");

    assert!(workflow.contains("A3S_BROWSER_REPOSITORY: A3S-Lab/Browser"));
    assert!(workflow.contains("repository: ${{ env.A3S_BROWSER_REPOSITORY }}"));
    assert!(workflow.contains("ref: ${{ env.A3S_BROWSER_REVISION }}"));
    assert!(workflow.contains("external/browser/crates/browser-driver/skill-data"));
    assert!(
        manifest.contains(&format!("rev = \"{revision}\"")),
        "Cargo dependency and release driver checkout must use one Browser revision"
    );
    assert!(
        lock.contains(&format!("#{revision}\"")),
        "Cargo.lock must resolve the exact Browser revision"
    );
}

#[test]
fn release_assembles_the_same_immutable_ocr_revision_as_cargo() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let manifest = include_str!("../Cargo.toml");
    let lock = include_str!("../Cargo.lock");
    let revision = value_after(workflow, "A3S_OCR_REVISION: ");

    assert!(workflow.contains("A3S_OCR_REPOSITORY: A3S-Lab/OCR"));
    assert!(workflow.contains("repository: ${{ env.A3S_OCR_REPOSITORY }}"));
    assert!(workflow.contains("ref: ${{ env.A3S_OCR_REVISION }}"));
    assert!(workflow.contains("cp -R external/ocr/skills/."));
    assert!(
        manifest.contains(&format!("rev = \"{revision}\"")),
        "Cargo dependency and release asset checkout must use one OCR revision"
    );
    assert!(
        lock.contains(&format!("#{revision}\"")),
        "Cargo.lock must resolve the exact OCR revision"
    );
}

#[test]
fn release_archives_are_byte_reproducible() {
    let temp = tempfile::tempdir().unwrap();
    let first_source = temp.path().join("first stage");
    let second_source = temp.path().join("second stage");
    write_release_fixture(&first_source, false);
    write_release_fixture(&second_source, true);

    for format in ["tar.gz", "zip"] {
        let first = temp.path().join(format!("first.{format}"));
        let second = temp.path().join(format!("second.{format}"));
        assert_packager_success(&first_source, &first, format);
        assert_packager_success(&second_source, &second, format);
        assert_eq!(
            fs::read(&first).unwrap(),
            fs::read(&second).unwrap(),
            "{format} output changed with source path, creation order, or filesystem timestamps"
        );
        assert_normalized_archive(&first, format);
    }
}

fn assert_normalized_archive(path: &Path, format: &str) {
    if format == "tar.gz" {
        assert_normalized_tar_gz(path);
    } else {
        assert_normalized_zip(path);
    }
}

fn assert_normalized_tar_gz(path: &Path) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..3], &[0x1f, 0x8b, 0x08]);
    assert_eq!(bytes[3] & 0x08, 0, "gzip header embedded an output name");
    assert_eq!(
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
        1_700_000_000
    );

    let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut names = Vec::new();
    for entry in archive.entries().unwrap() {
        let entry = entry.unwrap();
        let header = entry.header();
        let name = entry.path().unwrap().to_string_lossy().into_owned();
        assert_eq!(header.mtime().unwrap(), 1_700_000_000);
        assert_eq!(header.uid().unwrap(), 0);
        assert_eq!(header.gid().unwrap(), 0);
        assert_eq!(
            header.mode().unwrap(),
            if name == "bin/a3s-use.exe" || header.entry_type().is_dir() {
                0o755
            } else {
                0o644
            }
        );
        names.push(name);
    }
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "tar entries are not sorted");
}

fn assert_normalized_zip(path: &Path) {
    let bytes = fs::read(path).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut names = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).unwrap();
        let name = entry.name().to_owned();
        let modified = entry.last_modified().unwrap();
        assert_eq!(
            (
                modified.year(),
                modified.month(),
                modified.day(),
                modified.hour(),
                modified.minute(),
                modified.second(),
            ),
            (2023, 11, 14, 22, 13, 20)
        );
        assert_eq!(
            entry.unix_mode().unwrap() & 0o777,
            if name == "bin/a3s-use.exe" || entry.is_dir() {
                0o755
            } else {
                0o644
            }
        );
        assert!(entry.extra_data().is_none_or(<[u8]>::is_empty));
        names.push(name);
    }
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "ZIP entries are not sorted");
}

#[cfg(unix)]
#[test]
fn release_packager_rejects_links_without_output() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("stage");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("payload"), b"payload\n").unwrap();
    symlink("payload", source.join("linked-payload")).unwrap();
    let output_path = temp.path().join("release.tar.gz");

    let output = run_packager(&source, &output_path, "tar.gz");
    assert!(!output.status.success(), "packager accepted a link");
    assert!(String::from_utf8_lossy(&output.stderr).contains("link or reparse point"));
    assert!(
        !output_path.exists(),
        "failed packaging left an output file"
    );
}

#[cfg(unix)]
#[test]
fn release_packager_rejects_a_link_output_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("stage");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("payload"), b"payload\n").unwrap();
    let target = temp.path().join("existing-release.tar.gz");
    fs::write(&target, b"existing release\n").unwrap();
    let output_path = temp.path().join("release.tar.gz");
    symlink(&target, &output_path).unwrap();

    let output = run_packager(&source, &output_path, "tar.gz");
    assert!(!output.status.success(), "packager accepted a link output");
    assert!(String::from_utf8_lossy(&output.stderr).contains("link or reparse point"));
    assert_eq!(fs::read(&target).unwrap(), b"existing release\n");
    assert!(
        output_path.is_symlink(),
        "packager replaced the output link"
    );
}

#[test]
fn release_packager_rejects_an_output_inside_the_source_tree() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("stage");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("payload"), b"payload\n").unwrap();
    let output_path = source.join("release.tar.gz");

    let output = run_packager(&source, &output_path, "tar.gz");
    assert!(
        !output.status.success(),
        "packager accepted an output inside its source"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("inside --source"));
    assert!(
        !output_path.exists(),
        "failed packaging left an output file"
    );
}

#[test]
fn release_rebuild_verifier_accepts_exact_binaries_and_rejects_drift() {
    let temp = tempfile::tempdir().unwrap();
    let stage = temp.path().join("primary stage");
    let rebuilt = temp.path().join("independent rebuild");
    let subjects = [
        ("a3s-use", b"use binary\n".as_slice()),
        ("a3s-use-browser-driver", b"browser driver\n".as_slice()),
    ];
    for (archive_path, contents) in subjects {
        let primary = stage.join(archive_path);
        let rebuild = rebuilt.join(archive_path.replace('/', "-"));
        fs::create_dir_all(primary.parent().unwrap()).unwrap();
        fs::create_dir_all(rebuild.parent().unwrap()).unwrap();
        fs::write(primary, contents).unwrap();
        fs::write(rebuild, contents).unwrap();
    }

    for format in ["tar.gz", "zip"] {
        let archive = temp.path().join(format!("primary.{format}"));
        assert_packager_success(&stage, &archive, format);
        let first_evidence = temp.path().join(format!("first-{format}.json"));
        let second_evidence = temp.path().join(format!("second-{format}.json"));

        let first = run_rebuild_verifier(&archive, &rebuilt, &first_evidence);
        assert!(
            first.status.success(),
            "rebuild verifier rejected exact {format} binaries: {}",
            String::from_utf8_lossy(&first.stderr)
        );
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&first_evidence).unwrap()).unwrap();
        assert_eq!(value["schema"], "a3s.use.release-rebuild.v1");
        assert_eq!(value["platform"], "test-x86_64");
        assert_eq!(
            value["sourceRevision"],
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(value["subjects"].as_array().unwrap().len(), 2);

        let second = run_rebuild_verifier(&archive, &rebuilt, &second_evidence);
        assert!(second.status.success());
        assert_eq!(
            fs::read(&first_evidence).unwrap(),
            fs::read(&second_evidence).unwrap(),
            "rebuild evidence is not deterministic"
        );

        let drifted = rebuilt.join("a3s-use");
        fs::write(&drifted, b"drifted rebuild\n").unwrap();
        let rejected_evidence = temp.path().join(format!("rejected-{format}.json"));
        let rejected = run_rebuild_verifier(&archive, &rebuilt, &rejected_evidence);
        assert!(!rejected.status.success(), "rebuild drift was accepted");
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("does not match"));
        assert!(!rejected_evidence.exists());
        fs::write(drifted, b"use binary\n").unwrap();

        let archive_before = fs::read(&archive).unwrap();
        let overwrite = run_rebuild_verifier(&archive, &rebuilt, &archive);
        assert!(!overwrite.status.success(), "verifier overwrote its input");
        assert!(String::from_utf8_lossy(&overwrite.stderr).contains("input file"));
        assert_eq!(fs::read(&archive).unwrap(), archive_before);
    }
}

#[test]
fn release_portability_verifier_rejects_repository_local_paths() {
    let temp = tempfile::tempdir().unwrap();
    let release_root = temp.path().join("release");
    let checkout = temp.path().join("checkout");
    fs::create_dir_all(release_root.join("dashboard")).unwrap();
    fs::create_dir_all(&checkout).unwrap();
    fs::write(release_root.join("a3s-use"), b"portable executable\n").unwrap();

    let clean = run_portability_verifier(&release_root, &checkout);
    assert!(
        clean.status.success(),
        "clean release tree was rejected: {}",
        String::from_utf8_lossy(&clean.stderr)
    );

    fs::write(
        release_root.join("dashboard/app.js"),
        format!("sourceRoot={}\n", checkout.display()),
    )
    .unwrap();
    let leaked = run_portability_verifier(&release_root, &checkout);
    assert!(!leaked.status.success());
    assert!(String::from_utf8_lossy(&leaked.stderr).contains("repository-local path"));
}

#[test]
fn release_supply_chain_is_pinned_attested_and_keyless_signed() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let ci = include_str!("../.github/workflows/ci.yml");

    assert!(workflow.contains("scripts/package-release.py"));
    assert!(workflow.contains("SOURCE_DATE_EPOCH"));
    assert!(workflow.contains("source_revision: ${{ steps.version.outputs.source_revision }}"));
    assert!(workflow.contains(
        "ref: ${{ github.event_name == 'workflow_dispatch' && !inputs.qualification && inputs.release_tag || github.sha }}"
    ));
    assert!(!workflow.contains(
        "ref: ${{ github.event_name == 'workflow_dispatch' && inputs.release_tag || github.ref }}"
    ));
    assert!(workflow.contains("test \"${GITHUB_REF_NAME}\" = \"main\""));
    assert!(workflow.contains("git merge-base --is-ancestor"));
    assert!(workflow.contains("test \"${remote_revision}\" = \"${RELEASE_REVISION}\""));
    assert!(!workflow.contains("a3s-use-science"));
    assert!(!workflow.contains("release-bundle-sha256"));
    assert_eq!(
        workflow
            .matches("ref: ${{ needs.validate.outputs.source_revision }}")
            .count(),
        4,
        "every post-validation source checkout must use the frozen commit"
    );
    assert!(workflow.contains("reproducibility:"));
    assert!(workflow.contains("needs: [validate, binaries]"));
    assert!(workflow.contains("scripts/verify-release-rebuild.py"));
    assert!(workflow.contains(".reproducibility.json"));
    assert!(workflow.contains("needs: [validate, binaries, reproducibility, publish-crates]"));
    assert!(workflow.contains("test \"${#release_files[@]}\" -eq 17"));
    let rebuild_job = &workflow
        [position(workflow, "\n  reproducibility:")..position(workflow, "\n  publish-crates:")];
    assert!(
        !rebuild_job.contains("rust-cache"),
        "the independent rebuild must not reuse compiled artifacts"
    );
    assert!(rebuild_job.contains("without a build cache"));
    assert!(rebuild_job.contains("test ! -e target"));
    assert!(rebuild_job.contains("test ! -e external/browser/target"));
    assert!(ci.contains("cargo test -p a3s-use --test release_workflow --locked"));
    assert!(!workflow.contains("tar czf"));
    assert!(!workflow.contains("Compress-Archive"));
    assert!(workflow.contains(".spdx.json"));
    assert!(workflow.contains("anchore/sbom-action@"));
    assert!(workflow.contains("actions/attest@"));
    assert!(workflow.contains("sbom-path:"));
    assert!(workflow.contains("attestations: write"));
    assert!(workflow.contains("artifact-metadata: write"));
    assert!(workflow.contains("sigstore/cosign-installer@"));
    assert!(workflow.contains("cosign sign-blob --yes --bundle"));
    assert!(workflow.contains("cosign verify-blob"));
    assert!(workflow.contains("checksums.txt.sigstore.json"));
    assert!(workflow.contains("certificate-oidc-issuer"));

    for action in workflow.lines().filter_map(|line| {
        let line = line.trim();
        line.strip_prefix("- uses: ")
            .or_else(|| line.strip_prefix("uses: "))
            .filter(|value| !value.starts_with("./"))
    }) {
        let (_, revision) = action
            .rsplit_once('@')
            .unwrap_or_else(|| panic!("action reference has no revision: {action}"));
        assert!(
            revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "release action must use an immutable commit SHA: {action}"
        );
    }
}

#[test]
fn main_release_qualification_builds_and_reproduces_without_publication() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let dispatch =
        &workflow[position(workflow, "  workflow_dispatch:")..position(workflow, "\npermissions:")];
    assert!(dispatch.contains("qualification:"));
    assert!(dispatch.contains("type: boolean"));
    assert!(dispatch.contains("default: false"));
    assert!(dispatch.contains("release_tag:"));
    assert!(dispatch.contains("required: false"));

    let validation =
        &workflow[position(workflow, "  validate:")..position(workflow, "\n  binaries:")];
    assert!(validation.contains("test \"${{ inputs.qualification }}\" = \"true\""));
    assert!(validation.contains("test -z \"${{ inputs.release_tag }}\""));
    assert!(validation.contains("test \"${GITHUB_REF_TYPE}\" = \"branch\""));
    assert!(validation.contains("test \"${GITHUB_REF_NAME}\" = \"main\""));

    let publish =
        &workflow[position(workflow, "\n  publish-crates:")..position(workflow, "\n  release:")];
    let release = &workflow[position(workflow, "\n  release:")..];
    let publication_guard =
        "if: ${{ github.event_name != 'workflow_dispatch' || !inputs.qualification }}";
    assert!(publish.contains(publication_guard));
    assert!(release.contains(publication_guard));
}

#[test]
fn release_builds_remove_nondeterministic_native_metadata() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let binaries_job =
        &workflow[position(workflow, "\n  binaries:")..position(workflow, "\n  reproducibility:")];
    let rebuild_job = &workflow
        [position(workflow, "\n  reproducibility:")..position(workflow, "\n  publish-crates:")];

    for (name, job) in [("primary", binaries_job), ("independent", rebuild_job)] {
        assert!(
            job.contains("CARGO_PROFILE_RELEASE_STRIP: symbols"),
            "{name} release build must remove nondeterministic native symbol tables"
        );
        assert!(
            !job.contains("-Wl,-no_uuid"),
            "{name} release build must retain the content-derived Mach-O LC_UUID required by dyld"
        );
        assert!(
            !job.contains("-Wl,-random_uuid"),
            "{name} release build must use the linker's default content-derived Mach-O LC_UUID"
        );
        let windows_linker_setup = position(job, "Enable deterministic PE/COFF linking");
        assert!(
            job[windows_linker_setup..].contains("if: runner.os == 'Windows'"),
            "{name} release build must scope the PE/COFF linker flag to Windows"
        );
        assert!(
            job[windows_linker_setup..].contains("-C link-arg=/Brepro"),
            "{name} release build must replace wall-clock PE timestamps"
        );
        assert!(
            windows_linker_setup < position(job, "cargo build --release --locked"),
            "{name} deterministic PE/COFF configuration must precede the build"
        );
        let linux_linker_setup = position(job, "Disable nondeterministic ELF build IDs");
        assert!(
            job[linux_linker_setup..].contains("if: runner.os == 'Linux'"),
            "{name} release build must scope the ELF linker flag to Linux"
        );
        assert!(
            job[linux_linker_setup..].contains("-C link-arg=-Wl,--build-id=none"),
            "{name} release build must remove build IDs derived from unstable symbols"
        );
        assert!(
            linux_linker_setup < position(job, "cargo build --release --locked"),
            "{name} deterministic ELF configuration must precede the build"
        );
    }
}

#[test]
fn installed_release_smoke_is_checkout_independent() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let unix = &workflow[position(workflow, "- name: Verify installed Unix release")
        ..position(workflow, "- name: Verify installed Windows release")];
    let windows = &workflow[position(workflow, "- name: Verify installed Windows release")
        ..position(workflow, "- name: Generate SPDX SBOM")];

    assert!(unix.contains("scripts/verify-release-portability.py"));
    assert!(unix.contains("--release-root \"${install_root}\""));
    assert!(unix.contains("--forbid-path \"${GITHUB_WORKSPACE}\""));
    assert!(unix.contains("export A3S_USE_HOME=\"${RUNNER_TEMP}/release-smoke-home-"));
    assert!(unix.contains("cd \"${smoke_cwd}\""));
    assert!(unix.contains("\"${install_root}/a3s-use-browser-driver\" --version"));

    assert!(windows.contains("scripts/verify-release-portability.py"));
    assert!(windows.contains("--release-root $root"));
    assert!(windows.contains("--forbid-path $env:GITHUB_WORKSPACE"));
    assert!(windows.contains("$env:A3S_USE_HOME = Join-Path $env:RUNNER_TEMP"));
    assert!(windows.contains("Set-Location $smokeCwd"));
    assert!(windows.contains("& \"$root/a3s-use-browser-driver.exe\" --version"));
}

fn write_release_fixture(root: &Path, reverse: bool) {
    let mut files = vec![
        ("README.md", b"release fixture\n".as_slice()),
        ("bin/a3s-use.exe", b"fixture executable\n".as_slice()),
        ("skills/core/SKILL.md", b"# Core\n".as_slice()),
    ];
    if reverse {
        files.reverse();
    }
    for (relative, contents) in files {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        #[cfg(unix)]
        if relative == "bin/a3s-use.exe" {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

fn assert_packager_success(source: &Path, output_path: &Path, format: &str) {
    let output = run_packager(source, output_path, format);
    assert!(
        output.status.success(),
        "release packager failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_packager(source: &Path, output_path: &Path, format: &str) -> Output {
    Command::new(python_command())
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/package-release.py"))
        .args(["--format", format, "--source"])
        .arg(source)
        .arg("--output")
        .arg(output_path)
        .args(["--epoch", "1700000000"])
        .output()
        .unwrap()
}

fn run_rebuild_verifier(archive: &Path, rebuilt: &Path, output_path: &Path) -> Output {
    let mut command = Command::new(python_command());
    command
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/verify-release-rebuild.py"))
        .arg("--archive")
        .arg(archive)
        .args(["--platform", "test-x86_64"])
        .args([
            "--source-revision",
            "0123456789abcdef0123456789abcdef01234567",
        ])
        .arg("--output")
        .arg(output_path);
    for (archive_path, rebuilt_name) in [
        ("a3s-use", "a3s-use"),
        ("a3s-use-browser-driver", "a3s-use-browser-driver"),
    ] {
        command
            .arg("--binary")
            .arg(archive_path)
            .arg(rebuilt.join(rebuilt_name));
    }
    command.output().unwrap()
}

fn run_portability_verifier(release_root: &Path, checkout: &Path) -> Output {
    Command::new(python_command())
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/verify-release-portability.py"))
        .arg("--release-root")
        .arg(release_root)
        .arg("--forbid-path")
        .arg(checkout)
        .output()
        .unwrap()
}

fn python_command() -> &'static str {
    ["python3", "python"]
        .into_iter()
        .find(|candidate| {
            Command::new(candidate)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
        })
        .expect("Python 3 is required to test deterministic release packaging")
}

fn position(workflow: &str, command: &str) -> usize {
    workflow
        .find(command)
        .unwrap_or_else(|| panic!("release workflow omitted `{command}`"))
}

fn value_after<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .unwrap_or_else(|| panic!("release workflow omitted `{prefix}`"))
}
