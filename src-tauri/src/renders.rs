//! What photos edited in Argentum look like. Argentum renders them, started
//! as a separate program (its headless export; never linked, see
//! argentum.rs), and Shoebox keeps each result as an edited preview and
//! thumbnail, shown in place of the photo's own. Each edit of a photo is a
//! version (its own, each virtual copy's), named by the edit's fingerprint.
//!
//! Rendered only when asked: the window asks for the photos it shows (the
//! one open in the viewer first, then its view's, in order), the CLI for a
//! photo or a folder. An edit is rendered once, and again only when it
//! changes or when its photo's own preview is made again (new preview
//! settings, see [`stale`]): edited pictures are made as the photo's own are,
//! same size, quality and thumbnail height. The last edited pictures show
//! until the new ones are in. Only photos whose disk is plugged
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
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime};

/// Photos per Argentum start. Argentum answers only once its whole list is
/// done, so a batch is kept small: each lands within seconds, and other
/// photos asked for meanwhile wait at most for the batch under way.
const BATCH: usize = 2;
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
    /// There, but it couldn't be started (asked again next time).
    CantStart,
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
    /// Photos whose pictures shown changed so far.
    pub changed: u64,
    /// Those that changed since the last progress: the window shows them again.
    pub photos: Vec<Shown>,
    pub finished: bool,
}

/// A photo showing other pictures now, as the window draws it.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Shown {
    pub id: i64,
    pub rev: i64,
    /// Its thumbnail's size.
    pub w: i64,
    pub h: i64,
}

#[derive(Debug, Default, Clone)]
pub struct Options<'a> {
    /// Only these photos' edits, rendered in this order.
    pub photos: Option<Vec<i64>>,
    /// Only the edits in this folder (disk, folder as the catalog keeps it), not below it.
    pub folder: Option<(i64, String)>,
    /// Edits Argentum couldn't render are tried again too.
    pub retry: bool,
    /// Set once other photos are asked for: the look ends after its batch under way.
    pub newer: Option<&'a AtomicBool>,
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
    /// How long it may take to start, then per photo, before it is stopped.
    limits: (Duration, Duration),
}

impl Exporter {
    /// Argentum at `exe`, as [`crate::argentum::find`] gives it.
    pub fn new(exe: &Path) -> Self {
        Self { program: crate::argentum::executable(exe), before: Vec::new(), limits: (START_LIMIT, PHOTO_LIMIT) }
    }

    /// A program standing in for Argentum, started with `before` ahead of
    /// the arguments Argentum gets.
    pub fn wrapped(program: &Path, before: &[&str]) -> Self {
        let before = before.iter().map(|s| s.to_string()).collect();
        Self { program: program.to_path_buf(), before, limits: (START_LIMIT, PHOTO_LIMIT) }
    }

    /// Stopped after `start` and `per_photo` of its batch instead.
    pub fn limited(self, start: Duration, per_photo: Duration) -> Self {
        Self { limits: (start, per_photo), ..self }
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
        match run(cmd, self.limits.0, &AtomicBool::new(false), &mut |_| {}) {
            Ok(ran) if ran.stderr.contains("--files") => Support::Ready,
            Ok(_) => Support::Outdated,
            Err(e) => {
                log::warn!("asking {} what it can do: {e:#}", self.program.display());
                Support::CantStart
            }
        }
    }
}

/// Argentum as Settings has it, when it can render edits; otherwise why not.
/// Asked once per program and build (a new one is asked again); one that
/// couldn't start is asked again each time.
pub fn argentum(cat: &Catalog) -> Result<Exporter, Support> {
    static ASKED: Mutex<Option<(PathBuf, Option<SystemTime>, Support)>> = Mutex::new(None);
    let Some(exe) = crate::argentum::find(&cat.settings().argentum_path) else { return Err(Support::Missing) };
    let exporter = Exporter::new(&exe);
    let build = exporter.program.metadata().and_then(|m| m.modified()).ok();
    let known = match &*ASKED.lock().unwrap_or_else(|e| e.into_inner()) {
        Some((program, b, support)) if *program == exporter.program && *b == build => Some(*support),
        _ => None,
    };
    // Asked with nothing held: it can take seconds.
    let support = known.unwrap_or_else(|| {
        let support = exporter.probe();
        log::info!("{} renders edits: {support:?}", exporter.program.display());
        if support != Support::CantStart {
            *ASKED.lock().unwrap_or_else(|e| e.into_inner()) = Some((exporter.program.clone(), build, support));
        }
        support
    });
    if support == Support::Ready { Ok(exporter) } else { Err(support) }
}

/// A line of a program's output kept at most this long (a longer one is no
/// answer), this much of its output in all, and of its errors the last part.
const LINE_CAP: u64 = 64 * 1024;
const OUTPUT_CAP: u64 = 16 * 1024 * 1024;
const ERRORS_KEPT: usize = 16 * 1024;
/// How long what a program printed is still waited for once it ended (or was
/// stopped): a process it started can keep its output open forever.
const DRAIN_FOR: Duration = Duration::from_secs(2);

/// How a program started ended.
struct Ran {
    /// The end of what it said on stderr.
    stderr: String,
    /// Stopped before it ended: asked to (`cancel`), or it took too long.
    stopped: bool,
}

/// Runs `cmd` to its end, or stops it after `limit` or once `cancel` is set,
/// handing each line it prints to `on_line` as it comes.
fn run(mut cmd: Command, limit: Duration, cancel: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Result<Ran> {
    let mut child = cmd.spawn().with_context(|| format!("Couldn't start {}", cmd.get_program().display()))?;
    ends_with_shoebox(&child);
    let (out, err) = (child.stdout.take(), child.stderr.take());
    // Both read as they come (a full pipe would hold the program up), on
    // threads never waited for: they end when the output closes.
    let (lines, rx) = mpsc::channel();
    if let Some(out) = out {
        std::thread::spawn(move || read_lines(out, &lines));
    }
    let stderr = Arc::new(Mutex::new(String::new()));
    if let Some(err) = err {
        let kept = stderr.clone();
        std::thread::spawn(move || keep_tail(err, &kept));
    }
    let started = Instant::now();
    let (mut ended, mut stopped, mut reading) = (None::<Instant>, false, true);
    loop {
        if reading {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    on_line(&line);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => reading = false,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        match ended {
            Some(at) if !reading || at.elapsed() > DRAIN_FOR => break,
            Some(_) => {}
            None if child.try_wait()?.is_some() => ended = Some(Instant::now()),
            None if cancel.load(Ordering::Relaxed) || started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                (ended, stopped) = (Some(Instant::now()), true);
            }
            None if !reading => std::thread::sleep(Duration::from_millis(50)),
            None => {}
        }
    }
    let stderr = stderr.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(Ran { stderr, stopped })
}

/// Hands each line of `out` to `lines`, decoded as well as it can be: a line
/// longer than [`LINE_CAP`] is skipped, and past [`OUTPUT_CAP`] nothing is kept.
fn read_lines(out: impl Read, lines: &mpsc::Sender<String>) {
    let mut reader = BufReader::new(out);
    let (mut total, mut line) = (0, Vec::new());
    loop {
        line.clear();
        let Ok(n) = reader.by_ref().take(LINE_CAP).read_until(b'\n', &mut line) else { return };
        if n == 0 {
            return;
        }
        total += n as u64;
        if n as u64 == LINE_CAP && !line.ends_with(b"\n") {
            // Too long: the rest of it goes too.
            loop {
                line.clear();
                match reader.by_ref().take(LINE_CAP).read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => total += n as u64,
                }
                if line.ends_with(b"\n") {
                    break;
                }
            }
            continue;
        }
        if total <= OUTPUT_CAP && lines.send(String::from_utf8_lossy(&line).trim_end().to_string()).is_err() {
            return;
        }
    }
}

/// Keeps the last [`ERRORS_KEPT`] bytes of `err` in `kept`, as they come.
fn keep_tail(mut err: impl Read, kept: &Mutex<String>) {
    let mut buf = [0u8; 4096];
    while let Ok(n @ 1..) = err.read(&mut buf) {
        let mut kept = kept.lock().unwrap_or_else(|e| e.into_inner());
        kept.push_str(&String::from_utf8_lossy(&buf[..n]));
        if kept.len() > ERRORS_KEPT {
            let cut = kept.len() - ERRORS_KEPT;
            let cut = (cut..kept.len()).find(|&i| kept.is_char_boundary(i)).unwrap_or(kept.len());
            kept.drain(..cut);
        }
    }
}

/// Argentum goes when Shoebox does, however it ends: on Windows, in a job
/// that the system ends with Shoebox's last handle to it.
#[cfg(windows)]
fn ends_with_shoebox(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    /// The job, as a number: held open for as long as Shoebox runs.
    static JOB: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    let job = JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const limits).cast(), size).ok()?;
        Some(job.0 as usize)
    });
    let Some(job) = job else { return log::warn!("Argentum may outlive Shoebox: no job to put it in") };
    if let Err(e) = unsafe { AssignProcessToJobObject(HANDLE(*job as *mut _), HANDLE(child.as_raw_handle())) } {
        log::warn!("Argentum may outlive Shoebox: {e}");
    }
}

/// Elsewhere not yet: Argentum finishes its batch on its own.
#[cfg(not(windows))]
fn ends_with_shoebox(_: &std::process::Child) {}

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

    fn copy_id(&self) -> Option<&str> {
        (!self.copy.is_empty()).then_some(self.copy.as_str())
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
                AND f.photo_id IS NOT NULL"
        ))?
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    // Each photo asked for at its first place.
    let order: Option<HashMap<i64, usize>> =
        options.photos.as_ref().map(|p| p.iter().enumerate().rev().map(|(i, &id)| (id, i)).collect());
    let (mut jobs, mut waiting) = (Vec::new(), 0);
    for (file_id, copy, fingerprint, rendered, failed, photo, volume, rel, name) in rows {
        let asked = order.as_ref().is_none_or(|o| o.contains_key(&photo))
            && options.folder.as_ref().is_none_or(|(v, r)| *v == volume && *r == rel);
        if !asked {
            continue;
        }
        let there = fresh(cat, kept, photo, &version_of(&fingerprint));
        let done = rendered.as_ref() == Some(&fingerprint) && there;
        if done || (!options.retry && failed.as_ref() == Some(&fingerprint)) {
            continue;
        }
        match cat.located(volume, &rel, &name) {
            Ok(path) if Path::new(&path).is_file() => jobs.push(Job { file_id, copy, fingerprint, photo, path }),
            _ => waiting += 1,
        }
    }
    if let Some(order) = &order {
        jobs.sort_by_key(|j| order[&j.photo]);
    }
    Ok((jobs, waiting))
}

/// Whether version `version` of `photo` is kept (`kept`, see
/// [`crate::paths::DataDir::edited_versions`]) and made since the photo's own preview was.
fn fresh(cat: &Catalog, kept: &[(i64, String)], photo: i64, version: &str) -> bool {
    kept.binary_search_by(|(p, v)| (*p, v.as_str()).cmp(&(photo, version))).is_ok() && !stale(cat, photo, version)
}

/// Whether the kept pictures of an edit were made before the photo's own
/// preview last was (with other preview settings, after a rebuild): they are
/// made again, as the photo's own were.
fn stale(cat: &Catalog, photo: i64, version: &str) -> bool {
    let made = |path: PathBuf| {
        std::fs::metadata(&path)
            .or_else(|_| std::fs::metadata(crate::paths::DataDir::sealed_path(&path)))
            .and_then(|m| m.modified())
            .ok()
    };
    match (made(cat.data.edited_path(photo, version, false)), made(cat.data.preview_path(photo))) {
        (Some(edited), Some(own)) => edited < own,
        _ => false,
    }
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
    // One render per photo and edit, whichever of its files has it, in the
    // order asked; one already kept (another file's, a render cut short) needs none.
    let mut groups: Vec<Vec<Job>> = Vec::new();
    let mut at_group: HashMap<(i64, String), usize> = HashMap::new();
    for job in jobs {
        let i = *at_group.entry((job.photo, job.version())).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[i].push(job);
    }
    let (ready, todo): (Vec<_>, Vec<_>) =
        groups.into_iter().partition(|g| fresh(cat, &kept, g[0].photo, &g[0].version()));
    for group in &ready {
        rendered(cat, group)?;
    }
    let mut at = Progress { total: todo.len() as u64, ..Progress::default() };
    at.photos = settle_photos(cat, ready.iter().map(|g| g[0].photo), &[])?;
    report.changed += at.photos.len() as u64;
    let newer = || options.newer.is_some_and(|n| n.load(Ordering::SeqCst));
    for batch in todo.chunks(BATCH) {
        if cancel.load(Ordering::Relaxed) || newer() {
            break;
        }
        at.changed = report.changed;
        progress(&at);
        at.photos.clear();
        report.started += 1;
        let made = render_batch(cat, argentum, batch, cancel, &mut report)?;
        at.done += batch.len() as u64;
        // Shown as they come: with the next progress, right away.
        let shown = settle_photos(cat, batch.iter().map(|group| group[0].photo), &made)?;
        report.changed += shown.len() as u64;
        at.photos.extend(shown);
    }
    let tidied = tidy(cat)?;
    report.changed += tidied.len() as u64;
    at.photos.extend(tidied);
    at.changed = report.changed;
    progress(&Progress { finished: true, ..at });
    Ok(report)
}

/// Has Argentum render one batch, in a folder of Shoebox's own. Each picture
/// is kept as soon as Argentum says it is made (sealed at once for a private
/// photo), and those it couldn't make are recorded. Returns the photos it made pictures of.
fn render_batch(
    cat: &Catalog,
    argentum: &Exporter,
    batch: &[Vec<Job>],
    cancel: &AtomicBool,
    report: &mut Report,
) -> Result<Vec<i64>> {
    let dir = cat.data.render_dir()?;
    let (list, out) = (dir.path.join("list.txt"), dir.path.join("out"));
    let names: String = batch.iter().map(|group| group[0].listed() + "\n").collect();
    cat.data.write(&list, names.as_bytes())?;
    let mut cmd = argentum.command();
    cmd.arg("export").arg("--files").arg(&list).arg("--output").arg(&out);
    // As big as the photos' own previews: Argentum's picture is made into those.
    cmd.args(["--format", "jpeg", "--quality", "90", "--long-edge", &cat.settings().preview_size.to_string()]);
    let asked: HashMap<(&str, Option<&str>), usize> =
        batch.iter().enumerate().map(|(i, group)| ((group[0].path.as_str(), group[0].copy_id()), i)).collect();
    // Each one's answer: None kept, else why not.
    let mut answers: Vec<Option<Option<String>>> = vec![None; batch.len()];
    let (start, per_photo) = argentum.limits;
    let ran = run(cmd, start + per_photo * batch.len() as u32, cancel, &mut |line| {
        let Ok(a) = serde_json::from_str::<Answer>(line) else { return };
        let Some(&i) = asked.get(&(a.source.as_str(), a.copy.as_deref())) else { return };
        if answers[i].is_none() {
            answers[i] = Some(match (a.ok, &a.output) {
                (true, Some(file)) => keep(cat, &batch[i], file, &out).err().map(|e| format!("{e:#}")),
                _ => Some(a.error.unwrap_or_else(|| "Argentum couldn't render it".into())),
            });
        }
    })?;
    let cancelled = ran.stopped && cancel.load(Ordering::Relaxed);
    let mut made = Vec::new();
    for (group, answer) in batch.iter().zip(answers) {
        let failure = match answer {
            Some(failure) => failure,
            None if cancelled => continue,
            None if ran.stopped => Some("Argentum took too long and was stopped".into()),
            None => Some(match ran.stderr.lines().rev().find(|l| !l.trim().is_empty()) {
                Some(last) => format!("Argentum stopped without rendering it: {}", last.trim()),
                None => "Argentum stopped without rendering it".into(),
            }),
        };
        match failure {
            None => {
                report.rendered += 1;
                made.push(group[0].photo);
            }
            // Its disk went meanwhile: it waits, nothing failed.
            Some(_) if !Path::new(&group[0].path).is_file() => report.waiting += 1,
            Some(error) => {
                log::warn!("Argentum couldn't render {}: {error}", group[0].listed());
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
    Ok(made)
}

/// Keeps `file`, the picture Argentum says it made of `group`'s edit, once
/// it is sure to be in this batch's folder `out`: Argentum's answer is no
/// reason to read anything else. Its copy goes at once.
fn keep(cat: &Catalog, group: &[Job], file: &Path, out: &Path) -> Result<()> {
    let inside = file.canonicalize().ok().zip(out.canonicalize().ok()).is_some_and(|(f, o)| f.starts_with(o));
    if !inside {
        anyhow::bail!("Argentum's answer names a file outside its folder: {}", file.display());
    }
    let kept = store(cat, group, file);
    cat.data.remove(file)?;
    kept
}

/// Keeps Argentum's picture `file` as the edited preview and thumbnail of
/// the edit `group` has (sealed for a private photo).
fn store(cat: &Catalog, group: &[Job], file: &Path) -> Result<()> {
    let job = &group[0];
    let img = image::open(file).with_context(|| format!("Argentum's picture can't be read: {}", file.display()))?;
    let s = cat.settings();
    let r = imaging::render(&img, s.preview_size, s.preview_quality, s.thumb_height, 0)?;
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
/// the catalog. Returns the photos whose pictures shown changed.
fn tidy(cat: &Catalog) -> Result<Vec<Shown>> {
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
    settle_photos(cat, kept.into_iter().filter(|k| !shown.contains(k)).map(|(photo, _)| photo).chain(undone), &[])
}

/// [`settle`] for each of `photos` (once each), those in `made` just given
/// new pictures. Returns those whose pictures shown changed.
fn settle_photos(cat: &Catalog, photos: impl IntoIterator<Item = i64>, made: &[i64]) -> Result<Vec<Shown>> {
    let photos: BTreeSet<i64> = photos.into_iter().collect();
    let mut changed = Vec::new();
    for photo in photos {
        changed.extend(settle(cat, photo, &cat.data.edited_versions_of(photo), made.contains(&photo))?);
    }
    Ok(changed)
}

/// Photo `photo`, with edited pictures `kept` (versions): shows its own
/// edit's, else its first copy's (a missing file's only when no other has
/// one), else its own pictures; the window gets a new revision when that
/// changes, or its pictures were just made again (`made`); then returns how
/// it shows now. Edits undone forget their pictures.
fn settle(cat: &Catalog, photo: i64, kept: &[String], made: bool) -> Result<Option<Shown>> {
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
    let changed = was.as_ref() != shown || (made && shown.is_some());
    let mut now = None;
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
        now = db
            .query_row(
                "SELECT preview_rev, COALESCE(thumb_w, 0), COALESCE(thumb_h, 0) FROM photos WHERE id = ?1",
                [photo],
                |r| Ok(Shown { id: photo, rev: r.get(0)?, w: r.get(1)?, h: r.get(2)? }),
            )
            .optional()?;
    }
    drop(db);
    // Removed only once nothing shows them.
    let wanted: Vec<String> = wanted.iter().map(|fp| version_of(fp)).collect();
    for v in kept.iter().filter(|v| !wanted.contains(v)) {
        cat.remove_copy(&cat.data.edited_path(photo, v, false))?;
        cat.remove_copy(&cat.data.edited_path(photo, v, true))?;
    }
    Ok(now)
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
