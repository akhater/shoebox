pub mod ai;
pub mod catalog;
pub mod db;
pub mod embedded;
pub mod faces;
pub mod formats;
pub mod imaging;
pub mod models;
pub mod organize;
pub mod paths;
pub mod scan;
pub mod search;
pub mod settings;
pub mod tags;
pub mod volumes;
#[cfg(windows)]
pub mod wic;

mod commands;

use catalog::Catalog;
use paths::DataDir;
use scan::Progress;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Folders waiting to be scanned, one scan at a time.
#[derive(Default)]
pub struct ScanQueue {
    /// (folder id, regenerate previews)
    queue: Mutex<VecDeque<(i64, bool)>>,
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
        if let Some(item) = q.iter_mut().find(|(id, _)| *id == root_id) {
            item.1 |= regenerate;
        } else if !running || regenerate {
            q.push_back((root_id, regenerate));
            self.wake.notify_one();
        }
    }

    /// Drops a folder's queued scans (it was removed from the catalog).
    pub fn forget(&self, root_id: i64) {
        self.queue.lock().unwrap().retain(|(id, _)| *id != root_id);
    }

    pub fn pending(&self) -> Vec<i64> {
        self.queue.lock().unwrap().iter().map(|(id, _)| *id).collect()
    }

    /// Stops the running scan and drops the queued ones.
    pub fn cancel_all(&self) {
        self.queue.lock().unwrap().clear();
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn next(&self) -> (i64, bool) {
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
    pub ai: Arc<AiQueue>,
    /// A folder being moved to another disk (one at a time), and its Stop.
    pub moving: Arc<AtomicBool>,
    pub move_cancel: Arc<AtomicBool>,
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

/// Describes new photos for word search, then finds and groups their faces,
/// on the GPU, from Shoebox's previews. A feature whose model isn't here
/// (Settings > AI models) is skipped.
fn ai_worker(
    app: AppHandle,
    cat: Arc<Catalog>,
    index: Arc<search::SearchIndex>,
    faces: Arc<faces::FaceIndex>,
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
        let searched = if usable(models::CLIP) { index.index_pending(&cat, &q.cancel, &report) } else { Ok(()) };
        let found = if usable(models::FACES) { faces.index_pending(&cat, &q.cancel, &report) } else { Ok(()) };
        let result = searched.and(found);
        let last = q.current.lock().unwrap().take();
        let mut done = last.unwrap_or_else(|| search::AiProgress { task: "search".into(), ..Default::default() });
        done.finished = true;
        if let Err(e) = result {
            log::warn!("search indexing failed: {e:#}");
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

/// The folder a catalog move left behind goes to the Recycle Bin, now that
/// the new catalog has opened.
pub fn tidy_old_catalog(data: &DataDir) {
    match paths::remove_old_catalog(data) {
        Ok(Some(old)) => log::info!("old catalog folder {} sent to the Recycle Bin", old.display()),
        Ok(None) => {}
        Err(e) => log::warn!("couldn't remove the old catalog folder: {e:#}"),
    }
}

pub fn setup_logging(data: &DataDir) {
    let file = match fern::log_file(data.log_path()) {
        Ok(f) => f,
        Err(_) => return,
    };
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
        .chain(std::io::stdout())
        .chain(file)
        .apply();
}

fn scan_worker(app: AppHandle, cat: Arc<Catalog>, q: Arc<ScanQueue>, ai: Arc<AiQueue>) {
    loop {
        let (root_id, regenerate) = q.next();
        q.cancel.store(false, Ordering::Relaxed);
        let refresh_gate = Mutex::new(Instant::now());
        let result = scan::scan_root(&cat, root_id, &q.cancel, regenerate, &|p: &Progress| {
            *q.current.lock().unwrap() = Some(p.clone());
            let _ = app.emit("scan-progress", p);
            // Let the grid fill in while a long scan runs.
            let mut gate = refresh_gate.lock().unwrap();
            if p.phase == "reading" && gate.elapsed() > Duration::from_secs(3) {
                *gate = Instant::now();
                let _ = app.emit("catalog-changed", ());
            }
        });
        *q.current.lock().unwrap() = None;
        // A scan is the moment a moved or deleted catalog folder shows up.
        let _ = cat.refresh_roots();
        match result {
            Ok(report) => {
                let _ = app.emit("scan-done", report);
            }
            Err(e) => {
                log::warn!("scan {root_id} failed: {e:#}");
                let _ = app.emit("scan-error", ScanError { root_id, message: format!("{e:#}") });
            }
        }
        let _ = app.emit("catalog-changed", ());
        ai.kick();
    }
}

/// Notices disks being plugged in or out. Polls the drive-letter mask every
/// 2 s (free) and re-reads volume details on change or every 15 s.
fn volume_watcher(app: AppHandle, cat: Arc<Catalog>) {
    let mut mask = volumes::drive_mask();
    let mut last_full = Instant::now();
    loop {
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
                let _ = app.emit("volumes-changed", ());
            }
            Ok(false) => {}
            Err(e) => log::warn!("volume refresh failed: {e:#}"),
        }
    }
}

/// Serves `http://shoebox.localhost/thumb/<id>` and `/preview/<id>` from the data folder.
/// Serves `http://shoebox.localhost/full/<id>?long=<px>`: the photo read from
/// its original (disk plugged in) at that long edge, 0 = full size. Nothing
/// is stored; 404 when no copy is reachable.
fn serve_full(cat: &Catalog, path: &str, query: &str) -> tauri::http::Response<Vec<u8>> {
    let id = path.trim_start_matches("/full/").parse::<i64>().ok();
    let long =
        query.split('&').find_map(|kv| kv.strip_prefix("long=")).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let started = Instant::now();
    let bytes = id
        .ok_or_else(|| anyhow::anyhow!("bad photo id"))
        .and_then(|id| cat.original(id))
        .and_then(|(file, kind)| imaging::sharp(&file, kind, long));
    match bytes {
        Ok(bytes) => {
            log::info!("viewer: photo {} at {long} px in {} ms", id.unwrap_or(0), started.elapsed().as_millis());
            tauri::http::Response::builder()
                .header("Content-Type", "image/jpeg")
                .header("Cache-Control", "max-age=600")
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

fn serve_image(data: &DataDir, path: &str) -> tauri::http::Response<Vec<u8>> {
    let mut parts = path.trim_start_matches('/').split('/');
    let kind = parts.next();
    let id = parts.next().and_then(|s| s.parse::<i64>().ok());
    let file = match (kind, id) {
        (Some("thumb"), Some(id)) => Some(data.thumb_path(id)),
        (Some("preview"), Some(id)) => Some(data.preview_path(id)),
        (Some("face"), Some(id)) => Some(data.face_path(id)),
        _ => None,
    };
    match file.and_then(|f| std::fs::read(f).ok()) {
        Some(bytes) => tauri::http::Response::builder()
            .header("Content-Type", "image/jpeg")
            .header("Cache-Control", "max-age=31536000, immutable")
            .header("Access-Control-Allow-Origin", "*")
            .body(bytes)
            .unwrap(),
        None => tauri::http::Response::builder().status(404).body(Vec::new()).unwrap(),
    }
}

pub fn run() {
    volumes::quiet_errors();
    let data = DataDir::locate();
    if let Err(e) = data.ensure() {
        panic!("Can't create the Shoebox data folder at {}: {e}", data.root.display());
    }
    setup_logging(&data);
    log::info!("Shoebox {} starting, data in {}", env!("CARGO_PKG_VERSION"), data.root.display());
    let cat = Arc::new(Catalog::open(data.clone()).expect("open catalog"));
    let opened = data.clone();
    std::thread::spawn(move || tidy_old_catalog(&opened));
    let scans = Arc::new(ScanQueue::default());
    let index = Arc::new(search::SearchIndex::open(&cat).expect("open search index"));
    let face_index = Arc::new(faces::FaceIndex::open(&cat).expect("open face index"));
    let ai = Arc::new(AiQueue::default());
    let model_queue = Arc::new(ModelQueue::default());
    let protocol_data = data.clone();
    let protocol_cat = cat.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol("shoebox", move |_ctx, request, responder| {
            let data = protocol_data.clone();
            let cat = protocol_cat.clone();
            let path = request.uri().path().to_string();
            let query = request.uri().query().unwrap_or("").to_string();
            rayon::spawn(move || {
                let response = if path.starts_with("/full/") {
                    serve_full(&cat, &path, &query)
                } else {
                    serve_image(&data, &path)
                };
                responder.respond(response)
            });
        })
        .manage(AppState {
            cat: cat.clone(),
            models: model_queue.clone(),
            scans: scans.clone(),
            search: index.clone(),
            faces: face_index.clone(),
            ai: ai.clone(),
            moving: Arc::default(),
            move_cancel: Arc::default(),
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            let (c, q, a) = (cat.clone(), scans.clone(), ai.clone());
            std::thread::Builder::new().name("scan".into()).spawn(move || scan_worker(handle, c, q, a))?;
            let handle = app.handle().clone();
            let (c, i, f, a) = (cat.clone(), index.clone(), face_index.clone(), ai.clone());
            std::thread::Builder::new().name("ai".into()).spawn(move || ai_worker(handle, c, i, f, a))?;
            // Describe whatever isn't yet, from the start.
            ai.kick();
            let handle = app.handle().clone();
            let c = cat.clone();
            std::thread::Builder::new().name("volumes".into()).spawn(move || volume_watcher(handle, c))?;
            let handle = app.handle().clone();
            let (m, a) = (model_queue.clone(), ai.clone());
            std::thread::Builder::new().name("models".into()).spawn(move || model_worker(handle, m, a))?;
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
            commands::catalog_stats,
            commands::search_photos,
            commands::people,
            commands::visit_people,
            commands::rename_person,
            commands::merge_people,
            commands::ignore_people,
            commands::regroup_faces,
            commands::split_face,
            commands::person_strays,
            commands::move_faces_out,
            commands::photo_faces,
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
            commands::mark_photos,
            commands::unmark_all,
            commands::delete_plan,
            commands::delete_marked,
            commands::models_list,
            commands::model_download,
            commands::model_stop,
            commands::model_progress,
            commands::model_delete,
            commands::show_models_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Shoebox");
}
