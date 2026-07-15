//! Compatibility entry point for the A3S-managed Browser upgrade.

pub fn run_upgrade(json: bool) -> i32 {
    crate::lifecycle::upgrade(json)
}
