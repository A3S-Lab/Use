use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{UseError, UseResult};

const INSTALLATION_STORAGE_DOMAIN: &[u8] = b"a3s.use.installation-id.v1\0";
const MAX_INSTALLATION_ID_BYTES: usize = 256;

/// Canonical identity of one independently selected package installation.
///
/// The kind is part of the identity: a User and Workspace installation with
/// the same textual ID are distinct authorities and receive distinct storage
/// keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationId {
    pub kind: InstallationKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallationKind {
    User,
    Workspace,
}

impl InstallationId {
    pub fn new(kind: InstallationKind, id: impl Into<String>) -> UseResult<Self> {
        let installation = Self {
            kind,
            id: id.into(),
        };
        installation.validate()?;
        Ok(installation)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.id.is_empty()
            || self.id.len() > MAX_INSTALLATION_ID_BYTES
            || !self
                .id
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !self.id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
            })
        {
            return Err(UseError::new(
                "use.installation.id_invalid",
                "The installation identity is invalid.",
            ));
        }
        Ok(())
    }

    /// Stable lowercase path segment for this exact kind-and-ID pair.
    pub fn storage_key(&self) -> UseResult<String> {
        self.validate()?;
        let mut digest = Sha256::new();
        digest.update(INSTALLATION_STORAGE_DOMAIN);
        digest.update(self.kind.as_str().as_bytes());
        digest.update(b"\0");
        digest.update(self.id.as_bytes());
        Ok(format!("{:x}", digest.finalize()))
    }
}

impl InstallationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }
}

/// Compatibility name used by the existing reviewed-plan contracts.
///
/// Plans describe the installation selected by an operation. New storage and
/// composition APIs use `InstallationId` directly so scope is not duplicated
/// as a second authority.
pub type PlanScope = InstallationId;
pub type PlanScopeKind = InstallationKind;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_identity_includes_scope_kind() {
        let user = InstallationId::new(InstallationKind::User, "same/id").unwrap();
        let workspace = InstallationId::new(InstallationKind::Workspace, "same/id").unwrap();

        assert_ne!(user, workspace);
        assert_ne!(
            user.storage_key().unwrap(),
            workspace.storage_key().unwrap()
        );
        assert_eq!(user.storage_key().unwrap().len(), 64);
        assert!(user
            .storage_key()
            .unwrap()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')));
    }

    #[test]
    fn installation_ids_reject_ambiguous_or_unsafe_values() {
        for value in [
            "",
            "/workspace",
            "../escape",
            "scope\\escape",
            "scope value",
        ] {
            assert_eq!(
                InstallationId::new(InstallationKind::Workspace, value)
                    .unwrap_err()
                    .code,
                "use.installation.id_invalid"
            );
        }
        assert_eq!(
            InstallationId::new(InstallationKind::User, "a".repeat(257))
                .unwrap_err()
                .code,
            "use.installation.id_invalid"
        );
    }

    #[test]
    fn installation_identity_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}

        assert_send_sync::<InstallationId>();
    }
}
