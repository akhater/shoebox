//! What photos edited in Argentum look like. Argentum renders them, started
//! as a separate program (its headless export; never linked, see
//! argentum.rs), and Shoebox keeps each result as an edited preview and
//! thumbnail, shown in place of the photo's own. Each edit of a photo is a
//! version (its own, each virtual copy's), named by the edit's fingerprint.
//!
//! An edit is rendered once, and again only when it changes; the last edited
//! pictures show until the new ones are in. Only photos whose disk is plugged
//! in are rendered (Argentum reads the original); the others wait and keep
//! what they show (promise 2). An edit undone or gone shows the photo's own
//! pictures again. An edit Argentum can't render is tried again once it
//! changes, or when asked ([`Options::retry`]: at each start, from the CLI).

use crate::catalog::Catalog;
use crate::db::FILE_PRESENT;
use crate::imaging;
use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// Long edge of an edited preview, px: the viewer shows it as it is.
pub const LONG_EDGE: u32 = 2048;
/// Photos per Argentum start: they share its start-up, and progress shows between batches.
const BATCH: usize = 8;
/// How long Argentum may take before it is stopped: to start, then per photo.
const START_LIMIT: Duration = Duration::from_secs(60);
const PHOTO_LIMIT: Duration = Duration::from_secs(60);

/// Whether the Argentum Settings has can render edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Support {
    /// Not on this computer.
    Missing,
    /// From before its headless export took lists of photos and a size.
    Outdated,
    Ready,
}

/// A picture of a photo as the window shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture {
    Thumb,
    Preview,
    /// The photo's own preview, edited or not.
    Original,
}

/// What a look at the edits to render did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// Times Argentum was started.
    pub started: u64,
    pub rendered: u64,
    pub failed: u64,
    /// Edits on disks not plugged in.
    pub waiting: u64,
    /// Photos whose pictures shown changed.
    pub changed: u64,
}

/// How far a look has got: edits rendered (or not) of those it found to render.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    /// Photos whose pictures shown changed so far: the window shows them again.
    pub changed: u64,
    pub finished: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Options {
    /// Only this photo's edits.
    pub photo: Option<i64>,
    /// Edits Argentum couldn't render are tried again too.
    pub retry: bool,
}

/// The name of the version of an edit with this fingerprint.
pub fn version_of(fingerprint: &[u8]) -> String {
    fingerprint.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn is_version(v: &str) -> bool {
    v.len() == 16 && v.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Argentum's headless export.
pub struct Exporter {
    program: PathBuf,
    before: Vec<String>,
}

impl Exporter {
    /// Argentum at `exe`, as [`crate::argentum::find`] gives it.
    pub fn new(exe: &Path) -> Self {
        Self { program: crate::argentum::executable(exe), before: Vec::new() }
    }

    /// A program standing in for Argentum, started with `before` ahead of
    /// the arguments Argentum gets.
    pub fn wrapped(program: &Path, before: &[&str]) -> Self {
        Self { program: program.to_path_buf(), before: before.iter().map(|s| s.to_string()).collect() }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.before).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(folder) = self.program.parent() {
            cmd.current_dir(folder);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }

    /// Whether this Argentum renders lists of photos at a size. It has no
    /// `--help`: asked for `--files` without a list, it refuses at once and
    /// says so, while one from before ignores the flag it doesn't know.
    pub fn probe(&self) -> Support {
        let mut cmd = self.command();
        cmd.args(["export", "--files"]);
        match run(cmd, START_LIMIT, &AtomicBool::new(false)) {
            Ok(ran) if ran.stderr.contains("--files") => Support::Ready,
            Ok(_) => Support::Outdated,
            Err(e) => {
                log::warn!("asking {} what it can do: {e:#}", self.program.display());
                Support::Outdated
            }
        }
    }
}

/// Argentum as Settings has it, when it can render edits; otherwise why not.
/// Asked once per program and build (a new one is asked again).
pub fn argentum(cat: &Catalog) -> Result<Exporter, Support> {
    static ASKED: Mutex<Option<(PathBuf, Option<SystemTime>, Support)>> = Mutex::new(None);
    let Some(exe) = crate::argentum::find(&cat.settings().argentum_path) else { return Err(Support::Missing) };
    let exporter = Exporter::new(&exe);
    let build = exporter.program.metadata().and_then(|m| m.modified()).ok();
    let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
    let support = match &*asked {
        Some((program, b, support)) if *program == exporter.program && *b == build => *support,
        _ => {
            let support = exporter.probe();
            log::info!("{} renders edits: {support:?}", exporter.program.display());
            *asked = Some((exporter.program.clone(), build, support));
            support
        }
    };
    if support == Support::Ready { Ok(exporter) } else { Err(support) }
}

/// What a program started printed, and how it ended.
struct Ran {
    stdout: Vec<String>,
    stderr: String,
    /// Stopped before it ended: asked to (`cancel`), or it took too long.
    stopped: bool,
}

/// Runs `cmd` to its end, or stops it after `limit` or once `cancel` is set.
fn run(mut cmd: Command, limit: Duration, cancel: &AtomicBool) -> Result<Ran> {
    let mut child = cmd.spawn().with_context(|| format!("Couldn't start {}", cmd.get_program().display()))?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    // Both read as they come: a full pipe would hold the program up.
    let stdout = std::thread::spawn(move || out.map(|o| BufReader::new(o).lines().map_while(Result::ok).collect()));
    let stderr = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut e) = err {
            let _ = e.read_to_string(&mut text);
        }
        text
    });
    let started = Instant::now();
    let stopped = loop {
        if child.try_wait()?.is_some() {
            break false;
        }
        if cancel.load(Ordering::Relaxed) || started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break true;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = stdout.join().ok().flatten().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    Ok(Ran { stdout, stderr, stopped })
}

/// One line of Argentum's answer: a photo (and copy) asked for, and its file or why not.
#[derive(Deserialize)]
struct Answer {
    source: String,
    copy: Option<String>,
    ok: bool,
    output: Option<PathBuf>,
    error: Option<String>,
}

/// An edit to render: the sidecar of file `file_id` (copy `copy`, '' for the
/// file's own), of photo `photo`, at `path` on a disk plugged in.
struct Job {
    file_id: i64,
    copy: String,
    fingerprint: Vec<u8>,
    photo: i64,
    path: String,
}

impl Job {
    fn version(&self) -> String {
        version_of(&self.fingerprint)
    }

    /// How Argentum's list names it.
    fn listed(&self) -> String {
        if self.copy.is_empty() { self.path.clone() } else { format!("{}?vc={}", self.path, self.copy) }
    }
}

/// The edits to render, and how many wait for their disk.
fn pending(cat: &Catalog, options: &Options, kept: &[(i64, String)]) -> Result<(Vec<Job>, u64)> {
    type Row = (i64, String, Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>, i64, i64, String, String);
    let rows: Vec<Row> = cat
        .read()?
        .prepare(&format!(
            "SELECT e.file_id, e.copy_id, e.fingerprint, e.rendered, e.render_failed, f.photo_id, fo.volume_id,
                    fo.rel_path, f.name
               FROM edits e JOIN files f ON f.id = e.file_id JOIN folders fo ON fo.id = f.folder_id
              WHERE e.kind > 0 AND e.fingerprint IS NOT NULL AND f.state = {FILE_PRESENT}
                AND f.photo_id IS NOT NULL AND (?1 IS NULL OR f.photo_id = ?1)"
        ))?
        .query_map([options.photo], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let (mut jobs, mut waiting) = (Vec::new(), 0);
    for (file_id, copy, fingerprint, rendered, failed, photo, volume, rel, name) in rows {
        let there = kept.binary_search(&(photo, version_of(&fingerprint))).is_ok();
        let done = rendered.as_ref() == Some(&fingerprint) && there;
        if done || (!options.retry && failed.as_ref() == Some(&fingerprint)) {
            continue;
        }
        match cat.located(volume, &rel, &name) {
            Ok(path) if Path::new(&path).is_file() => jobs.push(Job { file_id, copy, fingerprint, photo, path }),
            _ => waiting += 1,
        }
    }
    Ok((jobs, waiting))
}

/// Renders the edits not rendered yet (see the module's notes) with
/// `argentum`, then lets go of the pictures of edits no longer there.
pub fn render_pending(
    cat: &Catalog,
    argentum: &Exporter,
    options: &Options,
    cancel: &AtomicBool,
    progress: &dyn Fn(&Progress),
) -> Result<Report> {
    let kept = cat.data.edited_versions();
    let (jobs, waiting) = pending(cat, options, &kept)?;
    let mut report = Report { waiting, ..Report::default() };
    // One render per photo and edit, whichever of its files has it; one
    // already kept (another file's, a render cut short) needs none.
    let mut groups: BTreeMap<(i64, String), Vec<Job>> = BTreeMap::new();
    for job in jobs {
        groups.entry((job.photo, job.version())).or_default().push(job);
    }
    let (ready, todo): (Vec<_>, Vec<_>) = groups.into_iter().partition(|(key, _)| kept.binary_search(key).is_ok());
    for (_, group) in &ready {
        rendered(cat, group)?;
    }
    report.changed += settle_photos(cat, ready.iter().map(|((photo, _), _)| *photo))?;
    let todo: Vec<Vec<Job>> = todo.into_iter().map(|(_, group)| group).collect();
    let mut at = Progress { total: todo.len() as u64, ..Progress::default() };
    for batch in todo.chunks(BATCH) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        progress(&at);
        report.started += 1;
        render_batch(cat, argentum, batch, cancel, &mut report)?;
        at.done += batch.len() as u64;
        // Shown as they come.
        report.changed += settle_photos(cat, batch.iter().map(|group| group[0].photo))?;
        at.changed = report.changed;
    }
    report.changed += tidy(cat)?;
    at.changed = report.changed;
    progress(&Progress { finished: true, ..at });
    Ok(report)
}

/// Has Argentum render one batch, in a folder of Shoebox's own, and keeps what it made.
fn render_batch(
    cat: &Catalog,
    argentum: &Exporter,
    batch: &[Vec<Job>],
    cancel: &AtomicBool,
    report: &mut Report,
) -> Result<()> {
    let dir = cat.data.fresh_render_dir()?;
    let list = dir.join("list.txt");
    let names: String = batch.iter().map(|group| group[0].listed() + "\n").collect();
    cat.data.write(&list, names.as_bytes())?;
    let mut cmd = argentum.command();
    cmd.arg("export").arg("--files").arg(&list).arg("--output").arg(dir.join("out"));
    cmd.args(["--format", "jpeg", "--quality", "90", "--long-edge", &LONG_EDGE.to_string()]);
    let kept = run(cmd, START_LIMIT + PHOTO_LIMIT * batch.len() as u32, cancel).and_then(|ran| {
        let cancelled = ran.stopped && cancel.load(Ordering::Relaxed);
        keep(cat, batch, &ran, cancelled, report)
    });
    cat.data.clear_render_dir()?;
    kept
}

/// Keeps the pictures Argentum made of a batch, and records those it couldn't.
fn keep(cat: &Catalog, batch: &[Vec<Job>], ran: &Ran, cancelled: bool, report: &mut Report) -> Result<()> {
    let answers: HashMap<(String, Option<String>), Answer> = ran
        .stdout
        .iter()
        .filter_map(|line| serde_json::from_str::<Answer>(line).ok())
        .map(|a| ((a.source.clone(), a.copy.clone()), a))
        .collect();
    for group in batch {
        let job = &group[0];
        let copy = (!job.copy.is_empty()).then(|| job.copy.clone());
        let failure = match answers.get(&(job.path.clone(), copy)) {
            Some(Answer { ok: true, output: Some(file), .. }) => match store(cat, group, file) {
                Ok(()) => None,
                Err(e) => Some(format!("{e:#}")),
            },
            Some(a) => Some(a.error.clone().unwrap_or_else(|| "Argentum couldn't render it".into())),
            None if cancelled => continue,
            None if ran.stopped => Some("Argentum took too long and was stopped".into()),
            None => Some(match ran.stderr.lines().rev().find(|l| !l.trim().is_empty()) {
                Some(last) => format!("Argentum stopped without rendering it: {}", last.trim()),
                None => "Argentum stopped without rendering it".into(),
            }),
        };
        match failure {
            None => report.rendered += 1,
            // Its disk went meanwhile: it waits, nothing failed.
            Some(_) if !Path::new(&job.path).is_file() => report.waiting += 1,
            Some(error) => {
                log::warn!("Argentum couldn't render {}: {error}", job.listed());
                report.failed += 1;
                let db = cat.db();
                for j in group {
                    db.execute(
                        "UPDATE edits SET render_failed = ?3, render_error = ?4 WHERE file_id = ?1 AND copy_id = ?2",
                        params![j.file_id, j.copy, j.fingerprint, error],
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Keeps Argentum's picture `file` as the edited preview and thumbnail of
/// the edit `group` has (sealed for a private photo).
fn store(cat: &Catalog, group: &[Job], file: &Path) -> Result<()> {
    let job = &group[0];
    let img = image::open(file).with_context(|| format!("Argentum's picture can't be read: {}", file.display()))?;
    let s = cat.settings();
    let r = imaging::render(&img, LONG_EDGE, s.preview_quality, s.thumb_height, 0)?;
    let v = job.version();
    // The preview last: a version is known by it (see `edited_versions`).
    cat.write_copy(job.photo, &cat.data.edited_path(job.photo, &v, true), &r.thumb)?;
    cat.write_copy(job.photo, &cat.data.edited_path(job.photo, &v, false), &r.preview)?;
    rendered(cat, group)
}

fn rendered(cat: &Catalog, group: &[Job]) -> Result<()> {
    let db = cat.db();
    for j in group {
        db.execute(
            "UPDATE edits SET rendered = ?3, render_failed = NULL, render_error = NULL
              WHERE file_id = ?1 AND copy_id = ?2",
            params![j.file_id, j.copy, j.fingerprint],
        )?;
    }
    Ok(())
}

/// The photos with edited pictures kept that no edit shows any more (its
/// sidecar gone, its edit undone, the photo gone): settled, so they show
/// their own pictures again and those are removed. Costs the edits, never
/// the catalog. Returns how many photos' pictures shown changed.
fn tidy(cat: &Catalog) -> Result<u64> {
    let kept = cat.data.edited_versions();
    let (shown, undone): (HashSet<(i64, String)>, Vec<i64>) = {
        let db = cat.read()?;
        let edits =
            "FROM edits e JOIN files f ON f.id = e.file_id WHERE f.photo_id IS NOT NULL AND e.rendered IS NOT NULL";
        let shown = db
            .prepare(&format!("SELECT f.photo_id, e.rendered {edits} AND e.kind > 0"))?
            .query_map([], |r| Ok((r.get(0)?, version_of(&r.get::<_, Vec<u8>>(1)?))))?
            .collect::<rusqlite::Result<_>>()?;
        let undone = db
            .prepare(&format!("SELECT DISTINCT f.photo_id {edits} AND e.kind = 0"))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        (shown, undone)
    };
    settle_photos(cat, kept.into_iter().filter(|k| !shown.contains(k)).map(|(photo, _)| photo).chain(undone))
}

/// [`settle`] for each of `photos` (once each). Returns how many changed.
fn settle_photos(cat: &Catalog, photos: impl IntoIterator<Item = i64>) -> Result<u64> {
    let photos: BTreeSet<i64> = photos.into_iter().collect();
    let mut changed = 0;
    for photo in photos {
        changed += u64::from(settle(cat, photo, &cat.data.edited_versions_of(photo))?);
    }
    Ok(changed)
}

/// Photo `photo`, with edited pictures `kept` (versions): shows its own
/// edit's, else its first copy's (a missing file's only when no other has
/// one), else its own pictures; the window gets a new revision when that
/// changes (returns true). Edits undone forget their pictures.
fn settle(cat: &Catalog, photo: i64, kept: &[String]) -> Result<bool> {
    let wanted: Vec<Vec<u8>> = {
        let db = cat.db();
        db.execute(
            "UPDATE edits SET rendered = NULL
              WHERE kind = 0 AND rendered IS NOT NULL AND file_id IN (SELECT id FROM files WHERE photo_id = ?1)",
            [photo],
        )?;
        db.prepare(
            "SELECT e.rendered FROM files f JOIN edits e ON e.file_id = f.id
              WHERE f.photo_id = ?1 AND e.kind > 0 AND e.rendered IS NOT NULL
              ORDER BY e.copy_id <> '', f.state, e.copy_id, f.id",
        )?
        .query_map([photo], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?
    };
    let shown = wanted.iter().find(|fp| kept.contains(&version_of(fp)));
    let db = cat.db();
    let was: Option<Vec<u8>> =
        db.query_row("SELECT edited FROM photos WHERE id = ?1", [photo], |r| r.get(0)).optional()?.flatten();
    let changed = was.as_ref() != shown;
    if changed {
        // The grid lays out the thumbnail it shows.
        let thumb = match shown {
            Some(fp) => cat.data.edited_path(photo, &version_of(fp), true),
            None => cat.data.thumb_path(photo),
        };
        let (w, h) = cat.copy_dimensions(&thumb).map_or((None, None), |(w, h)| (Some(w), Some(h)));
        db.execute(
            "UPDATE photos SET edited = ?2, preview_rev = preview_rev + 1,
                    thumb_w = COALESCE(?3, thumb_w), thumb_h = COALESCE(?4, thumb_h) WHERE id = ?1",
            params![photo, shown, w, h],
        )?;
    }
    drop(db);
    // Removed only once nothing shows them.
    let wanted: Vec<String> = wanted.iter().map(|fp| version_of(fp)).collect();
    for v in kept.iter().filter(|v| !wanted.contains(v)) {
        cat.remove_copy(&cat.data.edited_path(photo, v, false))?;
        cat.remove_copy(&cat.data.edited_path(photo, v, true))?;
    }
    Ok(changed)
}

/// The file of a photo's picture as the window shows it: its edited one when it has one.
pub fn shown(cat: &Catalog, photo: i64, what: Picture) -> Result<PathBuf> {
    let edited: Option<Vec<u8>> = match what {
        Picture::Original => None,
        _ => cat
            .read()?
            .query_row("SELECT edited FROM photos WHERE id = ?1", [photo], |r| r.get(0))
            .optional()?
            .flatten(),
    };
    Ok(match (what, edited) {
        (Picture::Thumb, Some(fp)) => cat.data.edited_path(photo, &version_of(&fp), true),
        (Picture::Preview, Some(fp)) => cat.data.edited_path(photo, &version_of(&fp), false),
        (Picture::Thumb, None) => cat.data.thumb_path(photo),
        _ => cat.data.preview_path(photo),
    })
}

/// The file of one edited version's preview or thumbnail; None for a name
/// that isn't a version's.
pub fn version(cat: &Catalog, photo: i64, version: &str, what: Picture) -> Option<PathBuf> {
    match what {
        _ if !is_version(version) => None,
        Picture::Original => Some(cat.data.preview_path(photo)),
        _ => Some(cat.data.edited_path(photo, version, what == Picture::Thumb)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_is_named_by_its_edit_and_nothing_else_is_one() {
        assert_eq!(version_of(&[0xab; 32]), "abababababababab");
        assert!(is_version("0123456789abcdef"));
        for bad in ["", "0123456789abcde", "../../../etc/pa", "0123456789abcdeg"] {
            assert!(!is_version(bad), "{bad}");
        }
    }
}
