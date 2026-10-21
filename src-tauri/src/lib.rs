pub mod ai;
pub mod catalog;
pub mod cull;
pub mod dates;
pub mod db;
pub mod embedded;
pub mod faces;
pub mod focus;
pub mod formats;
pub mod gear;
pub mod imaging;
pub mod models;
pub mod organize;
pub mod paths;
pub mod private;
pub mod psd;
pub mod repair;
pub mod repoint;
pub mod scan;
pub mod search;
pub mod settings;
pub mod stacks;
pub mod tags;
pub mod trash_bin;
pub mod turn;
pub mod updates;
pub mod upright;
pub mod volumes;
#[cfg(windows)]
pub mod wic;

mod commands;

use catalog::Catalog;
use paths::DataDir;
use scan::Progress;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// What the scan worker does next.
#[derive(Clone, Copy, PartialEq, Debug)]
enum ScanTask {
    /// Scan a catalog folder (and regenerate its previews).
    Folder(i64, bool),
    /// Read again the files of this disk that couldn't be read before.
    Retry(i64),
}

/// Folders waiting to be scanned, one scan at a time.
#[derive(Default)]
pub struct ScanQueue {
    queue: Mutex<VecDeque<ScanTask>>,
    wake: Condvar,
    pub cancel: AtomicBool,
    pub current: Mutex<Option<Progress>>,
    /// Set while the catalog is being moved: no new scans start.
    pub paused: AtomicBool,
}

impl ScanQueue {
    pub fn push(&self, root_id: i64, regenerate: bool) {
        if self.paused.load(Ordering::Relaxed) {
            return;
        }
        let mut q = self.queue.lock().unwrap();
        let running = self.current.lock().unwrap().as_ref().is_some_and(|p| p.root_id == root_id);
        if let Some(ScanTask::Folder(_, regen)) =
            q.iter_mut().find(|t| matches!(t, ScanTask::Folder(id, _) if *id == root_id))
        {
            *regen |= regenerate;
        } else if !running || regenerate {
            q.push_back(ScanTask::Folder(root_id, regenerate));
            self.wake.notify_one();
        }
    }

    /// A disk holding files that couldn't be read was plugged in: read those again.
    pub fn retry(&self, volume_id: i64) {
        if self.paused.load(Ordering::Relaxed) {
            return;
        }
        let mut q = self.queue.lock().unwrap();
        if !q.contains(&ScanTask::Retry(volume_id)) {
            q.push_back(ScanTask::Retry(volume_id));
            self.wake.notify_one();
        }
    }

    /// Drops a folder's queued scans (it was removed from the catalog).
    pub fn forget(&self, root_id: i64) {
        self.queue.lock().unwrap().retain(|t| !matches!(t, ScanTask::Folder(id, _) if *id == root_id));
    }

    pub fn pending(&self) -> Vec<i64> {
        self.queue
            .lock()
            .unwrap()
            .iter()
            .filter_map(|t| match t {
                ScanTask::Folder(id, _) => Some(*id),
                ScanTask::Retry(_) => None,
            })
            .collect()
    }

    /// Stops the running scan and drops the queued ones.
    pub fn cancel_all(&self) {
        self.queue.lock().unwrap().clear();
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn next(&self) -> ScanTask {
        let mut q = self.queue.lock().unwrap();
        loop {
            if let Some(item) = q.pop_front() {
                return item;
            }
            q = self.wake.wait(q).unwrap();
        }
    }
}

pub struct AppState {
    pub cat: Arc<Catalog>,
    pub models: Arc<ModelQueue>,
    pub scans: Arc<ScanQueue>,
    pub search: Arc<search::SearchIndex>,
    pub faces: Arc<faces::FaceIndex>,
    pub quality: Arc<cull::QualityIndex>,
    pub upright: Arc<upright::Upright>,
    pub measure: Arc<MeasureQueue>,
    pub closer: Arc<CloserQueue>,
    pub ai: Arc<AiQueue>,
    /// A folder being moved to another disk (one at a time), and its Stop.
    pub moving: Arc<AtomicBool>,
    pub move_cancel: Arc<AtomicBool>,
    /// What the last look for a newer Shoebox found.
    pub updates: Arc<Mutex<Option<updates::UpdateStatus>>>,
}

/// Wakes the AI worker when there may be new photos to describe.
#[derive(Default)]
pub struct AiQueue {
    kicked: Mutex<bool>,
    wake: Condvar,
    pub cancel: AtomicBool,
    pub current: Mutex<Option<search::AiProgress>>,
}

impl AiQueue {
    pub fn kick(&self) {
        *self.kicked.lock().unwrap() = true;
        self.wake.notify_one();
    }

    fn wait(&self) {
        let mut k = self.kicked.lock().unwrap();
        while !*k {
            k = self.wake.wait(k).unwrap();
        }
        *k = false;
    }
}

/// Photos of a shoot opened in Cull that need measuring, from their
/// previews: the latest asked for first.
#[derive(Default)]
pub struct MeasureQueue {
    queue: Mutex<VecDeque<i64>>,
    wake: Condvar,
}

impl MeasureQueue {
    pub fn push(&self, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let asked: HashSet<i64> = ids.iter().copied().collect();
        let mut q = self.queue.lock().unwrap();
        q.retain(|id| !asked.contains(id));
        for id in ids.iter().rev() {
            q.push_front(*id);
        }
        self.wake.notify_one();
    }

    /// Up to `n` photos, waiting for some.
    fn take(&self, n: usize) -> Vec<i64> {
        let mut q = self.queue.lock().unwrap();
        while q.is_empty() {
            q = self.wake.wait(q).unwrap();
        }
        let n = n.min(q.len());
        q.drain(..n).collect()
    }

    fn is_empty(&self) -> bool {
        self.queue.lock().unwrap().is_empty()
    }
}

/// Measures the shoots Cull opens, a few photos at a time, so a shoot opened
/// since goes first. Says `cull-changed` as measures come in (every 2 s at
/// most) and once they all have.
fn measure_worker(app: AppHandle, cat: Arc<Catalog>, quality: Arc<cull::QualityIndex>, q: Arc<MeasureQueue>) {
    let never = AtomicBool::new(false);
    let (mut said, mut unsaid) = (Instant::now(), false);
    // Photos measured since the queue was last empty, and since when.
    let mut run: Option<(usize, Instant)> = None;
    loop {
        let ids = q.take(64);
        let (count, _) = run.get_or_insert_with(|| (0, Instant::now()));
        let measured = quality.measure(&cat, &ids, &never, &|_, _| {}).unwrap_or_else(|e| {
            log::warn!("cull: measuring failed: {e:#}");
            0
        });
        *count += measured;
        unsaid |= measured > 0;
        let all = q.is_empty();
        if unsaid && (all || said.elapsed() > Duration::from_secs(2)) {
            (said, unsaid) = (Instant::now(), false);
            let _ = app.emit("cull-changed", ());
        }
        if all && let Some((n, at)) = run.take().filter(|(n, _)| *n > 0) {
            log::info!("cull: {n} photos measured in {:.1} s", at.elapsed().as_secs_f64());
        }
    }
}

/// Photos Cull asked to look closer at (their originals), in batches.
#[derive(Default)]
pub struct CloserQueue {
    queue: Mutex<Vec<i64>>,
    wake: Condvar,
    cancel: AtomicBool,
    pub current: Mutex<Option<cull::CloserProgress>>,
}

impl CloserQueue {
    pub fn push(&self, ids: &[i64]) {
        let mut q = self.queue.lock().unwrap();
        let queued: HashSet<i64> = q.iter().copied().collect();
        q.extend(ids.iter().filter(|id| !queued.contains(id)));
        self.wake.notify_one();
    }

    /// Stops the batch under way and forgets the queued ones.
    pub fn stop(&self) {
        self.queue.lock().unwrap().clear();
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn take(&self) -> Vec<i64> {
        let mut q = self.queue.lock().unwrap();
        while q.is_empty() {
            q = self.wake.wait(q).unwrap();
        }
        std::mem::take(&mut *q)
    }
}

/// Looks closer at the photos Cull asks for: their originals, read at half
/// size from the disks plugged in. Reports `cull-progress`, and `cull-changed`
/// as the measures come in.
fn closer_worker(app: AppHandle, cat: Arc<Catalog>, quality: Arc<cull::QualityIndex>, q: Arc<CloserQueue>) {
    loop {
        let ids = q.take();
        q.cancel.store(false, Ordering::Relaxed);
        let changed = Mutex::new(Instant::now());
        let progress = |done: u64, total: u64| {
            let p = cull::CloserProgress { done, total, finished: false };
            *q.current.lock().unwrap() = Some(p.clone());
            let _ = app.emit("cull-progress", p);
            let mut last = changed.lock().unwrap();
            if last.elapsed() > Duration::from_secs(2) {
                *last = Instant::now();
                let _ = app.emit("cull-changed", ());
            }
        };
        if let Err(e) = quality.look_closer(&cat, &ids, &q.cancel, &progress) {
            log::warn!("cull: looking closer failed: {e:#}");
        }
        let last = q.current.lock().unwrap().take().unwrap_or_default();
        let _ = app.emit("cull-progress", cull::CloserProgress { finished: true, ..last });
        let _ = app.emit("cull-changed", ());
    }
}

/// Wakes a background worker that may have something new to do.
#[derive(Default)]
pub struct Kick {
    kicked: Mutex<bool>,
    wake: Condvar,
}

impl Kick {
    pub fn kick(&self) {
        *self.kicked.lock().unwrap() = true;
        self.wake.notify_one();
    }

    fn wait(&self) {
        let mut k = self.kicked.lock().unwrap();
        while !*k {
            k = self.wake.wait(k).unwrap();
        }
        *k = false;
    }
}

/// Opens once an index has read what it keeps in memory (word search's
/// descriptions, people's average faces), so it can be read in the background
/// while the window is already open: until then, its users wait. If reading
/// failed, they get the error instead of working from half the data.
#[derive(Default)]
pub struct Ready {
    state: Mutex<Option<Result<(), String>>>,
    wake: Condvar,
}

impl Ready {
    pub fn open(&self, result: Result<(), String>) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        self.wake.notify_all();
    }

    pub fn wait(&self) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match state.as_ref() {
                Some(Ok(())) => return Ok(()),
                Some(Err(e)) => anyhow::bail!("{e}"),
                None => state = self.wake.wait(state).unwrap_or_else(|e| e.into_inner()),
            }
        }
    }

    /// Read, and read well: what waiting would return at once.
    pub fn is_open(&self) -> bool {
        matches!(*self.state.lock().unwrap_or_else(|e| e.into_inner()), Some(Ok(())))
    }
}

/// Reads what an older Shoebox couldn't from the originals plugged in
/// (metadata only): lenses, a shot's other file lending its lens, then focus
/// points. Kicked at start, after each scan, and when a disk is plugged in.
fn lens_worker(app: AppHandle, cat: Arc<Catalog>, q: Arc<Kick>) {
    let cancel = AtomicBool::new(false);
    loop {
        q.wait();
        match gear::find_lenses(&cat, &cancel) {
            Ok(r) if r.found + r.from_twins > 0 => {
                let _ = app.emit("catalog-changed", ());
            }
            Ok(_) => {}
            Err(e) => log::warn!("finding lenses failed: {e:#}"),
        }
        // Only the photo shown asks for them: nothing to tell the window.
        if let Err(e) = focus::find(&cat, &cancel) {
            log::warn!("finding focus points failed: {e:#}");
        }
        // Focus brackets found by a scan or just now: gathered into stacks.
        match stacks::assign(&cat) {
            Ok(n) if n > 0 => {
                let _ = app.emit("catalog-changed", ());
            }
            Ok(_) => {}
            Err(e) => log::warn!("gathering focus stacks failed: {e:#}"),
        }
    }
}

/// AI models waiting to be downloaded (Settings > AI models), one at a time.
#[derive(Default)]
pub struct ModelQueue {
    queue: Mutex<VecDeque<String>>,
    wake: Condvar,
    cancel: AtomicBool,
    pub current: Mutex<Option<ModelProgress>>,
}

#[derive(Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProgress {
    pub id: String,
    /// Bytes downloaded, of `total`.
    pub done: u64,
    pub total: u64,
    pub finished: bool,
    pub error: Option<String>,
    /// Waiting their turn.
    pub queued: Vec<String>,
}

impl ModelQueue {
    pub fn push(&self, id: &str) {
        let mut q = self.queue.lock().unwrap();
        let running = self.current.lock().unwrap().as_ref().is_some_and(|p| p.id == id);
        if !running && !q.iter().any(|q| q == id) {
            q.push_back(id.to_string());
            self.wake.notify_one();
        }
    }

    /// Stops this model's download, or takes it out of the queue.
    pub fn stop(&self, id: &str) {
        self.queue.lock().unwrap().retain(|q| q != id);
        if self.current.lock().unwrap().as_ref().is_some_and(|p| p.id == id) {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn pending(&self) -> Vec<String> {
        self.queue.lock().unwrap().iter().cloned().collect()
    }

    /// Downloading now or waiting.
    pub fn busy_with(&self, id: &str) -> bool {
        let waiting = self.pending().iter().any(|q| q == id);
        waiting || self.current.lock().unwrap().as_ref().is_some_and(|p| p.id == id)
    }

    fn next(&self) -> String {
        let mut q = self.queue.lock().unwrap();
        loop {
            if let Some(id) = q.pop_front() {
                return id;
            }
            q = self.wake.wait(q).unwrap();
        }
    }
}

/// Downloads the models asked for, reporting `model-download` events, and
/// wakes the AI worker when one arrives.
fn model_worker(app: AppHandle, q: Arc<ModelQueue>, ai: Arc<AiQueue>) {
    loop {
        let id = q.next();
        q.cancel.store(false, Ordering::Relaxed);
        let progress = |done: u64, total: u64, finished: bool, error: Option<String>| {
            let p = ModelProgress { id: id.clone(), done, total, finished, error, queued: q.pending() };
            *q.current.lock().unwrap() = (!finished).then(|| p.clone());
            let _ = app.emit("model-download", p);
        };
        let shown = Mutex::new(Instant::now() - Duration::from_secs(1));
        progress(0, 0, false, None);
        let result = models::download(&id, &q.cancel, &|done, total| {
            let mut last = shown.lock().unwrap();
            if last.elapsed() > Duration::from_millis(200) || done == total {
                *last = Instant::now();
                progress(done, total, false, None);
            }
        });
        match result {
            Ok(bytes) => {
                progress(bytes, bytes, true, None);
                ai.kick();
            }
            Err(e) => {
                log::warn!("models: {id}: {e:#}");
                progress(0, 0, true, Some(format!("{e:#}")));
            }
        }
    }
}

/// Looks for sideways photos first (the turns it suggests wait for the user),
/// describes new photos for word search, then finds and groups their faces,
/// on the GPU, from Shoebox's previews: every photo, since search and People
/// cover the whole catalog. A feature whose model isn't here (Settings > AI
/// models) is skipped. Cull's measures aren't taken here: Cull works on one
/// shoot at a time and measures it when it opens it ([`measure_worker`]).
fn ai_worker(
    app: AppHandle,
    cat: Arc<Catalog>,
    index: Arc<search::SearchIndex>,
    faces: Arc<faces::FaceIndex>,
    quality: Arc<cull::QualityIndex>,
    upright: Arc<upright::Upright>,
    q: Arc<AiQueue>,
) {
    loop {
        q.wait();
        q.cancel.store(false, Ordering::Relaxed);
        let report = |p: &search::AiProgress| {
            *q.current.lock().unwrap() = Some(p.clone());
            let _ = app.emit("ai-progress", p);
        };
        let usable = |id| models::ready(id) && models::runtime_ready();
        let checked = if usable(models::UPRIGHT) { upright.check_pending(&cat, &q.cancel, &report) } else { Ok(()) };
        let searched = if usable(models::CLIP) { index.index_pending(&cat, &q.cancel, &report) } else { Ok(()) };
        let found = if usable(models::FACES) { faces.index_pending(&cat, &q.cancel, &report) } else { Ok(()) };
        // Cull measures a shoot when it opens it; measures of photos gone or
        // turned since are dropped here, so none is shown for another picture.
        if let Err(e) = quality.forget_gone(&cat) {
            log::warn!("cull: forgetting old measures failed: {e:#}");
        }
        let result = checked.and(searched).and(found);
        let last = q.current.lock().unwrap().take();
        let mut done = last.unwrap_or_else(|| search::AiProgress { task: "search".into(), ..Default::default() });
        done.finished = true;
        if let Err(e) = result {
            log::warn!("{} failed: {e:#}", done.task);
            done.error = Some(format!("{e:#}"));
        }
        let _ = app.emit("ai-progress", done);
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanError {
    root_id: i64,
    message: String,
}

/// A catalog move a stopped Shoebox left halfway is undone, before the
/// catalog opens: the catalog stays where it was, whole.
pub fn undo_unfinished_move(data: &DataDir) {
    match data.undo_move() {
        Ok(Some(to)) => log::info!("the catalog move to {} didn't finish: undone", to.display()),
        Ok(None) => {}
        Err(e) => log::warn!("couldn't undo an unfinished catalog move: {e}"),
    }
}

/// Temporary files a stopped Shoebox left mid-write are removed.
pub fn remove_stray_temp_files(data: &DataDir) {
    let n = data.remove_stray_temp_files();
    if n > 0 {
        log::info!("{n} temporary files left by unfinished writes removed");
    }
}

/// The folder a catalog move left behind goes to the Recycle Bin (the Trash
/// on macOS and Linux), now that the new catalog has opened.
pub fn tidy_old_catalog(data: &DataDir) {
    match paths::remove_old_catalog(data) {
        Ok(Some(old)) => log::info!("old catalog folder {} sent to the {}", old.display(), trash_bin::NAME),
        Ok(None) => {}
        Err(e) => log::warn!("couldn't remove the old catalog folder: {e:#}"),
    }
}

/// Logs to stderr and to the day's file in `logs/`, every Shoebox (the
/// window, the CLI) appending to the same one; files older than
/// [`paths::LOGS_KEPT_DAYS`] are removed.
pub fn setup_logging(data: &DataDir) {
    let _ = fern::Dispatch::new()
        .format(|out, msg, rec| {
            out.finish(format_args!(
                "{} {:5} {}: {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                rec.level(),
                rec.target(),
                msg
            ))
        })
        .level(log::LevelFilter::Info)
        // rawler warns about every format feature it lacks; only its errors matter here.
        .level_for("rawler", log::LevelFilter::Error)
        // stderr: shoebox-cli's stdout is its JSON, nothing else.
        .chain(std::io::stderr())
        // Opened for appending, a new one past midnight: nothing is renamed,
        // which Windows refuses while another Shoebox has the file open.
        .chain(fern::DateBased::new(data.log_dir().join("shoebox-"), "%Y-%m-%d.log"))
        .apply();
    let n = data.remove_old_logs();
    if n > 0 {
        log::info!("{n} files in logs older than {} days removed", paths::LOGS_KEPT_DAYS);
    }
}

fn scan_worker(app: AppHandle, cat: Arc<Catalog>, q: Arc<ScanQueue>, ai: Arc<AiQueue>, lenses: Arc<Kick>) {
    loop {
        let task = q.next();
        q.cancel.store(false, Ordering::Relaxed);
        let refresh_gate = Mutex::new(Instant::now());
        let progress = |p: &Progress| {
            *q.current.lock().unwrap() = Some(p.clone());
            let _ = app.emit("scan-progress", p);
            // Let the grid fill in while a long scan runs.
            let mut gate = refresh_gate.lock().unwrap();
            if p.phase == "reading" && gate.elapsed() > Duration::from_secs(3) {
                *gate = Instant::now();
                let _ = app.emit("catalog-changed", ());
            }
        };
        let (root_id, result) = match task {
            ScanTask::Folder(root_id, regenerate) => {
                (root_id, scan::scan_root(&cat, root_id, &q.cancel, regenerate, &progress))
            }
            ScanTask::Retry(volume_id) => (0, scan::retry_failed(&cat, volume_id, &q.cancel, &progress)),
        };
        *q.current.lock().unwrap() = None;
        // A scan is the moment a moved or deleted catalog folder shows up.
        let _ = cat.refresh_roots();
        match result {
            Ok(report) => {
                let _ = app.emit("scan-done", report);
            }
            Err(e) => {
                log::warn!("scan {task:?} failed: {e:#}");
                // A retry is quiet: the disk may have gone again; the next plug-in tries again.
                if let ScanTask::Folder(..) = task {
                    let _ = app.emit("scan-error", ScanError { root_id, message: format!("{e:#}") });
                }
            }
        }
        let _ = app.emit("catalog-changed", ());
        ai.kick();
        lenses.kick();
    }
}

/// Notices disks being plugged in or out. Polls the drive-letter mask every
/// 2 s (free) and re-reads volume details on change or every 15 s. A disk
/// holding files that couldn't be read, or were left half-read by a Shoebox
/// stopped mid-scan, gets them read again when it shows up (and at start, for
/// the disks already there); lenses not read yet too.
fn volume_watcher(app: AppHandle, cat: Arc<Catalog>, scans: Arc<ScanQueue>, lenses: Arc<Kick>) {
    look_elsewhere(&app, &cat);
    let mut mask = volumes::drive_mask();
    let mut last_full = Instant::now();
    let mut online: HashSet<i64> = HashSet::new();
    loop {
        let now: HashSet<i64> = cat.online_ids().into_iter().collect();
        for &v in now.difference(&online) {
            if scan::failed_count(&cat, v).is_ok_and(|n| n > 0) {
                scans.retry(v);
            }
            // Listed by a Shoebox that read fewer formats: once more.
            for root in scan::roots_to_list_again(&cat, v).unwrap_or_default() {
                scans.push(root, false);
            }
        }
        online = now;
        std::thread::sleep(Duration::from_secs(2));
        let m = volumes::drive_mask();
        if m == mask && last_full.elapsed() < Duration::from_secs(15) {
            continue;
        }
        mask = m;
        last_full = Instant::now();
        match cat.refresh_volumes() {
            Ok(true) => {
                let _ = cat.refresh_roots();
                look_elsewhere(&app, &cat);
                let _ = app.emit("volumes-changed", ());
                lenses.kick();
                resume_changes(app.clone(), cat.clone());
            }
            Ok(false) => {}
            Err(e) => log::warn!("volume refresh failed: {e:#}"),
        }
    }
}

/// Looks for the sources that aren't here at the same path on the disks plugged in now (see
/// [`repoint::look_elsewhere`]); the window is told when that changed.
fn look_elsewhere(app: &AppHandle, cat: &Catalog) {
    match repoint::look_elsewhere(cat) {
        Ok(true) => {
            let _ = app.emit("volumes-changed", ());
        }
        Ok(false) => {}
        Err(e) => log::warn!("looking for sources elsewhere: {e:#}"),
    }
}

/// [`look_elsewhere`] in the background, after the sources changed.
pub(crate) fn look_elsewhere_soon(app: AppHandle, cat: Arc<Catalog>) {
    std::thread::spawn(move || look_elsewhere(&app, &cat));
}

/// Previews, thumbnails and face pictures a PC that stopped mid-write left
/// damaged (see [`repair`]): face pictures are cut again, and photos on a
/// plugged-in disk read again now; the others are when their disk is plugged in.
fn check_saved_files(cat: &Catalog, scans: &ScanQueue) {
    match repair::at_start(cat) {
        Ok(Some(c)) => {
            let online: HashSet<i64> = cat.online_ids().into_iter().collect();
            for &v in c.disks.iter().filter(|v| online.contains(v)) {
                if scan::failed_count(cat, v).is_ok_and(|n| n > 0) {
                    scans.retry(v);
                }
            }
        }
        Ok(None) => {}
        Err(e) => log::warn!("checking saved previews: {e:#}"),
    }
}

/// Settles the folder moves, renames and deletes cut short by a stop (see
/// [`organize::resume`]), in the background: checking a copy again can take a
/// while. Those still waiting for a disk get another go when one is plugged in.
fn resume_changes(app: AppHandle, cat: Arc<Catalog>) {
    std::thread::spawn(move || match organize::resume(&cat) {
        Ok(r) if !r.finished.is_empty() || !r.undone.is_empty() => {
            let _ = app.emit("catalog-changed", ());
        }
        Ok(_) => {}
        Err(e) => log::warn!("settling changes cut short: {e:#}"),
    });
}

/// Looks for a newer Shoebox: a published release on GitHub, never a commit on
/// a branch. First 15 s after start, then every 6 hours (half an hour after a
/// failure: offline is normal here), while Settings allow it. Wall-clock time,
/// so a PC woken from sleep catches up within a minute.
fn update_checker(app: AppHandle, cat: Arc<Catalog>, found: Arc<Mutex<Option<updates::UpdateStatus>>>) {
    std::thread::sleep(Duration::from_secs(15));
    loop {
        let previous = found.lock().unwrap().clone();
        let due = previous.as_ref().is_none_or(|s| s.due_in(chrono::Utc::now().timestamp()) <= 0);
        if due && cat.settings().check_updates {
            let status = updates::check(previous.as_ref());
            *found.lock().unwrap() = Some(status.clone());
            let _ = app.emit("update-status", status);
        }
        std::thread::sleep(Duration::from_secs(60));
    }
}

// The window's own address for Shoebox's pictures: `http://shoebox.localhost/`
// on Windows (and Android), `shoebox://localhost/` on macOS and Linux, where
// the web view takes custom schemes as they are. Only the path and query
// matter to the two functions below.

/// Serves `/full/<id>?long=<px>`: the photo read from its original (disk
/// plugged in) at that long edge, 0 = full size. Nothing is stored; 404 when
/// no copy is reachable.
fn serve_full(cat: &Catalog, path: &str, query: &str) -> tauri::http::Response<Vec<u8>> {
    let id = path.trim_start_matches("/full/").parse::<i64>().ok();
    let long =
        query.split('&').find_map(|kv| kv.strip_prefix("long=")).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let started = Instant::now();
    let bytes = id
        .ok_or_else(|| anyhow::anyhow!("bad photo id"))
        .and_then(|id| cat.original(id))
        .and_then(|(file, kind, turn)| imaging::sharp(&file, kind, long, turn));
    match bytes {
        Ok(bytes) => {
            log::info!("viewer: photo {} at {long} px in {} ms", id.unwrap_or(0), started.elapsed().as_millis());
            // A private photo is never kept by the window once shown.
            let private = id.is_some_and(|id| cat.is_private(id).unwrap_or(true));
            tauri::http::Response::builder()
                .header("Content-Type", "image/jpeg")
                .header("Cache-Control", if private { "no-store" } else { "max-age=600" })
                .header("Access-Control-Allow-Origin", "*")
                .body(bytes)
                .unwrap()
        }
        Err(e) => {
            log::info!("viewer: photo {:?} not readable now: {e:#}", id);
            tauri::http::Response::builder().status(404).body(Vec::new()).unwrap()
        }
    }
}

/// Serves `/thumb/<id>`, `/preview/<id>` and `/face/<id>` from the data folder.
fn serve_image(cat: &Catalog, path: &str) -> tauri::http::Response<Vec<u8>> {
    let mut parts = path.trim_start_matches('/').split('/');
    let kind = parts.next();
    let id = parts.next().and_then(|s| s.parse::<i64>().ok());
    let data = &cat.data;
    let file = match (kind, id) {
        (Some("thumb"), Some(id)) => Some(data.thumb_path(id)),
        (Some("preview"), Some(id)) => Some(data.preview_path(id)),
        (Some("face"), Some(id)) => Some(data.face_path(id)),
        _ => None,
    };
    // A private photo's copies are sealed: opened only while unlocked, and
    // never kept by the window once shown.
    let found = file.and_then(|f| match std::fs::read(&f) {
        Ok(bytes) => Some((bytes, false)),
        Err(_) => cat.read_copy(&f).ok().map(|bytes| (bytes, true)),
    });
    match found {
        Some((bytes, sealed)) => tauri::http::Response::builder()
            .header("Content-Type", "image/jpeg")
            .header("Cache-Control", if sealed { "no-store" } else { "max-age=31536000, immutable" })
            .header("Access-Control-Allow-Origin", "*")
            .body(bytes)
            .unwrap(),
        None => tauri::http::Response::builder().status(404).body(Vec::new()).unwrap(),
    }
}

pub fn run() {
    let started = Instant::now();
    volumes::quiet_errors();
    let data = DataDir::locate();
    if let Err(e) = data.ensure() {
        panic!("Can't create the Shoebox data folder at {}: {e}", data.root.display());
    }
    setup_logging(&data);
    log::info!("Shoebox {} starting, data in {}", env!("CARGO_PKG_VERSION"), data.root.display());
    undo_unfinished_move(&data);
    let cat = Arc::new(Catalog::open(data.clone()).expect("open catalog"));
    if let Err(e) = scan::heal_saved_previews(&cat) {
        log::warn!("checking previews of unreadable photos: {e:#}");
    }
    // Turns a stop cut short: previews brought to the turn the catalog says.
    if let Err(e) = turn::settle(&cat) {
        log::warn!("finishing turns cut short: {e:#}");
    }
    // Private photos' copies a stop left in the clear: sealed before anything shows.
    if let Err(e) = private::settle(&cat) {
        log::warn!("sealing private photos' copies: {e:#}");
    }
    let opened = data.clone();
    std::thread::spawn(move || {
        tidy_old_catalog(&opened);
        remove_stray_temp_files(&opened);
    });
    let scans = Arc::new(ScanQueue::default());
    let (c, q) = (cat.clone(), scans.clone());
    std::thread::spawn(move || check_saved_files(&c, &q));
    // Camera and lens names stored with photos, caught up if anything left them
    // behind (an older Shoebox renaming a camera in this catalog).
    let c = cat.clone();
    std::thread::spawn(move || match gear::store_shown(&c.db()) {
        Ok(0) => {}
        Ok(n) => log::info!("{n} photos' camera or lens names brought up to date"),
        Err(e) => log::warn!("bringing camera and lens names up to date: {e:#}"),
    });
    // Read in the background: the window opens at once, a search or a face
    // waits until theirs are read.
    let index = search::SearchIndex::open_in_background(&cat).expect("open search index");
    let face_index = faces::FaceIndex::open_in_background(cat.clone()).expect("open face index");
    let quality = cull::QualityIndex::open_in_background(&cat).expect("open Cull's measures");
    let upright = Arc::new(upright::Upright::default());
    let measure = Arc::new(MeasureQueue::default());
    let closer = Arc::new(CloserQueue::default());
    let ai = Arc::new(AiQueue::default());
    let model_queue = Arc::new(ModelQueue::default());
    let lens_kick = Arc::new(Kick::default());
    let update_status = Arc::new(Mutex::new(None));
    let protocol_cat = cat.clone();
    // Thumbnails, previews and face pictures are files read as they are: their
    // own threads, so they never wait behind a photo read from its original
    // (seconds each, on the shared pool).
    let saved_files = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .thread_name(|i| format!("saved-files-{i}"))
            .build()
            .expect("thread pool for saved pictures"),
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("shoebox", move |_ctx, request, responder| {
            let cat = protocol_cat.clone();
            let path = request.uri().path().to_string();
            let query = request.uri().query().unwrap_or("").to_string();
            if path.starts_with("/full/") {
                rayon::spawn(move || responder.respond(serve_full(&cat, &path, &query)));
            } else {
                saved_files.spawn(move || responder.respond(serve_image(&cat, &path)));
            }
        })
        .manage(AppState {
            cat: cat.clone(),
            models: model_queue.clone(),
            scans: scans.clone(),
            search: index.clone(),
            faces: face_index.clone(),
            quality: quality.clone(),
            upright: upright.clone(),
            measure: measure.clone(),
            closer: closer.clone(),
            ai: ai.clone(),
            moving: Arc::default(),
            move_cancel: Arc::default(),
            updates: update_status.clone(),
        })
        .setup(move |app| {
            log::info!("window ready {} ms after start", started.elapsed().as_millis());
            let handle = app.handle().clone();
            let (c, q, a, l) = (cat.clone(), scans.clone(), ai.clone(), lens_kick.clone());
            std::thread::Builder::new().name("scan".into()).spawn(move || scan_worker(handle, c, q, a, l))?;
            let handle = app.handle().clone();
            let (c, i, f, qi, u, a) =
                (cat.clone(), index.clone(), face_index.clone(), quality.clone(), upright.clone(), ai.clone());
            std::thread::Builder::new().name("ai".into()).spawn(move || ai_worker(handle, c, i, f, qi, u, a))?;
            // Describe whatever isn't yet, from the start.
            ai.kick();
            let handle = app.handle().clone();
            let (c, qi, mq) = (cat.clone(), quality.clone(), measure.clone());
            std::thread::Builder::new().name("measure".into()).spawn(move || measure_worker(handle, c, qi, mq))?;
            let handle = app.handle().clone();
            let (c, qi, cq) = (cat.clone(), quality.clone(), closer.clone());
            std::thread::Builder::new().name("closer".into()).spawn(move || closer_worker(handle, c, qi, cq))?;
            let handle = app.handle().clone();
            let (c, q, l) = (cat.clone(), scans.clone(), lens_kick.clone());
            std::thread::Builder::new().name("volumes".into()).spawn(move || volume_watcher(handle, c, q, l))?;
            let handle = app.handle().clone();
            let (c, l) = (cat.clone(), lens_kick.clone());
            std::thread::Builder::new().name("lenses".into()).spawn(move || lens_worker(handle, c, l))?;
            // Lenses an older Shoebox couldn't read, from the start.
            lens_kick.kick();
            let handle = app.handle().clone();
            let (m, a) = (model_queue.clone(), ai.clone());
            std::thread::Builder::new().name("models".into()).spawn(move || model_worker(handle, m, a))?;
            let handle = app.handle().clone();
            let (c, u) = (cat.clone(), update_status.clone());
            std::thread::Builder::new().name("updates".into()).spawn(move || update_checker(handle, c, u))?;
            // A folder move, rename or delete cut short last time: finished or undone.
            resume_changes(app.handle().clone(), cat.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_overview,
            commands::list_photos,
            commands::facets,
            commands::browse,
            commands::get_photo,
            commands::add_folder,
            commands::rescan,
            commands::rescan_all,
            commands::cancel_scan,
            commands::scan_status,
            commands::reveal_file,
            commands::open_file,
            commands::get_settings,
            commands::save_settings,
            commands::rename_volume,
            commands::regenerate,
            commands::remove_folder,
            commands::check_repoint,
            commands::repoint_folder,
            commands::catalog_stats,
            commands::search_photos,
            commands::people,
            commands::find_people,
            commands::read_people,
            commands::read_all_people,
            commands::rename_person,
            commands::merge_people,
            commands::unmerge_people,
            commands::check_people,
            commands::leave_as_is,
            commands::check_dates,
            commands::leave_dates,
            commands::dates_of,
            commands::change_dates,
            commands::undo_dates,
            commands::ignore_people,
            commands::regroup_faces,
            commands::reassign_faces,
            commands::restore_faces,
            commands::look_alikes,
            commands::person_strays,
            commands::person_faces,
            commands::more_faces,
            commands::photo_faces,
            commands::faces_in_photos,
            commands::ai_status,
            commands::check_catalog_folder,
            commands::backup_catalog,
            commands::last_backup,
            commands::move_catalog,
            commands::switch_catalog,
            commands::rename_folder,
            commands::move_folder,
            commands::cancel_folder_move,
            commands::new_folder,
            commands::show_folder,
            commands::tags_all,
            commands::tags_on,
            commands::tags_add,
            commands::tags_remove,
            commands::tag_rename,
            commands::tag_delete,
            commands::gear_rename,
            commands::gear_split,
            commands::private_status,
            commands::private_unlock,
            commands::private_lock,
            commands::private_rename,
            commands::private_hide,
            commands::private_hide_in,
            commands::private_show,
            commands::mark_photos,
            commands::unmark_all,
            commands::cull_shoot,
            commands::cull_flags,
            commands::cull_look_closer,
            commands::cull_stop,
            commands::cull_progress,
            commands::delete_plan,
            commands::delete_marked,
            commands::missing_photos,
            commands::forget_missing,
            commands::turn_photos,
            commands::turn_back,
            commands::sideways_accept,
            commands::sideways_leave,
            commands::models_list,
            commands::model_download,
            commands::model_stop,
            commands::model_progress,
            commands::model_delete,
            commands::show_models_folder,
            commands::update_status,
            commands::check_updates,
            commands::open_releases,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Shoebox");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_is_waited_for_until_read_then_says_if_reading_failed() {
        let ready = Arc::new(Ready::default());
        let (r, waited) = (ready.clone(), Arc::new(AtomicBool::new(false)));
        let w = waited.clone();
        let waiter = std::thread::spawn(move || {
            let result = r.wait();
            w.store(true, Ordering::SeqCst);
            result
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(!waited.load(Ordering::SeqCst), "answered before the index was read");
        ready.open(Ok(()));
        assert!(waiter.join().unwrap().is_ok());
        assert!(ready.wait().is_ok());

        let failed = Ready::default();
        failed.open(Err("faces.db is damaged".into()));
        assert_eq!(failed.wait().unwrap_err().to_string(), "faces.db is damaged");
    }
}
