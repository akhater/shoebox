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
}

impl DataDir {
    /// `SHOEBOX_DATA` wins (tests use it), then the folder chosen in Settings,
    /// then `<Shoebox folder>\data` - or, installed, `~/.shoebox` itself.
    pub fn locate() -> Self {
        if let Some(p) = std::env::var_os("SHOEBOX_DATA") {
            return Self { root: PathBuf::from(p) };
        }
        let chosen = std::fs::read_to_string(app_home().join(POINTER_FILE))
            .ok()
            .and_then(|s| serde_json::from_str::<Pointer>(&s).ok())
            .and_then(|p| p.data_dir);
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

    /// (number of files, total bytes) under a sub-folder of the catalog.
    pub fn usage(&self, sub: &str) -> (u64, u64) {
        walkdir::WalkDir::new(self.root.join(sub))
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file())
            .fold((0, 0), |(n, b), e| (n + 1, b + e.metadata().map(|m| m.len()).unwrap_or(0)))
    }

    /// Every file of the catalog except its databases (open while Shoebox runs:
    /// they are snapshotted instead) and half-written temp files.
    pub fn files(&self) -> Vec<PathBuf> {
        walkdir::WalkDir::new(&self.root)
            .into_iter()
            // Shoebox's own, not the catalog's: installed, `~/.shoebox` is both
            // the Shoebox folder and the first catalog.
            .filter_entry(|e| e.depth() != 1 || !matches!(e.file_name().to_str(), Some("models" | POINTER_FILE)))
            .flatten()
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                ![".db", ".db-wal", ".db-shm", ".db-journal", ".tmp"].iter().any(|end| name.ends_with(end))
            })
            .collect()
    }

    /// Moves this catalog's files into `target` (another, empty data folder).
    /// The big folders (previews, thumbnails, faces) move whole, the way the
    /// OS moves a folder: on the same disk an instant rename, nothing copied.
    /// What is left (settings, logs - and everything, on another disk) is
    /// copied. If the copying fails, the moved folders are put back. Returns
    /// what was moved, for [`DataDir::unmove`] if a later step fails.
    pub fn move_files_to(
        &self,
        target: &DataDir,
        progress: &dyn Fn(usize, usize),
    ) -> io::Result<Vec<(PathBuf, PathBuf)>> {
        std::fs::create_dir_all(&target.root)?;
        let mut moved = Vec::new();
        for sub in ["previews", "thumbs", "faces"] {
            let (from, to) = (self.root.join(sub), target.root.join(sub));
            if !from.is_dir() || to.exists() || !inside(&to, &target.root) {
                continue;
            }
            // A preview being read this very moment can hold the folder briefly.
            for attempt in 0..5 {
                match std::fs::rename(&from, &to) {
                    Ok(()) => {
                        moved.push((from.clone(), to.clone()));
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => break, // another disk: copied below
                    Err(_) if attempt < 4 => std::thread::sleep(std::time::Duration::from_millis(200)),
                    Err(_) => break,
                }
            }
        }
        let files = self.files();
        let copy = || -> io::Result<()> {
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
        };
        if let Err(e) = copy() {
            Self::unmove(&moved);
            return Err(e);
        }
        Ok(moved)
    }

    /// Puts back folders [`DataDir::move_files_to`] moved (a later step failed).
    pub fn unmove(moved: &[(PathBuf, PathBuf)]) {
        for (from, to) in moved.iter().rev() {
            let _ = std::fs::rename(to, from);
        }
    }
}

/// A consistent copy of the open catalog database into `target`, safe while
/// Shoebox is running (SQLite's `VACUUM INTO`).
pub fn snapshot_db(conn: &rusqlite::Connection, target: &DataDir) -> anyhow::Result<()> {
    let dst = target.db_path();
    if dst.exists() {
        anyhow::bail!("{} already has a catalog", target.root.display());
    }
    std::fs::create_dir_all(&target.root)?;
    conn.execute("VACUUM INTO ?1", [dst.to_string_lossy()])?;
    Ok(())
}

/// A consistent copy of one of the catalog's other databases (faces, search)
/// into `target` under the same name, safe while Shoebox has it open.
pub fn snapshot_sqlite(src: &Path, target: &DataDir) -> anyhow::Result<()> {
    let name = src.file_name().ok_or_else(|| anyhow::anyhow!("no file name in {}", src.display()))?;
    let dst = target.root.join(name);
    if !inside(&dst, &target.root) || dst.exists() {
        anyhow::bail!("refused {}", dst.display());
    }
    let conn = rusqlite::Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute("VACUUM INTO ?1", [dst.to_string_lossy()])?;
    Ok(())
}

/// Remembers `root` as the catalog folder for the next start.
pub fn remember_data_dir(root: &Path) -> io::Result<()> {
    let p = Pointer { data_dir: Some(root.to_path_buf()) };
    atomic_write(&app_home().join(POINTER_FILE), serde_json::to_string_pretty(&p)?.as_bytes())
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

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
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

    /// Promise 1 in CLAUDE.md, enforced: file-writing code may only appear in
    /// this file, and photo-file changes only in `organize.rs` (deleting what
    /// the user marked and confirmed; moves later), which has its own rules.
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
