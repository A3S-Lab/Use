#[test]
fn release_publishes_downstream_library_crates_in_dependency_order() {
    let workflow = include_str!("../.github/workflows/release.yml");
    let core = position(workflow, "publish_once a3s-use-core");
    let core_visible = position(workflow, "wait_until_visible a3s-use-core");
    let extension = position(workflow, "publish_once a3s-use-extension");
    let extension_visible = position(workflow, "wait_until_visible a3s-use-extension");
    let ocr = position(workflow, "publish_once a3s-use-ocr");
    let ocr_visible = position(workflow, "wait_until_visible a3s-use-ocr");
    let browser = position(workflow, "publish_once a3s-use-browser");

    assert!(
        core < core_visible
            && core_visible < extension
            && extension < extension_visible
            && extension_visible < ocr
            && ocr < ocr_visible
            && ocr_visible < browser,
        "release publication order must make every dependency visible before its downstream crate"
    );
}

fn position(workflow: &str, command: &str) -> usize {
    workflow
        .find(command)
        .unwrap_or_else(|| panic!("release workflow omitted `{command}`"))
}
