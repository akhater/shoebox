//! What the window can ask the backend. Errors go back as plain sentences.
//!
//! A command that isn't `async` runs on the window's main thread, which can't
//! draw or take a click until it returns. So anything that touches the
//! catalog or a disk is `async` and does its work in [`blocking`] (a question)
//! or [`in_order`] (a change); only answers already in memory stay plain.

use crate::AppState;
use crate::catalog::{
    Browse, Facets, Filters, FolderCheck, Overview, PhotoDetail, PhotoList, RemoveReport, Scope, Sort, Stats,
};
use crate::gear;
use crate::models;
use crate::organize::{self, DeletePlan, DeleteReport};
use crate::scan::Progress;
use crate::settings::Settings;
use crate::tags;
use anyhow::bail;
use serde::Serialize;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, mpsc};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};

type Res<T> = Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

/// Runs a command's work on a worker thread, off the window's main thread.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(work).await.map_err(|e| e.to_string())?.map_err(err)
}

type Change = Box<dyn FnOnce() + Send>;

/// The window's changes, done one after another on their own thread.
static CHANGES: LazyLock<mpsc::Sender<Change>> = LazyLock::new(|| {
    let (tx, rx) = mpsc::channel::<Change>();
    std::thread::Builder::new()
        .name("window-changes".into())
        .spawn(move || rx.into_iter().for_each(|change| change()))
        .expect("thread for the window's changes");
    tx
});

/// [`blocking`] for a change (a mark, a tag, a name, a folder renamed...): it
/// waits for the window's earlier changes and is done in the order asked, as
/// when they ran on the main thread. Marking a photo then unmarking it can't
/// land the other way round while a scan holds the catalog.
async fn in_order<T: Send + 'static>(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Res<T> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    // A change that panics answers nothing (its sender is dropped); the next ones still run.
    let change = move || {
        if let Ok(result) = std::panic::catch_unwind(AssertUnwindSafe(work)) {
            let _ = tx.blocking_send(result);
        }
    };
    CHANGES.send(Box::new(change)).map_err(|e| e.to_string())?;
    rx.recv().await.ok_or_else(|| "That change failed unexpectedly.".to_string())?.map_err(err)
}

#[tauri::command]
pub async fn get_overview(state: State<'_, AppState>) -> Res<Overview> {
    let cat = state.cat.clone();
    blocking(move || cat.overview()).await
}

#[tauri::command]
pub async fn list_photos(
    state: State<'_, AppState>,
    scope: Scope,
    sort: Option<Sort>,
    filters: Option<Filters>,
) -> Res<PhotoList> {
    let cat = state.cat.clone();
    blocking(move || cat.list_photos(&scope, sort.unwrap_or_default(), &filters.unwrap_or_default())).await
}

/// What each filter offers in a scope, with counts.
#[tauri::command]
pub async fn facets(state: State<'_, AppState>, scope: Scope, filters: Option<Filters>) -> Res<Facets> {
    let cat = state.cat.clone();
    blocking(move || cat.facets(&scope, &filters.unwrap_or_default())).await
}

#[tauri::command]
pub async fn browse(state: State<'_, AppState>) -> Res<Browse> {
    let cat = state.cat.clone();
    blocking(move || cat.browse()).await
}

#[tauri::command]
pub async fn get_photo(state: State<'_, AppState>, id: i64) -> Res<PhotoDetail> {
    let cat = state.cat.clone();
    blocking(move || cat.photo_detail(id)).await
}

/// Adds a folder and queues its first scan.
#[tauri::command]
pub async fn add_folder(state: State<'_, AppState>, path: String) -> Res<i64> {
    let (cat, scans) = (state.cat.clone(), state.scans.clone());
    in_order(move || {
        let id = cat.add_root(Path::new(&path))?;
        scans.push(id, false);
        Ok(id)
    })
    .await
}

#[tauri::command]
pub async fn rescan(state: State<'_, AppState>, root_id: i64) -> Res<()> {
    let (cat, scans) = (state.cat.clone(), state.scans.clone());
    blocking(move || {
        let root = cat.root(root_id)?;
        if cat.mount_of(root.volume_id).is_none() {
            bail!("{} is not plugged in", root.volume_label);
        }
        scans.push(root_id, false);
        Ok(())
    })
    .await
}

/// Queues every folder whose disk is plugged in. Unplugged disks are skipped,
/// never treated as empty.
#[tauri::command]
pub async fn rescan_all(state: State<'_, AppState>) -> Res<usize> {
    let (cat, scans) = (state.cat.clone(), state.scans.clone());
    blocking(move || {
        let _ = cat.refresh_roots();
        let mut n = 0;
        for id in cat.root_ids()? {
            if let Ok(root) = cat.root(id)
                && cat.mount_of(root.volume_id).is_some()
            {
                scans.push(id, false);
                n += 1;
            }
        }
        Ok(n)
    })
    .await
}

/// Reads the photos again and rebuilds their previews (after changing size
/// or quality). One folder, or every folder on a plugged-in disk. Photos on
/// unplugged disks keep the previews they have.
#[tauri::command]
pub async fn regenerate(state: State<'_, AppState>, root_id: Option<i64>) -> Res<usize> {
    let (cat, scans) = (state.cat.clone(), state.scans.clone());
    blocking(move || {
        let _ = cat.refresh_roots();
        let ids = match root_id {
            Some(id) => vec![id],
            None => cat.root_ids()?,
        };
        let mut n = 0;
        for id in ids {
            if let Ok(root) = cat.root(id)
                && cat.mount_of(root.volume_id).is_some()
            {
                scans.push(id, true);
                n += 1;
            }
        }
        if n == 0 {
            bail!("Plug the disk in first: previews are rebuilt from the original photos.");
        }
        Ok(n)
    })
    .await
}

/// Takes a folder out of the catalog. Its files on disk are not touched.
#[tauri::command]
pub async fn remove_folder(state: State<'_, AppState>, root_id: i64) -> Res<RemoveReport> {
    if state.scans.current.lock().unwrap().as_ref().is_some_and(|p| p.root_id == root_id) {
        return Err("That folder is being scanned. Stop the scan first.".into());
    }
    state.scans.forget(root_id);
    let cat = state.cat.clone();
    in_order(move || cat.remove_root(root_id)).await
}

#[tauri::command]
pub async fn catalog_stats(state: State<'_, AppState>) -> Res<Stats> {
    let cat = state.cat.clone();
    blocking(move || cat.stats()).await
}

/// Photos matching a description ("succulent", "airport lounge"), best first,
/// within the current scope.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    list: PhotoList,
    /// The first `clear` photos of `list` are clear matches; the rest are possible ones.
    clear: usize,
}

#[tauri::command]
pub async fn search_photos(
    state: State<'_, AppState>,
    query: String,
    scope: Scope,
    filters: Option<Filters>,
) -> Res<SearchResult> {
    let filters = filters.unwrap_or_default();
    let (cat, index) = (state.cat.clone(), state.search.clone());
    blocking(move || {
        let started = std::time::Instant::now();
        let (hits, cutoff) = index.search(query.trim(), 1000)?;
        let clear_ids: std::collections::HashSet<i64> = hits.iter().filter(|h| h.1 >= cutoff).map(|h| h.0).collect();
        let ranked: Vec<i64> = hits.iter().map(|h| h.0).collect();
        let list = cat.ranked_in_scope(&ranked, &scope, &filters)?;
        let clear = list.ids.iter().take_while(|id| clear_ids.contains(id)).count();
        log::info!(
            "search {:?}: {clear} clear + {} possible in {} ms",
            query.trim(),
            list.ids.len() - clear,
            started.elapsed().as_millis()
        );
        Ok(SearchResult { list, clear })
    })
    .await
}

#[tauri::command]
pub async fn people(state: State<'_, AppState>) -> Res<Vec<crate::faces::Person>> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    blocking(move || faces.people(&cat)).await
}

/// People's search box: whoever's name matches what's typed, best first.
#[tauri::command]
pub async fn find_people(state: State<'_, AppState>, query: String) -> Res<Vec<crate::faces::Person>> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    blocking(move || faces.find_people(&cat, &query)).await
}

/// The user has seen these people's new photos (they left their page).
#[tauri::command]
pub async fn read_people(state: State<'_, AppState>, ids: Vec<i64>) -> Res<usize> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    in_order(move || faces.mark_read(&cat, &ids)).await
}

/// "Mark all as read" in People.
#[tauri::command]
pub async fn read_all_people(state: State<'_, AppState>) -> Res<usize> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    in_order(move || faces.mark_all_read(&cat)).await
}

#[tauri::command]
pub async fn rename_person(state: State<'_, AppState>, person_id: i64, name: String) -> Res<()> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    in_order(move || faces.rename(&cat, person_id, &name)).await
}

/// Sorts again every face not named, ignored or placed by hand.
#[tauri::command]
pub async fn regroup_faces(state: State<'_, AppState>) -> Res<(usize, usize)> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    blocking(move || faces.regroup(&cat)).await
}

/// Hides strangers from People (or shows them again).
#[tauri::command]
pub async fn ignore_people(state: State<'_, AppState>, ids: Vec<i64>, ignored: bool) -> Res<()> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    in_order(move || faces.set_ignored(&cat, &ids, ignored)).await
}

/// Makes several people one (the same person found in two groups).
#[tauri::command]
pub async fn merge_people(state: State<'_, AppState>, ids: Vec<i64>, into: i64) -> Res<()> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    in_order(move || faces.merge(&cat, &ids, into)).await
}

/// "Who is this?": faces go to someone, someone new, or (not them, don't
/// know who) whoever they resemble. Placed by hand.
#[tauri::command]
pub async fn reassign_faces(
    state: State<'_, AppState>,
    face_ids: Vec<i64>,
    to: crate::faces::FaceTarget,
) -> Res<crate::faces::Moved> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    in_order(move || faces.reassign(&cat, &face_ids, &to)).await
}

/// Undo of a reassign: the faces go back as they were.
#[tauri::command]
pub async fn restore_faces(state: State<'_, AppState>, placements: Vec<crate::faces::Placement>) -> Res<usize> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    in_order(move || faces.restore(&cat, &placements)).await
}

/// The people some faces look like most, to suggest who they are.
#[tauri::command]
pub async fn look_alikes(
    state: State<'_, AppState>,
    face_ids: Vec<i64>,
    except: Vec<i64>,
) -> Res<Vec<crate::faces::LookAlike>> {
    let faces = state.faces.clone();
    blocking(move || faces.look_alikes(&face_ids, &except, 12)).await
}

/// Faces in a person that don't look like them, worst first.
#[tauri::command]
pub async fn person_strays(state: State<'_, AppState>, person_id: i64) -> Res<Vec<crate::faces::Stray>> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    blocking(move || faces.strays(&cat, person_id)).await
}

/// Every face of a person, the least like them first.
#[tauri::command]
pub async fn person_faces(state: State<'_, AppState>, person_id: i64) -> Res<Vec<crate::faces::Stray>> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    blocking(move || faces.person_faces(&cat, person_id)).await
}

#[tauri::command]
pub async fn photo_faces(state: State<'_, AppState>, photo_id: i64) -> Res<Vec<crate::faces::PhotoFace>> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    blocking(move || faces.photo_faces(&cat, photo_id)).await
}

/// The faces in some photos (a selection on a person's page).
#[tauri::command]
pub async fn faces_in_photos(state: State<'_, AppState>, photo_ids: Vec<i64>) -> Res<Vec<crate::faces::PhotoFace>> {
    let (cat, faces) = (state.cat.clone(), state.faces.clone());
    blocking(move || faces.faces_in(&cat, &photo_ids)).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiStatus {
    indexed: usize,
    current: Option<crate::search::AiProgress>,
}

#[tauri::command]
pub fn ai_status(state: State<'_, AppState>) -> AiStatus {
    AiStatus { indexed: state.search.indexed(), current: state.ai.current.lock().unwrap().clone() }
}

#[tauri::command]
pub fn cancel_scan(state: State<'_, AppState>) {
    state.scans.cancel_all();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatus {
    current: Option<Progress>,
    queued: Vec<i64>,
}

#[tauri::command]
pub fn scan_status(state: State<'_, AppState>) -> ScanStatus {
    ScanStatus { current: state.scans.current.lock().unwrap().clone(), queued: state.scans.pending() }
}

#[tauri::command]
pub async fn reveal_file(state: State<'_, AppState>, file_id: i64) -> Res<()> {
    let cat = state.cat.clone();
    let path = blocking(move || cat.file_path(file_id)).await?;
    shell_open(&path, true)
}

#[tauri::command]
pub async fn open_file(state: State<'_, AppState>, file_id: i64) -> Res<()> {
    let cat = state.cat.clone();
    let path = blocking(move || cat.file_path(file_id)).await?;
    shell_open(&path, false)
}

/// Opens a file or folder with its usual app, or (`select`) shows the file
/// selected in its folder: Explorer, Finder, or the desktop's file manager.
fn shell_open(path: &str, select: bool) -> Res<()> {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        // Explorer parses its own command line: "/select," glued to the quoted path.
        let mut cmd = std::process::Command::new("explorer.exe");
        cmd.raw_arg(if select { format!("/select,\"{path}\"") } else { format!("\"{path}\"") });
        cmd
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut cmd = std::process::Command::new("open");
        if select {
            cmd.arg("-R");
        }
        cmd.arg(path);
        cmd
    };
    // xdg-open can't select a file: it opens the file's folder instead.
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = {
        let target = match Path::new(path).parent() {
            Some(folder) if select => folder,
            _ => Path::new(path),
        };
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(target);
        cmd
    };
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    // Reaped in the background so no finished process lingers.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.cat.settings()
}

#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, settings: Settings) -> Res<Settings> {
    let cat = state.cat.clone();
    in_order(move || cat.save_settings(settings)).await
}

/// Consistent copies of the databases in backup\ (for Kopia), and when that last ran.
#[tauri::command]
pub async fn backup_catalog(state: State<'_, AppState>) -> Res<crate::catalog::BackupReport> {
    let cat = state.cat.clone();
    blocking(move || cat.backup()).await
}

#[tauri::command]
pub fn last_backup(state: State<'_, AppState>) -> Option<i64> {
    state.cat.last_backup()
}

#[tauri::command]
pub async fn check_catalog_folder(state: State<'_, AppState>, path: String) -> Res<FolderCheck> {
    let cat = state.cat.clone();
    blocking(move || cat.check_catalog_folder(Path::new(&path))).await
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MoveProgress {
    done: usize,
    total: usize,
    finished: bool,
    error: Option<String>,
}

/// Stops scanning, copies the catalog to the chosen folder, then restarts
/// Shoebox on it. Runs in the background and reports `catalog-move` events.
#[tauri::command]
pub async fn move_catalog(app: AppHandle, state: State<'_, AppState>, path: String) -> Res<()> {
    let cat = state.cat.clone();
    let check = blocking(move || cat.check_catalog_folder(Path::new(&path))).await?;
    if let Some(p) = check.problem {
        return Err(p);
    }
    if check.has_catalog {
        return Err("That folder already has a catalog: switch to it instead.".into());
    }
    let (cat, scans, ai) = (state.cat.clone(), state.scans.clone(), state.ai.clone());
    scans.paused.store(true, Ordering::Relaxed);
    scans.cancel_all();
    ai.cancel.store(true, Ordering::Relaxed); // no face pictures written mid-move
    std::thread::spawn(move || {
        for _ in 0..600 {
            if scans.current.lock().unwrap().is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let emit = |m: MoveProgress| {
            let _ = app.emit("catalog-move", m);
        };
        let result = cat.move_catalog(Path::new(&check.target), &|done, total| {
            emit(MoveProgress { done, total, finished: false, error: None })
        });
        match result {
            Ok(()) => {
                emit(MoveProgress { done: 0, total: 0, finished: true, error: None });
                std::thread::sleep(Duration::from_millis(800));
                app.restart();
            }
            Err(e) => {
                scans.paused.store(false, Ordering::Relaxed);
                ai.kick();
                emit(MoveProgress { done: 0, total: 0, finished: true, error: Some(format!("{e:#}")) });
            }
        }
    });
    Ok(())
}

/// Restarts Shoebox on a folder that already holds a catalog.
#[tauri::command]
pub fn switch_catalog(app: AppHandle, state: State<'_, AppState>, path: String) -> Res<()> {
    state.cat.use_catalog(Path::new(&path)).map_err(err)?;
    app.restart();
}

#[tauri::command]
pub async fn rename_volume(state: State<'_, AppState>, volume_id: i64, label: String) -> Res<()> {
    let cat = state.cat.clone();
    in_order(move || cat.rename_volume(volume_id, &label)).await
}

/// Renames a folder on its disk and in the catalog. Returns its new path.
#[tauri::command]
pub async fn rename_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    volume_id: i64,
    rel_path: String,
    name: String,
) -> Res<String> {
    if state.scans.current.lock().unwrap().is_some() {
        return Err("A scan is running. Let it finish (or stop it), then rename.".into());
    }
    let cat = state.cat.clone();
    let new_rel = in_order(move || organize::rename_folder(&cat, volume_id, &rel_path, &name)).await?;
    let _ = app.emit("catalog-changed", ());
    Ok(new_rel)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderMoveEvent {
    name: String,
    progress: Option<organize::MoveProgress>,
    result: Option<organize::MoveResult>,
    error: Option<String>,
    finished: bool,
}

/// Drag and drop: moves a folder into another catalog folder. On the same
/// disk it's done at once and the result comes back. To another disk it runs
/// in the background (copy, check every byte, then remove the original) with
/// `folder-move` events, and this returns None.
#[tauri::command]
pub async fn move_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    volume_id: i64,
    rel_path: String,
    to_volume_id: i64,
    to_parent_rel: String,
) -> Res<Option<organize::MoveResult>> {
    if state.scans.current.lock().unwrap().is_some() {
        return Err("A scan is running. Let it finish (or stop it), then move.".into());
    }
    let busy = || "Another folder is being moved. Wait for it to finish.".to_string();
    if state.moving.load(Ordering::Relaxed) {
        return Err(busy());
    }
    let m = organize::FolderMove { from: volume_id, rel: rel_path, to: to_volume_id, to_parent: to_parent_rel };
    let cat = state.cat.clone();
    if m.from == m.to {
        let r = in_order(move || organize::move_folder(&cat, &m, &AtomicBool::new(false), &|_| {})).await?;
        let _ = app.emit("catalog-changed", ());
        return Ok(Some(r));
    }
    let (scans, moving, cancel) = (state.scans.clone(), state.moving.clone(), state.move_cancel.clone());
    // Taken now: two folders dropped at once don't both start.
    if moving.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return Err(busy());
    }
    let checked = {
        let cat = cat.clone();
        blocking(move || organize::check_move(&cat, &m).map(|_| m)).await
    };
    let m = match checked {
        Ok(m) => m,
        Err(e) => {
            moving.store(false, Ordering::Relaxed);
            return Err(e);
        }
    };
    cancel.store(false, Ordering::Relaxed);
    scans.paused.store(true, Ordering::Relaxed); // nothing scans while files move
    let name = m.rel.rsplit('\\').next().unwrap_or("").to_string();
    std::thread::spawn(move || {
        let emit = |e: FolderMoveEvent| {
            let _ = app.emit("folder-move", e);
        };
        let last = std::sync::Mutex::new(std::time::Instant::now() - Duration::from_secs(1));
        let result = organize::move_folder(&cat, &m, &cancel, &|p| {
            let mut l = last.lock().unwrap();
            if l.elapsed() > Duration::from_millis(150) || p.files == p.total_files {
                *l = std::time::Instant::now();
                emit(FolderMoveEvent {
                    name: name.clone(),
                    progress: Some(p.clone()),
                    result: None,
                    error: None,
                    finished: false,
                });
            }
        });
        scans.paused.store(false, Ordering::Relaxed);
        moving.store(false, Ordering::Relaxed);
        let (result, error) = match result {
            Ok(r) => (Some(r), None),
            Err(e) => {
                log::warn!("move {name}: {e:#}");
                (None, Some(format!("{e:#}")))
            }
        };
        emit(FolderMoveEvent { name, progress: None, result, error, finished: true });
        let _ = app.emit("catalog-changed", ());
    });
    Ok(None)
}

#[tauri::command]
pub fn cancel_folder_move(state: State<'_, AppState>) {
    state.move_cancel.store(true, Ordering::Relaxed);
}

/// Makes a new folder inside a catalog folder. Returns its path.
#[tauri::command]
pub async fn new_folder(app: AppHandle, state: State<'_, AppState>, volume_id: i64, parent_rel: String) -> Res<String> {
    let cat = state.cat.clone();
    let rel = in_order(move || organize::create_folder(&cat, volume_id, &parent_rel, "New folder")).await?;
    let _ = app.emit("catalog-changed", ());
    Ok(rel)
}

#[tauri::command]
pub async fn show_folder(state: State<'_, AppState>, volume_id: i64, rel_path: String) -> Res<()> {
    let cat = state.cat.clone();
    let path = blocking(move || cat.folder_path(volume_id, &rel_path)).await?;
    shell_open(&path, false)
}

/// Every tag, most used first.
#[tauri::command]
pub async fn tags_all(state: State<'_, AppState>) -> Res<Vec<tags::Tag>> {
    let cat = state.cat.clone();
    blocking(move || tags::all(&cat)).await
}

/// The tags on these photos; `count` = how many of them have each.
#[tauri::command]
pub async fn tags_on(state: State<'_, AppState>, photo_ids: Vec<i64>) -> Res<Vec<tags::Tag>> {
    let cat = state.cat.clone();
    blocking(move || tags::on(&cat, &photo_ids)).await
}

#[tauri::command]
pub async fn tags_add(state: State<'_, AppState>, photo_ids: Vec<i64>, names: Vec<String>) -> Res<Vec<tags::Tag>> {
    let cat = state.cat.clone();
    in_order(move || tags::add(&cat, &photo_ids, &names)).await
}

#[tauri::command]
pub async fn tags_remove(state: State<'_, AppState>, photo_ids: Vec<i64>, tag_ids: Vec<i64>) -> Res<usize> {
    let cat = state.cat.clone();
    in_order(move || tags::remove(&cat, &photo_ids, &tag_ids)).await
}

#[tauri::command]
pub async fn tag_rename(state: State<'_, AppState>, tag_id: i64, name: String) -> Res<()> {
    let cat = state.cat.clone();
    in_order(move || tags::rename(&cat, tag_id, &name)).await
}

/// Gear: renames a camera or lens (onto a name shown = merge); returns the name it goes by.
#[tauri::command]
pub async fn gear_rename(state: State<'_, AppState>, kind: gear::Kind, name: String, to: String) -> Res<String> {
    let cat = state.cat.clone();
    in_order(move || gear::rename(&cat.db(), kind, &name, &to)).await
}

/// Gear: back to the names in the files, kept apart.
#[tauri::command]
pub async fn gear_split(state: State<'_, AppState>, kind: gear::Kind, name: String) -> Res<Vec<String>> {
    let cat = state.cat.clone();
    in_order(move || gear::split(&cat.db(), kind, &name)).await
}

#[tauri::command]
pub async fn tag_delete(state: State<'_, AppState>, tag_id: i64) -> Res<()> {
    let cat = state.cat.clone();
    in_order(move || tags::delete(&cat, tag_id)).await
}

/// Culling: marks or unmarks photos for deletion. Catalog only.
#[tauri::command]
pub async fn mark_photos(state: State<'_, AppState>, ids: Vec<i64>, marked: bool) -> Res<usize> {
    let cat = state.cat.clone();
    in_order(move || organize::mark(&cat, &ids, marked)).await
}

#[tauri::command]
pub async fn unmark_all(state: State<'_, AppState>) -> Res<usize> {
    let cat = state.cat.clone();
    in_order(move || organize::unmark_all(&cat)).await
}

/// What "Delete marked files" would remove now, by disk.
#[tauri::command]
pub async fn delete_plan(state: State<'_, AppState>) -> Res<DeletePlan> {
    let cat = state.cat.clone();
    blocking(move || organize::plan(&cat)).await
}

/// Deletes the plan the user confirmed (`token`), on the disks they ticked.
#[tauri::command]
pub async fn delete_marked(
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
    volume_ids: Vec<i64>,
) -> Res<DeleteReport> {
    if state.scans.current.lock().unwrap().is_some() {
        return Err("A scan is running. Let it finish (or stop it), then delete.".into());
    }
    let cat = state.cat.clone();
    let report = blocking(move || organize::delete_confirmed(&cat, &token, &volume_ids)).await?;
    let _ = app.emit("catalog-changed", ());
    Ok(report)
}

/// Settings > AI models: what Shoebox can use, and what is here.
#[tauri::command]
pub async fn models_list() -> Res<models::ModelsOverview> {
    tauri::async_runtime::spawn_blocking(models::overview).await.map_err(|e| e.to_string())
}

/// Queues a model's download (from its publisher, checked by SHA-256).
/// Progress comes as `model-download` events.
#[tauri::command]
pub fn model_download(state: State<'_, AppState>, id: String) {
    state.models.push(&id);
}

/// Stops a model's download, or takes it out of the queue.
#[tauri::command]
pub fn model_stop(state: State<'_, AppState>, id: String) {
    state.models.stop(&id);
}

/// The download running now (and those waiting), for a window opened midway.
#[tauri::command]
pub fn model_progress(state: State<'_, AppState>) -> Option<crate::ModelProgress> {
    let mut p = state.models.current.lock().unwrap().clone()?;
    p.queued = state.models.pending();
    Some(p)
}

/// Deletes a model. Its feature lets go of it first; what it already did
/// (descriptions, faces, people) stays. Returns the bytes freed.
#[tauri::command]
pub async fn model_delete(state: State<'_, AppState>, id: String) -> Res<u64> {
    if state.models.busy_with(&id) {
        return Err("It's downloading. Stop the download first.".into());
    }
    let (search, faces, ai) = (state.search.clone(), state.faces.clone(), state.ai.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let in_use = id == models::CLIP || id == models::FACES;
        if in_use {
            ai.cancel.store(true, Ordering::Relaxed); // it lets go after the photo it's on
            if id == models::CLIP {
                search.unload();
            } else {
                faces.unload();
            }
        }
        let freed = models::delete(&id);
        if in_use {
            ai.kick(); // the other feature carries on
        }
        freed
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(err)
}

#[tauri::command]
pub fn show_models_folder() -> Res<()> {
    let dir = crate::paths::models_dir();
    if !dir.is_dir() {
        return Err(format!("{} doesn't exist yet: it's made with the first download.", dir.display()));
    }
    shell_open(&dir.to_string_lossy(), false)
}

/// What the last look for a newer Shoebox found; none before the first one.
#[tauri::command]
pub fn update_status(state: State<'_, AppState>) -> Option<crate::updates::UpdateStatus> {
    state.updates.lock().unwrap().clone()
}

/// Settings > "Check now": asks GitHub at once, even with the regular check off.
#[tauri::command]
pub async fn check_updates(app: AppHandle, state: State<'_, AppState>) -> Res<crate::updates::UpdateStatus> {
    let found = state.updates.clone();
    let status = tauri::async_runtime::spawn_blocking(move || {
        let previous = found.lock().unwrap().clone();
        let status = crate::updates::check(previous.as_ref());
        *found.lock().unwrap() = Some(status.clone());
        status
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit("update-status", status.clone());
    Ok(status)
}

/// Opens the latest release's page in the browser.
#[tauri::command]
pub fn open_releases(state: State<'_, AppState>) -> Res<()> {
    let url = state.updates.lock().unwrap().as_ref().map(|s| s.url.clone());
    shell_open(url.as_deref().unwrap_or(crate::updates::RELEASES_PAGE), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_that_fails_does_not_stop_the_next_ones() {
        tauri::async_runtime::block_on(async {
            let failed = in_order(|| -> anyhow::Result<()> { panic!("a change gone wrong") }).await;
            assert!(failed.is_err());
            let refused = in_order(|| -> anyhow::Result<()> { bail!("not allowed") }).await;
            assert_eq!(refused, Err("not allowed".to_string()));
            for i in 0..5 {
                assert_eq!(in_order(move || Ok(i * 2)).await, Ok(i * 2));
            }
        });
    }
}
