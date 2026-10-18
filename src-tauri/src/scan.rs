//! Scanning a catalog folder.
//!
//! 1. List: walk the folder, record every photo file with its size and date.
//! 2. Missing: files the catalog knew under this folder that were not seen are
//!    marked missing - only if the whole walk succeeded on a plugged-in disk.
//! 3. Read: new or changed files are fingerprinted. Known content is just linked
//!    (that's how moves and copies keep their photo); unknown content is read
//!    and gets its preview and thumbnail.
//!
//! Files are only ever opened for reading.

use crate::catalog::Catalog;
use crate::db::{self, FILE_MISSING, FILE_PRESENT, PREVIEW_FAILED, PREVIEW_OK, PREVIEW_PENDING, under_sql};
use crate::focus;
use crate::formats::{self, Kind};
use crate::imaging::{self, Meta};
use crate::organize;
use crate::settings::Settings;
use crate::volumes;
use anyhow::{Result, anyhow, bail};
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};
use walkdir::WalkDir;

/// A file the catalog already knows: (file id, photo id, size, mtime, state, the photo's preview state).
type KnownFile = (i64, Option<i64>, i64, i64, i64, Option<i64>);

#[derive(Clone, Default, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub root_id: i64,
    pub root_name: String,
    /// "listing", "reading" or "done"
    pub phase: String,
    pub found: u64,
    pub to_read: u64,
    pub read: u64,
    pub current: String,
}

#[derive(Clone, Default, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub root_id: i64,
    pub found: u64,
    pub new_files: u64,
    pub changed: u64,
    pub unchanged: u64,
    pub restored: u64,
    pub missing: u64,
    pub photos_added: u64,
    /// Files whose photo was already in the catalog before this scan (moved or copied).
    pub linked: u64,
    /// More copies of a photo this scan read: the same file twice in the folder.
    pub duplicates: u64,
    /// Known photos read again (regenerated previews, edited files, retried failures).
    pub reread: u64,
    pub errors: u64,
    /// Only the files that couldn't be read before, on a disk just plugged in.
    pub retry: bool,
    pub cancelled: bool,
    pub seconds: f64,
}

struct Job {
    file_id: i64,
    path: PathBuf,
    size: u64,
    mtime: i64,
    kind: Kind,
    ext: String,
    prev_photo: Option<i64>,
    /// Re-read even though the content is known (regenerate previews).
    force: bool,
    /// Read again at a plug-in, after it couldn't be read before.
    retry: bool,
}

struct Seen {
    rel_folder: String,
    name: String,
    size: u64,
    mtime: i64,
}

pub fn scan_root(
    cat: &Catalog,
    root_id: i64,
    cancel: &AtomicBool,
    regenerate: bool,
    on_progress: &(dyn Fn(&Progress) + Sync),
) -> Result<Report> {
    let started = Instant::now();
    let root = cat.root(root_id)?;
    let mount = cat.mount_of(root.volume_id).ok_or_else(|| anyhow!("{} is not plugged in", root.volume_label))?;
    let base = PathBuf::from(volumes::join(&mount, &root.rel_path));
    if !base.is_dir() {
        bail!("{} can't be found. Nothing was changed in the catalog.", base.display());
    }
    // Folders a move, rename or delete is working in (or one cut short, until
    // it is settled) are left as they are: neither read nor marked missing.
    let busy: Vec<String> =
        organize::busy_folders(cat)?.into_iter().filter(|(v, _)| *v == root.volume_id).map(|(_, rel)| rel).collect();
    if busy.iter().any(|b| db::is_under(&root.rel_path, b)) {
        bail!(
            "Shoebox is still finishing a move, rename or delete in {}: it finishes once the disks it needs are \
             plugged in. Nothing was changed in the catalog.",
            base.display()
        );
    }
    let root_name =
        base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| base.display().to_string());

    let scan_id = chrono::Utc::now().timestamp_millis();
    let mut report = Report { root_id, ..Default::default() };
    let mut progress = Progress { root_id, root_name, phase: "listing".into(), ..Default::default() };
    on_progress(&progress);

    // 1. List.
    let mut folder_ids: HashMap<String, i64> = HashMap::new();
    let mut errored: Vec<String> = busy.iter().filter(|b| db::is_under(b, &root.rel_path)).cloned().collect();
    let mut batch: Vec<Seen> = Vec::new();
    let mut jobs: Vec<Job> = Vec::new();
    let mut last_emit = Instant::now();
    // Never catalog Shoebox's own previews, wherever its catalog folder lives.
    let own = cat.data.root.to_string_lossy().to_lowercase();
    let walker = WalkDir::new(&base).follow_links(false).into_iter().filter_entry(|e| {
        !(e.depth() > 0
            && e.file_type().is_dir()
            && (formats::skip_dir(&e.file_name().to_string_lossy())
                || e.path().to_string_lossy().to_lowercase() == own
                || busy.iter().any(|b| db::is_under(&rel_of(&mount, e.path()), b))))
    });
    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                log::warn!("scan: can't read {:?}: {err}", err.path());
                errored.push(err.path().map(|p| rel_of(&mount, p)).unwrap_or_else(|| root.rel_path.clone()));
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if formats::kind_of(&name).is_none() {
            continue;
        }
        let parent = entry.path().parent().unwrap_or(&base);
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(err) => {
                log::warn!("scan: can't stat {}: {err}", entry.path().display());
                errored.push(rel_of(&mount, parent));
                continue;
            }
        };
        let mtime =
            md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        batch.push(Seen { rel_folder: rel_of(&mount, parent), name, size: md.len(), mtime });
        report.found += 1;
        if batch.len() >= 500 {
            record(
                cat,
                root.volume_id,
                &mount,
                scan_id,
                regenerate,
                &mut batch,
                &mut folder_ids,
                &mut jobs,
                &mut report,
            )?;
        }
        if last_emit.elapsed() > Duration::from_millis(250) {
            progress.found = report.found;
            on_progress(&progress);
            last_emit = Instant::now();
        }
    }
    record(cat, root.volume_id, &mount, scan_id, regenerate, &mut batch, &mut folder_ids, &mut jobs, &mut report)?;
    progress.found = report.found;

    // 2. Missing - only after a complete walk of a folder that is still there.
    let still_there = cat.mount_of(root.volume_id).as_deref() == Some(mount.as_str()) && base.is_dir();
    if !report.cancelled && still_there {
        report.missing = mark_missing(&cat.db(), root.volume_id, &root.rel_path, scan_id, &errored)?;
    }

    // 3. Read.
    read_all(cat, &jobs, cancel, &mut progress, on_progress, &mut report)?;

    if !report.cancelled {
        cat.db().execute(
            "UPDATE roots SET last_scan = ?1, reader = ?2 WHERE id = ?3",
            params![db::now(), imaging::PICTURE_READER, root_id],
        )?;
    }
    report.seconds = started.elapsed().as_secs_f64();
    progress.phase = "done".into();
    progress.current.clear();
    on_progress(&progress);
    log::info!("scan {root_id} done: {report:?}");
    Ok(report)
}

/// Files on this disk whose photo couldn't be read (or that couldn't even be
/// opened) last time, or whose reading was cut short: a photo is saved as
/// pending before its preview is written, so one still pending between scans
/// was left half-read by a Shoebox stopped in the middle. Not a photo given
/// up on: one still unreadable when retried, which no retry will change
/// (until Shoebox reads more formats; a folder scan tries it again).
const FAILED_FILES: &str = "SELECT f.id, f.photo_id, fo.rel_path, f.name, f.size, f.mtime
     FROM files f JOIN folders fo ON fo.id = f.folder_id LEFT JOIN photos p ON p.id = f.photo_id
    WHERE fo.volume_id = ?1 AND f.state = ?2
      AND (p.preview_state = ?4 OR (p.preview_state = ?3 AND COALESCE(p.given_up, 0) < ?5)
           OR (f.photo_id IS NULL AND f.error IS NOT NULL))";

/// Folders on this plugged-in disk last listed by a Shoebox that read fewer
/// formats ([`imaging::PICTURE_READER`]): scanned once more, so the files it
/// left out (Photoshop files...) join. Only those still there.
pub fn roots_to_list_again(cat: &Catalog, volume_id: i64) -> Result<Vec<i64>> {
    let Some(mount) = cat.mount_of(volume_id) else { return Ok(Vec::new()) };
    let roots: Vec<(i64, String)> = cat
        .db()
        .prepare("SELECT id, rel_path FROM roots WHERE volume_id = ?1 AND COALESCE(reader, 0) < ?2")?
        .query_map(params![volume_id, imaging::PICTURE_READER], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(roots.into_iter().filter(|(_, rel)| Path::new(&volumes::join(&mount, rel)).is_dir()).map(|(id, _)| id).collect())
}

/// How many files on this disk are waiting to be read again.
pub fn failed_count(cat: &Catalog, volume_id: i64) -> Result<u64> {
    let n: i64 = cat.db().query_row(
        &format!("SELECT COUNT(*) FROM ({FAILED_FILES})"),
        params![volume_id, FILE_PRESENT, PREVIEW_FAILED, PREVIEW_PENDING, imaging::PICTURE_READER],
        |r| r.get(0),
    )?;
    Ok(n as u64)
}

/// Reads again, on a plugged-in disk, only the files that couldn't be read
/// last time (the disk pulled out mid-read, a flaky cable...) or were left
/// half-read (Shoebox stopped mid-scan). Nothing is listed and nothing is
/// marked missing: this is not a scan of the folder. Runs between scans
/// (one at a time), when no photo is pending for any other reason.
pub fn retry_failed(
    cat: &Catalog,
    volume_id: i64,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(&Progress) + Sync),
) -> Result<Report> {
    let started = Instant::now();
    let mount = cat.mount_of(volume_id).ok_or_else(|| anyhow!("disk {volume_id} is not plugged in"))?;
    let label: String =
        cat.db().query_row("SELECT label FROM volumes WHERE id = ?1", [volume_id], |r| r.get(0)).unwrap_or_default();
    let failed: Vec<(i64, Option<i64>, String, String, i64, i64)> = cat
        .db()
        .prepare(FAILED_FILES)?
        .query_map(params![volume_id, FILE_PRESENT, PREVIEW_FAILED, PREVIEW_PENDING, imaging::PICTURE_READER], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut report = Report { retry: true, ..Default::default() };
    let mut jobs = Vec::new();
    for (file_id, photo_id, rel, name, size, mtime) in failed {
        let Some(kind) = formats::kind_of(&name) else { continue };
        let path = PathBuf::from(format!("{}\\{}", volumes::join(&mount, &rel).trim_end_matches('\\'), name));
        // Not there right now: left as it is (only a full scan may call it missing).
        let Ok(md) = std::fs::metadata(&path) else { continue };
        let now_mtime =
            md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        if md.len() as i64 != size || now_mtime != mtime {
            cat.db().execute(
                "UPDATE files SET size = ?1, mtime = ?2 WHERE id = ?3",
                params![md.len() as i64, now_mtime, file_id],
            )?;
        }
        report.found += 1;
        jobs.push(Job {
            file_id,
            path,
            size: md.len(),
            mtime: now_mtime,
            kind,
            ext: formats::extension(&name),
            prev_photo: photo_id,
            force: false,
            retry: true,
        });
    }
    // root_id 0: not a folder scan (the window words it as a retry).
    let mut progress = Progress { root_name: label, found: report.found, ..Default::default() };
    read_all(cat, &jobs, cancel, &mut progress, on_progress, &mut report)?;
    report.cancelled = cancel.load(Ordering::Relaxed);
    report.seconds = started.elapsed().as_secs_f64();
    progress.phase = "done".into();
    progress.current.clear();
    on_progress(&progress);
    log::info!("retry on disk {volume_id} done: {report:?}");
    Ok(report)
}

/// A photo marked unreadable only because saving its preview failed, while
/// its preview and thumbnail are in fact there and open (two copies of it
/// were read at once, before that was fixed): it is fine. Faces and word
/// search work from those files, so no disk is needed. Returns how many.
pub fn heal_saved_previews(cat: &Catalog) -> Result<usize> {
    let suspects: Vec<i64> = cat
        .db()
        .prepare("SELECT id FROM photos WHERE preview_state = ?1 AND error LIKE 'saving preview:%'")?
        .query_map([PREVIEW_FAILED], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut healed = 0;
    for pid in suspects {
        let (Ok(_), Ok((tw, th))) =
            (image::image_dimensions(cat.data.preview_path(pid)), image::image_dimensions(cat.data.thumb_path(pid)))
        else {
            continue;
        };
        cat.db().execute(
            "UPDATE photos SET preview_state = ?1, preview_rev = preview_rev + 1, thumb_w = ?2, thumb_h = ?3, error = NULL
              WHERE id = ?4 AND preview_state = ?5",
            params![PREVIEW_OK, tw, th, pid, PREVIEW_FAILED],
        )?;
        healed += 1;
    }
    if healed > 0 {
        log::info!("{healed} photos marked unreadable have their preview after all: marked ready");
    }
    Ok(healed)
}

/// Reads the files of `jobs`: fingerprint, then link to a known photo or read
/// it and make its preview. Copies of one photo are read once.
fn read_all(
    cat: &Catalog,
    jobs: &[Job],
    cancel: &AtomicBool,
    progress: &mut Progress,
    on_progress: &(dyn Fn(&Progress) + Sync),
    report: &mut Report,
) -> Result<()> {
    progress.phase = "reading".into();
    progress.to_read = jobs.len() as u64;
    on_progress(progress);
    let settings = cat.settings();
    let read = AtomicU64::new(0);
    let added = AtomicU64::new(0);
    let linked = AtomicU64::new(0);
    let duplicates = AtomicU64::new(0);
    let reread = AtomicU64::new(0);
    let errors = AtomicU64::new(0);
    let emit_gate = Mutex::new(Instant::now());
    let reading = Mutex::new(Reading::default());
    // Copies of a photo another worker was reading at that moment.
    let later: Mutex<Vec<(&Job, String)>> = Mutex::new(Vec::new());
    let tally = |job: &Job, outcome: Result<Outcome>| {
        let counter = match outcome {
            Ok(Outcome::Linked) => &linked,
            Ok(Outcome::Duplicate) => &duplicates,
            Ok(Outcome::Added) => &added,
            Ok(Outcome::Updated) => &reread,
            // Busy is set aside before it gets here; after the parallel pass nobody is reading.
            Ok(Outcome::Unreadable | Outcome::Busy) | Err(_) => &errors,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        let n = read.fetch_add(1, Ordering::Relaxed) + 1;
        let mut gate = emit_gate.lock().unwrap();
        if gate.elapsed() > Duration::from_millis(250) {
            *gate = Instant::now();
            let mut p = progress.clone();
            p.read = n;
            p.current = job.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            on_progress(&p);
        }
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(settings.workers).build()?;
    pool.install(|| {
        jobs.par_iter().for_each(|job| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let fp = match fingerprint(&job.path, job.size) {
                Ok(fp) => fp,
                Err(e) => {
                    set_file_error(cat, job.file_id, &e.to_string());
                    return tally(job, Err(e.into()));
                }
            };
            match ingest(cat, job, &fp, &settings, &reading) {
                Ok(Outcome::Busy) => later.lock().unwrap().push((job, fp)),
                outcome => tally(job, outcome),
            }
        })
    });
    // Their photo has been read by now: they are only linked to it.
    for (job, fp) in later.into_inner().unwrap() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        tally(job, ingest(cat, job, &fp, &settings, &reading));
    }
    if cancel.load(Ordering::Relaxed) {
        report.cancelled = true;
    }
    report.photos_added = added.into_inner();
    report.linked = linked.into_inner();
    report.duplicates = duplicates.into_inner();
    report.reread = reread.into_inner();
    report.errors = errors.into_inner();
    // New names grouped, new photos without a lens given their twin's.
    if let Err(e) = crate::gear::after_scan(&cat.db()) {
        log::warn!("gear names after reading: {e:#}");
    }
    progress.read = read.into_inner();
    Ok(())
}

/// Path relative to the volume root, backslashes, no leading separator.
fn rel_of(mount: &str, p: &Path) -> String {
    let s = p.to_string_lossy();
    s.get(mount.len()..).unwrap_or("").trim_matches('\\').to_string()
}

#[allow(clippy::too_many_arguments)]
fn record(
    cat: &Catalog,
    volume_id: i64,
    mount: &str,
    scan_id: i64,
    regenerate: bool,
    batch: &mut Vec<Seen>,
    folder_ids: &mut HashMap<String, i64>,
    jobs: &mut Vec<Job>,
    report: &mut Report,
) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let db = cat.db();
    let tx = db.unchecked_transaction()?;
    for s in batch.drain(..) {
        let folder_id = match folder_ids.get(&s.rel_folder) {
            Some(id) => *id,
            None => {
                tx.execute(
                    "INSERT OR IGNORE INTO folders (volume_id, rel_path) VALUES (?1, ?2)",
                    params![volume_id, s.rel_folder],
                )?;
                let id: i64 = tx.query_row(
                    "SELECT id FROM folders WHERE volume_id = ?1 AND rel_path = ?2",
                    params![volume_id, s.rel_folder],
                    |r| r.get(0),
                )?;
                folder_ids.insert(s.rel_folder.clone(), id);
                id
            }
        };
        let existing: Option<KnownFile> = tx
            .query_row(
                "SELECT f.id, f.photo_id, f.size, f.mtime, f.state, p.preview_state
                   FROM files f LEFT JOIN photos p ON p.id = f.photo_id
                  WHERE f.folder_id = ?1 AND f.name = ?2",
                params![folder_id, s.name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        let kind = formats::kind_of(&s.name).expect("filtered while listing");
        let path = PathBuf::from(format!("{}\\{}", volumes::join(mount, &s.rel_folder).trim_end_matches('\\'), s.name));
        let job = |file_id: i64, prev_photo: Option<i64>, force: bool| Job {
            file_id,
            path: path.clone(),
            size: s.size,
            mtime: s.mtime,
            kind,
            ext: formats::extension(&s.name),
            prev_photo,
            force,
            retry: false,
        };
        match existing {
            None => {
                tx.execute(
                    "INSERT INTO files (folder_id, name, size, mtime, state, seen_scan) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![folder_id, s.name, s.size as i64, s.mtime, FILE_PRESENT, scan_id],
                )?;
                jobs.push(job(tx.last_insert_rowid(), None, false));
                report.new_files += 1;
            }
            Some((id, Some(pid), size, mtime, state, preview)) if size == s.size as i64 && mtime == s.mtime => {
                tx.execute(
                    "UPDATE files SET seen_scan = ?1, state = ?2 WHERE id = ?3",
                    params![scan_id, FILE_PRESENT, id],
                )?;
                if state == FILE_MISSING {
                    report.restored += 1;
                }
                report.unchanged += 1;
                // Interrupted last time: finish the preview. Failed last time: try again.
                if regenerate || preview == Some(PREVIEW_PENDING) || preview == Some(PREVIEW_FAILED) {
                    jobs.push(job(id, Some(pid), regenerate));
                }
            }
            Some((id, prev_photo, ..)) => {
                tx.execute(
                    "UPDATE files SET size = ?1, mtime = ?2, seen_scan = ?3, state = ?4, error = NULL WHERE id = ?5",
                    params![s.size as i64, s.mtime, scan_id, FILE_PRESENT, id],
                )?;
                jobs.push(job(id, prev_photo, regenerate));
                report.changed += 1;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

fn mark_missing(db: &Connection, volume_id: i64, root_rel: &str, scan_id: i64, errored: &[String]) -> Result<u64> {
    let candidates: Vec<(i64, String)> = db
        .prepare(&format!(
            "SELECT f.id, fo.rel_path FROM files f JOIN folders fo ON fo.id = f.folder_id
              WHERE fo.volume_id = ?1 AND {} AND f.state = ?3 AND (f.seen_scan IS NULL OR f.seen_scan <> ?4)",
            under_sql("fo", 2)
        ))?
        .query_map(params![volume_id, root_rel, FILE_PRESENT, scan_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let tx = db.unchecked_transaction()?;
    let mut n = 0;
    for (id, rel) in candidates {
        // A folder we couldn't read is not evidence that its files are gone.
        if errored.iter().any(|e| db::is_under(&rel, e)) {
            continue;
        }
        tx.execute("UPDATE files SET state = ?1 WHERE id = ?2", params![FILE_MISSING, id])?;
        n += 1;
    }
    tx.commit()?;
    Ok(n)
}

enum Outcome {
    /// Its photo was already in the catalog.
    Linked,
    /// Another copy of a photo this scan read.
    Duplicate,
    Added,
    Updated,
    Unreadable,
    /// Another worker is reading the same content right now: try again after
    /// the parallel pass, when it will only be linked.
    Busy,
}

/// The contents being read right now, and those already read in this scan,
/// by fingerprint. Two copies of one photo are never read at the same time
/// (both would write its preview at once), nor twice in one scan.
#[derive(Default)]
struct Reading {
    now: HashSet<String>,
    done: HashSet<String>,
}

enum Claim<'a> {
    /// This worker reads it; the claim ends when the guard is dropped.
    Mine(ReadingGuard<'a>),
    Busy,
    Done,
}

struct ReadingGuard<'a> {
    reading: &'a Mutex<Reading>,
    fp: String,
}

impl Drop for ReadingGuard<'_> {
    fn drop(&mut self) {
        let mut r = self.reading.lock().unwrap_or_else(|e| e.into_inner());
        r.now.remove(&self.fp);
        r.done.insert(std::mem::take(&mut self.fp));
    }
}

fn claim<'a>(reading: &'a Mutex<Reading>, fp: &str) -> Claim<'a> {
    let mut r = reading.lock().unwrap();
    if r.done.contains(fp) {
        Claim::Done
    } else if !r.now.insert(fp.to_string()) {
        Claim::Busy
    } else {
        Claim::Mine(ReadingGuard { reading, fp: fp.to_string() })
    }
}

/// Size + BLAKE3 of the first and last 256 KB. Camera files carry their capture
/// time, serial and image data in those ranges, so this identifies the content
/// while reading 0.5 MB instead of 30.
pub fn fingerprint(path: &Path, size: u64) -> std::io::Result<String> {
    const CHUNK: usize = 256 * 1024;
    let mut f = File::open(path)?;
    let mut h = blake3::Hasher::new();
    h.update(&size.to_le_bytes());
    if size <= 2 * CHUNK as u64 {
        let mut buf = Vec::with_capacity(size as usize);
        f.read_to_end(&mut buf)?;
        h.update(&buf);
    } else {
        let mut buf = vec![0u8; CHUNK];
        f.read_exact(&mut buf)?;
        h.update(&buf);
        f.seek(SeekFrom::End(-(CHUNK as i64)))?;
        f.read_exact(&mut buf)?;
        h.update(&buf);
    }
    Ok(h.finalize().to_hex()[..32].to_string())
}

/// Second identity for a photo whose file was both moved and modified
/// (e.g. metadata rewritten by another app). Includes the extension so a
/// RAW+JPEG pair stays two photos.
fn capture_key(meta: &Meta, ext: &str) -> Option<String> {
    let t = meta.taken?;
    if meta.subsec.is_none() && meta.image_number.is_none() {
        return None;
    }
    Some(format!(
        "{ext}|{}|{}|{}|{}|{}|{}",
        meta.make.as_deref().unwrap_or(""),
        meta.model.as_deref().unwrap_or(""),
        meta.serial.as_deref().unwrap_or(""),
        t.format("%Y%m%d%H%M%S"),
        meta.subsec.as_deref().unwrap_or(""),
        meta.image_number.map(|n| n.to_string()).unwrap_or_default()
    ))
}

fn set_file_error(cat: &Catalog, file_id: i64, err: &str) {
    let _ = cat.db().execute("UPDATE files SET error = ?1 WHERE id = ?2", params![err, file_id]);
}

/// Points a file at its photo. Locations of that photo already confirmed
/// missing are dropped: the photo has been found again, so that was a move.
/// Runs inside the caller's transaction.
fn link(db: &Connection, file_id: i64, photo_id: i64) -> Result<()> {
    db.execute("UPDATE files SET photo_id = ?1, error = NULL WHERE id = ?2", params![photo_id, file_id])?;
    db.execute("DELETE FROM files WHERE photo_id = ?1 AND state = ?2", params![photo_id, FILE_MISSING])?;
    Ok(())
}

/// [`link`] in a transaction of its own: both changes land, or neither.
fn link_now(db: &Connection, file_id: i64, photo_id: i64) -> Result<()> {
    let tx = db.unchecked_transaction()?;
    link(&tx, file_id, photo_id)?;
    tx.commit()?;
    Ok(())
}

fn ingest(cat: &Catalog, job: &Job, fp: &str, s: &Settings, reading: &Mutex<Reading>) -> Result<Outcome> {
    let (preview_size, thumb_h) = (s.preview_size, s.thumb_height);
    let fp = fp.to_string();
    let photo_of = |db: &Connection| -> rusqlite::Result<Option<(i64, i64)>> {
        db.query_row("SELECT id, preview_state FROM photos WHERE fingerprint = ?1", [&fp], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
    };

    let _claim = match claim(reading, &fp) {
        Claim::Busy => return Ok(Outcome::Busy),
        // Read already in this scan (even if that failed): the same file again.
        Claim::Done => {
            let db = cat.db();
            if let Some((pid, _)) = photo_of(&db)? {
                link_now(&db, job.file_id, pid)?;
                return Ok(Outcome::Duplicate);
            }
            None
        }
        Claim::Mine(guard) => Some(guard),
    };

    // Known content: a move, a copy, or a file that didn't change. A photo
    // whose preview isn't done (interrupted, or failed last time) is read again.
    {
        let db = cat.db();
        if let Some((pid, state)) = photo_of(&db)?
            && state == PREVIEW_OK
            && !job.force
        {
            link_now(&db, job.file_id, pid)?;
            return Ok(Outcome::Linked);
        }
    }

    // New content: read it (the slow part, outside the lock).
    let loaded = imaging::load(&job.path, job.kind, preview_size);
    let meta = &loaded.meta;
    let (taken_ts, taken_exif) = match meta.taken {
        Some(t) => (t.and_utc().timestamp(), 1),
        None => (local_naive_ts(job.mtime), 0),
    };
    let ckey = capture_key(meta, &job.ext);
    let error = loaded.image.as_ref().err().cloned();

    let (pid, outcome, turn) = {
        let db = cat.db();
        let tx = db.unchecked_transaction()?;
        let others_present = |pid: i64| -> rusqlite::Result<bool> {
            tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM files WHERE photo_id = ?1 AND state = ?2 AND id <> ?3)",
                params![pid, FILE_PRESENT, job.file_id],
                |r| r.get(0),
            )
        };
        // Known content read again (preview unfinished or failed, or regenerating).
        let same_fp: Option<i64> =
            tx.query_row("SELECT id FROM photos WHERE fingerprint = ?1", [&fp], |r| r.get(0)).optional()?;
        // Same path, new content, and no other copy of the old content: edited in place.
        let edited = match (same_fp, job.prev_photo) {
            (None, Some(prev)) if !others_present(prev)? => Some(prev),
            _ => None,
        };
        // Moved and modified: same capture, and no copy of that photo left anywhere.
        let recaptured: Option<i64> = match (same_fp, edited, &ckey) {
            (None, None, Some(k)) => tx
                .query_row(
                    "SELECT p.id FROM photos p WHERE p.capture_key = ?1 AND NOT EXISTS
                       (SELECT 1 FROM files f WHERE f.photo_id = p.id AND f.state = ?2 AND f.id <> ?3) LIMIT 1",
                    params![k, FILE_PRESENT, job.file_id],
                    |r| r.get(0),
                )
                .optional()?,
            _ => None,
        };

        let focus = focus::stored(&tx, meta.focus.as_ref())?;
        let result = if let Some(pid) = same_fp.or(edited).or(recaptured) {
            // New content is looked at again by the upright model, unless the
            // user has decided how the photo stands.
            tx.execute(
                "UPDATE photos SET fingerprint = ?1, capture_key = ?2, taken_ts = ?3, taken_exif = ?4, make = ?5,
                        model = ?6, lens = COALESCE(?7, lens), width = ?8, height = ?9, iso = ?10, fnumber = ?11, exposure = ?12,
                        focal = ?13, preview_state = ?14, error = ?15, lens_read = ?17,
                        sideways = CASE WHEN ?18 AND COALESCE(sideways, 0) <> ?19 THEN NULL ELSE sideways END,
                        focus = ?20, focus_read = ?21
                  WHERE id = ?16",
                params![
                    fp, ckey, taken_ts, taken_exif, meta.make, meta.model, meta.lens, meta.width, meta.height,
                    meta.iso, meta.fnumber, meta.exposure, meta.focal, PREVIEW_PENDING, error, pid,
                    imaging::LENS_READER, same_fp.is_none(), db::SIDEWAYS_DECIDED, focus, focus::FOCUS_READER
                ],
            )?;
            // The user's turn stays with the photo.
            let turn: i64 = tx.query_row("SELECT turn FROM photos WHERE id = ?1", [pid], |r| r.get(0))?;
            (pid, Outcome::Updated, turn)
        } else {
            tx.execute(
                "INSERT INTO photos (fingerprint, capture_key, kind, taken_ts, taken_exif, make, model, lens, width,
                        height, iso, fnumber, exposure, focal, preview_state, error, added_at, lens_read, focus, focus_read)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
                params![
                    fp,
                    ckey,
                    job.kind.as_str(),
                    taken_ts,
                    taken_exif,
                    meta.make,
                    meta.model,
                    meta.lens,
                    meta.width,
                    meta.height,
                    meta.iso,
                    meta.fnumber,
                    meta.exposure,
                    meta.focal,
                    PREVIEW_PENDING,
                    error,
                    db::now(),
                    imaging::LENS_READER,
                    focus,
                    focus::FOCUS_READER
                ],
            )?;
            (tx.last_insert_rowid(), Outcome::Added, 0)
        };
        link(&tx, job.file_id, result.0)?;
        tx.commit()?;
        result
    };

    // Write the two JPEGs, turned as the user turned the photo, then mark it ready.
    let rendered = match &loaded.image {
        Ok(img) if turn == 0 => imaging::render(img, preview_size, s.preview_quality, thumb_h, 0),
        Ok(img) => {
            let t = turn.rem_euclid(4) as u8;
            imaging::render(&imaging::turned(img.clone(), t), preview_size, s.preview_quality, thumb_h, t)
        }
        Err(e) => Err(anyhow!("{e}")),
    }
    .map_err(|e| e.to_string());
    let saved = rendered.and_then(|r| {
        cat.data
            .write(&cat.data.preview_path(pid), &r.preview)
            .and_then(|_| cat.data.write(&cat.data.thumb_path(pid), &r.thumb))
            .map(|_| (r.thumb_w, r.thumb_h))
            .map_err(|e| format!("saving preview: {e}"))
    });
    let db = cat.db();
    match saved {
        Ok((tw, th)) => {
            db.execute(
                "UPDATE photos SET preview_state = ?1, preview_rev = preview_rev + 1, thumb_w = ?2, thumb_h = ?3,
                        error = NULL, given_up = NULL WHERE id = ?4",
                params![PREVIEW_OK, tw, th, pid],
            )?;
            // Turned while it was being read (the window): its files follow.
            let now: i64 = db.query_row("SELECT turn FROM photos WHERE id = ?1", [pid], |r| r.get(0))?;
            if now != turn {
                crate::turn::turn_files(cat, &db, pid, now, s.preview_quality)?;
            }
            Ok(outcome)
        }
        Err(e) => {
            log::warn!("no preview for {}: {e}", job.path.display());
            // Unreadable again when retried, its file still there (not a disk
            // pulled out meanwhile): retrying it at each plug-in changes nothing.
            let given_up = (job.retry && job.path.is_file()).then_some(imaging::PICTURE_READER);
            db.execute(
                "UPDATE photos SET preview_state = ?1, error = ?2, given_up = ?3 WHERE id = ?4",
                params![PREVIEW_FAILED, e, given_up, pid],
            )?;
            Ok(Outcome::Unreadable)
        }
    }
}

/// File times are UTC; photo times are the camera's local clock. Show a
/// photo without EXIF at the local time its file was written.
fn local_naive_ts(utc_secs: i64) -> i64 {
    chrono::DateTime::from_timestamp(utc_secs, 0)
        .map(|t| t.with_timezone(&chrono::Local).naive_local().and_utc().timestamp())
        .unwrap_or(0)
}
