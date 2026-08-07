use a3s_use_core::{
    OkfKnowledgeObservation, OkfKnowledgeObservedState, OkfProjectionReceipt,
    OkfSelectedGeneration, UseError, UseResult, OKF_KNOWLEDGE_OBSERVATION_SCHEMA,
};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::index::{PreparedIndex, INDEX_SCHEMA};
use crate::okf_knowledge::OkfKnowledgeStageSpec;

const MAX_REMOVED_TOMBSTONES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectionState {
    Staged,
    Promoted,
    Removed,
}

impl ProjectionState {
    fn parse(value: &str) -> UseResult<Self> {
        match value {
            "staged" => Ok(Self::Staged),
            "promoted" => Ok(Self::Promoted),
            "removed" => Ok(Self::Removed),
            _ => Err(database_invalid(
                "The Knowledge database contains an unknown projection state.",
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct StoredProjection {
    pub receipt: OkfProjectionReceipt,
    pub receipt_digest: String,
    pub index_digest: String,
    pub state: ProjectionState,
    pub observed_at_ms: u64,
}

pub(super) fn require_projection(
    connection: &Connection,
    receipt: &OkfProjectionReceipt,
) -> UseResult<StoredProjection> {
    let stored = load_projection(
        connection,
        &receipt.surface.package_id,
        &receipt.surface.surface.id,
        receipt.generation,
    )?
    .ok_or_else(|| {
        UseError::new(
            "use.okf.knowledge_projection_missing",
            "The exact receipt-owned OKF projection is absent from the Knowledge database.",
        )
    })?;
    if stored.receipt != *receipt || stored.receipt_digest != receipt.descriptor_digest()? {
        return Err(database_conflict(
            "The Knowledge database projection does not match the exact supplied receipt.",
        ));
    }
    Ok(stored)
}

pub(super) fn selected_generation(
    connection: &Connection,
    package_id: &str,
    surface_id: &str,
) -> UseResult<Option<OkfSelectedGeneration>> {
    let selected = connection
        .query_row(
            "SELECT p.receipt_json, p.index_digest
             FROM knowledge_selection s
             JOIN knowledge_projections p
               ON p.package_id = s.package_id
              AND p.surface_id = s.surface_id
              AND p.generation = s.generation
             WHERE s.package_id = ?1 AND s.surface_id = ?2",
            params![package_id, surface_id],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| database_io("read selected Knowledge generation", error))?;
    selected
        .map(|(receipt, index_digest)| {
            let receipt = OkfProjectionReceipt::from_json(&receipt).map_err(|error| {
                database_invalid(format!(
                    "The selected Knowledge receipt is invalid: {}",
                    error.message
                ))
            })?;
            selected_generation_from_receipt(&receipt, &index_digest)
        })
        .transpose()
}

pub(super) fn load_projection(
    connection: &Connection,
    package_id: &str,
    surface_id: &str,
    generation: u64,
) -> UseResult<Option<StoredProjection>> {
    let row = connection
        .query_row(
            "SELECT receipt_json, receipt_digest, index_digest, state, observed_at_ms
             FROM knowledge_projections
             WHERE package_id = ?1 AND surface_id = ?2 AND generation = ?3",
            params![package_id, surface_id, generation_i64(generation)?],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| database_io("read Knowledge projection", error))?;
    row.map(
        |(receipt, receipt_digest, index_digest, state, observed_at_ms)| {
            let receipt = OkfProjectionReceipt::from_json(&receipt).map_err(|error| {
                database_invalid(format!(
                    "The retained Knowledge receipt is invalid: {}",
                    error.message
                ))
            })?;
            if receipt.descriptor_digest()? != receipt_digest || !valid_sha256(&index_digest) {
                return Err(database_invalid(
                    "The retained Knowledge projection digest evidence is invalid.",
                ));
            }
            Ok(StoredProjection {
                receipt,
                receipt_digest,
                index_digest,
                state: ProjectionState::parse(&state)?,
                observed_at_ms: timestamp_u64(observed_at_ms)?,
            })
        },
    )
    .transpose()
}

pub(super) fn load_projection_for_search(
    connection: &Connection,
    package_id: &str,
    surface_id: &str,
    generation: u64,
) -> UseResult<Option<StoredProjection>> {
    load_projection(connection, package_id, surface_id, generation)
}

pub(super) fn validate_stage_replay(
    stored: &StoredProjection,
    spec: &OkfKnowledgeStageSpec,
    index: &PreparedIndex,
) -> UseResult<()> {
    let receipt = &stored.receipt;
    if receipt.operation_id != spec.operation_id
        || receipt.scope != spec.scope
        || receipt.surface != spec.surface
        || receipt.generation != spec.generation
        || receipt.package_digest != spec.package_digest
        || receipt.manifest_digest != spec.manifest_digest
        || receipt.bundle != spec.bundle
        || receipt.index_schema != INDEX_SCHEMA
        || receipt.index_build_id != index.build_id
        || stored.index_digest != index.digest
    {
        return Err(database_conflict(
            "The staged OKF generation conflicts with retained immutable index evidence.",
        ));
    }
    Ok(())
}

pub(super) fn observation(
    receipt: &OkfProjectionReceipt,
    state: ProjectionState,
    index_digest: &str,
    observed_at_ms: u64,
    selected: Option<OkfSelectedGeneration>,
) -> UseResult<OkfKnowledgeObservation> {
    let observed_state = match state {
        ProjectionState::Staged => OkfKnowledgeObservedState::Staged,
        ProjectionState::Promoted => OkfKnowledgeObservedState::Promoted,
        ProjectionState::Removed => OkfKnowledgeObservedState::Removed,
    };
    // Promotion proves that this immutable index is eligible for an exact
    // capability/session projection. A newer promotion must not invalidate an
    // older in-flight session before the package cutover drains it, so every
    // retained promoted generation reports its own selection evidence. The
    // database-wide selection remains useful only while a candidate is staged.
    let selected = match state {
        ProjectionState::Promoted => Some(selected_generation_from_receipt(receipt, index_digest)?),
        ProjectionState::Staged => selected,
        ProjectionState::Removed => None,
    };
    let observation = OkfKnowledgeObservation {
        schema: OKF_KNOWLEDGE_OBSERVATION_SCHEMA.to_owned(),
        scope: receipt.scope.clone(),
        surface: receipt.surface.clone(),
        generation: receipt.generation,
        package_digest: receipt.package_digest.clone(),
        bundle_digest: receipt.bundle.content_digest.clone(),
        projection_receipt_digest: receipt.descriptor_digest()?,
        index_schema: receipt.index_schema.clone(),
        index_build_id: receipt.index_build_id.clone(),
        state: observed_state,
        observed_at_ms,
        index_digest: (state != ProjectionState::Removed).then(|| index_digest.to_owned()),
        selected,
    };
    observation.validate_for_receipt(receipt)?;
    Ok(observation)
}

fn selected_generation_from_receipt(
    receipt: &OkfProjectionReceipt,
    index_digest: &str,
) -> UseResult<OkfSelectedGeneration> {
    if !valid_sha256(index_digest) {
        return Err(database_invalid(
            "The promoted Knowledge index digest is invalid.",
        ));
    }
    Ok(OkfSelectedGeneration {
        generation: receipt.generation,
        package_digest: receipt.package_digest.clone(),
        bundle_digest: receipt.bundle.content_digest.clone(),
        projection_receipt_digest: receipt.descriptor_digest()?,
        index_schema: receipt.index_schema.clone(),
        index_build_id: receipt.index_build_id.clone(),
        index_digest: index_digest.to_owned(),
    })
}

pub(super) fn prune_removed_tombstones(
    connection: &Connection,
    package_id: &str,
    surface_id: &str,
) -> UseResult<()> {
    connection
        .execute(
            "DELETE FROM knowledge_projections
             WHERE package_id = ?1 AND surface_id = ?2 AND state = 'removed'
               AND generation NOT IN (
                   SELECT generation FROM knowledge_projections
                   WHERE package_id = ?1 AND surface_id = ?2 AND state = 'removed'
                   ORDER BY generation DESC LIMIT ?3
               )",
            params![package_id, surface_id, MAX_REMOVED_TOMBSTONES as i64],
        )
        .map_err(|error| database_io("prune removed Knowledge tombstones", error))?;
    Ok(())
}

pub(super) fn projection_id(spec: &OkfKnowledgeStageSpec) -> String {
    let identity = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        spec.scope.kind.as_str(),
        spec.scope.id,
        spec.surface.package_id,
        spec.surface.surface.id,
        spec.generation,
        spec.bundle.content_digest
    );
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    format!("projection-{}", &digest[..24])
}

pub(super) fn advancing_timestamp(now_ms: u64, previous: u64) -> UseResult<u64> {
    Ok(now_ms.max(
        previous
            .checked_add(1)
            .ok_or_else(|| database_invalid("The Knowledge observation timestamp overflowed."))?,
    ))
}

pub(super) fn generation_i64(value: u64) -> UseResult<i64> {
    i64::try_from(value).map_err(|_| database_invalid("The OKF generation exceeds SQLite bounds."))
}

pub(super) fn timestamp_i64(value: u64) -> UseResult<i64> {
    i64::try_from(value)
        .map_err(|_| database_invalid("The Knowledge timestamp exceeds SQLite bounds."))
}

fn timestamp_u64(value: i64) -> UseResult<u64> {
    u64::try_from(value).map_err(|_| database_invalid("The Knowledge timestamp is negative."))
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(super) fn database_conflict(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_database_conflict", message)
}

fn database_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_database_invalid", message)
}

pub(super) fn database_io(action: &str, error: rusqlite::Error) -> UseError {
    UseError::new(
        "use.okf.knowledge_database_io",
        format!("Failed to {action}: {error}"),
    )
}
