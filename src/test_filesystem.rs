use std::path::Path;

#[cfg(unix)]
pub(crate) fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create test directory symlink");
}

#[cfg(windows)]
pub(crate) fn create_directory_link(target: &Path, link: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(windows_command_path(link))
        .arg(windows_command_path(target))
        .output()
        .expect("invoke mklink for test directory junction");
    assert!(
        output.status.success(),
        "mklink /J failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
fn windows_command_path(path: &Path) -> std::ffi::OsString {
    a3s_use_core::windows_extended_length_path(path)
        .expect("normalize test junction path")
        .into_os_string()
}

#[cfg(unix)]
pub(crate) fn remove_directory_link(link: &Path) {
    std::fs::remove_file(link).expect("remove test directory symlink");
}

#[cfg(windows)]
pub(crate) fn remove_directory_link(link: &Path) {
    std::fs::remove_dir(link).expect("remove test directory junction");
}
