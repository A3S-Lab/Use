#[test]
fn release_publishes_only_use_owned_crates_in_dependency_order() {
    let workflow = include_str!("../.github/workflows/release.yml");
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
