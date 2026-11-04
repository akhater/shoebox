//! Photos edited in Argentum. Argentum keeps a photo's edit in a sidecar
//! beside its file (`<file>.agdata`, and `<file>.<copy>.agdata` for each
//! virtual copy); the `agdata` crate tells from one whether the photo is
//! edited, and how (an edit, or a film look), as Argentum marks it.
//!
//! A sidecar is read only when it is new or its size or date changed since
//! it was last read; one never downloaded from the cloud is never read. A
//! sidecar gone is forgotten only after a complete listing of a folder that
//! could be read, like a file marked missing; a missing file keeps its
//! sidecars, as an offline one does. The photo's mark (`photos.edit_kind`,
//! `edit_versions`) follows from its files' sidecars by triggers (see db.rs).
//!
//! Each look stamps the sidecars it sees (`edits.seen`), and stamps only go
//! up: a scan and a check after editing in Argentum can run at once, and
//! each forgets only what neither saw since it began.
//!
//! Sidecars are only ever opened for reading.

use crate::catalog::Catalog;
use crate::db::{self, FILE_PRESENT, under_sql};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// A photo's mark, as `photos.edit_kind` and `edits.kind` keep it.
pub const NONE: i64 = 0;
pub const EDIT: i64 = 1;
pub const FILM: i64 = 2;

/// How long a folder handed to Argentum is looked at again on each return
/// to the window while nothing in it changes.
pub const WATCH_FOR: Duration = Duration::from_secs(10 * 60);

/// A sidecar as its folder lists it.
#[derive(Debug, Clone)]
pub struct Listed {
    pub name: String,
    pub size: i64,
    /// In nanoseconds: a rewrite within the same second still shows.
    pub mtime: i64,
    /// Kept only in the cloud (OneDrive...): reading it would download it.
    pub placeholder: bool,
}

impl Listed {
    /// The file `name`, when it is a sidecar's, with what its folder says of it.
    pub fn of(name: &str, md: &Metadata) -> Option<Self> {
        agdata::parse_sidecar_name(name)?;
        let mtime =
            md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i64);
        Some(Self { name: name.to_string(), size: md.len() as i64, mtime, placeholder: placeholder(md) })
    }
}

#[cfg(windows)]
fn placeholder(md: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const OFFLINE: u32 = 0x1000;
    const RECALL_ON_OPEN: u32 = 0x4_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
    md.file_attributes() & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(not(windows))]
fn placeholder(_: &Metadata) -> bool {
    false
}

/// What a look at sidecars did: how many it read, and how many photos'
/// marks or edits changed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Checked {
    pub read: u64,
    pub changed: u64,
}

/// A sidecar's mark and its edit's fingerprint; one that doesn't read as a
/// sidecar has none (Argentum shows its photo unedited).
fn mark(bytes: &[u8], photo: &str) -> (i64, Option<Vec<u8>>) {
    match agdata::status(bytes, agdata::is_raw(photo)) {
        Ok(s) => {
            let kind = match s.kind {
                agdata::Kind::None => NONE,
                agdata::Kind::Edit => EDIT,
                agdata::Kind::Film => FILM,
            };
            (kind, Some(s.fingerprint.as_bytes().to_vec()))
        }
        Err(_) => (NONE, None),
    }
}

/// Whether going from `old` to `new` (mark, fingerprint) changes what a photo shows.
fn differs(old: (i64, Option<&[u8]>), new: (i64, Option<&[u8]>)) -> bool {
    old.0 != new.0 || (new.0 != NONE && old.1 != new.1)
}

/// A listed sidecar of a file the catalog knows: its photo's file name, file,
/// copy, and the size, date, mark and fingerprint it was last read with.
struct Known<'a> {
    listed: &'a Listed,
    photo: &'a str,
    file_id: i64,
    copy: String,
    old: Option<(i64, i64, i64, Option<Vec<u8>>)>,
}

fn known<'a>(db: &Connection, folder_id: i64, listed: &'a [Listed]) -> Result<Vec<Known<'a>>> {
    let mut file = db.prepare_cached("SELECT id FROM files WHERE folder_id = ?1 AND name = ?2")?;
    let mut edit =
        db.prepare_cached("SELECT size, mtime, kind, fingerprint FROM edits WHERE file_id = ?1 AND copy_id = ?2")?;
    let mut known = Vec::new();
    for l in listed {
        let Some(name) = agdata::parse_sidecar_name(&l.name) else { continue };
        let Some(file_id) = file.query_row(params![folder_id, name.photo], |r| r.get(0)).optional()? else {
            continue;
        };
        let copy = name.copy.unwrap_or_default().to_string();
        let old =
            edit.query_row(params![file_id, copy], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
        known.push(Known { listed: l, photo: name.photo, file_id, copy, old });
    }
    Ok(known)
}

/// Reads, in `dir`, the sidecars new or changed since last read: each one's
/// mark and fingerprint, or None for one kept as it was.
fn read(dir: &Path, known: &[Known]) -> Vec<Option<(i64, Option<Vec<u8>>)>> {
    known
        .iter()
        .map(|k| {
            let same =
                k.old.as_ref().is_some_and(|(size, mtime, ..)| *size == k.listed.size && *mtime == k.listed.mtime);
            if same || k.listed.placeholder {
                return None;
            }
            match std::fs::read(dir.join(&k.listed.name)) {
                Ok(bytes) => Some(mark(&bytes, k.photo)),
                Err(e) => {
                    log::warn!("can't read {}: {e}", dir.join(&k.listed.name).display());
                    None
                }
            }
        })
        .collect()
}

/// Stores what was read, and stamps every sidecar listed as seen by `seen`
/// (never back: a later look's stamp stays).
fn write(db: &Connection, known: &[Known], read: Vec<Option<(i64, Option<Vec<u8>>)>>, seen: i64) -> Result<Checked> {
    let mut checked = Checked::default();
    for (k, new) in known.iter().zip(read) {
        match new {
            Some((kind, fingerprint)) => {
                db.execute(
                    "INSERT INTO edits (file_id, copy_id, size, mtime, kind, fingerprint, seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT (file_id, copy_id) DO UPDATE SET size = excluded.size, mtime = excluded.mtime,
                        kind = excluded.kind, fingerprint = excluded.fingerprint, seen = MAX(seen, excluded.seen)",
                    params![k.file_id, k.copy, k.listed.size, k.listed.mtime, kind, fingerprint, seen],
                )?;
                checked.read += 1;
                let before = k.old.as_ref().map_or((NONE, None), |(_, _, kind, f)| (*kind, f.as_deref()));
                checked.changed += u64::from(differs(before, (kind, fingerprint.as_deref())));
            }
            // Unchanged, in the cloud, or unreadable right now: kept as it was.
            None if k.old.is_some() => {
                db.execute(
                    "UPDATE edits SET seen = MAX(seen, ?1) WHERE file_id = ?2 AND copy_id = ?3",
                    params![seen, k.file_id, k.copy],
                )?;
            }
            None => {}
        }
    }
    Ok(checked)
}

/// Records the sidecars `listed` in the catalog folder `folder_id`, at `dir`
/// on its disk, as seen by `seen`. Reads those new or changed since last read.
pub fn record(cat: &Catalog, folder_id: i64, dir: &Path, listed: &[Listed], seen: i64) -> Result<Checked> {
    let known = known(&cat.db(), folder_id, listed)?;
    // Read with the catalog free: a sidecar can be megabytes.
    let read = read(dir, &known);
    let db = cat.db();
    let tx = db.unchecked_transaction()?;
    let checked = write(&tx, &known, read, seen)?;
    tx.commit()?;
    Ok(checked)
}

/// Forgets the sidecars no longer there: those of the files present on disk
/// `volume_id` in folder `rel` (and, with `below`, the folders under it) that
/// no look stamped since `seen`, except in folders that couldn't be read
/// (`errored`, and those under them). Returns how many photos' marks or edits
/// that changed.
pub fn forget_unseen(
    db: &Connection,
    volume_id: i64,
    rel: &str,
    below: bool,
    seen: i64,
    errored: &[String],
) -> Result<u64> {
    let tx = db.unchecked_transaction()?;
    let changed = forget(&tx, volume_id, rel, below, seen, errored)?;
    tx.commit()?;
    Ok(changed)
}

fn forget(db: &Connection, volume_id: i64, rel: &str, below: bool, seen: i64, errored: &[String]) -> Result<u64> {
    let folders = if below { under_sql("fo", 2) } else { "fo.rel_path = ?2".to_string() };
    // A missing file keeps its sidecars: its photo keeps its mark, as an offline one does.
    let unseen: Vec<(i64, String, i64, String)> = db
        .prepare(&format!(
            "SELECT e.file_id, e.copy_id, e.kind, fo.rel_path
               FROM folders fo JOIN files f ON f.folder_id = fo.id JOIN edits e ON e.file_id = f.id
              WHERE fo.volume_id = ?1 AND {folders} AND f.state = {FILE_PRESENT} AND e.seen < ?3"
        ))?
        .query_map(params![volume_id, rel, seen], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut changed = 0;
    for (file_id, copy, kind, folder) in unseen {
        // A folder that couldn't be read is not evidence that its sidecars are gone.
        if errored.iter().any(|e| db::is_under(&folder, e)) {
            continue;
        }
        db.execute("DELETE FROM edits WHERE file_id = ?1 AND copy_id = ?2", params![file_id, copy])?;
        changed += u64::from(kind != NONE);
    }
    Ok(changed)
}

/// Looks at the sidecars of one folder again, as a scan would, and forgets
/// those gone. Nothing happens to a folder that isn't in the catalog, on a
/// disk plugged in now, or that can't be listed.
///
/// All of it holds the catalog, from listing the folder to forgetting: a scan
/// can't record this folder from an older listing in between.
pub fn check_folder(cat: &Catalog, folder: &Path) -> Result<Checked> {
    let Some((volume_id, rel)) = cat.folder_at(folder) else { return Ok(Checked::default()) };
    let db = cat.db();
    let folder_id: Option<i64> = db
        .query_row("SELECT id FROM folders WHERE volume_id = ?1 AND rel_path = ?2", params![volume_id, rel], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(folder_id) = folder_id else { return Ok(Checked::default()) };
    let seen = crate::scan::next_scan_id();
    let Ok(entries) = std::fs::read_dir(folder) else { return Ok(Checked::default()) };
    let mut listed = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { return Ok(Checked::default()) };
        let Ok(md) = entry.metadata() else { return Ok(Checked::default()) };
        if md.is_file()
            && let Some(l) = Listed::of(&entry.file_name().to_string_lossy(), &md)
        {
            listed.push(l);
        }
    }
    let known = known(&db, folder_id, &listed)?;
    let read = read(folder, &known);
    let tx = db.unchecked_transaction()?;
    let mut checked = write(&tx, &known, read, seen)?;
    checked.changed += forget(&tx, volume_id, &rel, false, seen, &[])?;
    tx.commit()?;
    Ok(checked)
}

/// Remembers the folder of `file`, just handed to Argentum: its sidecars are
/// looked at again when the user comes back ([`check_handed_over`]).
pub fn handed_over(cat: &Catalog, file: &Path) {
    if let Some(folder) = file.parent() {
        cat.handed_to_argentum.lock().unwrap().insert(folder.to_path_buf(), Instant::now());
    }
}

/// Whether a folder handed to Argentum is on a disk plugged in now.
pub fn any_handed_over_here(cat: &Catalog) -> bool {
    cat.handed_to_argentum.lock().unwrap().keys().any(|folder| cat.folder_at(folder).is_some())
}

/// Looks again at the sidecars of every folder handed to Argentum (see
/// [`handed_over`]), at `now`. A folder is no longer looked at once a look
/// finds nothing new in it [`WATCH_FOR`] after it was handed over. Returns
/// how many photos' marks or edits changed.
pub fn check_handed_over(cat: &Catalog, now: Instant) -> Result<u64> {
    let folders: Vec<(PathBuf, Instant)> =
        cat.handed_to_argentum.lock().unwrap().iter().map(|(f, at)| (f.clone(), *at)).collect();
    let mut changed = 0;
    for (folder, at) in folders {
        let checked = check_folder(cat, &folder)?;
        changed += checked.changed;
        if checked == Checked::default() && now.saturating_duration_since(at) > WATCH_FOR {
            let mut handed = cat.handed_to_argentum.lock().unwrap();
            // Unless handed over again meanwhile.
            if handed.get(&folder) == Some(&at) {
                handed.remove(&folder);
            }
        }
    }
    Ok(changed)
}

/// The edits of each of a photo's files, by file id: the photo's own
/// (`copy` None) and each virtual copy's, the edited ones only.
pub fn of_photo(db: &Connection, photo: i64) -> Result<Vec<(i64, FileEdit)>> {
    let edits = db
        .prepare(
            "SELECT e.file_id, e.copy_id, e.kind, e.fingerprint, e.rendered, e.render_failed, e.render_error
               FROM files f JOIN edits e ON e.file_id = f.id
              WHERE f.photo_id = ?1 AND e.kind > 0 ORDER BY e.file_id, e.copy_id",
        )?
        .query_map([photo], |r| {
            let copy: String = r.get(1)?;
            let (now, rendered, failed): (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>) =
                (r.get(3)?, r.get(4)?, r.get(5)?);
            let failed = failed.is_some() && failed == now;
            Ok((
                r.get(0)?,
                FileEdit {
                    copy: (!copy.is_empty()).then_some(copy),
                    film: r.get::<_, i64>(2)? == FILM,
                    version: rendered.as_deref().map(crate::renders::version_of),
                    waiting: rendered != now && !failed,
                    error: if failed { r.get(6)? } else { None },
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(edits)
}

/// An edit Argentum keeps beside one of a photo's files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEdit {
    /// The virtual copy it is the edit of; None for the photo's own.
    pub copy: Option<String>,
    /// A film look is on.
    pub film: bool,
    /// The version its edited pictures are kept as (see renders.rs); None
    /// until Argentum rendered it.
    pub version: Option<String>,
    /// A newer edit than `version` waits to be rendered.
    pub waiting: bool,
    /// Why Argentum couldn't render this edit.
    pub error: Option<String>,
}
