//! Photos edited in Argentum. Argentum keeps a photo's edit in a sidecar
//! beside its file (`<file>.agdata`, and `<file>.<copy>.agdata` for each
//! virtual copy); the `agdata` crate tells from one whether the photo is
//! edited, and how (an edit, or a film look), as Argentum marks it.
//!
//! A sidecar is read only when it is new or its size or date changed since
//! it was last read; one never downloaded from the cloud is never read. A
//! sidecar gone is forgotten only after a complete listing of a folder that
//! could be read, like a file marked missing. The photo's mark (`photos.edit_kind`,
//! `edit_versions`) follows from its files' sidecars by triggers (see db.rs).
//!
//! Sidecars are only ever opened for reading.

use crate::catalog::Catalog;
use crate::db::{self, under_sql};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// A photo's mark, as `photos.edit_kind` and `edits.kind` keep it.
pub const NONE: i64 = 0;
pub const EDIT: i64 = 1;
pub const FILM: i64 = 2;

/// A sidecar as its folder lists it.
#[derive(Debug, Clone)]
pub struct Listed {
    pub name: String,
    pub size: i64,
    pub mtime: i64,
    /// Kept only in the cloud (OneDrive...): reading it would download it.
    pub placeholder: bool,
}

impl Listed {
    /// The file `name`, when it is a sidecar's, with what its folder says of it.
    pub fn of(name: &str, md: &Metadata) -> Option<Self> {
        agdata::parse_sidecar_name(name)?;
        let mtime =
            md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
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

impl std::ops::AddAssign for Checked {
    fn add_assign(&mut self, other: Self) {
        self.read += other.read;
        self.changed += other.changed;
    }
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

/// Records the sidecars `listed` in the catalog folder `folder_id`, at `dir`
/// on its disk, as seen by `seen`. Reads those new or changed since last read.
pub(crate) fn record(cat: &Catalog, folder_id: i64, dir: &Path, listed: &[Listed], seen: i64) -> Result<Checked> {
    type Known = (i64, i64, i64, Option<Vec<u8>>);
    // Each sidecar of a file the catalog knows: its file, copy, and what was read of it last.
    let mut known: Vec<(&Listed, &str, i64, String, Option<Known>)> = Vec::new();
    {
        let db = cat.db();
        let mut file = db.prepare_cached("SELECT id FROM files WHERE folder_id = ?1 AND name = ?2")?;
        let mut edit =
            db.prepare_cached("SELECT size, mtime, kind, fingerprint FROM edits WHERE file_id = ?1 AND copy_id = ?2")?;
        for l in listed {
            let Some(name) = agdata::parse_sidecar_name(&l.name) else { continue };
            let Some(file_id) = file.query_row(params![folder_id, name.photo], |r| r.get(0)).optional()? else {
                continue;
            };
            let copy = name.copy.unwrap_or_default().to_string();
            let old = edit
                .query_row(params![file_id, copy], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .optional()?;
            known.push((l, name.photo, file_id, copy, old));
        }
    }
    // Read with the catalog free: a sidecar can be megabytes.
    let read: Vec<Option<(i64, Option<Vec<u8>>)>> = known
        .iter()
        .map(|(l, photo, _, _, old)| {
            let same = old.as_ref().is_some_and(|(size, mtime, ..)| *size == l.size && *mtime == l.mtime);
            if same || l.placeholder {
                return None;
            }
            match std::fs::read(dir.join(&l.name)) {
                Ok(bytes) => Some(mark(&bytes, photo)),
                Err(e) => {
                    log::warn!("can't read {}: {e}", dir.join(&l.name).display());
                    None
                }
            }
        })
        .collect();
    let db = cat.db();
    let tx = db.unchecked_transaction()?;
    let mut checked = Checked::default();
    for ((l, _, file_id, copy, old), new) in known.iter().zip(read) {
        match new {
            Some((kind, fingerprint)) => {
                tx.execute(
                    "INSERT INTO edits (file_id, copy_id, size, mtime, kind, fingerprint, seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT (file_id, copy_id) DO UPDATE SET size = excluded.size, mtime = excluded.mtime,
                        kind = excluded.kind, fingerprint = excluded.fingerprint, seen = excluded.seen",
                    params![file_id, copy, l.size, l.mtime, kind, fingerprint, seen],
                )?;
                checked.read += 1;
                let before = old.as_ref().map_or((NONE, None), |(_, _, k, f)| (*k, f.as_deref()));
                checked.changed += u64::from(differs(before, (kind, fingerprint.as_deref())));
            }
            // Unchanged, in the cloud, or unreadable right now: kept as it was.
            None if old.is_some() => {
                tx.execute(
                    "UPDATE edits SET seen = ?1 WHERE file_id = ?2 AND copy_id = ?3",
                    params![seen, file_id, copy],
                )?;
            }
            None => {}
        }
    }
    tx.commit()?;
    Ok(checked)
}

/// Forgets the sidecars no longer there: those of files on disk `volume_id`
/// in folder `rel` (and, with `below`, the folders under it) not seen by
/// `seen`, except in folders that couldn't be read (`errored`, and those
/// under them). Returns how many photos' marks or edits that changed.
pub(crate) fn forget_unseen(
    db: &Connection,
    volume_id: i64,
    rel: &str,
    below: bool,
    seen: i64,
    errored: &[String],
) -> Result<u64> {
    let folders = if below { under_sql("fo", 2) } else { "fo.rel_path = ?2".to_string() };
    let unseen: Vec<(i64, String, i64, String)> = db
        .prepare(&format!(
            "SELECT e.file_id, e.copy_id, e.kind, fo.rel_path
               FROM folders fo JOIN files f ON f.folder_id = fo.id JOIN edits e ON e.file_id = f.id
              WHERE fo.volume_id = ?1 AND {folders} AND e.seen <> ?3"
        ))?
        .query_map(params![volume_id, rel, seen], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let tx = db.unchecked_transaction()?;
    let mut changed = 0;
    for (file_id, copy, kind, folder) in unseen {
        // A folder that couldn't be read is not evidence that its sidecars are gone.
        if errored.iter().any(|e| db::is_under(&folder, e)) {
            continue;
        }
        tx.execute("DELETE FROM edits WHERE file_id = ?1 AND copy_id = ?2", params![file_id, copy])?;
        changed += u64::from(kind != NONE);
    }
    tx.commit()?;
    Ok(changed)
}

/// Looks at the sidecars of one folder again, as a scan would, and forgets
/// those gone. Nothing happens to a folder that isn't in the catalog, on a
/// disk plugged in now, or that can't be listed.
pub fn check_folder(cat: &Catalog, folder: &Path) -> Result<Checked> {
    let Some((volume_id, rel)) = cat.folder_at(folder) else { return Ok(Checked::default()) };
    let folder_id: Option<i64> = cat
        .db()
        .query_row("SELECT id FROM folders WHERE volume_id = ?1 AND rel_path = ?2", params![volume_id, rel], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(folder_id) = folder_id else { return Ok(Checked::default()) };
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
    let seen = crate::scan::next_scan_id();
    let mut checked = record(cat, folder_id, folder, &listed, seen)?;
    checked.changed += forget_unseen(&cat.db(), volume_id, &rel, false, seen, &[])?;
    Ok(checked)
}

/// Remembers the folder of `file`, just handed to Argentum: its sidecars are
/// looked at again when the user comes back ([`check_handed_over`]).
pub fn handed_over(cat: &Catalog, file: &Path) {
    if let Some(folder) = file.parent() {
        cat.handed_to_argentum.lock().unwrap().insert(folder.to_path_buf());
    }
}

/// Looks again at the sidecars of every folder handed to Argentum (see
/// [`handed_over`]). Returns how many photos' marks or edits changed.
pub fn check_handed_over(cat: &Catalog) -> Result<u64> {
    let folders: Vec<PathBuf> = cat.handed_to_argentum.lock().unwrap().iter().cloned().collect();
    let mut changed = 0;
    for folder in folders {
        changed += check_folder(cat, &folder)?.changed;
    }
    Ok(changed)
}

/// The edits of each of a photo's files, by file id: the photo's own
/// (`copy` None) and each virtual copy's, the edited ones only.
pub fn of_photo(db: &Connection, photo: i64) -> Result<Vec<(i64, FileEdit)>> {
    let edits = db
        .prepare(
            "SELECT e.file_id, e.copy_id, e.kind FROM files f JOIN edits e ON e.file_id = f.id
              WHERE f.photo_id = ?1 AND e.kind > 0 ORDER BY e.file_id, e.copy_id",
        )?
        .query_map([photo], |r| {
            let copy: String = r.get(1)?;
            Ok((r.get(0)?, FileEdit { copy: (!copy.is_empty()).then_some(copy), film: r.get::<_, i64>(2)? == FILM }))
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
}
