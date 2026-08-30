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

#[cfg(unix)]
pub(crate) fn remove_directory_link(link: &Path) {
    std::fs::remove_file(link).expect("remove test directory symlink");
}

#[cfg(windows)]
pub(crate) fn remove_directory_link(link: &Path) {
    std::fs::remove_dir(link).expect("remove test directory junction");
}

#[cfg(windows)]
pub(crate) fn open_reading_scanner_without_delete_share(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;

    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(path)
        .expect("open test scanner handle")
}

#[cfg(windows)]
pub(crate) fn open_directory_scanner_without_delete_share(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .expect("open test directory scanner handle")
}

#[cfg(windows)]
fn windows_command_path(path: &Path) -> std::ffi::OsString {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let path = path
        .as_os_str()
        .encode_wide()
        .map(|unit| {
            if unit == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                unit
            }
        })
        .collect::<Vec<_>>();
    std::ffi::OsString::from_wide(&path)
}
