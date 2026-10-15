//! The catalog database.
//!
//! Identity rules, which everything else relies on:
//! - A **disk** is its filesystem serial number + filesystem + size, never its drive letter.
//! - A **folder** is a path relative to its disk's root, so a new letter changes nothing.
//! - A **photo** is its content (a fingerprint), not its path. One photo can live in
//!   several files (copies on other disks); tags will hang off the photo.
//! - Nothing is ever deleted because a disk is unplugged. A file only becomes
//!   "missing" when its disk is plugged in, scanned, and the file is not there.

use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

const SCHEMA_V1: &str = r#"
CREATE TABLE volumes (
    id          INTEGER PRIMARY KEY,
    serial      INTEGER NOT NULL,
    fs          TEXT    NOT NULL,
    size_bytes  INTEGER NOT NULL,
    label       TEXT    NOT NULL DEFAULT '',
    last_mount  TEXT,
    last_seen   INTEGER,
    UNIQUE (serial, fs, size_bytes)
);

-- Folders the user added to the catalog.
CREATE TABLE roots (
    id          INTEGER PRIMARY KEY,
    volume_id   INTEGER NOT NULL REFERENCES volumes(id),
    rel_path    TEXT    NOT NULL,
    added_at    INTEGER NOT NULL,
    last_scan   INTEGER,
    UNIQUE (volume_id, rel_path)
);

CREATE TABLE folders (
    id          INTEGER PRIMARY KEY,
    volume_id   INTEGER NOT NULL REFERENCES volumes(id),
    rel_path    TEXT    NOT NULL,
    UNIQUE (volume_id, rel_path)
);

CREATE TABLE photos (
    id            INTEGER PRIMARY KEY,
    fingerprint   TEXT    NOT NULL UNIQUE,
    capture_key   TEXT,
    kind          TEXT    NOT NULL,
    taken_ts      INTEGER,          -- camera local time, as seconds since 1970 read as UTC
    taken_exif    INTEGER NOT NULL DEFAULT 0,
    make          TEXT,
    model         TEXT,
    lens          TEXT,
    width         INTEGER,
    height        INTEGER,
    iso           INTEGER,
    fnumber       REAL,
    exposure      TEXT,
    focal         REAL,
    preview_state INTEGER NOT NULL DEFAULT 0,   -- 0 pending, 1 ok, 2 failed
    preview_rev   INTEGER NOT NULL DEFAULT 0,
    thumb_w       INTEGER,
    thumb_h       INTEGER,
    error         TEXT,
    added_at      INTEGER NOT NULL
);
CREATE INDEX photos_taken   ON photos (taken_ts);
CREATE INDEX photos_capture ON photos (capture_key);

CREATE TABLE files (
    id          INTEGER PRIMARY KEY,
    photo_id    INTEGER REFERENCES photos(id),   -- NULL until read
    folder_id   INTEGER NOT NULL REFERENCES folders(id),
    name        TEXT    NOT NULL,
    size        INTEGER NOT NULL,
    mtime       INTEGER NOT NULL,
    state       INTEGER NOT NULL DEFAULT 0,      -- 0 present, 1 missing
    seen_scan   INTEGER,
    error       TEXT,
    UNIQUE (folder_id, name)
);
CREATE INDEX files_photo ON files (photo_id);
"#;

/// People and the faces found in photos. Names and groupings are the user's work, so
/// they live in the catalog (backed up). The face fingerprints themselves are
/// rebuildable and live in `faces.db`.
const SCHEMA_V2: &str = r#"
CREATE TABLE people (
    id          INTEGER PRIMARY KEY,
    name        TEXT,
    created_at  INTEGER NOT NULL
);

CREATE TABLE faces (
    id          INTEGER PRIMARY KEY,
    photo_id    INTEGER NOT NULL REFERENCES photos(id),
    person_id   INTEGER REFERENCES people(id),
    x1 REAL NOT NULL, y1 REAL NOT NULL, x2 REAL NOT NULL, y2 REAL NOT NULL,  -- 0..1 of the preview
    kps         TEXT NOT NULL,          -- 5 landmarks, 0..1, comma-separated
    score       REAL NOT NULL,
    by_hand     INTEGER NOT NULL DEFAULT 0  -- the user put it in this person: never regroup it
);
CREATE INDEX faces_photo  ON faces (photo_id);
CREATE INDEX faces_person ON faces (person_id);

-- Photos already looked at for faces (including those with none).
CREATE TABLE face_scan (
    photo_id    INTEGER PRIMARY KEY,
    model       TEXT NOT NULL,
    faces       INTEGER NOT NULL
);
"#;

pub const FILE_PRESENT: i64 = 0;
pub const FILE_MISSING: i64 = 1;

pub const PREVIEW_PENDING: i64 = 0;
pub const PREVIEW_OK: i64 = 1;
pub const PREVIEW_FAILED: i64 = 2;

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(30))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(&format!("BEGIN; {SCHEMA_V1} PRAGMA user_version = 1; COMMIT;"))?;
    }
    if version < 2 {
        conn.execute_batch(&format!("BEGIN; {SCHEMA_V2} PRAGMA user_version = 2; COMMIT;"))?;
    }
    if version < 3 {
        // Strangers (street photography): kept grouped so they never come back as
        // "new", but hidden from People.
        conn.execute_batch(
            "BEGIN; ALTER TABLE people ADD COLUMN ignored INTEGER NOT NULL DEFAULT 0; PRAGMA user_version = 3; COMMIT;",
        )?;
    }
    if version < 4 {
        // Small key/value facts about the catalog itself (e.g. which face
        // grouping rules it was last sorted with).
        conn.execute_batch(
            "BEGIN; CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); PRAGMA user_version = 4; COMMIT;",
        )?;
    }
    if version < 5 {
        // Culling: marked for deletion. Catalog-only until the user confirms a delete.
        conn.execute_batch(
            "BEGIN; ALTER TABLE photos ADD COLUMN marked INTEGER NOT NULL DEFAULT 0; PRAGMA user_version = 5; COMMIT;",
        )?;
    }
    if version < 6 {
        conn.execute_batch(&format!("BEGIN; {SCHEMA_V6} PRAGMA user_version = 6; COMMIT;"))?;
    }
    if version < 7 {
        // Each face remembers whether the user has seen it: a person with unseen faces
        // shows "New" or "+N" in People until the user has been on their page. Faces
        // already shown as new (found since the visit to People before last) stay so.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE faces ADD COLUMN seen INTEGER NOT NULL DEFAULT 0;
             UPDATE faces SET seen = 1 WHERE id <= COALESCE(
                 (SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'people_new_after'), (SELECT MAX(id) FROM faces));
             DELETE FROM meta WHERE key IN ('people_new_after', 'people_seen_upto');
             PRAGMA user_version = 7;
             COMMIT;",
        )?;
    }
    if version < 8 {
        // Safe to run again on a catalog that already has it (its version set back by hand).
        let has_lens_read =
            conn.prepare("SELECT 1 FROM pragma_table_info('photos') WHERE name = 'lens_read'")?.exists([])?;
        let lens_read =
            if has_lens_read { "" } else { "ALTER TABLE photos ADD COLUMN lens_read INTEGER NOT NULL DEFAULT 0;" };
        conn.execute_batch(&format!("BEGIN; {SCHEMA_V8} {lens_read} PRAGMA user_version = 8; COMMIT;"))?;
    }
    if version < 9 {
        conn.execute_batch(&format!("BEGIN; {SCHEMA_V9} PRAGMA user_version = 9; COMMIT;"))?;
    }
    if version < 10 {
        // A photo still unreadable when retried at a plug-in: the version of
        // the picture readers that gave up on it (imaging::PICTURE_READER).
        // Plug-ins leave it alone until a newer one; a folder scan tries again.
        // Safe to run again on a catalog that already has it.
        let has_given_up =
            conn.prepare("SELECT 1 FROM pragma_table_info('photos') WHERE name = 'given_up'")?.exists([])?;
        let given_up = if has_given_up { "" } else { "ALTER TABLE photos ADD COLUMN given_up INTEGER;" };
        conn.execute_batch(&format!("BEGIN; {given_up} PRAGMA user_version = 10; COMMIT;"))?;
    }
    Ok(())
}

/// Folder moves, renames and deletes under way (see organize.rs): what the
/// next start needs to finish or undo one that was cut short. Each step
/// changes here in the same transaction as the catalog change it goes with,
/// so the two never disagree. Empty when nothing is under way.
const SCHEMA_V9: &str = r#"
CREATE TABLE IF NOT EXISTS ops (
    id          TEXT    PRIMARY KEY,
    step        TEXT    NOT NULL,
    plan        TEXT    NOT NULL,   -- JSON
    started     INTEGER NOT NULL
);
"#;

/// Gear names (see gear.rs): what a camera or lens is shown as, by the name
/// its files give. `by_hand` = the user's (renamed, merged, split: backed up with
/// the catalog); the others are spellings grouped by Shoebox, rebuilt after
/// every scan. `photos.lens_read`: which lens reader last read the photo, so
/// photos read before it improved are read again (metadata only).
const SCHEMA_V8: &str = r#"
CREATE TABLE IF NOT EXISTS gear_names (
    kind        TEXT    NOT NULL,   -- 'camera' or 'lens'
    name        TEXT    NOT NULL,   -- as the files give it
    shown       TEXT    NOT NULL,
    by_hand     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, name)
) WITHOUT ROWID;
"#;

/// Tags: the user's own keywords. They hang off the photo (its content), so they
/// survive moves, copies and renames - the "Only Tag Once" core. Flat today;
/// `parent_id` is there so tags can nest later (Places > France > Paris).
const SCHEMA_V6: &str = r#"
CREATE TABLE tags (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL,
    parent_id   INTEGER REFERENCES tags(id),
    created_at  INTEGER NOT NULL
);
-- One name per level, whatever its case ("paris" is "Paris").
CREATE UNIQUE INDEX tags_name ON tags (COALESCE(parent_id, 0), name COLLATE NOCASE);

CREATE TABLE photo_tags (
    photo_id    INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    tag_id      INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    added_at    INTEGER NOT NULL,
    PRIMARY KEY (photo_id, tag_id)
) WITHOUT ROWID;
CREATE INDEX photo_tags_tag ON photo_tags (tag_id);
"#;

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// SQL condition: folder `alias.rel_path` is `rel` or inside it. `rel` is passed
/// as parameter `?N`. Uses substr, not LIKE, so `_` and `%` in names are literal.
pub fn under_sql(alias: &str, param: usize) -> String {
    format!(
        "(?{param} = '' OR {alias}.rel_path = ?{param} OR substr({alias}.rel_path, 1, length(?{param}) + 1) = ?{param} || '\\')"
    )
}

/// Rust-side twin of [`under_sql`].
pub fn is_under(path: &str, root: &str) -> bool {
    root.is_empty()
        || path == root
        || (path.len() > root.len() && path.starts_with(root) && path.as_bytes()[root.len()] == b'\\')
}
