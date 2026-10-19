//! Shoebox's own files left damaged by a PC that stopped mid-write (a crash, a
//! power cut, a forced restart): the file is there at its full size, but what
//! was in it never reached the disk - zeros. Writes are on the disk before
//! they take a file's place now ([`crate::paths`]), so this finds what
//! happened before that, or a failing disk.
//!
//! Looked into: the previews, thumbnails and face pictures written since the
//! last check, or all of them the first time; at start, only once the PC has
//! restarted since that check (until it does, nothing written can be lost).
//! Missing ones are noticed too. A thumbnail or face picture is made again
//! from its photo's preview. A photo whose preview is damaged or missing is
//! read again from its original, like one left half-read: at once when its
//! disk is plugged in, otherwise when it is.

use crate::catalog::Catalog;
use crate::db::{FILE_PRESENT, PREVIEW_OK, PREVIEW_PENDING};
use anyhow::{Result, bail};
use rayon::prelude::*;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Instant, UNIX_EPOCH};
use walkdir::WalkDir;

#[derive(Clone, Default, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Checkup {
    /// Files looked into.
    pub checked: u64,
    /// Photos whose preview was damaged or missing: waiting to be read again
    /// from their original.
    pub to_read_again: Vec<i64>,
    /// Thumbnails made again from their photo's preview, no disk needed.
    pub thumbs_made_again: u64,
    /// Disks holding those photos (`shoebox-cli retry <disk>` reads them).
    pub disks: Vec<i64>,
    /// Face pictures cut again from their photo's preview.
    pub faces_cut_again: u64,
    /// Face pictures whose photo's preview is damaged too: cut by a later
    /// check, once the photo has been read again.
    pub faces_waiting: u64,
    pub seconds: f64,
}

/// When the last check began, in the catalog's `meta`.
const CHECKED_KEY: &str = "files_checked";

/// A file written this long before a check began is looked at again by the
/// next one: file times and the clock are not exact to the second.
const MARGIN_SECS: i64 = 120;

/// The check Shoebox makes at start: none while the PC hasn't restarted since
/// the last one.
pub fn at_start(cat: &Catalog) -> Result<Option<Checkup>> {
    if !due(last_checked(cat)?, booted_at()) {
        return Ok(None);
    }
    check_files(cat, false).map(Some)
}

/// Looks into the files written since the last check (`all`: every file),
/// and repairs what it can (see the module's notes).
pub fn check_files(cat: &Catalog, all: bool) -> Result<Checkup> {
    let started = Instant::now();
    let began = now_secs();
    let since = if all { None } else { last_checked(cat)?.map(|t| t - MARGIN_SECS) };
    // Read before looking: a photo ready by then had its files written before.
    let ready: HashSet<i64> = ids(cat, &format!("SELECT id FROM photos WHERE preview_state = {PREVIEW_OK}"))?;
    let faces: HashSet<i64> = ids(cat, "SELECT id FROM faces")?;

    let mut c = Checkup::default();
    // Opening each file is the slow part (the antivirus looks at it too): a
    // few at a time, in a pool of their own so the window's pictures still come.
    let pool = rayon::ThreadPoolBuilder::new().num_threads(8).build()?;
    let mut look = |sub: &str| -> (HashSet<i64>, HashSet<i64>) {
        let mut there = HashSet::new();
        let mut fresh = Vec::new();
        for e in WalkDir::new(cat.data.root.join(sub)).min_depth(2).max_depth(2).into_iter().flatten() {
            // A private photo's copy is sealed (see private.rs).
            let name = e.file_name().to_string_lossy();
            let id = name.strip_suffix(".jpg").or_else(|| name.strip_suffix(".sealed"));
            let Some(id) = id.and_then(|s| s.parse::<i64>().ok()) else { continue };
            there.insert(id);
            let modified = e.metadata().ok().and_then(|m| m.modified().ok());
            let secs =
                modified.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(i64::MAX, |d| d.as_secs() as i64);
            if since.is_none_or(|s| secs >= s) {
                fresh.push((id, e.into_path()));
            }
        }
        c.checked += fresh.len() as u64;
        let damaged = pool.install(|| {
            fresh.par_iter().filter(|(_, path)| whole_copy(path) == Some(false)).map(|(id, _)| *id).collect()
        });
        (there, damaged)
    };
    let (previews, bad_previews) = look("previews");
    let (thumbs, bad_thumbs) = look("thumbs");
    let (pictures, bad_pictures) = look("faces");

    // A crash spoils the files of its last seconds. Most of them gone is
    // something else (a catalog move under way, a folder taken away): left alone.
    let gone = ready.iter().filter(|id| !previews.contains(id) || !thumbs.contains(id)).count();
    let gone_faces = faces.iter().filter(|f| !pictures.contains(f)).count();
    if (gone > 50 && gone * 4 > ready.len()) || (gone_faces > 50 && gone_faces * 4 > faces.len()) {
        bail!(
            "{gone} of {} previews and {gone_faces} of {} face pictures aren't in {}: nothing changed",
            ready.len(),
            faces.len(),
            cat.data.root.display()
        );
    }

    // A thumbnail is a small copy of the preview: made again from it when the
    // preview is fine. A photo whose preview isn't is read again from its original.
    let bad_preview = |id: &i64| !previews.contains(id) || bad_previews.contains(id);
    let bad_thumb = |id: &i64| !thumbs.contains(id) || bad_thumbs.contains(id);
    let mut again: BTreeSet<i64> = ready.iter().copied().filter(bad_preview).collect();
    let thumb_h = cat.settings().thumb_height;
    for &id in ready.iter().filter(|id| !bad_preview(id) && bad_thumb(id)) {
        match thumb_from_preview(cat, id, thumb_h) {
            Ok(()) => c.thumbs_made_again += 1,
            Err(e) => {
                log::warn!("photo {id}: thumbnail can't be made from the preview: {e:#}");
                again.insert(id);
            }
        }
    }
    let mut disks = BTreeSet::new();
    {
        let db = cat.db();
        let tx = db.unchecked_transaction()?;
        for &id in &again {
            if tx.execute(
                "UPDATE photos SET preview_state = ?1 WHERE id = ?2 AND preview_state = ?3",
                params![PREVIEW_PENDING, id, PREVIEW_OK],
            )? > 0
            {
                c.to_read_again.push(id);
            }
            let mut stmt = tx.prepare_cached(
                "SELECT DISTINCT fo.volume_id FROM files f JOIN folders fo ON fo.id = f.folder_id
                  WHERE f.photo_id = ?1 AND f.state = ?2",
            )?;
            for v in stmt.query_map(params![id, FILE_PRESENT], |r| r.get::<_, i64>(0))? {
                disks.insert(v?);
            }
        }
        tx.commit()?;
    }
    c.disks = disks.into_iter().collect();

    // Face pictures: cut again from a preview that is fine.
    let lost: Vec<i64> = faces.iter().copied().filter(|f| !pictures.contains(f) || bad_pictures.contains(f)).collect();
    if !lost.is_empty() {
        let photo_of: HashMap<i64, i64> = cat
            .db()
            .prepare("SELECT id, photo_id FROM faces")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let (cuttable, waiting): (Vec<i64>, Vec<i64>) =
            lost.into_iter().partition(|f| photo_of.get(f).is_some_and(|p| ready.contains(p) && !again.contains(p)));
        c.faces_cut_again = crate::faces::cut_pictures_again(cat, &cuttable)?.len() as u64;
        // A damaged picture goes, so a later check sees it missing, whatever its date.
        for f in &waiting {
            if bad_pictures.contains(f) {
                cat.remove_copy(&cat.data.face_path(*f))?;
            }
        }
        c.faces_waiting = (cuttable.len() as u64 - c.faces_cut_again) + waiting.len() as u64;
    }

    cat.db()
        .execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![CHECKED_KEY, began.to_string()])?;
    c.seconds = started.elapsed().as_secs_f64();
    if !c.to_read_again.is_empty() || c.thumbs_made_again > 0 || c.faces_cut_again > 0 || c.faces_waiting > 0 {
        log::warn!(
            "saved files checked: {} photos with a damaged or missing preview to read again (disks {:?}), \
             {} thumbnails made again, {} face pictures cut again, {} waiting for their preview",
            c.to_read_again.len(),
            c.disks,
            c.thumbs_made_again,
            c.faces_cut_again,
            c.faces_waiting
        );
    }
    log::info!("saved files checked: {} files in {:.1} s", c.checked, c.seconds);
    Ok(c)
}

/// A photo's thumbnail made again from its preview, as a scan makes it
/// (a private photo's only while unlocked: otherwise read again from its original).
fn thumb_from_preview(cat: &Catalog, id: i64, thumb_h: u32) -> Result<()> {
    let bytes = cat.read_copy(&cat.data.preview_path(id))?;
    let img = image::load_from_memory(&bytes)?;
    // It shows what the preview shows, turn included.
    let turn = crate::imaging::turn_mark(&bytes);
    let r = crate::imaging::render(&img, img.width().max(img.height()), 80, thumb_h, turn)?;
    cat.write_copy(id, &cat.data.thumb_path(id), &r.thumb)?;
    // A new revision: the window asks for the new thumbnail.
    cat.db().execute(
        "UPDATE photos SET preview_rev = preview_rev + 1, thumb_w = ?1, thumb_h = ?2 WHERE id = ?3",
        params![r.thumb_w, r.thumb_h, id],
    )?;
    Ok(())
}

/// A copy as Shoebox writes it: a JPEG, or sealed (a private photo's, see
/// private.rs). None: it couldn't be opened right now (not a sign of damage).
fn whole_copy(path: &Path) -> Option<bool> {
    if path.extension().is_some_and(|e| e == "sealed") { whole_sealed(path) } else { whole_jpeg(path) }
}

/// A sealed copy: its mark first, and an end that isn't zeros (the seal's
/// last 16 bytes, which can't all be zero). Its content can only be checked
/// with the key.
fn whole_sealed(path: &Path) -> Option<bool> {
    let mut f = File::open(path).ok()?;
    let (mut head, mut tail) = ([0u8; 4], [0u8; 16]);
    if f.read_exact(&mut head).is_err() || f.seek(SeekFrom::End(-16)).is_err() || f.read_exact(&mut tail).is_err() {
        return Some(false);
    }
    Some(&head == crate::private::SEALED_MAGIC && tail.iter().any(|b| *b != 0))
}

/// A JPEG as Shoebox writes them: its start marker first, its end marker
/// last. A file the PC never finished putting on the disk is zeros, in whole
/// or at its end. None: it couldn't be opened right now (not a sign of damage).
fn whole_jpeg(path: &Path) -> Option<bool> {
    let mut f = File::open(path).ok()?;
    let (mut head, mut tail) = ([0u8; 2], [0u8; 2]);
    if f.read_exact(&mut head).is_err() || f.seek(SeekFrom::End(-2)).is_err() || f.read_exact(&mut tail).is_err() {
        return Some(false);
    }
    Some(head == [0xFF, 0xD8] && tail == [0xFF, 0xD9])
}

/// A check is due when there never was one, or the PC has started since
/// (or when that can't be told).
fn due(last_checked: Option<i64>, booted_at: Option<i64>) -> bool {
    match (last_checked, booted_at) {
        (Some(last), Some(booted)) => booted >= last - MARGIN_SECS,
        _ => true,
    }
}

fn last_checked(cat: &Catalog) -> Result<Option<i64>> {
    let v: Option<String> =
        cat.db().query_row("SELECT value FROM meta WHERE key = ?1", [CHECKED_KEY], |r| r.get(0)).optional()?;
    Ok(v.and_then(|v| v.parse().ok()))
}

fn ids(cat: &Catalog, sql: &str) -> Result<HashSet<i64>> {
    Ok(cat.db().prepare(sql)?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// When the PC last started (the time since includes sleep).
#[cfg(windows)]
fn booted_at() -> Option<i64> {
    let up_ms = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
    Some(now_secs() - (up_ms / 1000) as i64)
}

#[cfg(not(windows))]
fn booted_at() -> Option<i64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_is_due_after_a_restart() {
        assert!(due(None, Some(1_000)));
        assert!(due(Some(1_000), None));
        assert!(due(Some(1_000), Some(5_000)), "the PC started after the last check");
        assert!(!due(Some(5_000), Some(1_000)), "same PC session as the last check");
    }
}
