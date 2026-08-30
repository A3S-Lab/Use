use std::path::Path;
use std::time::Duration;

use a3s_use_core::{InstallationId, InstallationKind, UseError, UseResult};
use rusqlite::{params, Connection, ErrorCode, OpenFlags, TransactionBehavior};

mod definition;

use definition::{CREATE_SCHEMA, EXPECTED_SCHEMA};

pub(super) const CONTROL_STORE_SCHEMA_VERSION: u32 = 3;
pub(super) const SQLITE_SYNCHRONOUS_FULL: u32 = 2;
const CONTROL_STORE_APPLICATION_ID: u32 = 0x4133_5355;
const DATABASE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ControlStoreMetadata {
    pub(super) installation: InstallationId,
    pub(super) schema_version: u32,
    pub(super) current_generation: u64,
    pub(super) published_capability_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ControlStoreInspection {
    pub(super) metadata: ControlStoreMetadata,
    pub(super) journal_mode: String,
    pub(super) foreign_keys_enabled: bool,
    pub(super) synchronous: u32,
}

pub(super) fn initialize(
    path: &Path,
    expected_installation: &InstallationId,
) -> UseResult<ControlStoreMetadata> {
    expected_installation.validate()?;
    let mut connection = open(path, OpenMode::Create)?;
    let version = pragma_u32(&connection, "user_version")?;
    match version {
        0 => initialize_empty(&mut connection, expected_installation)?,
        CONTROL_STORE_SCHEMA_VERSION => {}
        _ => return Err(schema_unsupported(version)),
    }
    Ok(inspect_connection(&connection, expected_installation)?.metadata)
}

pub(super) fn inspect(
    path: &Path,
    expected_installation: &InstallationId,
) -> UseResult<ControlStoreInspection> {
    expected_installation.validate()?;
    let connection = open(path, OpenMode::ReadOnly)?;
    inspect_connection(&connection, expected_installation)
}

#[derive(Debug, Clone, Copy)]
enum OpenMode {
    Create,
    ReadWrite,
    ReadOnly,
}

fn open(path: &Path, mode: OpenMode) -> UseResult<Connection> {
    let mut flags = OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
        | OpenFlags::SQLITE_OPEN_NOFOLLOW
        | OpenFlags::SQLITE_OPEN_EXRESCODE;
    flags |= match mode {
        OpenMode::Create => OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        OpenMode::ReadWrite => OpenFlags::SQLITE_OPEN_READ_WRITE,
        OpenMode::ReadOnly => OpenFlags::SQLITE_OPEN_READ_ONLY,
    };
    #[cfg(windows)]
    let path = a3s_use_core::windows_extended_length_path(path)
        .map_err(|_| path_error("The Control Store database path is invalid."))?;
    let connection = Connection::open_with_flags(path, flags)
        .map_err(|error| sqlite_error("open Control Store database", error))?;
    connection
        .busy_timeout(DATABASE_BUSY_TIMEOUT)
        .map_err(|error| sqlite_error("configure Control Store busy timeout", error))?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = FULL;
             PRAGMA trusted_schema = OFF;",
        )
        .map_err(|error| sqlite_error("configure Control Store connection", error))?;
    if matches!(mode, OpenMode::ReadOnly) {
        connection
            .execute_batch("PRAGMA query_only = ON;")
            .map_err(|error| sqlite_error("make Control Store connection read-only", error))?;
    }
    Ok(connection)
}

fn initialize_empty(connection: &mut Connection, installation: &InstallationId) -> UseResult<()> {
    let application_id = pragma_u32(connection, "application_id")?;
    if application_id != 0 || schema_object_count(connection)? != 0 {
        return Err(schema_unsupported(0));
    }
    let journal_mode: String = connection
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
        .map_err(|error| sqlite_error("enable Control Store WAL mode", error))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(corruption_error(
            "The Control Store could not enter WAL journal mode.",
        ));
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| sqlite_error("begin Control Store initialization", error))?;
    for ddl in CREATE_SCHEMA {
        transaction
            .execute_batch(ddl)
            .map_err(|error| sqlite_error("create Control Store schema", error))?;
    }
    transaction
        .execute(
            "INSERT INTO control_installation(
                singleton, scope_kind, scope_id, current_generation,
                published_capability_generation
             ) VALUES (1, ?1, ?2, 0, 0)",
            params![installation.kind.as_str(), installation.id],
        )
        .map_err(|error| sqlite_error("create Control Store schema", error))?;
    transaction
        .pragma_update(None, "application_id", CONTROL_STORE_APPLICATION_ID)
        .map_err(|error| sqlite_error("set Control Store application identity", error))?;
    transaction
        .pragma_update(None, "user_version", CONTROL_STORE_SCHEMA_VERSION)
        .map_err(|error| sqlite_error("set Control Store schema version", error))?;
    transaction
        .commit()
        .map_err(|error| sqlite_error("commit Control Store initialization", error))
}

pub(super) fn open_verified_write(
    path: &Path,
    expected_installation: &InstallationId,
) -> UseResult<Connection> {
    expected_installation.validate()?;
    let connection = open(path, OpenMode::ReadWrite)?;
    inspect_connection(&connection, expected_installation)?;
    Ok(connection)
}

pub(super) fn open_verified_read(
    path: &Path,
    expected_installation: &InstallationId,
) -> UseResult<Connection> {
    expected_installation.validate()?;
    let connection = open(path, OpenMode::ReadOnly)?;
    inspect_connection(&connection, expected_installation)?;
    Ok(connection)
}

fn inspect_connection(
    connection: &Connection,
    expected_installation: &InstallationId,
) -> UseResult<ControlStoreInspection> {
    let version = pragma_u32(connection, "user_version")?;
    if version != CONTROL_STORE_SCHEMA_VERSION {
        return Err(schema_unsupported(version));
    }
    if pragma_u32(connection, "application_id")? != CONTROL_STORE_APPLICATION_ID {
        return Err(corruption_error(
            "The SQLite file is not an A3S Use Control Store.",
        ));
    }
    validate_integrity(connection)?;
    validate_exact_schema(connection)?;
    let metadata = read_metadata(connection)?;
    if metadata.installation != *expected_installation {
        return Err(identity_error());
    }
    let journal_mode = connection
        .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
        .map_err(|error| sqlite_error("read Control Store journal mode", error))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(corruption_error(
            "The Control Store is not using its required WAL journal mode.",
        ));
    }
    let foreign_keys = pragma_u32(connection, "foreign_keys")?;
    let synchronous = pragma_u32(connection, "synchronous")?;
    if foreign_keys != 1 || synchronous != SQLITE_SYNCHRONOUS_FULL {
        return Err(corruption_error(
            "The Control Store connection safety settings are invalid.",
        ));
    }
    Ok(ControlStoreInspection {
        metadata,
        journal_mode: journal_mode.to_ascii_lowercase(),
        foreign_keys_enabled: true,
        synchronous,
    })
}

fn read_metadata(connection: &Connection) -> UseResult<ControlStoreMetadata> {
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM control_installation", [], |row| {
            row.get(0)
        })
        .map_err(|error| sqlite_error("count Control Store installation rows", error))?;
    if count != 1 {
        return Err(corruption_error(
            "The Control Store must contain exactly one installation identity.",
        ));
    }
    let (kind, id, generation, capability_generation): (String, String, i64, i64) = connection
        .query_row(
            "SELECT scope_kind, scope_id, current_generation,
                    published_capability_generation
             FROM control_installation WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| sqlite_error("read Control Store installation identity", error))?;
    let kind = match kind.as_str() {
        "user" => InstallationKind::User,
        "workspace" => InstallationKind::Workspace,
        _ => {
            return Err(corruption_error(
                "The Control Store installation kind is invalid.",
            ))
        }
    };
    let installation = InstallationId::new(kind, id)
        .map_err(|_| corruption_error("The Control Store installation identity is invalid."))?;
    let current_generation = u64::try_from(generation)
        .map_err(|_| corruption_error("The Control Store installation generation is invalid."))?;
    let published_capability_generation = u64::try_from(capability_generation)
        .map_err(|_| corruption_error("The Control Store capability generation is invalid."))?;
    Ok(ControlStoreMetadata {
        installation,
        schema_version: CONTROL_STORE_SCHEMA_VERSION,
        current_generation,
        published_capability_generation,
    })
}

fn validate_integrity(connection: &Connection) -> UseResult<()> {
    let mut statement = connection
        .prepare("PRAGMA quick_check")
        .map_err(|error| sqlite_error("prepare Control Store integrity check", error))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| sqlite_error("run Control Store integrity check", error))?;
    let results = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| sqlite_error("read Control Store integrity result", error))?;
    if results.as_slice() != ["ok"] {
        return Err(corruption_error(
            "SQLite integrity verification rejected the Control Store.",
        ));
    }
    let foreign_key_violation: Option<i64> = connection
        .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
        .optional()
        .map_err(|error| sqlite_error("verify Control Store foreign keys", error))?;
    if foreign_key_violation.is_some() {
        return Err(corruption_error(
            "Foreign-key verification rejected the Control Store.",
        ));
    }
    Ok(())
}

fn validate_exact_schema(connection: &Connection) -> UseResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, sql
             FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
             ORDER BY type, name
             LIMIT ?1",
        )
        .map_err(|error| sqlite_error("prepare Control Store schema inspection", error))?;
    let rows = statement
        .query_map(
            [i64::try_from(EXPECTED_SCHEMA.len() + 1).unwrap_or(i64::MAX)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(|error| sqlite_error("inspect Control Store schema", error))?;
    let rows = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| sqlite_error("read Control Store schema", error))?;
    let expected = EXPECTED_SCHEMA
        .iter()
        .map(|(name, ddl)| {
            (
                "table".to_string(),
                (*name).to_string(),
                (*name).to_string(),
                (*ddl).to_string(),
            )
        })
        .collect::<Vec<_>>();
    if rows != expected {
        return Err(corruption_error(
            "The Control Store schema differs from its exact supported definition.",
        ));
    }
    Ok(())
}

fn schema_object_count(connection: &Connection) -> UseResult<u64> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error("inspect empty Control Store schema", error))?;
    u64::try_from(count)
        .map_err(|_| corruption_error("The Control Store schema object count is invalid."))
}

fn pragma_u32(connection: &Connection, name: &str) -> UseResult<u32> {
    let value: i64 = connection
        .pragma_query_value(None, name, |row| row.get(0))
        .map_err(|error| sqlite_error("read Control Store pragma", error))?;
    u32::try_from(value).map_err(|_| corruption_error("A Control Store pragma value is invalid."))
}

pub(super) fn sqlite_error(action: &str, error: rusqlite::Error) -> UseError {
    let corrupt = matches!(
        error,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase,
                ..
            },
            _
        )
    );
    let code = if corrupt {
        "use.control_store.corrupt"
    } else {
        "use.control_store.io"
    };
    UseError::new(code, format!("Failed to {action}: {error}"))
}

fn schema_unsupported(version: u32) -> UseError {
    UseError::new(
        "use.control_store.schema_unsupported",
        "The Control Store schema is unsupported; preview schemas are not migrated in place.",
    )
    .with_detail("schemaVersion", version)
}

fn identity_error() -> UseError {
    UseError::new(
        "use.control_store.identity_mismatch",
        "The Control Store belongs to a different installation.",
    )
}

fn corruption_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.corrupt", message)
}

#[cfg(windows)]
fn path_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.path_invalid", message)
}

use rusqlite::OptionalExtension as _;
