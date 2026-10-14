//! Where Shoebox keeps its catalog, and **every file write Shoebox makes**.
//!
//! The catalog is one folder (default `<Shoebox folder>\data`, or `~/.shoebox`
//! itself once installed; movable from Settings), never AppData. Its location
//! is remembered in `shoebox.json` in the Shoebox folder. All writes live in
//! this file and are refused outside Shoebox's own folders, so no bug can ever
//! write next to a photo; a test enforces that no other source file writes at
//! all.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug)]
pub struct DataDir {
    pub root: PathBuf,
}

/// The Shoebox folder, holding `shoebox.json`: the app's own folder when it
/// runs portable, otherwise (installed) `~/.shoebox`. An installed app's
/// folder belongs to its installer (replaced on update, removed on
/// uninstall) or isn't writable at all (a .app, /usr/bin).
pub fn app_home() -> PathBuf {
    portable_home().unwrap_or_else(installed_home)
}

/// The folder Shoebox runs from, when it keeps everything beside itself: the
/// repository root in dev builds, the app's folder when `shoebox.json` sits
/// next to it (the portable download ships one).
fn portable_home() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        return Some(Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("src-tauri has a parent").to_path_buf());
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    dir.join(POINTER_FILE).is_file().then_some(dir)
}

fn installed_home() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".shoebox")
}

/// The AI models: in the folder Shoebox runs from - the portable folder, or
/// the install folder (Windows installs per user, so it's writable). A macOS
/// .app and /usr/bin can't take them, so installed there they go in
/// `~/.shoebox`. `SHOEBOX_MODELS` points elsewhere (tests use it).
pub fn models_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("SHOEBOX_MODELS") {
        return PathBuf::from(p);
    }
    let installed_dir = || std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    match portable_home() {
        Some(home) => home.join("models"),
        None if cfg!(windows) => installed_dir().unwrap_or_else(installed_home).join("models"),
        None => installed_home().join("models"),
    }
}

const POINTER_FILE: &str = "shoebox.json";

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Pointer {
    data_dir: Option<PathBuf>,
    /// The folder a move left behind, removed once Shoebox has started on
    /// the new one (see [`remove_old_catalog`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    old_data_dir: Option<PathBuf>,
}

fn read_pointer() -> Option<Pointer> {
    serde_json::from_str(&std::fs::read_to_string(app_home().join(POINTER_FILE)).ok()?).ok()
}

fn write_pointer(p: &Pointer) -> io::Result<()> {
    atomic_write(&app_home().join(POINTER_FILE), serde_json::to_string_pretty(p)?.as_bytes())
}

impl DataDir {
    /// `SHOEBOX_DATA` wins (tests use it), then the folder chosen in Settings,
    /// then `<Shoebox folder>\data` - or, installed, `~/.shoebox` itself.
    pub fn locate() -> Self {
        if let Some(p) = std::env::var_os("SHOEBOX_DATA") {
            return Self { root: PathBuf::from(p) };
        }
        let chosen = read_pointer().and_then(|p| p.data_dir);
        let default = || portable_home().map_or_else(installed_home, |home| home.join("data"));
        Self { root: chosen.unwrap_or_else(default) }
    }

    pub fn ensure(&self) -> io::Result<()> {
        for sub in ["thumbs", "previews", "logs"] {
            std::fs::create_dir_all(self.root.join(sub))?;
        }
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join("catalog.db")
    }

    pub fn settings_path(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn log_path(&self) -> PathBuf {
        self.root.join("logs").join("shoebox.log")
    }

    pub fn thumb_path(&self, photo_id: i64) -> PathBuf {
        shard(&self.root.join("thumbs"), photo_id)
    }

    pub fn preview_path(&self, photo_id: i64) -> PathBuf {
        shard(&self.root.join("previews"), photo_id)
    }

    pub fn face_path(&self, face_id: i64) -> PathBuf {
        shard(&self.root.join("faces"), face_id)
    }

    /// Where `backup` puts its consistent copies of the databases.
    pub fn backup_dir(&self) -> PathBuf {
        self.root.join("backup")
    }

    /// A consistent snapshot of an SQLite database (open or not, through
    /// `conn`) at `dst` inside the data folder. The previous snapshot is
    /// replaced only once the new one is complete.
    pub fn snapshot_into(&self, conn: &rusqlite::Connection, dst: &Path) -> anyhow::Result<()> {
        if !inside(dst, &self.root) {
            anyhow::bail!("Shoebox only writes inside {}, refused {}", self.root.display(), dst.display());
        }
        if let Some(dir) = dst.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = dst.with_extension("db.tmp");
        let _ = std::fs::remove_file(&tmp);
        conn.execute("VACUUM INTO ?1", [tmp.to_string_lossy()])?;
        std::fs::rename(&tmp, dst)?;
        Ok(())
    }

    pub fn has_catalog(&self) -> bool {
        self.db_path().is_file()
    }

    /// **The only way Shoebox writes a catalog file.** Refuses any path outside
    /// the data folder.
    pub fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        if !inside(path, &self.root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("Shoebox only writes inside {}, refused {}", self.root.display(), path.display()),
            ));
        }
        atomic_write(path, bytes)
    }

    /// Deletes one of Shoebox's own files (a preview or thumbnail). Refused
    /// outside the data folder, like every other write.
    pub fn remove(&self, path: &Path) -> io::Result<()> {
        if !inside(path, &self.root) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("refused {}", path.display())));
        }
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Takes the lock of folder operation `id` (see [`OpLock`]), or None when
    /// something else holds it: that operation is still running.
    pub fn try_op_lock(&self, id: &str) -> io::Result<Option<OpLock>> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad operation id {id:?}")));
        }
        let path = self.root.join(OPS_DIR).join(format!("{id}.lock"));
        std::fs::create_dir_all(self.root.join(OPS_DIR))?;
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(OpLock { file: Some(file), path })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }

    /// Ids of the folder operations with a lock file: running, or cut short
    /// (the system let go of their lock, the file stayed).
    pub fn op_lock_ids(&self) -> Vec<String> {
        std::fs::read_dir(self.root.join(OPS_DIR))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".lock").map(str::to_string))
            .collect()
    }

    /// (number of files, total bytes) under a sub-folder of the catalog.
    pub fn usage(&self, sub: &str) -> (u64, u64) {
        walkdir::WalkDir::new(self.root.join(sub))
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file())
            .fold((0, 0), |(n, b), e| (n + 1, b + e.metadata().map(|m| m.len()).unwrap_or(0)))
    }

    /// Every file of the catalog except its databases (open while Shoebox runs:
    /// they are snapshotted instead), half-written temp files and the locks of
    /// folder operations (held by a running one; nothing to keep).
    pub fn files(&self) -> Vec<PathBuf> {
        walkdir::WalkDir::new(&self.root)
            .into_iter()
            // Shoebox's own, not the catalog's: installed, `~/.shoebox` is both
            // the Shoebox folder and the first catalog. A move's own notes stay too.
            .filter_entry(|e| {
                e.depth() != 1
                    || !matches!(
                        e.file_name().to_str(),
                        Some("models" | POINTER_FILE | MOVE_JOURNAL | MOVE_LOCK | OPS_DIR)
                    )
            })
            .flatten()
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                ![".db", ".db-wal", ".db-shm", ".db-journal", ".tmp"].iter().any(|end| name.ends_with(end))
            })
            .collect()
    }

    /// Starts moving this catalog into `target`, an empty or new folder. The
    /// move is written down here ([`MOVE_JOURNAL`]) until it is done, so a
    /// failure or a Shoebox stopped halfway can be undone
    /// ([`DataDir::undo_move`]), and held ([`MoveLock`]) so another Shoebox
    /// starting meanwhile leaves it alone. A move cut short earlier, not
    /// undone yet, is undone first.
    pub fn begin_move(&self, target: &DataDir) -> io::Result<MoveLock> {
        let lock = MoveLock::take(self)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "The catalog is already being moved."))?;
        self.undo_move_held()?;
        let made_target = !target.root.exists();
        if !made_target && std::fs::read_dir(&target.root)?.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists and isn't empty.", target.root.display()),
            ));
        }
        std::fs::create_dir_all(&target.root)?;
        self.write_journal(&MoveJournal { to: target.root.clone(), made_target, renamed: Vec::new() })?;
        Ok(lock)
    }

    fn journal(&self) -> Option<MoveJournal> {
        serde_json::from_str(&std::fs::read_to_string(self.root.join(MOVE_JOURNAL)).ok()?).ok()
    }

    fn write_journal(&self, journal: &MoveJournal) -> io::Result<()> {
        self.write(&self.root.join(MOVE_JOURNAL), serde_json::to_string_pretty(journal)?.as_bytes())
    }

    /// Moves this catalog's files into `target` (after [`DataDir::begin_move`]).
    /// The big folders (previews, thumbnails, faces) move whole, the way the
    /// OS moves a folder: on the same disk an instant rename, nothing copied;
    /// each is written down just before. What is left (settings, logs - and
    /// everything, on another disk) is copied.
    pub fn move_files_to(&self, target: &DataDir, progress: &dyn Fn(usize, usize)) -> io::Result<()> {
        let mut journal = self.journal().ok_or_else(|| io::Error::other("the catalog move wasn't begun"))?;
        for sub in ["previews", "thumbs", "faces"] {
            let (from, to) = (self.root.join(sub), target.root.join(sub));
            if !from.is_dir() || to.exists() || !inside(&to, &target.root) {
                continue;
            }
            journal.renamed.push(sub.to_string());
            self.write_journal(&journal)?;
            // A preview being read this very moment can hold the folder briefly.
            let mut renamed = false;
            for attempt in 0..5 {
                match std::fs::rename(&from, &to) {
                    Ok(()) => {
                        renamed = true;
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => break, // another disk: copied below
                    Err(_) if attempt < 4 => std::thread::sleep(std::time::Duration::from_millis(200)),
                    Err(_) => break,
                }
            }
            if !renamed {
                journal.renamed.pop();
                self.write_journal(&journal)?;
            }
        }
        let files = self.files();
        for (i, src) in files.iter().enumerate() {
            let rel = src.strip_prefix(&self.root).map_err(io::Error::other)?;
            let dst = target.root.join(rel);
            let same = dst.metadata().ok().map(|m| m.len()) == src.metadata().ok().map(|m| m.len());
            if !same {
                target.write(&dst, &std::fs::read(src)?)?;
            }
            if i % 50 == 0 || i + 1 == files.len() {
                progress(i + 1, files.len());
            }
        }
        Ok(())
    }

    /// Puts the databases staged in this folder (a move's target, see
    /// [`snapshot_db`]) in their place, the catalog's last: only then does the
    /// folder hold a catalog.
    pub fn place_databases(&self) -> io::Result<()> {
        for name in ["faces.db", "search.db", "catalog.db"] {
            let staged = staged(&self.root, name);
            if staged.is_file() {
                std::fs::rename(&staged, self.root.join(name))?;
            }
        }
        if !self.has_catalog() {
            return Err(io::Error::other(format!("no catalog was copied into {}", self.root.display())));
        }
        Ok(())
    }

    /// Undoes a catalog move of this catalog that didn't finish: Shoebox was
    /// stopped halfway (run at start, before the catalog opens). The folders
    /// moved whole come back - joining whatever was saved in their place
    /// since - and what the move put in the target goes. Left alone: a move
    /// still under way in another Shoebox, and one already done (shoebox.json
    /// points at its target). Returns the target of a move undone.
    pub fn undo_move(&self) -> io::Result<Option<PathBuf>> {
        if !self.root.join(MOVE_JOURNAL).is_file() {
            return Ok(None);
        }
        match MoveLock::take(self)? {
            Some(_held) => self.undo_move_held(),
            None => Ok(None),
        }
    }

    /// [`DataDir::undo_move`], the move's [`MoveLock`] held.
    fn undo_move_held(&self) -> io::Result<Option<PathBuf>> {
        if !self.root.join(MOVE_JOURNAL).is_file() {
            return Ok(None);
        }
        // Kept, not guessed at: it says which folders to bring back.
        let journal = self
            .journal()
            .ok_or_else(|| io::Error::other(format!("{} can't be read", self.root.join(MOVE_JOURNAL).display())))?;
        let target = journal.to.clone();
        let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
        let (t, c) = (lower(&target), lower(&self.root));
        let done = read_pointer().and_then(|p| p.data_dir).is_some_and(|d| lower(&d) == t);
        let undone = !done && !t.starts_with(&c) && !c.starts_with(&t);
        if undone {
            for sub in &journal.renamed {
                bring_back(&target.join(sub), &self.root.join(sub))?;
            }
            // Everything else in the target came from the move (it was empty):
            // the staged databases, copies of the catalog's files.
            if let Ok(entries) = std::fs::read_dir(&target) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                    let copied = is_catalog_entry(&name, is_dir) || self.root.join(&name).exists();
                    if !copied || journal.renamed.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                        continue;
                    }
                    let gone = if is_dir { std::fs::remove_dir_all(e.path()) } else { std::fs::remove_file(e.path()) };
                    if let Err(err) = gone {
                        log::warn!("catalog move undone: couldn't remove {}: {err}", e.path().display());
                    }
                }
            }
            if journal.made_target {
                let _ = std::fs::remove_dir(&target);
            }
        }
        self.remove(&self.root.join(MOVE_JOURNAL))?;
        Ok(undone.then_some(target))
    }

    /// Removes the temporary files a Shoebox stopped mid-write left behind
    /// ([`atomic_write`]), in the catalog folder and beside `shoebox.json`.
    /// Only those older than [`STRAY_AFTER`]: a newer one may be another
    /// Shoebox's (the CLI's) write under way. Returns how many.
    pub fn remove_stray_temp_files(&self) -> usize {
        let in_catalog = walkdir::WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| e.depth() != 1 || e.file_name() != "models")
            .flatten()
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path());
        let pointer = std::fs::read_dir(app_home())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("shoebox.")));
        let old = |p: &Path| {
            p.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|a| a >= STRAY_AFTER)
        };
        in_catalog
            .chain(pointer)
            .filter(|p| p.file_name().is_some_and(|n| is_write_temp(&n.to_string_lossy())))
            .filter(|p| old(p) && std::fs::remove_file(p).is_ok())
            .count()
    }
}

/// A catalog move under way, written down in the folder it leaves until it is
/// done: where to, and which folders moved whole.
const MOVE_JOURNAL: &str = "moving.json";
/// Held by the Shoebox moving the catalog, for as long as it does.
const MOVE_LOCK: &str = "moving.lock";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveJournal {
    to: PathBuf,
    /// The move made the target folder: undone, it goes too.
    made_target: bool,
    /// Folders moved whole by a rename, written down just before: undone,
    /// they come back (never deleted).
    renamed: Vec<String>,
}

/// The lock of a catalog move ([`DataDir::begin_move`]): the folder it leaves
/// holds `moving.lock`, locked by the Shoebox moving it. The system lets go
/// of it when that Shoebox ends, however it ends, so a journal whose lock can
/// be taken belongs to a move cut short.
pub struct MoveLock {
    file: std::fs::File,
    data: DataDir,
}

impl MoveLock {
    /// None: another Shoebox (or this one) is moving the catalog right now.
    fn take(data: &DataDir) -> io::Result<Option<Self>> {
        let file =
            std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(data.root.join(MOVE_LOCK))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { file, data: data.clone() })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }

    /// The move failed: everything goes back (see [`DataDir::undo_move`]).
    pub fn undo(self) -> io::Result<()> {
        self.data.undo_move_held().map(|_| ())
    }

    /// The move is done and the new catalog remembered: the journal goes.
    pub fn finish(self) {
        let _ = self.data.remove(&self.data.root.join(MOVE_JOURNAL));
    }
}

impl Drop for MoveLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = std::fs::remove_file(self.data.root.join(MOVE_LOCK));
    }
}

/// Puts a folder a catalog move renamed back in its place. Whatever was saved
/// there since (an empty folder made at start, a face picture saved as the
/// move began) stays, and the rest joins it file by file.
fn bring_back(from: &Path, to: &Path) -> io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    let files = |dir: &Path| -> Vec<PathBuf> {
        walkdir::WalkDir::new(dir)
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .collect()
    };
    if to.is_dir() && files(to).is_empty() {
        std::fs::remove_dir_all(to)?;
    }
    if !to.exists() {
        return std::fs::rename(from, to);
    }
    for src in files(from) {
        let dst = to.join(src.strip_prefix(from).map_err(io::Error::other)?);
        if !dst.exists() {
            if let Some(dir) = dst.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::rename(&src, &dst)?;
        }
    }
    if files(from).is_empty() {
        std::fs::remove_dir_all(from)?;
    }
    Ok(())
}

/// A temporary file of [`atomic_write`]: `<name>.<pid>-<n>.tmp`.
fn is_write_temp(name: &str) -> bool {
    let tag = name.strip_suffix(".tmp").and_then(|s| s.rsplit_once('.')).map(|(_, tag)| tag);
    let Some((pid, n)) = tag.and_then(|t| t.split_once('-')) else { return false };
    [pid, n].iter().all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}

/// A write takes a moment: a temporary file this old belongs to no write
/// under way.
const STRAY_AFTER: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Where a catalog move's target keeps a database until
/// [`DataDir::place_databases`]: `<name>.tmp`, which no Shoebox takes for a
/// catalog.
fn staged(root: &Path, name: &str) -> PathBuf {
    root.join(format!("{name}.tmp"))
}

/// A consistent copy of `conn`'s database, staged in `target` as `name`
/// ([`staged`]), safe while Shoebox has it open (SQLite's `VACUUM INTO`).
fn stage(conn: &rusqlite::Connection, target: &DataDir, name: &str) -> anyhow::Result<()> {
    let dst = staged(&target.root, name);
    if !inside(&dst, &target.root) || target.root.join(name).exists() {
        anyhow::bail!("refused {}", dst.display());
    }
    std::fs::create_dir_all(&target.root)?;
    let _ = std::fs::remove_file(&dst);
    conn.execute("VACUUM INTO ?1", [dst.to_string_lossy()])?;
    Ok(())
}

/// A consistent copy of the open catalog database for a catalog move into
/// `target`, safe while Shoebox is running (SQLite's `VACUUM INTO`). Staged:
/// it becomes `catalog.db` only through [`DataDir::place_databases`].
pub fn snapshot_db(conn: &rusqlite::Connection, target: &DataDir) -> anyhow::Result<()> {
    if target.has_catalog() {
        anyhow::bail!("{} already has a catalog", target.root.display());
    }
    stage(conn, target, "catalog.db")
}

/// The same for one of the catalog's other databases (faces, search), under
/// its own name, safe while Shoebox has it open.
pub fn snapshot_sqlite(src: &Path, target: &DataDir) -> anyhow::Result<()> {
    let name = src.file_name().ok_or_else(|| anyhow::anyhow!("no file name in {}", src.display()))?;
    let conn = rusqlite::Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    stage(&conn, target, &name.to_string_lossy())
}

/// Remembers `root` as the catalog folder for the next start.
/// `left_behind`: the folder a move came from, removed at the next start.
pub fn remember_data_dir(root: &Path, left_behind: Option<&Path>) -> io::Result<()> {
    write_pointer(&Pointer { data_dir: Some(root.to_path_buf()), old_data_dir: left_behind.map(Path::to_path_buf) })
}

/// Where the locks of folder operations live, in the catalog folder.
const OPS_DIR: &str = "ops";

/// Held for as long as a folder move, rename or delete runs (organize.rs): a
/// lock on `ops\<id>.lock` in the catalog folder. The system lets go of it
/// when the process ends, however it ends, so an operation whose lock is free
/// was cut short, and one whose lock is held is still running - in this
/// process or another (the window and `shoebox-cli` share a catalog).
pub struct OpLock {
    file: Option<std::fs::File>,
    path: PathBuf,
}

impl Drop for OpLock {
    fn drop(&mut self) {
        drop(self.file.take()); // closing it lets go of the lock
        let _ = std::fs::remove_file(&self.path);
    }
}

/// What a catalog folder holds of its own: the databases (and their SQLite
/// side files), settings, logs, backups, previews, thumbnails, faces, and a
/// move's notes.
fn is_catalog_entry(name: &str, is_dir: bool) -> bool {
    let n = name.to_lowercase();
    if is_dir {
        return ["previews", "thumbs", "faces", "logs", "backup", OPS_DIR].contains(&n.as_str());
    }
    let db_file = ["catalog.db", "faces.db", "search.db"]
        .iter()
        .any(|db| n.strip_prefix(db).is_some_and(|rest| ["", "-wal", "-shm", "-journal", ".tmp"].contains(&rest)));
    db_file || ["settings.json", "moved.txt", ".kopiaignore", MOVE_JOURNAL, MOVE_LOCK].contains(&n.as_str())
}

/// The old catalog's own entries in `dir`, and whether anything else is there
/// (installed, `~/.shoebox` also holds `models` and `shoebox.json`).
fn old_catalog_entries(dir: &Path) -> io::Result<(Vec<PathBuf>, bool)> {
    let mut own = Vec::new();
    let mut other = false;
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if is_catalog_entry(&e.file_name().to_string_lossy(), e.file_type()?.is_dir()) {
            own.push(e.path());
        } else {
            other = true;
        }
    }
    Ok((own, other))
}

/// After a catalog move, the old folder still holds the old databases (and,
/// moved to another disk, a full copy of everything). Once Shoebox has opened
/// the new catalog, they go to the Recycle Bin: only the catalog's own files
/// and folders, never `models` or `shoebox.json` - the whole folder when
/// nothing else is in it. Not found (its disk unplugged), it waits for a later
/// start; gone from a disk that is there, it is done (a Shoebox stopped right
/// after sending it to the Recycle Bin, before noting so). Returns the folder
/// cleared.
pub fn remove_old_catalog(current: &DataDir) -> anyhow::Result<Option<PathBuf>> {
    let Some(mut p) = read_pointer() else { return Ok(None) };
    let Some(old) = p.old_data_dir.clone() else { return Ok(None) };
    let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    let (o, c) = (lower(&old), lower(&current.root));
    let in_use = o.starts_with(&c) || c.starts_with(&o) || lower(&models_dir()).starts_with(&o);
    // Never the catalog (or models) in use; and nothing left to remove.
    if in_use || is_gone(&old) {
        p.old_data_dir = None;
        write_pointer(&p)?;
        return Ok(None);
    }
    if !current.has_catalog() || !old.is_dir() {
        return Ok(None);
    }
    let (own, other) = old_catalog_entries(&old)?;
    if !other {
        trash::delete(&old)?;
    } else if !own.is_empty() {
        trash::delete_all(&own)?;
    }
    p.old_data_dir = None;
    write_pointer(&p)?;
    Ok(Some(old))
}

/// Not there while the folder holding it is: deleted, not on an unplugged disk.
fn is_gone(dir: &Path) -> bool {
    !dir.exists() && dir.parent().is_some_and(Path::is_dir)
}

/// An AI model file being downloaded, written as `<name>.part` beside where it
/// goes. It takes its place only through [`ModelFile::keep`] (once its SHA-256
/// matched); dropped without that, the part file is removed.
pub struct ModelFile {
    part: PathBuf,
    dst: PathBuf,
    kept: bool,
}

impl ModelFile {
    /// The part file, complete, to check or unpack before keeping it.
    pub fn part(&self) -> &Path {
        &self.part
    }

    pub fn keep(mut self) -> io::Result<()> {
        std::fs::rename(&self.part, &self.dst)?;
        self.kept = true;
        Ok(())
    }
}

impl Drop for ModelFile {
    fn drop(&mut self) {
        if !self.kept {
            let _ = std::fs::remove_file(&self.part);
        }
    }
}

/// Streams `src` into the models folder for `dst` (see [`ModelFile`]).
/// Refused anywhere else.
pub fn model_file(dst: &Path, src: &mut dyn io::Read) -> io::Result<ModelFile> {
    let dir = models_dir();
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !inside(dst, &dir) || dst == dir || name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("Shoebox only puts models inside {}, refused {}", dir.display(), dst.display()),
        ));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = ModelFile { part: dst.with_file_name(format!("{name}.part")), dst: dst.to_path_buf(), kept: false };
    let mut out = std::fs::File::create(&file.part)?;
    io::copy(src, &mut out)?;
    out.sync_all()?;
    Ok(file)
}

/// Deletes an AI model's file or folder (Settings > AI models), then any
/// folder it leaves empty. Refused outside the models folder, and on the
/// models folder itself.
pub fn remove_model_path(path: &Path) -> io::Result<()> {
    let dir = models_dir();
    if !inside(path, &dir) || path.components().count() <= dir.components().count() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("refused {}", path.display())));
    }
    let gone = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
    match gone {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut parent = path.parent();
    while let Some(p) = parent {
        if p.components().count() <= dir.components().count() || std::fs::remove_dir(p).is_err() {
            break;
        }
        parent = p.parent();
    }
    Ok(())
}

/// `path` is inside `root` and does not climb out with `..`.
pub fn inside(path: &Path, root: &Path) -> bool {
    path.starts_with(root) && !path.components().any(|c| c == Component::ParentDir)
}

/// Each write gets its own temporary file: two writers of the same file must
/// never share one (one rename would take the other's file away).
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// 256 sub-folders so no single folder holds 100k files.
fn shard(base: &Path, id: i64) -> PathBuf {
    base.join(format!("{:02x}", id & 0xff)).join(format!("{id}.jpg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_writes_outside_the_data_folder() {
        let data = DataDir { root: std::env::temp_dir().join("shoebox-write-guard") };
        let photo = Path::new(r"C:\Photos\IMG_0001.CR3");
        assert!(data.write(photo, b"x").is_err());
        assert!(data.write(&data.root.join("..").join("escape.jpg"), b"x").is_err());
        assert!(data.write(&data.root.join("thumbs").join("ok.jpg"), b"x").is_ok());
        let _ = std::fs::remove_dir_all(&data.root);
    }

    /// A running operation's lock can't be taken by anyone else - even another
    /// handle in the same process; once let go, it can, and its file is gone.
    #[test]
    fn an_operation_lock_is_held_until_let_go() {
        let data = DataDir { root: std::env::temp_dir().join("shoebox-op-lock") };
        let _ = std::fs::remove_dir_all(&data.root);
        assert!(data.try_op_lock("../escape").is_err() && data.try_op_lock("").is_err());
        let held = data.try_op_lock("1-2-3").unwrap().expect("free");
        assert!(data.try_op_lock("1-2-3").unwrap().is_none());
        assert_eq!(data.op_lock_ids(), ["1-2-3"]);
        drop(held);
        assert!(data.op_lock_ids().is_empty());
        assert!(data.try_op_lock("1-2-3").unwrap().is_some());
        let _ = std::fs::remove_dir_all(&data.root);
    }

    /// Downloaded models only ever land in (and leave from) the models folder,
    /// and a download not kept leaves nothing behind.
    #[test]
    fn model_files_stay_in_the_models_folder() {
        let models = models_dir();
        let mut src: &[u8] = b"x";
        assert!(model_file(Path::new(r"C:\Photos\IMG_0001.CR3"), &mut src).is_err());
        assert!(model_file(&models.join("..").join("escape.onnx"), &mut src).is_err());
        assert!(remove_model_path(&models).is_err());
        assert!(remove_model_path(Path::new(r"C:\Photos")).is_err());

        let dst = models.join(".guard-test").join("ok.onnx");
        let dropped = model_file(&dst, &mut src).unwrap();
        let part = dropped.part().to_path_buf();
        assert!(part.is_file() && !dst.exists());
        drop(dropped);
        assert!(!part.exists());
        // A stopped download leaves its folder empty: tidied away.
        remove_model_path(&part).unwrap();
        assert!(!part.parent().unwrap().exists());

        model_file(&dst, &mut &b"x"[..]).unwrap().keep().unwrap();
        assert!(dst.is_file());
        remove_model_path(&dst).unwrap();
        assert!(!dst.parent().unwrap().exists());
    }

    /// Only the old catalog's own files go after a move: installed, the
    /// folder is also `~/.shoebox` with the models and `shoebox.json`.
    #[test]
    fn old_catalog_keeps_what_isnt_the_catalogs() {
        let data = DataDir { root: std::env::temp_dir().join("shoebox-old-catalog") };
        let _ = std::fs::remove_dir_all(&data.root);
        for f in [
            "catalog.db",
            "catalog.db-wal",
            "search.db-shm",
            "settings.json",
            "MOVED.txt",
            "previews/01/1.jpg",
            "logs/shoebox.log",
            "models/clip/model.onnx",
            "shoebox.json",
            "notes.txt",
        ] {
            data.write(&data.root.join(f), b"x").unwrap();
        }
        let (own, other) = old_catalog_entries(&data.root).unwrap();
        let mut own: Vec<String> = own.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        own.sort();
        assert_eq!(
            own,
            ["MOVED.txt", "catalog.db", "catalog.db-wal", "logs", "previews", "search.db-shm", "settings.json"]
        );
        assert!(other);
        let _ = std::fs::remove_dir_all(&data.root);
        assert!(!is_catalog_entry("faces.db.bak", false) && !is_catalog_entry("faces", false));
    }

    /// A Shoebox stopped mid-write leaves `<name>.<pid>-<n>.tmp` behind: those
    /// go at the next start, once old enough not to be another Shoebox's write
    /// under way. Nothing else does.
    #[test]
    fn stray_temp_files_are_removed() {
        let data = DataDir { root: std::env::temp_dir().join("shoebox-stray-temp") };
        let _ = std::fs::remove_dir_all(&data.root);
        let old = std::time::SystemTime::now() - 2 * STRAY_AFTER;
        let file = |rel: &str, aged: bool| {
            let p = data.root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
            if aged {
                std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
            }
            p
        };
        let stray = [file("previews/01/1.4242-7.tmp", true), file("settings.4242-8.tmp", true)];
        let kept = [
            file("previews/01/2.4242-9.tmp", false), // being written right now
            file("previews/01/1.jpg", true),
            file("backup/catalog.db.tmp", true),
            file("models/clip/model.4242-1.tmp", true),
        ];
        assert_eq!(data.remove_stray_temp_files(), 2);
        assert!(stray.iter().all(|p| !p.exists()));
        assert!(kept.iter().all(|p| p.exists()));
        assert!(is_write_temp(".kopiaignore.12-0.tmp") && !is_write_temp("catalog.db.tmp"));
        assert!(!is_write_temp("x.12-.tmp") && !is_write_temp("x.a1-2.tmp") && !is_write_temp("12-3.tmp"));
        let _ = std::fs::remove_dir_all(&data.root);
    }

    /// The folder a catalog move left is done with once it's gone from a disk
    /// that is there; on an unplugged disk it waits.
    #[test]
    fn a_folder_is_gone_only_from_a_disk_that_is_there() {
        let base = std::env::temp_dir().join("shoebox-gone");
        std::fs::create_dir_all(&base).unwrap();
        assert!(is_gone(&base.join("old catalog")));
        assert!(!is_gone(&base));
        assert!(!is_gone(&base.join("unplugged").join("old catalog")));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Installed, `~/.shoebox` holds the models and `shoebox.json` beside the
    /// first catalog; moving the catalog must not take them along.
    #[test]
    fn catalog_files_leave_out_models_and_pointer() {
        let data = DataDir { root: std::env::temp_dir().join("shoebox-own-files") };
        let _ = std::fs::remove_dir_all(&data.root);
        for f in ["previews/01/1.jpg", "settings.json", "models/clip/model.onnx", "shoebox.json"] {
            data.write(&data.root.join(f), b"x").unwrap();
        }
        let mut files: Vec<PathBuf> =
            data.files().iter().map(|p| p.strip_prefix(&data.root).unwrap().to_path_buf()).collect();
        files.sort();
        assert_eq!(files, [Path::new("previews").join("01").join("1.jpg"), PathBuf::from("settings.json")]);
        let _ = std::fs::remove_dir_all(&data.root);
    }

    /// "Shoebox never writes to a photo file" (README), enforced: file-writing
    /// code may only appear in this file, and photo-file changes only in
    /// `organize.rs` (deleting what the user marked and confirmed; moves later),
    /// which has its own rules.
    #[test]
    fn no_other_file_writes_anywhere() {
        const FORBIDDEN: &[&str] = &[
            "fs::write",
            "File::create",
            "OpenOptions",
            "fs::rename",
            "fs::copy",
            "remove_file",
            "remove_dir",
            "set_permissions",
            "GENERIC_WRITE",
            "set_len",
            "write_all(",
            "VACUUM INTO",
            "create_dir",
            "trash::",
        ];
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut bad = Vec::new();
        for entry in walkdir::WalkDir::new(&src).into_iter().flatten() {
            let p = entry.path();
            if p.extension().is_none_or(|e| e != "rs") || p.ends_with("paths.rs") || p.ends_with("organize.rs") {
                continue;
            }
            let text = std::fs::read_to_string(p).unwrap();
            for (n, line) in text.lines().enumerate() {
                if FORBIDDEN.iter().any(|f| line.contains(f)) {
                    bad.push(format!("{}:{}: {}", p.display(), n + 1, line.trim()));
                }
            }
        }
        assert!(bad.is_empty(), "file writes outside paths.rs:\n{}", bad.join("\n"));
    }
}
