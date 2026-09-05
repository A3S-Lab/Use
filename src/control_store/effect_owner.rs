pub(in crate::control_store) mod capability_plane;
pub(in crate::control_store) mod flow;
pub(in crate::control_store) mod knowledge;
pub(in crate::control_store) mod runtime;
pub(in crate::control_store) mod static_surface;

pub(in crate::control_store) use capability_plane::validate_descriptor_snapshot_backup_bytes;
