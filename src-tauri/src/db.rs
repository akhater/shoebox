//! The catalog database.
//!
//! Identity rules, which everything else relies on:
//! - A **disk** is its filesystem serial number + filesystem + size, never its drive letter.
//! - A **folder** is a path relative to its disk's root, so a new letter changes nothing.
//! - A **photo** is its content (a fingerprint), not its path. One photo can live in
//!   several files (copies on other disks); tags will hang off the photo.
//! - Nothing is ever deleted because a disk is unplugged. A file only becomes
//!   "missing" when its disk is plugged in, scanned, and the file is not there.

use crate::gear;
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

/// A file's type, its extension in lower case ("cr3", "jpg"), on file `f`.
/// Also an index's key (v14), so queries must spell it exactly so.
pub const FILE_TYPE_SQL: &str = "lower(replace(f.name, rtrim(f.name, replace(f.name, '.', '')), ''))";

pub const FILE_PRESENT: i64 = 0;
pub const FILE_MISSING: i64 = 1;

pub const PREVIEW_PENDING: i64 = 0;
pub const PREVIEW_OK: i64 = 1;
pub const PREVIEW_FAILED: i64 = 2;

/// `photos.sideways`: the user decided how the photo stands (turned it, or
/// said it's fine as it is). The upright model never suggests turning it again.
pub const SIDEWAYS_DECIDED: i64 = -1;

/// What a photo's preview shows, on photo alias `p`: its content as the user
/// turned it. What is worked out from the preview (word search's descriptions,
/// Cull's measures) is kept with it, and worked out again once it changes.
pub const SHOWN_SQL: &str = "p.fingerprint || CASE WHEN p.turn > 0 THEN '@' || p.turn ELSE '' END";

pub fn open(path: &Path, memory: Memory) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(30))?;
    memory.apply(&conn)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

/// One more connection to an open catalog, for reading only: in WAL mode a
/// reader never waits for the writer, so the window's queries run side by
/// side and don't queue behind a scan or the AI. `query_only` refuses any write.
pub fn open_reader(path: &Path, memory: Memory) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(30))?;
    conn.pragma_update(None, "query_only", true)?;
    memory.apply(&conn)?;
    Ok(conn)
}

/// How much memory SQLite uses to read the catalog: pages kept by each
/// connection, and how much of the catalog file is mapped into memory (read
/// through the system's file cache, shared by every connection and given back
/// when other programs need it). SQLite's default, 2 MB of pages and nothing
/// mapped, read most of a 190,000-photo catalog from disk again for each view:
/// about twice as slow. `shoebox-cli bench` times other values on any computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub cache_mb: u32,
    pub map_mb: u32,
}

impl Memory {
    /// The settings' values, 0 meaning automatic: 16 MB of pages, and the
    /// whole catalog file mapped with room to grow (twice its size, from 256 MB
    /// to 8 GB).
    pub fn for_catalog(cache_mb: u32, map_mb: u32, catalog_bytes: u64) -> Self {
        let auto_map = u32::try_from((catalog_bytes >> 20).saturating_mul(2)).unwrap_or(u32::MAX).clamp(256, 8192);
        Self {
            cache_mb: if cache_mb == 0 { 16 } else { cache_mb },
            map_mb: if map_mb == 0 { auto_map } else { map_mb },
        }
    }

    pub fn apply(&self, conn: &Connection) -> Result<()> {
        conn.pragma_update(None, "cache_size", -(i64::from(self.cache_mb) * 1024))?;
        conn.pragma_update(None, "mmap_size", i64::from(self.map_mb) << 20)?;
        Ok(())
    }
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
    if version < 11 {
        // Which version of what Shoebox can read (imaging::PICTURE_READER)
        // last listed each folder: an older one left out formats it didn't know.
        let has_reader = conn.prepare("SELECT 1 FROM pragma_table_info('roots') WHERE name = 'reader'")?.exists([])?;
        let reader = if has_reader { "" } else { "ALTER TABLE roots ADD COLUMN reader INTEGER;" };
        conn.execute_batch(&format!("BEGIN; {reader} PRAGMA user_version = 11; COMMIT;"))?;
    }
    if version < 12 {
        // Photoshop files read before (imaging::PICTURE_READER 2) may show the
        // white placeholder of one saved without "Maximize compatibility": read again.
        conn.execute_batch(&format!(
            "BEGIN;
             UPDATE photos SET preview_state = {PREVIEW_PENDING} WHERE preview_state = {PREVIEW_OK} AND id IN
                 (SELECT photo_id FROM files WHERE lower(name) LIKE '%.psd' OR lower(name) LIKE '%.psb');
             PRAGMA user_version = 12;
             COMMIT;"
        ))?;
    }
    if version < 13 {
        // A person's faces by photo: "is this photo new for them?" was a walk
        // through all their faces for each of their photos (seconds per page).
        // It covers everything faces_person did.
        conn.execute_batch(
            "BEGIN;
             CREATE INDEX IF NOT EXISTS faces_person_photo ON faces (person_id, photo_id);
             DROP INDEX IF EXISTS faces_person;
             PRAGMA user_version = 13;
             COMMIT;",
        )?;
    }
    if version < 14 {
        // Each photo's camera and lens as shown, stored with it: views and
        // filters read them instead of working them out for every photo of
        // every query (30 times quicker on a big catalog). Triggers fill them
        // whenever a photo is added or its camera or lens changes, whatever
        // writes it; crate::gear::store_shown when the names themselves do.
        // Each file's type sits in an index with its photo, state and folder,
        // so the grid and the filters read them without opening the file rows.
        let (camera, lens) = (gear::Kind::Camera.shown_sql(), gear::Kind::Lens.shown_sql());
        let file_type = FILE_TYPE_SQL.replace("f.", "");
        let triggers = gear::shown_triggers_sql();
        // Safe to run again on a catalog that already has them (its version set back by hand).
        let has_shown =
            conn.prepare("SELECT 1 FROM pragma_table_info('photos') WHERE name = 'camera_shown'")?.exists([])?;
        let columns = if has_shown {
            ""
        } else {
            "ALTER TABLE photos ADD COLUMN camera_shown TEXT; ALTER TABLE photos ADD COLUMN lens_shown TEXT;"
        };
        conn.execute_batch(&format!(
            "BEGIN;
             {columns}
             UPDATE photos AS p SET camera_shown = {camera}, lens_shown = {lens};
             CREATE INDEX IF NOT EXISTS photos_camera_shown ON photos (camera_shown);
             CREATE INDEX IF NOT EXISTS photos_lens_shown ON photos (lens_shown);
             {triggers}
             CREATE INDEX IF NOT EXISTS files_photo_state ON files (photo_id, state, folder_id, {file_type});
             DROP INDEX IF EXISTS files_photo;
             PRAGMA user_version = 14;
             COMMIT;"
        ))?;
    }
    if version < 15 {
        conn.execute_batch(&format!("BEGIN; {SCHEMA_V15} PRAGMA user_version = 15; COMMIT;"))?;
    }
    if version < 16 {
        // Nothing: a "picked" column tried while Cull was built, then dropped.
        // Catalogs that got it keep it, unused; the version stays in step.
        conn.execute_batch("PRAGMA user_version = 16;")?;
    }
    if version < 17 {
        // Safe to run again on a catalog that already has them (its version set back by hand).
        let has_turn = conn.prepare("SELECT 1 FROM pragma_table_info('photos') WHERE name = 'turn'")?.exists([])?;
        let columns = if has_turn {
            ""
        } else {
            "ALTER TABLE photos ADD COLUMN turn INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE photos ADD COLUMN turning INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE photos ADD COLUMN sideways INTEGER;
             ALTER TABLE photos ADD COLUMN sideways_p REAL;"
        };
        conn.execute_batch(&format!("BEGIN; {columns} {SCHEMA_V17} PRAGMA user_version = 17; COMMIT;"))?;
    }
    if version < 18 {
        // Safe to run again on a catalog that already has them (its version set back by hand).
        let has_focus = conn.prepare("SELECT 1 FROM pragma_table_info('photos') WHERE name = 'focus'")?.exists([])?;
        let columns = if has_focus {
            ""
        } else {
            "ALTER TABLE photos ADD COLUMN focus BLOB;
             ALTER TABLE photos ADD COLUMN focus_read INTEGER NOT NULL DEFAULT 0;"
        };
        conn.execute_batch(&format!("BEGIN; {columns} {SCHEMA_V18} PRAGMA user_version = 18; COMMIT;"))?;
    }
    // Private photos: looked for at every open (one look at the catalog), not
    // by version, so they don't depend on the order changes reach a catalog.
    let has = |sql: &str| -> Result<bool> { Ok(conn.prepare(sql)?.exists([])?) };
    if !has("SELECT 1 FROM sqlite_master WHERE type = 'trigger' AND name = 'photos_private_faces'")? {
        let mut columns = String::new();
        for (table, column, def) in [
            ("photos", "private", "INTEGER NOT NULL DEFAULT 0"),
            ("photos", "seal_to", "BLOB"),
            ("faces", "private", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            if !has(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = '{column}'"))? {
                columns.push_str(&format!("ALTER TABLE {table} ADD COLUMN {column} {def};"));
            }
        }
        conn.execute_batch(&format!("BEGIN; {columns} {SCHEMA_PRIVATE} COMMIT;"))?;
    }
    // A private space's name, sealed to each of its photos (see private.rs).
    conn.execute_batch("CREATE TABLE IF NOT EXISTS private_names (photo_id INTEGER PRIMARY KEY, name BLOB NOT NULL);")?;
    // Corrected dates: looked for at every open too.
    if !has("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'dates_left'")? {
        let column = if has("SELECT 1 FROM pragma_table_info('photos') WHERE name = 'taken_original'")? {
            ""
        } else {
            "ALTER TABLE photos ADD COLUMN taken_original INTEGER;"
        };
        conn.execute_batch(&format!("BEGIN; {column} {SCHEMA_DATES} COMMIT;"))?;
    }
    Ok(())
}

/// Dates the user corrected (see dates.rs). `photos.taken_original`: the date
/// the photo's file gives, kept once the user moves `taken_ts` (the date
/// shown and sorted by) away from it; NULL while nobody did. Few photos: an
/// index of their own keeps counting them as small as they are.
/// `dates_left`: findings of the date check the user looked at and kept
/// ("Leave as is"), by folder, camera and the day of its first photo.
const SCHEMA_DATES: &str = r#"
CREATE INDEX IF NOT EXISTS photos_corrected ON photos (id) WHERE taken_original IS NOT NULL;
CREATE TABLE IF NOT EXISTS dates_left (
    folder_id   INTEGER NOT NULL,
    camera      TEXT    NOT NULL,
    first_day   TEXT    NOT NULL,
    left_at     INTEGER NOT NULL,
    PRIMARY KEY (folder_id, camera, first_day)
) WITHOUT ROWID;
"#;

/// Private photos (see private.rs): `photos.private` = 1 hides a photo unless
/// the space it is in is open; `photos.seal_to` is the public key its copies
/// are sealed to (its own: it says nothing about its space). Few of them: an
/// index of their own keeps finding them as small as they are.
/// Their faces carry the flag too (`faces.private`, kept by triggers), and
/// it sits in the indexes the views count through (dates, cameras, lenses,
/// a person's faces): leaving private photos out reads nothing more.
const SCHEMA_PRIVATE: &str = r#"
CREATE INDEX IF NOT EXISTS photos_private ON photos (id) WHERE private = 1;
CREATE INDEX IF NOT EXISTS photos_taken_private ON photos (taken_ts, private);
CREATE INDEX IF NOT EXISTS photos_camera_private ON photos (camera_shown, private);
CREATE INDEX IF NOT EXISTS photos_lens_private ON photos (lens_shown, private);
DROP INDEX IF EXISTS photos_taken;
DROP INDEX IF EXISTS photos_camera_shown;
DROP INDEX IF EXISTS photos_lens_shown;
UPDATE faces SET private = 1 WHERE photo_id IN (SELECT id FROM photos WHERE private = 1);
CREATE INDEX IF NOT EXISTS faces_person_private ON faces (person_id, photo_id, private);
CREATE INDEX IF NOT EXISTS faces_person_cover ON faces (person_id, score, private);
DROP INDEX IF EXISTS faces_person_photo;
DROP INDEX IF EXISTS faces_person_score;
CREATE TRIGGER IF NOT EXISTS faces_private_added AFTER INSERT ON faces
  WHEN EXISTS (SELECT 1 FROM photos WHERE id = NEW.photo_id AND private = 1) BEGIN
    UPDATE faces SET private = 1 WHERE id = NEW.id;
END;
CREATE TRIGGER IF NOT EXISTS faces_private_moved AFTER UPDATE OF photo_id ON faces BEGIN
    UPDATE faces SET private = (SELECT private FROM photos WHERE id = NEW.photo_id) WHERE id = NEW.id;
END;
CREATE TRIGGER IF NOT EXISTS photos_private_faces AFTER UPDATE OF private ON photos
  WHEN NEW.private IS NOT OLD.private BEGIN
    UPDATE faces SET private = NEW.private WHERE photo_id = NEW.id;
END;
"#;

/// Where the camera focused (see focus.rs). `photos.focus`: the points a
/// photo used, and which layout of points its camera has: each layout once
/// here, by its points. `photos.focus_read`: which focus reader last read the
/// photo, so photos read before it learned more are read again (metadata only).
const SCHEMA_V18: &str = r#"
CREATE TABLE IF NOT EXISTS focus_grids (
    id          INTEGER PRIMARY KEY,
    points      BLOB    NOT NULL UNIQUE
);
"#;

/// Turning photos (see `Catalog::turn`). `turn`: quarter turns clockwise the
/// user asked for, on top of what the file says - the user's work, kept with
/// the catalog; the photo file is never touched. `turning`: the previews may
/// not show `turn` yet (a turn cut short; finished at the next start).
/// `sideways`: what the upright model found (see upright.rs): NULL not looked
/// at yet, 0 upright, 1-3 the quarter turns clockwise it suggests, -1 the
/// user has decided (turned it, or said it's fine): never suggested again.
/// `sideways_p`: how sure the model was. The indexes keep each look-up as
/// small as what it finds: the photos being turned, suggested, or not looked at.
const SCHEMA_V17: &str = r#"
CREATE INDEX IF NOT EXISTS photos_turning ON photos (id) WHERE turning = 1;
CREATE INDEX IF NOT EXISTS photos_sideways ON photos (sideways) WHERE sideways > 0;
CREATE INDEX IF NOT EXISTS photos_unchecked ON photos (id) WHERE sideways IS NULL;
"#;

/// People check findings the user looked at and kept ("Leave as is", see
/// faces.rs): left out of the check from then on. Keyed by what each is
/// about: twice in a photo by (photo, person), a face's age jump or look-alike
/// by (face, 0), two people by (lower id, higher id). `seen` is the finding as
/// it was then (JSON): once that changes, the finding comes back by itself.
const SCHEMA_V15: &str = r#"
CREATE TABLE IF NOT EXISTS check_left (
    kind        TEXT    NOT NULL,   -- 'twice', 'ageJump', 'likeOther', 'samePerson'
    id          INTEGER NOT NULL,
    id2         INTEGER NOT NULL,
    seen        TEXT    NOT NULL,
    left_at     INTEGER NOT NULL,
    PRIMARY KEY (kind, id, id2)
) WITHOUT ROWID;
"#;

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

#[cfg(test)]
mod tests {
    use super::Memory;

    #[test]
    fn automatic_memory_follows_the_catalog_size() {
        let mb = |n: u64| n << 20;
        // Small or new catalogs: 16 MB of pages, 256 MB mapped.
        assert_eq!(Memory::for_catalog(0, 0, 0), Memory { cache_mb: 16, map_mb: 256 });
        assert_eq!(Memory::for_catalog(0, 0, mb(100)), Memory { cache_mb: 16, map_mb: 256 });
        // Bigger ones: twice the file, up to 8 GB.
        assert_eq!(Memory::for_catalog(0, 0, mb(700)).map_mb, 1400);
        assert_eq!(Memory::for_catalog(0, 0, mb(20_000)).map_mb, 8192);
        // Values set by hand are kept.
        assert_eq!(Memory::for_catalog(64, 512, mb(700)), Memory { cache_mb: 64, map_mb: 512 });
    }
}
