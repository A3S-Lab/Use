#[test]
fn release_publishes_only_use_owned_crates_in_dependency_order() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let core = position(workflow, "publish_once a3s-use-core");
    let core_visible = position(workflow, "wait_until_visible a3s-use-core");
    let extension = position(workflow, "publish_once a3s-use-extension");
    let extension_visible = position(workflow, "wait_until_visible a3s-use-extension");
    let browser = position(workflow, "publish_once a3s-use-browser");

    assert!(
        core < core_visible
            && core_visible < extension
            && extension < extension_visible
            && extension_visible < browser,
        "release publication order must make every dependency visible before its downstream crate"
    );
    assert!(
        !workflow.contains("publish_once a3s-use-ocr"),
        "the independent OCR repository must own OCR crate publication"
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
