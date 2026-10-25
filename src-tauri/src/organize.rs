//! The only code in Shoebox allowed to change photo files on disk: deleting what
//! the user marked and then confirmed, renaming and creating folders (and, next,
//! drag-and-drop moves). File contents are never touched. Everything
//! else in Shoebox only ever reads photos; a test enforces that this file and
//! `paths.rs` are the only ones with file-changing code.
//!
//! Rules:
//! - Marking is catalog-only. Files are touched only by `delete_confirmed`,
//!   and only the exact plan the user was shown (checked by its token), on the
//!   disks they ticked.
//! - A photo is its content, so a marked photo's copies on every plugged-in
//!   disk are in the plan, grouped by disk. Copies on an unticked disk stay,
//!   and so does the photo's mark.
//! - Fixed disks: files go to the Recycle Bin. Removable media (SD cards, USB
//!   sticks) have none: those are deleted permanently and the plan says so
//!   before the user confirms.
//! - A marked RAW takes its camera JPEG twin (same folder, same name) and
//!   Argentum's edits (`<file>.agdata`, `<file>.<id>.agdata`) with it.
//! - Files on unplugged disks stay marked until their disk is plugged in.
//! - The catalog forgets a file only once it is really gone from its folder.
//!
//! Shoebox can be stopped at any moment (killed, crashed). Every
//! rename, move and delete is written down first (the `ops` table) and its
//! steps change there in the same transaction as the catalog. One cut short
//! is settled by [`resume`] at the next start (and when a disk comes back):
//! finished if the catalog had already taken it, undone otherwise. An
//! original is removed only once its copy was compared byte for byte; a copy
//! is taken back only while its original is there.

use crate::catalog::{Catalog, forget_photos};
use crate::db::{self, FILE_PRESENT};
use crate::formats::{self, Kind};
use crate::paths::OpLock;
use crate::volumes;
use anyhow::{Result, anyhow, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use testing::point as kill_point;

/// Marks (or unmarks) photos for deletion. Catalog only: no file is touched.
pub fn mark(cat: &Catalog, photo_ids: &[i64], marked: bool) -> Result<usize> {
    let photo_ids = cat.shown_ids(photo_ids)?;
    let mut db = cat.db();
    let tx = db.transaction()?;
    let mut n = 0;
    for id in photo_ids {
        n += tx.execute("UPDATE photos SET marked = ?1 WHERE id = ?2", params![marked as i64, id])?;
    }
    tx.commit()?;
    Ok(n)
}

pub fn unmark_all(cat: &Catalog) -> Result<usize> {
    Ok(cat
        .db()
        .execute(&format!("UPDATE photos AS p SET marked = 0 WHERE marked = 1 AND {}", cat.shown_sql("p")), [])?)
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlanFile {
    pub path: String,
    pub size: u64,
    /// "photo", "twin" (the RAW's camera JPEG) or "edits" (Argentum sidecar)
    pub role: String,
    pub volume_id: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlanDisk {
    pub volume_id: i64,
    pub label: String,
    pub mount: String,
    /// Recycle Bin (true) or permanent (removable media).
    pub recycle: bool,
    /// Marked photos with a file on this disk.
    pub photos: usize,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeletePlan {
    /// Every marked photo.
    pub marked: usize,
    /// Marked photos with a file here now.
    pub photos: usize,
    /// Marked photos on unplugged disks (or folders not found): they wait.
    pub waiting: usize,
    /// Marked photos already gone: missing, or deleted outside Shoebox since
    /// the last scan; no copy left on any disk. The delete only takes them out
    /// of the catalog.
    pub gone: Vec<i64>,
    pub disks: Vec<PlanDisk>,
    pub files: Vec<PlanFile>,
    pub bytes: u64,
    /// Identifies this exact list; a delete must quote it.
    pub token: String,
}

/// What deleting the marked photos would remove, right now. Private photos
/// marked while unlocked are left alone while they are hidden: nothing is
/// deleted that the user can't see.
pub fn plan(cat: &Catalog) -> Result<DeletePlan> {
    let shown = cat.shown_sql("p");
    let (rows, marked, gone, labels) = {
        let db = cat.db();
        let rows: Vec<(i64, String, i64, String, String, i64)> = db
            .prepare(&format!(
                "SELECT p.id, p.kind, fo.volume_id, fo.rel_path, f.name, f.size FROM photos p
                   JOIN files f ON f.photo_id = p.id JOIN folders fo ON fo.id = f.folder_id
                  WHERE p.marked = 1 AND f.state = {FILE_PRESENT} AND {shown}
                  ORDER BY fo.volume_id, fo.rel_path, f.name"
            ))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let marked: i64 =
            db.query_row(&format!("SELECT COUNT(*) FROM photos p WHERE marked = 1 AND {shown}"), [], |r| r.get(0))?;
        let gone: Vec<i64> = db
            .prepare(&format!(
                "SELECT p.id FROM photos p WHERE p.marked = 1 AND {shown}
                    AND NOT EXISTS (SELECT 1 FROM files f WHERE f.photo_id = p.id AND f.state = {FILE_PRESENT})
                  ORDER BY p.id"
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let labels: HashMap<i64, String> = db
            .prepare("SELECT id, label FROM volumes")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        (rows, marked as usize, gone, labels)
    };

    let mut plan = DeletePlan { marked, ..Default::default() };
    let mut seen = BTreeSet::new();
    let mut here = BTreeSet::new();
    // Files still listed that are gone from their folder (deleted outside
    // Shoebox since the last scan), and those that can't be checked now.
    let (mut absent, mut unsure) = (BTreeSet::new(), BTreeSet::new());
    let mut on_disk = BTreeSet::new();
    let mut disks: BTreeMap<i64, PlanDisk> = BTreeMap::new();
    // Each folder is listed once, however many marked photos it holds.
    let mut listings: HashMap<PathBuf, Vec<(String, u64)>> = HashMap::new();
    for (pid, kind, vid, rel, name, size) in rows {
        let Some(mount) = cat.mount_of(vid) else {
            unsure.insert(pid);
            continue;
        };
        let folder = PathBuf::from(volumes::join(&mount, &rel));
        let path = folder.join(&name);
        if !path.is_file() {
            (if volumes::gone_from(&mount, &rel, &name) { &mut absent } else { &mut unsure }).insert(pid);
            continue;
        }
        here.insert(pid);
        let disk = disks.entry(vid).or_insert_with(|| PlanDisk {
            volume_id: vid,
            label: labels.get(&vid).cloned().unwrap_or_default(),
            recycle: volumes::has_recycle_bin(&mount),
            mount: mount.clone(),
            photos: 0,
            files: 0,
            bytes: 0,
        });
        if on_disk.insert((vid, pid)) {
            disk.photos += 1;
        }
        let entries = listings.entry(folder.clone()).or_insert_with(|| list_folder(&folder));
        let mut files = vec![(path, size as u64, "photo")];
        files.extend(companions(&name, &kind, entries).into_iter().map(|(n, s, role)| (folder.join(n), s, role)));
        for (p, size, role) in files {
            let s = p.to_string_lossy().into_owned();
            if seen.insert(db::place_key(&p)) {
                disk.files += 1;
                disk.bytes += size;
                plan.bytes += size;
                plan.files.push(PlanFile { path: s, size, role: role.into(), volume_id: vid });
            }
        }
    }
    plan.files.sort_by(|a, b| (a.volume_id, &a.path).cmp(&(b.volume_id, &b.path)));
    // Every file it still lists gone from its folder: already gone too.
    let mut gone = gone;
    gone.extend(absent.into_iter().filter(|p| !here.contains(p) && !unsure.contains(p)));
    gone.sort_unstable();
    plan.photos = here.len();
    plan.waiting = marked.saturating_sub(here.len() + gone.len());
    plan.disks = disks.into_values().collect();
    plan.token = token(&plan.files, &gone);
    plan.gone = gone;
    Ok(plan)
}

/// The files in a folder, with their sizes. A file whose name can't be noted
/// down in the journal and opened again from it (see [`noted`]) is left out: it
/// is never taken along with a photo.
fn list_folder(folder: &Path) -> Vec<(String, u64)> {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok().filter(|m| m.is_file())?;
            let name = e.file_name().into_string().ok().filter(|n| volumes::name_is_storable(n))?;
            Some((name, m.len()))
        })
        .collect()
}

/// Files that leave with `name` (a photo of `kind`), among its folder's
/// `entries`: Argentum's edits of it and, for a RAW, its camera JPEG twin and
/// the twin's edits.
fn companions(name: &str, kind: &str, entries: &[(String, u64)]) -> Vec<(String, u64, &'static str)> {
    let lower = name.to_lowercase();
    let stem = lower.rsplit_once('.').map_or(lower.as_str(), |(s, _)| s);
    let edits_of = |file: &str| -> Vec<(String, u64, &'static str)> {
        let prefix = format!("{file}.");
        entries
            .iter()
            .filter(|(n, _)| {
                let n = n.to_lowercase();
                n.starts_with(&prefix) && n.ends_with(".agdata")
            })
            .map(|(n, s)| (n.clone(), *s, "edits"))
            .collect()
    };
    let mut out = edits_of(&lower);
    if kind == Kind::Raw.as_str() {
        for (n, s) in entries {
            let nl = n.to_lowercase();
            if nl != lower
                && nl.rsplit_once('.').is_some_and(|(st, _)| st == stem)
                && formats::kind_of(n) == Some(Kind::Image)
            {
                out.push((n.clone(), *s, "twin"));
                out.extend(edits_of(&nl));
            }
        }
    }
    out
}

fn token(files: &[PlanFile], gone: &[i64]) -> String {
    let mut h = blake3::Hasher::new();
    for f in files {
        h.update(f.path.as_bytes());
        h.update(&f.size.to_le_bytes());
    }
    for id in gone {
        h.update(&id.to_le_bytes());
    }
    h.finalize().to_hex()[..16].to_string()
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReport {
    pub deleted: usize,
    pub bytes: u64,
    pub recycled: usize,
    pub permanent: usize,
    /// Photos that left the catalog (no copy left anywhere).
    pub photos_removed: usize,
    /// Of those, marked photos that were already gone from every disk.
    pub forgotten: usize,
    pub failed: Vec<String>,
}

/// Deletes the plan the user was shown (`token`), on the disks they ticked, and
/// takes the marked photos already gone out of the catalog. Refuses if the plan
/// changed since.
pub fn delete_confirmed(cat: &Catalog, token: &str, volume_ids: &[i64]) -> Result<DeleteReport> {
    let current = plan(cat)?;
    if current.token != token {
        bail!("The list of files changed since you looked at it. Nothing was deleted - please review it again.");
    }
    let chosen: Vec<&PlanFile> = current.files.iter().filter(|f| volume_ids.contains(&f.volume_id)).collect();
    if chosen.is_empty() && current.gone.is_empty() {
        bail!("No disk ticked: nothing was deleted.");
    }
    let mut report = if chosen.is_empty() { DeleteReport::default() } else { delete_files(cat, &current, &chosen)? };
    // Only the catalog still has them: no file to touch.
    let forgotten = cat.forget_missing(&current.gone)?;
    report.forgotten = forgotten.photos;
    report.photos_removed += forgotten.photos;
    Ok(report)
}

/// The files `chosen` from the `current` plan: to the bin (or for good on
/// removable media), then out of the catalog once really gone.
fn delete_files(cat: &Catalog, current: &DeletePlan, chosen: &[&PlanFile]) -> Result<DeleteReport> {
    let disk = |vid: i64| current.disks.iter().find(|d| d.volume_id == vid).expect("every file's disk is in the plan");
    // Each file as the catalog knows it: (disk, folder, name).
    let places: Vec<(i64, String, String)> = chosen
        .iter()
        .map(|f| {
            let (rel, name) = volumes::split_file(&disk(f.volume_id).mount, &f.path);
            (f.volume_id, rel, name)
        })
        .collect();
    refuse_if_busy(cat, &places.iter().map(|(v, rel, _)| (*v, rel.as_str())).collect::<Vec<_>>())?;
    let (to_bin, to_nuke): (Vec<&PlanFile>, Vec<&PlanFile>) =
        chosen.iter().copied().partition(|f| disk(f.volume_id).recycle);

    let journal = Journal::begin(cat, &Op::Delete(DeleteOp { files: places.clone(), photos: Vec::new() }), DELETING)?;
    kill_point("delete:journaled");
    let mut report = DeleteReport::default();
    if !to_bin.is_empty() {
        let paths: Vec<&Path> = to_bin.iter().map(|f| Path::new(&f.path)).collect();
        if let Err(e) = trash::delete_all(&paths) {
            log::warn!("{}: {e}", crate::trash_bin::NAME);
            report.failed.push(format!("{}: {e}", crate::trash_bin::NAME));
        }
    }
    for f in &to_nuke {
        if let Err(e) = std::fs::remove_file(&f.path) {
            report.failed.push(format!("{}: {e}", f.path));
        }
    }
    kill_point("delete:sent");

    // The catalog forgets only what is really gone.
    let mut gone = Vec::new();
    for (f, place) in chosen.iter().zip(&places) {
        if Path::new(&f.path).exists() {
            continue;
        }
        let d = disk(f.volume_id);
        report.deleted += 1;
        report.bytes += f.size;
        if d.recycle {
            report.recycled += 1;
        } else {
            report.permanent += 1;
        }
        log::info!("deleted ({}): {}", if d.recycle { "recycle bin" } else { "permanent" }, f.path);
        gone.push(place.clone());
    }
    let removed = forget_files(cat, &gone, &journal, &places)?;
    report.photos_removed = removed.len();
    kill_point("delete:forgotten");
    cat.drop_previews(&removed)?;
    journal.end(cat);
    Ok(report)
}

/// Drops the catalog rows of deleted files (volume id, folder, name). Photos
/// left with no file leave the catalog with their faces; their previews go
/// next ([`Catalog::drop_previews`]). The delete's journal (`journal`, listing
/// all of `files`) moves on in the same transaction. Returns the photos that left.
fn forget_files(
    cat: &Catalog,
    gone: &[(i64, String, String)],
    journal: &Journal,
    files: &[(i64, String, String)],
) -> Result<Vec<i64>> {
    let mut removed = Vec::new();
    {
        let mut db = cat.db();
        let tx = db.transaction()?;
        let mut touched = BTreeSet::new();
        for (vid, rel, name) in gone {
            let row: Option<(i64, Option<i64>)> = tx
                .query_row(
                    "SELECT f.id, f.photo_id FROM files f JOIN folders fo ON fo.id = f.folder_id
                      WHERE fo.volume_id = ?1 AND fo.rel_path = ?2 AND f.name = ?3",
                    params![vid, rel, name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            if let Some((fid, pid)) = row {
                tx.execute("DELETE FROM files WHERE id = ?1", [fid])?;
                touched.extend(pid);
            }
        }
        for pid in &touched {
            let left: bool =
                tx.query_row("SELECT EXISTS (SELECT 1 FROM files WHERE photo_id = ?1)", [pid], |r| r.get(0))?;
            if !left {
                removed.push(*pid);
            }
        }
        forget_photos(&tx, &removed)?;
        let next = Op::Delete(DeleteOp { files: files.to_vec(), photos: removed.clone() });
        journal.set_step(&tx, FORGOTTEN, Some(&next))?;
        tx.commit()?;
    }
    Ok(removed)
}

/// A name every system accepts for a folder, Windows included: a disk goes
/// from one system to another.
fn valid_name(name: &str) -> Result<&str> {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".." || n.ends_with('.') {
        bail!("A folder name can't be empty or end with a dot.");
    }
    if let Some(c) = n.chars().find(|c| r#"\/:*?"<>|"#.contains(*c) || c.is_control()) {
        bail!("A folder name can't contain {c:?}.");
    }
    let stem = n.split('.').next().unwrap_or("").to_ascii_uppercase();
    let numbered =
        (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str()) || numbered {
        bail!("Windows reserves the name {n}.");
    }
    Ok(n)
}

fn child_rel(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{parent}{}{name}", volumes::SEP) }
}

/// A path below a folder as the journal notes it: its names joined by
/// [`volumes::SEP`], the same on every system. None when a name can't be noted
/// down (not Unicode, or holding what would read as a separator).
fn noted(inside: &Path) -> Option<String> {
    let mut out = String::new();
    for part in inside.components() {
        let name = part.as_os_str().to_str().filter(|n| volumes::name_is_storable(n))?;
        if !out.is_empty() {
            out.push(volumes::SEP);
        }
        out.push_str(name);
    }
    Some(out)
}

/// The path on disk of a place noted by [`noted`], below `base`.
fn below(base: &Path, inside: &str) -> PathBuf {
    inside.split(volumes::SEP).fold(base.to_path_buf(), |path, name| path.join(name))
}

/// A catalog folder as the system writes it, for a message.
fn shown(rel: &str) -> String {
    rel.replace(volumes::SEP, std::path::MAIN_SEPARATOR_STR)
}

/// Gives a copy its original's modification time, which copying a file keeps
/// on Windows but not on macOS or Linux: a scan then sees the file unchanged.
#[cfg(unix)]
fn keep_modified(original: &Path, copy: &Path) -> std::io::Result<()> {
    let modified = std::fs::metadata(original)?.modified()?;
    std::fs::File::open(copy)?.set_modified(modified)
}

/// Windows copies the modification time with the file.
#[cfg(not(unix))]
fn keep_modified(_original: &Path, _copy: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Renames a catalog folder on its disk and in the catalog in one step: its
/// photos and sub-folders go with it and nothing is read again. Returns the
/// folder's new path on its disk.
pub fn rename_folder(cat: &Catalog, volume_id: i64, rel: &str, new_name: &str) -> Result<String> {
    let name = valid_name(new_name)?;
    if rel.is_empty() {
        bail!("A whole disk can't be renamed here: double-click the disk to change its name in Shoebox.");
    }
    let (parent, old) = rel.rsplit_once(volumes::SEP).unwrap_or(("", rel));
    if old == name {
        return Ok(rel.to_string());
    }
    let from = PathBuf::from(cat.folder_path(volume_id, rel)?);
    let new_rel = child_rel(parent, name);
    let to = from.with_file_name(name);
    // "trip" -> "Trip" is the same folder where the file system ignores case
    // (Windows, macOS by default): then the folder is listed under its old
    // name. Anything else must be free.
    let case_only = old.eq_ignore_ascii_case(name);
    if to.exists() && (!case_only || exact_entry(&to)) {
        bail!("There is already a folder or file called {name} there.");
    }
    let taken: bool = cat.db().query_row(
        &format!("SELECT EXISTS (SELECT 1 FROM folders WHERE volume_id = ?1 AND {})", db::under_sql("folders", 2)),
        params![volume_id, new_rel],
        |r| r.get(0),
    )?;
    if taken && !case_only {
        bail!(
            "The catalog still remembers a folder called {name} there (its photos are missing). Choose another name."
        );
    }
    refuse_if_busy(cat, &[(volume_id, rel), (volume_id, &new_rel)])?;
    rename_on_disk(cat, volume_id, (rel, &new_rel), (&from, &to), Roots::Follow, "rename")?;
    log::info!("renamed folder {} -> {}", from.display(), to.display());
    let _ = cat.refresh_roots();
    Ok(new_rel)
}

/// Renames a catalog folder on its own disk (`from` -> `to`, which are
/// `rels` there), then points the catalog at it. Written down first: a stop
/// in between is finished at the next start.
fn rename_on_disk(
    cat: &Catalog,
    volume_id: i64,
    rels: (&str, &str),
    (from, to): (&Path, &Path),
    roots: Roots,
    verb: &str,
) -> Result<()> {
    let op = RenameOp { volume_id, rel: rels.0.into(), new_rel: rels.1.into(), absorb: roots == Roots::Absorb };
    let journal = Journal::begin(cat, &Op::Rename(op), RENAMING)?;
    kill_point("rename:journaled");
    if let Err(e) = std::fs::rename(from, to) {
        journal.end(cat);
        bail!("Couldn't {verb} {}: {e}", from.display());
    }
    kill_point("rename:renamed");
    if let Err(e) = move_in_catalog(cat, (volume_id, rels.0), (volume_id, rels.1), roots, Then::Done(&journal)) {
        // Keep disk and catalog in step: put the folder back (failing that,
        // the next start finishes the rename).
        if std::fs::rename(to, from).is_ok() {
            journal.end(cat);
        }
        return Err(e);
    }
    Ok(())
}

/// What happens to catalog roots at or under a folder that moves.
#[derive(Clone, Copy, PartialEq)]
enum Roots {
    /// Renamed in place: they keep being roots, at their new path.
    Follow,
    /// Moved into another root's folder: they become part of that root.
    Absorb,
}

/// What a catalog change does to its journal, in the same transaction.
enum Then<'a> {
    /// The change is complete.
    Done(&'a Journal),
    /// It goes on to this step.
    Step(&'a Journal, &'static str),
}

/// Points the catalog's folders at or under `old` (disk, path) to `new`. On
/// the disk before it returns: the originals of a copy go next.
fn move_in_catalog(cat: &Catalog, old: (i64, &str), new: (i64, &str), roots: Roots, then: Then) -> Result<()> {
    flushed(cat, |conn| {
        let tx = conn.transaction()?;
        for table in ["folders", "roots"] {
            let rows: Vec<(i64, String)> = tx
                .prepare(&format!(
                    "SELECT id, rel_path FROM {table} WHERE volume_id = ?1 AND {}",
                    db::under_sql(table, 2)
                ))?
                .query_map(params![old.0, old.1], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (id, rel) in rows {
                if table == "roots" && roots == Roots::Absorb {
                    tx.execute("DELETE FROM roots WHERE id = ?1", [id])?;
                    continue;
                }
                let moved = format!("{}{}", new.1, &rel[old.1.len()..]);
                tx.execute(
                    &format!("UPDATE {table} SET volume_id = ?1, rel_path = ?2 WHERE id = ?3"),
                    params![new.0, moved, id],
                )?;
            }
        }
        match then {
            Then::Done(journal) => journal.remove(&tx)?,
            Then::Step(journal, step) => journal.set_step(&tx, step, None)?,
        }
        tx.commit()?;
        Ok(())
    })
}

/// A folder (`rel` on disk `from`) going into the folder `to_parent` on disk `to`.
pub struct FolderMove {
    pub from: i64,
    pub rel: String,
    pub to: i64,
    pub to_parent: String,
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct MoveProgress {
    /// "copying", "checking" or "removing"
    pub phase: String,
    pub files: usize,
    pub total_files: usize,
    pub bytes: u64,
    pub total_bytes: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MoveResult {
    pub volume_id: i64,
    /// The folder's new path on its disk.
    pub rel: String,
    /// Copied to another disk (true) or just moved on the same disk.
    pub copied: bool,
    pub files: usize,
    pub bytes: u64,
    /// Originals that couldn't be removed after a verified copy (locked,
    /// read-only, or added during the move). Nothing is lost; they're reported.
    pub left_behind: Vec<String>,
}

/// Everything that must hold before a folder moves. Returns (from, to, new path on its disk).
pub fn check_move(cat: &Catalog, m: &FolderMove) -> Result<(PathBuf, PathBuf, String)> {
    if m.rel.is_empty() {
        bail!("A whole disk can't be moved.");
    }
    let (parent, name) = m.rel.rsplit_once(volumes::SEP).unwrap_or(("", &m.rel));
    if m.from == m.to {
        if db::is_under(&m.to_parent, &m.rel) {
            bail!("A folder can't go inside itself.");
        }
        if m.to_parent == parent {
            bail!("{name} is already there.");
        }
    }
    let from = PathBuf::from(cat.folder_path(m.from, &m.rel)?);
    let to_parent = PathBuf::from(cat.folder_path(m.to, &m.to_parent)?);
    let in_source = cat
        .root_ids()?
        .into_iter()
        .filter_map(|id| cat.root(id).ok())
        .any(|r| r.volume_id == m.to && db::is_under(&m.to_parent, &r.rel_path));
    if !in_source {
        bail!("{} isn't in one of Shoebox's sources.", to_parent.display());
    }
    let to = to_parent.join(name);
    if to.exists() {
        bail!("There is already a folder or file called {name} in {}.", to_parent.display());
    }
    let new_rel = child_rel(&m.to_parent, name);
    let taken: bool = cat.db().query_row(
        &format!("SELECT EXISTS (SELECT 1 FROM folders WHERE volume_id = ?1 AND {})", db::under_sql("folders", 2)),
        params![m.to, new_rel],
        |r| r.get(0),
    )?;
    if taken {
        bail!(
            "The catalog still remembers a folder called {name} there (its photos are missing). Rename one of them first."
        );
    }
    refuse_if_busy(cat, &[(m.from, &m.rel), (m.to, &new_rel)])?;
    Ok((from, to, new_rel))
}

/// Moves a folder with everything in it into another catalog folder, on the
/// disk and in the catalog in one step; nothing is read again.
/// - Same disk: a rename on the disk, instant, nothing copied.
/// - Another disk: copy every file, flush it to the disk, compare every byte,
///   update the catalog, then remove the originals one by one. A failure or
///   `cancel` before the catalog changes removes the partial copy and leaves
///   the original as it was.
///
/// Either way it is written down first, so a stop at any point is settled
/// at the next start ([`resume`]).
pub fn move_folder(
    cat: &Catalog,
    m: &FolderMove,
    cancel: &AtomicBool,
    progress: &dyn Fn(&MoveProgress),
) -> Result<MoveResult> {
    let (from, to, new_rel) = check_move(cat, m)?;
    if m.from == m.to {
        rename_on_disk(cat, m.from, (&m.rel, &new_rel), (&from, &to), Roots::Absorb, "move")?;
        log::info!("moved folder {} -> {}", from.display(), to.display());
        let _ = cat.refresh_roots();
        return Ok(MoveResult {
            volume_id: m.to,
            rel: new_rel,
            copied: false,
            files: 0,
            bytes: 0,
            left_behind: Vec::new(),
        });
    }

    // Everything in the folder, or nothing: a part that can't be read stops the move.
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in walkdir::WalkDir::new(&from).follow_links(false).min_depth(1) {
        let e = e.map_err(|e| anyhow!("Can't read part of {}: {e}. Nothing was moved.", from.display()))?;
        let inside = noted(e.path().strip_prefix(&from)?);
        let inside = inside.ok_or_else(|| {
            anyhow!("{} has a name Shoebox can't note down; it won't move it. Nothing was moved.", e.path().display())
        })?;
        let kind = e.file_type();
        if kind.is_symlink() {
            bail!("{} is a link to somewhere else; Shoebox won't move it. Nothing was moved.", e.path().display());
        } else if kind.is_dir() {
            dirs.push(inside);
        } else {
            files.push((inside, e.metadata()?.len()));
        }
    }
    let total_bytes: u64 = files.iter().map(|f| f.1).sum();
    if let Some(free) = volumes::free_bytes(to.parent().unwrap_or(&to))
        && free < total_bytes + (256 << 20)
    {
        bail!(
            "There isn't enough room on the other disk for {} ({} MB). Nothing was moved.",
            from.display(),
            total_bytes >> 20
        );
    }
    let plan = CopyOp { from: m.from, rel: m.rel.clone(), to: m.to, new_rel: new_rel.clone(), dirs, files };
    let journal = Journal::begin(cat, &Op::Copy(plan.clone()), COPYING)?;
    kill_point("copy:journaled");
    // Only a folder this move made is ever taken back.
    if let Err(e) = std::fs::create_dir(&to) {
        journal.end(cat);
        bail!("Couldn't create {}: {e}. Nothing was moved.", to.display());
    }
    let mut p =
        MoveProgress { phase: "copying".into(), total_files: plan.files.len(), total_bytes, ..Default::default() };
    progress(&p);

    let copy_all = |p: &mut MoveProgress| -> Result<()> {
        for d in &plan.dirs {
            std::fs::create_dir(below(&to, d))?;
        }
        for (f, size) in &plan.files {
            if cancel.load(Ordering::Relaxed) {
                bail!("Stopped. Nothing was moved.");
            }
            let (src, dst) = (below(&from, f), below(&to, f));
            std::fs::copy(&src, &dst)?;
            // The copy has the file's dates, so scans see it unchanged.
            if let Err(e) = keep_modified(&src, &dst) {
                log::warn!("move: couldn't give {} the date of its original: {e}", dst.display());
            }
            // On the disk, not just in memory (a read-only copy can't be opened to flush; Windows writes it soon anyway).
            if let Ok(out) = std::fs::OpenOptions::new().write(true).open(&dst) {
                out.sync_all()?;
            }
            kill_point("copy:copied");
            p.files += 1;
            p.bytes += size;
            progress(p);
        }
        *p =
            MoveProgress { phase: "checking".into(), total_files: plan.files.len(), total_bytes, ..Default::default() };
        progress(p);
        for (f, size) in &plan.files {
            if cancel.load(Ordering::Relaxed) {
                bail!("Stopped. Nothing was moved.");
            }
            let original = below(&from, f);
            if !same_bytes(&original, &below(&to, f))? {
                bail!("The copy of {} came out different. Nothing was moved.", original.display());
            }
            p.files += 1;
            p.bytes += size;
            progress(p);
        }
        kill_point("copy:checked");
        Ok(())
    };
    if let Err(e) = copy_all(&mut p) {
        return Err(give_up_copy(cat, journal, &plan, e));
    }
    let then = Then::Step(&journal, REMOVING);
    if let Err(e) = move_in_catalog(cat, (m.from, &m.rel), (m.to, &new_rel), Roots::Absorb, then) {
        return Err(give_up_copy(cat, journal, &plan, e));
    }
    kill_point("copy:cataloged");
    log::info!(
        "copied and checked {} files ({total_bytes} bytes): {} -> {}",
        plan.files.len(),
        from.display(),
        to.display()
    );

    // The catalog now points at the checked copy; the originals can go.
    p = MoveProgress { phase: "removing".into(), total_files: plan.files.len(), total_bytes, ..Default::default() };
    progress(&p);
    let left_behind = match finish_copy(cat, &plan, false, &mut || {
        p.files += 1;
        progress(&p);
    })? {
        Some(left) => {
            journal.end(cat);
            left
        }
        None => {
            // The journal stays: the next start (or the disk coming back) finishes it.
            log::warn!("move: a disk went away while {} was being removed", from.display());
            Vec::new()
        }
    };
    let _ = cat.refresh_roots();
    Ok(MoveResult {
        volume_id: m.to,
        rel: new_rel,
        copied: true,
        files: plan.files.len(),
        bytes: total_bytes,
        left_behind,
    })
}

/// A copy to another disk that failed or was stopped before the catalog
/// changed: the copy is taken back, the original stays as it was. Returns `e`.
fn give_up_copy(cat: &Catalog, journal: Journal, plan: &CopyOp, e: anyhow::Error) -> anyhow::Error {
    match undo_copy(cat, plan) {
        Ok(true) => journal.end(cat),
        // The journal stays: the next start takes the rest back.
        Ok(false) => log::warn!("move: part of the copy at {} is still there; Shoebox removes it later", plan.new_rel),
        Err(undo) => log::warn!("move: couldn't take back the copy at {}: {undo:#}", plan.new_rel),
    }
    e
}

/// Removes an original after its copy was checked. On macOS and Linux what
/// stops a removal is the folder, not the file: nothing is changed to try again.
#[cfg(not(windows))]
fn remove_original(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

/// Removes an original after its copy was checked; a read-only one is made
/// writable first (it is going anyway).
#[cfg(windows)]
fn remove_original(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            let mut perm = std::fs::metadata(path)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)] // Windows only, and the file is being deleted
            perm.set_readonly(false);
            std::fs::set_permissions(path, perm)?;
            std::fs::remove_file(path)
        }
        other => other,
    }
}

/// Byte-for-byte comparison of two files.
fn same_bytes(a: &Path, b: &Path) -> Result<bool> {
    use std::io::Read;
    let (mut fa, mut fb) = (std::fs::File::open(a)?, std::fs::File::open(b)?);
    if fa.metadata()?.len() != fb.metadata()?.len() {
        return Ok(false);
    }
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    let fill = |f: &mut std::fs::File, buf: &mut [u8]| -> std::io::Result<usize> {
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..])? {
                0 => break,
                k => n += k,
            }
        }
        Ok(n)
    };
    loop {
        let (n, k) = (fill(&mut fa, &mut ba)?, fill(&mut fb, &mut bb)?);
        if n != k || ba[..n] != bb[..k] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

/// Makes a new, empty folder inside a catalog folder - "New folder", or
/// "New folder (2)" if that's taken - and returns its path on its disk.
pub fn create_folder(cat: &Catalog, volume_id: i64, parent_rel: &str, name: &str) -> Result<String> {
    let name = valid_name(name)?;
    let parent = PathBuf::from(cat.folder_path(volume_id, parent_rel)?);
    let mut chosen = name.to_string();
    let mut n = 2;
    while parent.join(&chosen).exists() {
        chosen = format!("{name} ({n})");
        n += 1;
    }
    let path = parent.join(&chosen);
    std::fs::create_dir(&path).map_err(|e| anyhow::anyhow!("Couldn't create {}: {e}", path.display()))?;
    let rel = child_rel(parent_rel, &chosen);
    cat.db().execute("INSERT OR IGNORE INTO folders (volume_id, rel_path) VALUES (?1, ?2)", params![volume_id, rel])?;
    log::info!("new folder {}", path.display());
    Ok(rel)
}

// The journal: every rename, move and delete is written down before a disk
// changes, so one cut short (a kill, a crash) is settled at the next start.

const RENAMING: &str = "renaming";
const COPYING: &str = "copying";
const REMOVING: &str = "removing";
const DELETING: &str = "deleting";
const FORGOTTEN: &str = "forgotten";

/// A change under way, as the `ops` table keeps it (JSON). Places are
/// (disk id, folder on that disk), never drive letters: a disk can come back
/// as another one.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Op {
    Rename(RenameOp),
    Copy(CopyOp),
    Delete(DeleteOp),
}

/// A folder renamed, or moved on its own disk: one rename on the disk.
/// Step: renaming (its journal goes in the catalog's transaction).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct RenameOp {
    volume_id: i64,
    rel: String,
    new_rel: String,
    /// Into another source's folder: roots inside it join that source.
    absorb: bool,
}

/// A folder moved to another disk. Steps: copying (the catalog still points
/// at the original), then removing (it points at the checked copy).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct CopyOp {
    from: i64,
    rel: String,
    to: i64,
    new_rel: String,
    /// Sub-folders and files (with their size), inside the folder.
    dirs: Vec<String>,
    files: Vec<(String, u64)>,
}

/// Files confirmed for deletion: (disk, folder, name). Steps: deleting, then
/// forgotten (the catalog has dropped them; `photos` left with them and
/// their previews go).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct DeleteOp {
    files: Vec<(i64, String, String)>,
    #[serde(default)]
    photos: Vec<i64>,
}

/// A change's row in `ops`, and its lock, held until it is done.
struct Journal {
    id: String,
    _lock: OpLock,
}

impl Journal {
    /// Writes down what is about to happen, before anything on a disk changes.
    fn begin(cat: &Catalog, op: &Op, step: &str) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = format!(
            "{}-{}-{}",
            chrono::Utc::now().timestamp_millis(),
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        // Locked before the row exists: no one can take it for one cut short.
        let lock = cat.data.try_op_lock(&id)?.ok_or_else(|| anyhow!("operation {id} is taken"))?;
        let plan = serde_json::to_string(op)?;
        flushed(cat, |conn| {
            conn.execute(
                "INSERT INTO ops (id, step, plan, started) VALUES (?1, ?2, ?3, ?4)",
                params![id, step, plan, db::now()],
            )?;
            Ok(())
        })?;
        Ok(Self { id, _lock: lock })
    }

    /// Moves on to `step` (and `op`, when it changed), inside the caller's transaction.
    fn set_step(&self, tx: &rusqlite::Connection, step: &str, op: Option<&Op>) -> Result<()> {
        match op {
            Some(op) => tx.execute(
                "UPDATE ops SET step = ?2, plan = ?3 WHERE id = ?1",
                params![self.id, step, serde_json::to_string(op)?],
            )?,
            None => tx.execute("UPDATE ops SET step = ?2 WHERE id = ?1", params![self.id, step])?,
        };
        Ok(())
    }

    /// Done, inside the caller's transaction.
    fn remove(&self, tx: &rusqlite::Connection) -> Result<()> {
        tx.execute("DELETE FROM ops WHERE id = ?1", [&self.id])?;
        Ok(())
    }

    /// Done (or undone): nothing left to settle. Failing that, the next start
    /// settles it again, which changes nothing.
    fn end(self, cat: &Catalog) {
        if let Err(e) = self.remove(&cat.db()) {
            log::warn!("operation {}: {e:#}", self.id);
        }
    }
}

/// Runs `f` on the catalog with each commit on the disk before it returns
/// (SQLite's synchronous = FULL, back to Shoebox's usual NORMAL after): a
/// step written down must be there before the files it covers change.
fn flushed<T>(cat: &Catalog, f: impl FnOnce(&mut rusqlite::Connection) -> Result<T>) -> Result<T> {
    let mut db = cat.db();
    db.pragma_update(None, "synchronous", "FULL")?;
    let out = f(&mut db);
    let back = db.pragma_update(None, "synchronous", "NORMAL");
    let out = out?;
    back?;
    Ok(out)
}

/// What [`resume`] did.
#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Resumed {
    /// Changes finished as they were asked.
    pub finished: Vec<String>,
    /// Changes undone: disk and catalog are as they were before.
    pub undone: Vec<String>,
    /// Changes waiting for a disk to be plugged in.
    pub waiting: Vec<String>,
}

impl Resumed {
    pub fn is_empty(&self) -> bool {
        self.finished.is_empty() && self.undone.is_empty() && self.waiting.is_empty()
    }
}

enum Settled {
    Finished,
    Undone,
    Waiting,
}

/// Settles every rename, move and delete cut short (Shoebox killed or
/// crashed), as far as the disks plugged in allow: finished if the
/// catalog had already taken it, undone otherwise. The rest waits for its
/// disk. Runs at start and when a disk is plugged in. A change still running,
/// in this Shoebox or another (the window and `shoebox-cli`), is left alone.
pub fn resume(cat: &Catalog) -> Result<Resumed> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Resumed::default();
    let ids: Vec<String> = cat
        .db()
        .prepare("SELECT id FROM ops ORDER BY started, id")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &ids {
        let Some(lock) = cat.data.try_op_lock(id)? else { continue }; // still running
        let row: Option<(String, String)> = cat
            .db()
            .query_row("SELECT step, plan FROM ops WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((step, plan)) = row else { continue }; // it finished meanwhile
        let op: Op = match serde_json::from_str(&plan) {
            Ok(op) => op,
            Err(e) => {
                log::warn!("operation {id}: unreadable ({e}), left as it is");
                continue;
            }
        };
        let what = op.describe(cat);
        let journal = Journal { id: id.clone(), _lock: lock };
        match settle(cat, journal, &step, &op) {
            Ok(Settled::Finished) => {
                log::info!("cut short, now finished: {what}");
                out.finished.push(what);
            }
            Ok(Settled::Undone) => {
                log::info!("cut short, now undone: {what}");
                out.undone.push(what);
            }
            Ok(Settled::Waiting) => {
                log::info!("cut short, waiting for a disk: {what}");
                out.waiting.push(what);
            }
            Err(e) => {
                log::warn!("cut short, couldn't settle yet: {what}: {e:#}");
                out.waiting.push(what);
            }
        }
    }
    // Locks of changes cut short before they were written down: taking one
    // and letting go removes it.
    for id in cat.data.op_lock_ids() {
        if !ids.contains(&id) {
            drop(cat.data.try_op_lock(&id));
        }
    }
    if !out.finished.is_empty() || !out.undone.is_empty() {
        let _ = cat.refresh_roots();
    }
    Ok(out)
}

fn settle(cat: &Catalog, journal: Journal, step: &str, op: &Op) -> Result<Settled> {
    Ok(match (op, step) {
        (Op::Rename(r), RENAMING) => settle_rename(cat, journal, r)?,
        (Op::Copy(c), COPYING) => {
            if !undo_copy(cat, c)? {
                return Ok(Settled::Waiting);
            }
            journal.end(cat);
            Settled::Undone
        }
        (Op::Copy(c), REMOVING) => {
            let Some(left) = finish_copy(cat, c, true, &mut || {})? else { return Ok(Settled::Waiting) };
            if !left.is_empty() {
                log::warn!("move: {} original(s) left behind: {}", left.len(), left.join(", "));
            }
            journal.end(cat);
            Settled::Finished
        }
        (Op::Delete(d), DELETING) => settle_delete(cat, journal, d)?,
        (Op::Delete(d), FORGOTTEN) => {
            cat.drop_previews(&d.photos)?;
            journal.end(cat);
            Settled::Finished
        }
        (_, other) => bail!("unknown step {other:?}"),
    })
}

/// A rename on one disk, cut short: the catalog never took it (its
/// transaction ends the journal). If the disk did, the catalog follows.
fn settle_rename(cat: &Catalog, journal: Journal, r: &RenameOp) -> Result<Settled> {
    let Some(mount) = cat.mount_of(r.volume_id) else { return Ok(Settled::Waiting) };
    let from = PathBuf::from(volumes::join(&mount, &r.rel));
    let to = PathBuf::from(volumes::join(&mount, &r.new_rel));
    let roots = if r.absorb { Roots::Absorb } else { Roots::Follow };
    match renamed(&from, &to) {
        Some(true) => {
            move_in_catalog(cat, (r.volume_id, &r.rel), (r.volume_id, &r.new_rel), roots, Then::Done(&journal))?;
            Ok(Settled::Finished)
        }
        Some(false) => {
            journal.end(cat);
            Ok(Settled::Undone)
        }
        None => {
            log::warn!("neither {} nor {} is there; the catalog stays as it was", from.display(), to.display());
            journal.end(cat);
            Ok(Settled::Undone)
        }
    }
}

/// Whether the rename `from` -> `to` happened on the disk; None when neither
/// is there.
fn renamed(from: &Path, to: &Path) -> Option<bool> {
    // "trip" -> "Trip" is one folder on Windows: its exact name tells.
    let one_folder = from.to_string_lossy().eq_ignore_ascii_case(&to.to_string_lossy());
    if one_folder && exact_entry(to) {
        return Some(true);
    }
    if from.exists() {
        Some(false)
    } else if to.exists() {
        Some(true)
    } else {
        None
    }
}

/// Whether `path`'s folder lists it under exactly that name, case included.
fn exact_entry(path: &Path) -> bool {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return false };
    std::fs::read_dir(parent).into_iter().flatten().flatten().any(|e| e.file_name() == name)
}

/// (original folder, copy) of a move to another disk, while both disks are plugged in.
fn copy_paths(cat: &Catalog, c: &CopyOp) -> Option<(PathBuf, PathBuf)> {
    let from = volumes::join(&cat.mount_of(c.from)?, &c.rel);
    let to = volumes::join(&cat.mount_of(c.to)?, &c.new_rel);
    Some((from.into(), to.into()))
}

/// A disk the catalog knows as plugged in, still there.
fn plugged(cat: &Catalog, volume_id: i64) -> bool {
    cat.mount_of(volume_id).is_some_and(|m| Path::new(&m).is_dir())
}

/// Takes back a copy to another disk whose catalog change never happened:
/// each copied file whose original is still there (a file whose original
/// isn't stays), then the folders the copy made, if empty. False while a disk
/// is missing or a copy can't be removed: tried again later.
fn undo_copy(cat: &Catalog, c: &CopyOp) -> Result<bool> {
    let Some((from, to)) = copy_paths(cat, c) else { return Ok(false) };
    if !to.exists() {
        return Ok(true);
    }
    let mut all = true;
    for (f, _) in &c.files {
        let (orig, copy) = (below(&from, f), below(&to, f));
        if copy.symlink_metadata().is_err() {
            continue;
        }
        if !orig.is_file() {
            log::warn!("move: kept {}: its original isn't there", copy.display());
        } else if let Err(e) = remove_original(&copy) {
            log::warn!("move: couldn't remove the copy {}: {e}", copy.display());
            all = false;
        }
    }
    for d in c.dirs.iter().rev().map(|d| below(&to, d)).chain(std::iter::once(to.clone())) {
        if std::fs::remove_dir(&d).is_err() && d.exists() {
            log::warn!("move: left {} (not empty)", d.display());
        }
    }
    Ok(all && plugged(cat, c.from) && plugged(cat, c.to))
}

/// The catalog points at the copy: each original goes once its copy is there
/// and the same, then the folders it leaves empty. `check`: compare every
/// byte again (after a restart; a move that has just compared them doesn't).
/// Returns the originals left behind, or None while a disk is missing.
fn finish_copy(cat: &Catalog, c: &CopyOp, check: bool, done_one: &mut dyn FnMut()) -> Result<Option<Vec<String>>> {
    let Some((from, to)) = copy_paths(cat, c) else { return Ok(None) };
    let mut left = Vec::new();
    for (f, _) in &c.files {
        let (orig, copy) = (below(&from, f), below(&to, f));
        if orig.symlink_metadata().is_ok() {
            let copied = !check || (copy.is_file() && same_bytes(&orig, &copy).unwrap_or(false));
            if !copied {
                log::warn!("move: kept {}: its copy {} isn't the same", orig.display(), copy.display());
                left.push(orig.to_string_lossy().into_owned());
            } else if let Err(e) = remove_original(&orig) {
                log::warn!("move: couldn't remove {}: {e}", orig.display());
                left.push(orig.to_string_lossy().into_owned());
            } else {
                kill_point("copy:removed");
            }
        }
        done_one();
    }
    // Folders bottom-up; one that isn't empty (something left, or added meanwhile) stays.
    for d in c.dirs.iter().rev().map(|d| below(&from, d)).chain(std::iter::once(from.clone())) {
        if std::fs::remove_dir(&d).is_err() && d.exists() {
            log::warn!("move: left {} (not empty)", d.display());
        }
    }
    kill_point("copy:tidied");
    // A disk that went away halfway: what it hid is seen to next time.
    if !plugged(cat, c.from) || !plugged(cat, c.to) {
        return Ok(None);
    }
    Ok(Some(left))
}

/// A delete cut short: the catalog forgets each file that is gone from its
/// folder, keeps (still marked) each that is there. A folder that isn't
/// there says nothing about its files (promise 3): they stay.
fn settle_delete(cat: &Catalog, journal: Journal, d: &DeleteOp) -> Result<Settled> {
    let mut gone = Vec::new();
    for (vid, rel, name) in &d.files {
        let Some(mount) = cat.mount_of(*vid) else { return Ok(Settled::Waiting) };
        let folder = PathBuf::from(volumes::join(&mount, rel));
        if folder.is_dir() && folder.join(name).symlink_metadata().is_err() {
            gone.push((*vid, rel.clone(), name.clone()));
        }
    }
    let removed = forget_files(cat, &gone, &journal, &d.files)?;
    cat.drop_previews(&removed)?;
    journal.end(cat);
    Ok(if gone.is_empty() { Settled::Undone } else { Settled::Finished })
}

impl Op {
    fn describe(&self, cat: &Catalog) -> String {
        match self {
            Op::Rename(r) => format!("rename of {} to {}", place(cat, r.volume_id, &r.rel), shown(&r.new_rel)),
            Op::Copy(c) => format!("move of {} to {}", place(cat, c.from, &c.rel), place(cat, c.to, &c.new_rel)),
            Op::Delete(d) => format!("delete of {} file(s)", d.files.len()),
        }
    }

    /// The folders it works in.
    fn folders(&self) -> Vec<(i64, String)> {
        match self {
            Op::Rename(r) => vec![(r.volume_id, r.rel.clone()), (r.volume_id, r.new_rel.clone())],
            Op::Copy(c) => vec![(c.from, c.rel.clone()), (c.to, c.new_rel.clone())],
            Op::Delete(d) => d.files.iter().map(|(v, rel, _)| (*v, rel.clone())).collect(),
        }
    }
}

/// A folder's full path when its disk is plugged in, else its disk's name and path.
fn place(cat: &Catalog, volume_id: i64, rel: &str) -> String {
    if let Some(mount) = cat.mount_of(volume_id) {
        return volumes::join(&mount, rel);
    }
    let label: String = cat
        .db()
        .query_row("SELECT label FROM volumes WHERE id = ?1", [volume_id], |r| r.get(0))
        .unwrap_or_else(|_| format!("disk {volume_id}"));
    format!("{label}: {}", shown(rel))
}

/// Folders (disk id, path) that a rename, move or delete under way - or cut
/// short and not settled yet - works in. Scans leave them alone and no other
/// change may touch them meanwhile.
pub fn busy_folders(cat: &Catalog) -> Result<Vec<(i64, String)>> {
    let plans: Vec<String> =
        cat.db().prepare("SELECT plan FROM ops")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut out: Vec<(i64, String)> =
        plans.iter().filter_map(|p| serde_json::from_str::<Op>(p).ok()).flat_map(|op| op.folders()).collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// Refuses a change touching a folder another change is still working in.
fn refuse_if_busy(cat: &Catalog, folders: &[(i64, &str)]) -> Result<()> {
    let busy = busy_folders(cat)?;
    let clash =
        busy.iter().find(|(v, b)| folders.iter().any(|(fv, f)| fv == v && (db::is_under(f, b) || db::is_under(b, f))));
    if let Some((v, b)) = clash {
        bail!(
            "Shoebox hasn't finished an earlier move, rename or delete in {} yet: it finishes once the disks it \
             needs are plugged in. Nothing was changed.",
            place(cat, *v, b)
        );
    }
    Ok(())
}

/// Tests stop a change dead at a named step, the way a kill would: nothing
/// after it runs.
#[doc(hidden)]
pub mod testing {
    use std::cell::Cell;

    thread_local! {
        /// (step, how many more times, end the process)
        static KILL_AT: Cell<Option<(&'static str, usize, bool)>> = const { Cell::new(None) };
    }

    /// Stops this thread the `nth` time (from 1) it reaches `step`: a panic,
    /// so only destructors run (they roll back an open transaction, as a kill does).
    pub fn kill_at(step: &'static str, nth: usize) {
        KILL_AT.set(Some((step, nth, false)));
    }

    /// Ends the whole process there instead (exit code 9): not even
    /// destructors run, a lock's file stays, the catalog is never closed.
    pub fn exit_at(step: &'static str, nth: usize) {
        KILL_AT.set(Some((step, nth, true)));
    }

    pub(super) fn point(step: &str) {
        if let Some((at, n, exit)) = KILL_AT.get()
            && at == step
        {
            if n > 1 {
                KILL_AT.set(Some((at, n - 1, exit)));
                return;
            }
            KILL_AT.set(None);
            if exit {
                std::process::exit(9);
            }
            panic!("killed at {step}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_are_compared_byte_for_byte() {
        let dir = std::env::temp_dir().join("shoebox-same-bytes");
        std::fs::create_dir_all(&dir).unwrap();
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c"));
        std::fs::write(&a, &big).unwrap();
        std::fs::write(&b, &big).unwrap();
        let mut one_off = big.clone();
        one_off[2_500_000] ^= 1; // one bit, past the first 1 MB block
        std::fs::write(&c, &one_off).unwrap();
        assert!(same_bytes(&a, &b).unwrap());
        assert!(!same_bytes(&a, &c).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_names_windows_accepts() {
        assert_eq!(valid_name("  Trip to Paris ").unwrap(), "Trip to Paris");
        for bad in ["", " ", "..", "a.", "a/b", "a\\b", "what?", "CON", "com1", "LPT9.txt"] {
            assert!(valid_name(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(valid_name("Console").is_ok());
    }

    fn names(v: Vec<(String, u64, &'static str)>) -> Vec<String> {
        let mut v: Vec<String> = v.into_iter().map(|(n, _, role)| format!("{n} {role}")).collect();
        v.sort();
        v
    }

    #[test]
    fn a_raw_takes_its_twin_and_all_edits() {
        let entries: Vec<(String, u64)> = [
            "IMG_0001.CR3",
            "IMG_0001.JPG",
            "IMG_0001.CR3.agdata",
            "IMG_0001.CR3.2.agdata",
            "IMG_0001.JPG.agdata",
            "IMG_0001.xmp",
            "IMG_0010.CR3",
            "IMG_0010.JPG",
            "IMG_00011.JPG",
        ]
        .iter()
        .map(|n| (n.to_string(), 1))
        .collect();
        assert_eq!(
            names(companions("IMG_0001.CR3", "raw", &entries)),
            [
                "IMG_0001.CR3.2.agdata edits",
                "IMG_0001.CR3.agdata edits",
                "IMG_0001.JPG twin",
                "IMG_0001.JPG.agdata edits",
            ]
        );
    }

    #[test]
    fn folders_are_joined_and_noted_with_the_catalogs_separator() {
        assert_eq!(child_rel("", "Trip"), "Trip");
        assert_eq!(child_rel("Photos", "Trip"), "Photos\\Trip");
        assert_eq!(child_rel("Photos\\2026", "Trip"), "Photos\\2026\\Trip");
        // Below a folder, the journal notes the same on every system...
        let inside: PathBuf = ["Day 2", "RAW", "IMG_1.CR3"].into_iter().collect();
        assert_eq!(noted(&inside).as_deref(), Some("Day 2\\RAW\\IMG_1.CR3"));
        assert_eq!(noted(Path::new("IMG_1.CR3")).as_deref(), Some("IMG_1.CR3"));
        // ...and finds its way back to the files with the system's own separator.
        assert_eq!(below(Path::new("base"), "Day 2\\RAW\\IMG_1.CR3"), Path::new("base").join(&inside));
        assert_eq!(below(Path::new("base"), "IMG_1.CR3"), Path::new("base").join("IMG_1.CR3"));
        // Shown in a message as the system writes a path.
        assert_eq!(shown("Photos\\Trip"), ["Photos", "Trip"].join(std::path::MAIN_SEPARATOR_STR));
    }

    #[cfg(unix)]
    #[test]
    fn a_name_the_journal_cannot_note_stops_a_move() {
        use std::os::unix::ffi::OsStrExt;
        // A backslash is a name here, and would read as two folders in the journal.
        assert_eq!(noted(&Path::new("Day 2").join("a\\b")), None);
        let latin1 = Path::new("Day 2").join(std::ffi::OsStr::from_bytes(b"IMG_\xe9.jpg"));
        assert_eq!(noted(&latin1), None);
    }

    #[test]
    fn a_jpeg_takes_only_its_own_edits() {
        let entries: Vec<(String, u64)> =
            ["IMG_0001.CR3", "IMG_0001.JPG", "IMG_0001.JPG.agdata", "IMG_0001.CR3.agdata"]
                .iter()
                .map(|n| (n.to_string(), 1))
                .collect();
        assert_eq!(names(companions("IMG_0001.JPG", "image", &entries)), ["IMG_0001.JPG.agdata edits"]);
    }
}
