use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

pub(super) const DATABASE_SCHEMA_VERSION: i64 = 2;

pub(super) fn open(path: &Path, create: bool) -> rusqlite::Result<Connection> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_FULL_MUTEX;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    #[cfg(windows)]
    let path = a3s_use_core::windows_extended_length_path(path)
        .map_err(|_| rusqlite::Error::InvalidPath(path.to_path_buf()))?;
    let mut connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(Duration::from_secs(10))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = FULL;",
    )?;

    let version = connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
    match version {
        0 if create => {
            // The product has no released Knowledge database format to
            // migrate. Create the one current schema atomically and reject
            // every other version below instead of carrying compatibility
            // branches for pre-release state.
            connection.execute_batch("PRAGMA journal_mode = WAL;")?;
            let transaction = connection.transaction()?;
            transaction.execute_batch(DDL)?;
            transaction.pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)?;
            transaction.commit()?;
        }
        DATABASE_SCHEMA_VERSION => {}
        _ => return Err(rusqlite::Error::InvalidQuery),
    }
    Ok(connection)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn opens_database_beyond_the_legacy_windows_path_limit() {
        let temporary = tempfile::tempdir().unwrap();
        let mut directory = temporary.path().to_path_buf();
        while directory.as_os_str().len() < 280 {
            directory.push("installation-state-segment");
        }
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("knowledge.sqlite3");

        let connection = open(&path, true).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            DATABASE_SCHEMA_VERSION
        );
    }
}

const DDL: &str = r#"
CREATE TABLE knowledge_projections (
    package_id             TEXT    NOT NULL,
    surface_id             TEXT    NOT NULL,
    generation             INTEGER NOT NULL CHECK (generation > 0),
    receipt_json           BLOB    NOT NULL,
    receipt_digest         TEXT    NOT NULL,
    index_digest           TEXT    NOT NULL,
    state                  TEXT    NOT NULL CHECK (state IN ('staged', 'promoted', 'removed')),
    staged_at_ms           INTEGER NOT NULL CHECK (staged_at_ms > 0),
    observed_at_ms         INTEGER NOT NULL CHECK (observed_at_ms >= staged_at_ms),
    PRIMARY KEY (package_id, surface_id, generation)
);

CREATE INDEX knowledge_projection_state
    ON knowledge_projections (package_id, surface_id, state, generation);

CREATE TABLE knowledge_documents (
    row_id          INTEGER PRIMARY KEY AUTOINCREMENT,
    package_id      TEXT    NOT NULL,
    surface_id      TEXT    NOT NULL,
    generation      INTEGER NOT NULL,
    concept_id      TEXT    NOT NULL,
    path            TEXT    NOT NULL,
    type_name       TEXT    NOT NULL,
    title           TEXT    NOT NULL,
    search_text     TEXT    NOT NULL,
    source_digest   TEXT    NOT NULL,
    content         BLOB    NOT NULL,
    UNIQUE (package_id, surface_id, generation, concept_id),
    FOREIGN KEY (package_id, surface_id, generation)
        REFERENCES knowledge_projections (package_id, surface_id, generation)
        ON DELETE CASCADE
);

CREATE INDEX knowledge_document_generation
    ON knowledge_documents (package_id, surface_id, generation, path);

CREATE VIRTUAL TABLE knowledge_documents_fts USING fts5(
    title,
    search_text,
    content='knowledge_documents',
    content_rowid='row_id',
    tokenize='unicode61 remove_diacritics 1'
);

CREATE TRIGGER knowledge_documents_insert AFTER INSERT ON knowledge_documents BEGIN
    INSERT INTO knowledge_documents_fts (rowid, title, search_text)
    VALUES (new.row_id, new.title, new.search_text);
END;

CREATE TRIGGER knowledge_documents_delete AFTER DELETE ON knowledge_documents BEGIN
    INSERT INTO knowledge_documents_fts (
        knowledge_documents_fts,
        rowid,
        title,
        search_text
    ) VALUES ('delete', old.row_id, old.title, old.search_text);
END;

CREATE TRIGGER knowledge_documents_update AFTER UPDATE ON knowledge_documents BEGIN
    INSERT INTO knowledge_documents_fts (
        knowledge_documents_fts,
        rowid,
        title,
        search_text
    ) VALUES ('delete', old.row_id, old.title, old.search_text);
    INSERT INTO knowledge_documents_fts (rowid, title, search_text)
    VALUES (new.row_id, new.title, new.search_text);
END;

CREATE TABLE knowledge_selection (
    package_id       TEXT    NOT NULL,
    surface_id       TEXT    NOT NULL,
    generation       INTEGER NOT NULL,
    selected_at_ms   INTEGER NOT NULL CHECK (selected_at_ms > 0),
    PRIMARY KEY (package_id, surface_id),
    FOREIGN KEY (package_id, surface_id, generation)
        REFERENCES knowledge_projections (package_id, surface_id, generation)
        ON DELETE RESTRICT
);
"#;
