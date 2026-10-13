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

use crate::catalog::Catalog;
use crate::db::{self, FILE_PRESENT};
use crate::formats::{self, Kind};
use crate::volumes;
use anyhow::{Result, bail};
use rusqlite::params;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Marks (or unmarks) photos for deletion. Catalog only: no file is touched.
pub fn mark(cat: &Catalog, photo_ids: &[i64], marked: bool) -> Result<usize> {
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
    Ok(cat.db().execute("UPDATE photos SET marked = 0 WHERE marked = 1", [])?)
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
    pub disks: Vec<PlanDisk>,
    pub files: Vec<PlanFile>,
    pub bytes: u64,
    /// Identifies this exact list; a delete must quote it.
    pub token: String,
}

/// What deleting the marked photos would remove, right now.
pub fn plan(cat: &Catalog) -> Result<DeletePlan> {
    let (rows, marked, labels) = {
        let db = cat.db();
        let rows: Vec<(i64, String, i64, String, String, i64)> = db
            .prepare(&format!(
                "SELECT p.id, p.kind, fo.volume_id, fo.rel_path, f.name, f.size FROM photos p
                   JOIN files f ON f.photo_id = p.id JOIN folders fo ON fo.id = f.folder_id
                  WHERE p.marked = 1 AND f.state = {FILE_PRESENT} ORDER BY fo.volume_id, fo.rel_path, f.name"
            ))?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let marked: i64 = db.query_row("SELECT COUNT(*) FROM photos WHERE marked = 1", [], |r| r.get(0))?;
        let labels: HashMap<i64, String> = db
            .prepare("SELECT id, label FROM volumes")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        (rows, marked as usize, labels)
    };

    let mut plan = DeletePlan { marked, ..Default::default() };
    let mut seen = BTreeSet::new();
    let mut here = BTreeSet::new();
    let mut on_disk = BTreeSet::new();
    let mut disks: BTreeMap<i64, PlanDisk> = BTreeMap::new();
    // Each folder is listed once, however many marked photos it holds.
    let mut listings: HashMap<PathBuf, Vec<(String, u64)>> = HashMap::new();
    for (pid, kind, vid, rel, name, size) in rows {
        let Some(mount) = cat.mount_of(vid) else { continue };
        let folder = PathBuf::from(volumes::join(&mount, &rel));
        let path = folder.join(&name);
        if !path.is_file() {
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
            if seen.insert(s.to_lowercase()) {
                disk.files += 1;
                disk.bytes += size;
                plan.bytes += size;
                plan.files.push(PlanFile { path: s, size, role: role.into(), volume_id: vid });
            }
        }
    }
    plan.files.sort_by(|a, b| (a.volume_id, &a.path).cmp(&(b.volume_id, &b.path)));
    plan.photos = here.len();
    plan.waiting = marked.saturating_sub(here.len());
    plan.disks = disks.into_values().collect();
    plan.token = token(&plan.files);
    Ok(plan)
}

fn list_folder(folder: &Path) -> Vec<(String, u64)> {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok().filter(|m| m.is_file())?;
            Some((e.file_name().to_string_lossy().into_owned(), m.len()))
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

fn token(files: &[PlanFile]) -> String {
    let mut h = blake3::Hasher::new();
    for f in files {
        h.update(f.path.as_bytes());
        h.update(&f.size.to_le_bytes());
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
    pub failed: Vec<String>,
}

/// Deletes the plan the user was shown (`token`), on the disks they ticked. Refuses
/// if the plan changed since.
pub fn delete_confirmed(cat: &Catalog, token: &str, volume_ids: &[i64]) -> Result<DeleteReport> {
    let current = plan(cat)?;
    if current.token != token {
        bail!("The list of files changed since you looked at it. Nothing was deleted - please review it again.");
    }
    let chosen: Vec<&PlanFile> = current.files.iter().filter(|f| volume_ids.contains(&f.volume_id)).collect();
    if chosen.is_empty() {
        bail!("No disk ticked: nothing was deleted.");
    }
    let disk = |vid: i64| current.disks.iter().find(|d| d.volume_id == vid).expect("every file's disk is in the plan");
    let (to_bin, to_nuke): (Vec<&PlanFile>, Vec<&PlanFile>) =
        chosen.iter().copied().partition(|f| disk(f.volume_id).recycle);

    let mut report = DeleteReport::default();
    if !to_bin.is_empty() {
        let paths: Vec<&Path> = to_bin.iter().map(|f| Path::new(&f.path)).collect();
        if let Err(e) = trash::delete_all(&paths) {
            log::warn!("recycle bin: {e}");
            report.failed.push(format!("Recycle Bin: {e}"));
        }
    }
    for f in &to_nuke {
        if let Err(e) = std::fs::remove_file(&f.path) {
            report.failed.push(format!("{}: {e}", f.path));
        }
    }

    // The catalog forgets only what is really gone.
    let mut gone = Vec::new();
    for f in &chosen {
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
        let rest = f.path.get(d.mount.trim_end_matches('\\').len()..).unwrap_or("").trim_start_matches('\\');
        let (rel, name) = rest.rsplit_once('\\').unwrap_or(("", rest));
        gone.push((f.volume_id, rel.to_string(), name.to_string()));
    }
    report.photos_removed = forget_files(cat, &gone)?;
    Ok(report)
}

/// Drops the catalog rows of deleted files (volume id, folder, name). Photos
/// left with no file leave the catalog with their previews and faces.
/// Returns how many photos left.
fn forget_files(cat: &Catalog, gone: &[(i64, String, String)]) -> Result<usize> {
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
        let mut people = BTreeSet::new();
        for pid in &touched {
            let left: bool =
                tx.query_row("SELECT EXISTS (SELECT 1 FROM files WHERE photo_id = ?1)", [pid], |r| r.get(0))?;
            if left {
                continue;
            }
            let mut stmt = tx.prepare("SELECT person_id FROM faces WHERE photo_id = ?1 AND person_id IS NOT NULL")?;
            people.extend(stmt.query_map([pid], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
            drop(stmt);
            tx.execute("DELETE FROM faces WHERE photo_id = ?1", [pid])?;
            tx.execute("DELETE FROM face_scan WHERE photo_id = ?1", [pid])?;
            tx.execute("DELETE FROM photos WHERE id = ?1", [pid])?;
            removed.push(*pid);
        }
        // People whose only faces were in deleted photos.
        for person in people {
            tx.execute(
                "DELETE FROM people WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = ?1)",
                [person],
            )?;
        }
        tx.commit()?;
    }
    for pid in &removed {
        let _ = cat.data.remove(&cat.data.preview_path(*pid));
        let _ = cat.data.remove(&cat.data.thumb_path(*pid));
    }
    Ok(removed.len())
}

/// A name Windows accepts for a folder.
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
    if parent.is_empty() { name.to_string() } else { format!("{parent}\\{name}") }
}

/// Renames a catalog folder on its disk and in the catalog in one step: its
/// photos and sub-folders go with it and nothing is read again. Returns the
/// folder's new path on its disk.
pub fn rename_folder(cat: &Catalog, volume_id: i64, rel: &str, new_name: &str) -> Result<String> {
    let name = valid_name(new_name)?;
    if rel.is_empty() {
        bail!("A whole disk can't be renamed here: double-click the disk to change its name in Shoebox.");
    }
    let (parent, old) = rel.rsplit_once('\\').unwrap_or(("", rel));
    if old == name {
        return Ok(rel.to_string());
    }
    let from = PathBuf::from(cat.folder_path(volume_id, rel)?);
    let new_rel = child_rel(parent, name);
    let to = from.with_file_name(name);
    // "trip" -> "Trip" is the same folder on Windows; anything else must be free.
    if to.exists() && !old.eq_ignore_ascii_case(name) {
        bail!("There is already a folder or file called {name} there.");
    }
    let taken: bool = cat.db().query_row(
        &format!("SELECT EXISTS (SELECT 1 FROM folders WHERE volume_id = ?1 AND {})", db::under_sql("folders", 2)),
        params![volume_id, new_rel],
        |r| r.get(0),
    )?;
    if taken && !old.eq_ignore_ascii_case(name) {
        bail!(
            "The catalog still remembers a folder called {name} there (its photos are missing). Choose another name."
        );
    }
    std::fs::rename(&from, &to).map_err(|e| anyhow::anyhow!("Couldn't rename {}: {e}", from.display()))?;
    if let Err(e) = move_in_catalog(cat, (volume_id, rel), (volume_id, &new_rel), Roots::Follow) {
        // Keep disk and catalog in step: put the folder back.
        let _ = std::fs::rename(&to, &from);
        return Err(e);
    }
    log::info!("renamed folder {} -> {}", from.display(), to.display());
    let _ = cat.refresh_roots();
    Ok(new_rel)
}

/// What happens to catalog roots at or under a folder that moves.
#[derive(Clone, Copy, PartialEq)]
enum Roots {
    /// Renamed in place: they keep being roots, at their new path.
    Follow,
    /// Moved into another root's folder: they become part of that root.
    Absorb,
}

/// Points the catalog's folders at or under `old` (disk, path) to `new`.
fn move_in_catalog(cat: &Catalog, old: (i64, &str), new: (i64, &str), roots: Roots) -> Result<()> {
    let mut db = cat.db();
    let tx = db.transaction()?;
    for table in ["folders", "roots"] {
        let rows: Vec<(i64, String)> = tx
            .prepare(&format!("SELECT id, rel_path FROM {table} WHERE volume_id = ?1 AND {}", db::under_sql(table, 2)))?
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
    tx.commit()?;
    Ok(())
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
    let (parent, name) = m.rel.rsplit_once('\\').unwrap_or(("", &m.rel));
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
    Ok((from, to, new_rel))
}

/// Moves a folder with everything in it into another catalog folder, on the
/// disk and in the catalog in one step; nothing is read again.
/// - Same disk: a rename on the disk, instant, nothing copied.
/// - Another disk: copy every file, flush it to the disk, compare every byte,
///   update the catalog, then remove the originals one by one. A failure or
///   `cancel` before the catalog changes removes the partial copy and leaves
///   the original as it was.
pub fn move_folder(
    cat: &Catalog,
    m: &FolderMove,
    cancel: &AtomicBool,
    progress: &dyn Fn(&MoveProgress),
) -> Result<MoveResult> {
    let (from, to, new_rel) = check_move(cat, m)?;
    if m.from == m.to {
        std::fs::rename(&from, &to).map_err(|e| anyhow::anyhow!("Couldn't move {}: {e}", from.display()))?;
        if let Err(e) = move_in_catalog(cat, (m.from, &m.rel), (m.to, &new_rel), Roots::Absorb) {
            let _ = std::fs::rename(&to, &from);
            return Err(e);
        }
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
        let e = e.map_err(|e| anyhow::anyhow!("Can't read part of {}: {e}. Nothing was moved.", from.display()))?;
        let inside = e.path().strip_prefix(&from)?.to_path_buf();
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
    let mut p = MoveProgress { phase: "copying".into(), total_files: files.len(), total_bytes, ..Default::default() };
    progress(&p);

    // Only what this move made is ever undone.
    let mut made_dirs = Vec::new();
    let mut made_files = Vec::new();
    let undo = |made_files: &[PathBuf], made_dirs: &[PathBuf]| {
        for f in made_files {
            let _ = remove_original(f);
        }
        for d in made_dirs.iter().rev() {
            let _ = std::fs::remove_dir(d);
        }
    };
    let copy_all = |made_files: &mut Vec<PathBuf>, made_dirs: &mut Vec<PathBuf>, p: &mut MoveProgress| -> Result<()> {
        std::fs::create_dir(&to)?;
        made_dirs.push(to.clone());
        for d in &dirs {
            std::fs::create_dir(to.join(d))?;
            made_dirs.push(to.join(d));
        }
        for (f, size) in &files {
            if cancel.load(Ordering::Relaxed) {
                bail!("Stopped. Nothing was moved.");
            }
            let dst = to.join(f);
            std::fs::copy(from.join(f), &dst)?; // keeps the file's dates, so scans see it unchanged
            made_files.push(dst.clone());
            // On the disk, not just in memory (a read-only copy can't be opened to flush; Windows writes it soon anyway).
            if let Ok(out) = std::fs::OpenOptions::new().write(true).open(&dst) {
                out.sync_all()?;
            }
            p.files += 1;
            p.bytes += size;
            progress(p);
        }
        *p = MoveProgress { phase: "checking".into(), total_files: files.len(), total_bytes, ..Default::default() };
        progress(p);
        for (f, size) in &files {
            if cancel.load(Ordering::Relaxed) {
                bail!("Stopped. Nothing was moved.");
            }
            if !same_bytes(&from.join(f), &to.join(f))? {
                bail!("The copy of {} came out different. Nothing was moved.", from.join(f).display());
            }
            p.files += 1;
            p.bytes += size;
            progress(p);
        }
        Ok(())
    };
    if let Err(e) = copy_all(&mut made_files, &mut made_dirs, &mut p) {
        undo(&made_files, &made_dirs);
        return Err(e);
    }
    if let Err(e) = move_in_catalog(cat, (m.from, &m.rel), (m.to, &new_rel), Roots::Absorb) {
        undo(&made_files, &made_dirs);
        return Err(e);
    }
    log::info!(
        "copied and checked {} files ({total_bytes} bytes): {} -> {}",
        files.len(),
        from.display(),
        to.display()
    );

    // The catalog now points at the checked copy; the originals can go.
    p = MoveProgress { phase: "removing".into(), total_files: files.len(), total_bytes, ..Default::default() };
    progress(&p);
    let mut left_behind = Vec::new();
    for (f, _) in &files {
        let path = from.join(f);
        if let Err(e) = remove_original(&path) {
            log::warn!("move: couldn't remove {}: {e}", path.display());
            left_behind.push(path.to_string_lossy().into_owned());
        }
        p.files += 1;
        progress(&p);
    }
    // Folders bottom-up; one that isn't empty (something left, or added meanwhile) stays.
    for d in dirs.iter().rev().map(|d| from.join(d)).chain(std::iter::once(from.clone())) {
        if std::fs::remove_dir(&d).is_err() && d.exists() {
            log::warn!("move: left {} (not empty)", d.display());
        }
    }
    let _ = cat.refresh_roots();
    Ok(MoveResult { volume_id: m.to, rel: new_rel, copied: true, files: files.len(), bytes: total_bytes, left_behind })
}

/// Removes an original after its copy was checked; a read-only one is made
/// writable first (it is going anyway).
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
    fn a_jpeg_takes_only_its_own_edits() {
        let entries: Vec<(String, u64)> =
            ["IMG_0001.CR3", "IMG_0001.JPG", "IMG_0001.JPG.agdata", "IMG_0001.CR3.agdata"]
                .iter()
                .map(|n| (n.to_string(), 1))
                .collect();
        assert_eq!(names(companions("IMG_0001.JPG", "image", &entries)), ["IMG_0001.JPG.agdata edits"]);
    }
}
