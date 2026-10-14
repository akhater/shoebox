//! A folder rename, move or delete stopped dead at any step (Shoebox killed by
//! a rebuild, the PC switched off) is settled by the next start: finished if
//! the catalog had already taken it, undone otherwise. Never half-done, never
//! an original removed without a checked copy, and a rescan afterwards finds
//! nothing to change.
//!
//! A kill is `organize::testing::kill_at`: the change stops dead at that step
//! and nothing after it runs. The catalog is then opened again, as a start
//! does, and `organize::resume` settles what was cut short. Two folders stand
//! for two disks (`Catalog::plug_in_test_disk`), so a disk can be "unplugged"
//! by not plugging it in again.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use rusqlite::params;
use shoebox_lib::catalog::Catalog;
use shoebox_lib::organize::{self, FolderMove, testing::kill_at};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const TRIP: &str = r"Photos\Trip";
/// Six photos and one Argentum sidecar.
const TRIP_FILES: usize = 7;

fn jpeg(path: &Path, shade: u8) {
    image::RgbImage::from_fn(160, 120, |x, y| image::Rgb([shade.wrapping_mul(40), (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

/// Everything a change may touch.
#[derive(PartialEq, Debug)]
struct State {
    /// (disk, folder, name, state, photo fingerprint) of every file the catalog knows.
    files: Vec<(i64, String, String, i64, Option<String>)>,
    folders: Vec<(i64, String)>,
    roots: Vec<(i64, String)>,
    photos: i64,
    marked: i64,
    previews: usize,
    /// Every folder ("") and file (its BLAKE3) on both disks.
    disks: BTreeMap<String, String>,
    /// Changes not settled yet.
    ops: i64,
}

/// Disk A: Photos (a source) with Trip (4 photos, a sidecar, and Day 2 with 2
/// more) and an empty 2026. Disk B: Archive (a source), empty.
struct World {
    base: PathBuf,
    data: DataDir,
}

impl World {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("shoebox-kill-{name}"));
        let _ = std::fs::remove_dir_all(&base);
        for d in [r"disk-a\Photos\Trip\Day 2", r"disk-a\Photos\2026", r"disk-b\Archive"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        let trip = base.join("disk-a").join(TRIP);
        for i in 0..4u8 {
            jpeg(&trip.join(format!("IMG_{i:04}.JPG")), i);
        }
        for i in 4..6u8 {
            jpeg(&trip.join("Day 2").join(format!("IMG_{i:04}.JPG")), i);
        }
        std::fs::write(trip.join("IMG_0000.JPG.agdata"), b"Argentum's edits").unwrap();
        let w = Self { data: DataDir { root: base.join("catalog") }, base };
        w.data.ensure().unwrap();
        let (cat, a, b) = w.open(&["a", "b"]);
        for (disk, rel) in [(a, "Photos"), (b, "Archive")] {
            cat.db()
                .execute("INSERT INTO roots (volume_id, rel_path, added_at) VALUES (?1, ?2, 0)", params![disk, rel])
                .unwrap();
        }
        for id in cat.root_ids().unwrap() {
            let r = scan_root(&cat, id, &AtomicBool::new(false), false, &|_| {}).unwrap();
            assert_eq!(r.errors, 0, "{r:?}");
        }
        w
    }

    fn disk(&self, d: &str) -> PathBuf {
        self.base.join(format!("disk-{d}"))
    }

    /// Opens the catalog as a start does, with these disks plugged in.
    /// Returns it with the ids of disks A and B.
    fn open(&self, plugged: &[&str]) -> (Catalog, i64, i64) {
        let cat = Catalog::open(self.data.clone()).unwrap();
        let a = cat.plug_in_test_disk("Disk A", &self.disk("a")).unwrap();
        let b = cat.plug_in_test_disk("Disk B", &self.disk("b")).unwrap();
        cat.refresh_volumes().unwrap(); // test disks are never found: all unplugged
        for d in plugged {
            cat.plug_in_test_disk(&format!("Disk {}", d.to_uppercase()), &self.disk(d)).unwrap();
        }
        (cat, a, b)
    }

    fn state(&self, cat: &Catalog) -> State {
        let db = cat.db();
        let all = |sql: &str| -> Vec<(i64, String)> {
            db.prepare(sql)
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let count = |sql: &str| -> i64 { db.query_row(sql, [], |r| r.get(0)).unwrap() };
        let files = db
            .prepare(
                "SELECT fo.volume_id, fo.rel_path, f.name, f.state, p.fingerprint
                   FROM files f JOIN folders fo ON fo.id = f.folder_id LEFT JOIN photos p ON p.id = f.photo_id
                  ORDER BY 1, 2, 3",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut disks = BTreeMap::new();
        for e in walkdir::WalkDir::new(&self.base).min_depth(1).into_iter().filter_entry(|e| e.file_name() != "catalog")
        {
            let e = e.unwrap();
            let rel = e.path().strip_prefix(&self.base).unwrap().to_string_lossy().into_owned();
            let hash = if e.file_type().is_dir() {
                String::new()
            } else {
                blake3::hash(&std::fs::read(e.path()).unwrap()).to_hex().to_string()
            };
            disks.insert(rel, hash);
        }
        State {
            files,
            folders: all("SELECT volume_id, rel_path FROM folders ORDER BY 1, 2"),
            roots: all("SELECT volume_id, rel_path FROM roots ORDER BY 1, 2"),
            photos: count("SELECT COUNT(*) FROM photos"),
            marked: count("SELECT COUNT(*) FROM photos WHERE marked = 1"),
            previews: self.data.usage("previews").0 as usize,
            disks,
            ops: count("SELECT COUNT(*) FROM ops"),
        }
    }

    /// A rescan of every source plugged in finds nothing to change.
    fn rescan_changes_nothing(&self, cat: &Catalog) {
        let before = self.state(cat);
        for id in cat.root_ids().unwrap() {
            if cat.mount_of(cat.root(id).unwrap().volume_id).is_none() {
                continue;
            }
            let r = scan_root(cat, id, &AtomicBool::new(false), false, &|_| {}).unwrap();
            let changes = (r.new_files, r.changed, r.missing, r.photos_added, r.linked, r.duplicates, r.errors);
            assert_eq!(changes, (0, 0, 0, 0, 0, 0, 0), "{r:?}");
        }
        assert_eq!(self.state(cat), before);
    }

    /// No change left, nor its lock.
    fn settled(&self, cat: &Catalog) {
        assert_eq!(self.state(cat).ops, 0);
        assert!(self.data.op_lock_ids().is_empty(), "{:?}", self.data.op_lock_ids());
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Runs `change` and stops it dead the `nth` time it reaches `step`.
fn killed(step: &'static str, nth: usize, change: impl FnOnce() -> anyhow::Result<()>) {
    kill_at(step, nth);
    let outcome = catch_unwind(AssertUnwindSafe(change));
    assert!(outcome.is_err(), "the change should have stopped at {step} #{nth}");
}

fn trip_to_b(a: i64, b: i64) -> FolderMove {
    FolderMove { from: a, rel: TRIP.into(), to: b, to_parent: "Archive".into() }
}

fn move_folder(cat: &Catalog, m: &FolderMove) -> anyhow::Result<organize::MoveResult> {
    organize::move_folder(cat, m, &AtomicBool::new(false), &|_| {})
}

/// How the world looks once `change` is done, uninterrupted.
fn done_state(name: &str, change: impl FnOnce(&Catalog, i64, i64)) -> State {
    let w = World::new(name);
    let (cat, a, b) = w.open(&["a", "b"]);
    change(&cat, a, b);
    w.settled(&cat);
    w.state(&cat)
}

#[test]
fn a_move_to_another_disk_stopped_at_any_step_is_finished_or_undone() {
    let after = done_state("copy-done", |cat, a, b| {
        let r = move_folder(cat, &trip_to_b(a, b)).unwrap();
        assert_eq!((r.copied, r.files, r.left_behind.len()), (true, TRIP_FILES, 0));
    });
    assert!(!after.disks.contains_key(r"disk-a\Photos\Trip"));
    assert!(after.disks.contains_key(r"disk-b\Archive\Trip\Day 2\IMG_0005.JPG"));

    // (step, nth time, whether the catalog had taken the move)
    let steps = [
        ("copy:journaled", 1, false),
        ("copy:copied", 1, false),
        ("copy:copied", 4, false),
        ("copy:checked", 1, false),
        ("copy:cataloged", 1, true),
        ("copy:removed", 1, true),
        ("copy:removed", TRIP_FILES, true),
        ("copy:tidied", 1, true),
    ];
    for (i, (step, nth, taken)) in steps.into_iter().enumerate() {
        let w = World::new(&format!("copy-{i}"));
        let (cat, a, b) = w.open(&["a", "b"]);
        let before = w.state(&cat);
        killed(step, nth, || move_folder(&cat, &trip_to_b(a, b)).map(drop));
        assert_eq!(w.state(&cat).ops, 1, "{step} #{nth}: written down");
        drop(cat);

        let (cat, a, b) = w.open(&["a", "b"]);
        let r = organize::resume(&cat).unwrap();
        let (finished, undone) = (r.finished.len(), r.undone.len());
        assert_eq!((finished, undone, r.waiting.len()), (taken as usize, !taken as usize, 0), "{step} #{nth}");
        assert_eq!(&w.state(&cat), if taken { &after } else { &before }, "{step} #{nth}");
        w.settled(&cat);
        w.rescan_changes_nothing(&cat);
        if !taken {
            // Nothing in the way of doing it again.
            move_folder(&cat, &trip_to_b(a, b)).unwrap();
            assert_eq!(w.state(&cat), after, "{step} #{nth}: moved again");
        }
    }
}

/// Stopped in the middle of copying a file: its copy is cut short. A scan of
/// the other disk before the next start doesn't take any of the copy in; the
/// start takes the copy back.
#[test]
fn a_half_written_copy_is_never_cataloged_and_is_taken_back() {
    let w = World::new("copy-half");
    let (cat, a, b) = w.open(&["a", "b"]);
    let before = w.state(&cat);
    killed("copy:copied", 2, || move_folder(&cat, &trip_to_b(a, b)).map(drop));
    let (src, dst) = (w.disk("a").join(TRIP), w.disk("b").join(r"Archive\Trip"));
    let next = walkdir::WalkDir::new(&src)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().strip_prefix(&src).unwrap().to_path_buf())
        .find(|f| !dst.join(f).exists())
        .unwrap();
    let bytes = std::fs::read(src.join(&next)).unwrap();
    std::fs::write(dst.join(&next), &bytes[..bytes.len() / 2]).unwrap();

    let archive = cat.root_ids().unwrap().into_iter().find(|id| cat.root(*id).unwrap().volume_id == b).unwrap();
    let r = scan_root(&cat, archive, &AtomicBool::new(false), false, &|_| {}).unwrap();
    assert_eq!((r.found, r.photos_added), (0, 0), "{r:?}");
    assert_eq!(w.state(&cat).files, before.files);
    drop(cat);

    let (cat, a, b) = w.open(&["a", "b"]);
    assert_eq!(organize::resume(&cat).unwrap().undone.len(), 1);
    assert_eq!(w.state(&cat), before);
    assert!(!dst.exists());
    w.settled(&cat);
    move_folder(&cat, &trip_to_b(a, b)).unwrap();
}

/// The same, killed for real: this test runs itself again as another process,
/// which ends dead at the step - no destructor runs, its lock's file stays on
/// the disk, the catalog is never closed.
#[test]
fn a_move_really_killed_is_finished_or_undone() {
    if let Ok(base) = std::env::var("SHOEBOX_KILL_CHILD") {
        return move_and_die(Path::new(&base));
    }
    let after = done_state("real-done", |cat, a, b| drop(move_folder(cat, &trip_to_b(a, b)).unwrap()));
    for (i, (step, nth, taken)) in [("copy:copied", 3, false), ("copy:removed", 2, true)].into_iter().enumerate() {
        let w = World::new(&format!("real-{i}"));
        let before = w.state(&w.open(&["a", "b"]).0);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "a_move_really_killed_is_finished_or_undone"])
            .env("SHOEBOX_KILL_CHILD", &w.base)
            .env("SHOEBOX_KILL_AT", format!("{step}#{nth}"))
            .output()
            .unwrap();
        assert_eq!(child.status.code(), Some(9), "{}", String::from_utf8_lossy(&child.stdout));
        assert_eq!(w.data.op_lock_ids().len(), 1, "{step}: a kill leaves its lock's file");

        let (cat, ..) = w.open(&["a", "b"]);
        let r = organize::resume(&cat).unwrap();
        assert_eq!((r.finished.len(), r.undone.len()), (taken as usize, !taken as usize), "{step}");
        assert_eq!(&w.state(&cat), if taken { &after } else { &before }, "{step}");
        w.settled(&cat);
        w.rescan_changes_nothing(&cat);
    }
}

/// The other process: moves Trip to disk B and ends at SHOEBOX_KILL_AT (`step#nth`).
fn move_and_die(base: &Path) {
    let at = std::env::var("SHOEBOX_KILL_AT").unwrap();
    let (step, nth) = at.split_once('#').unwrap();
    // Its folders belong to the test that started this process.
    let w =
        std::mem::ManuallyDrop::new(World { base: base.to_path_buf(), data: DataDir { root: base.join("catalog") } });
    let (cat, a, b) = w.open(&["a", "b"]);
    organize::testing::exit_at(String::leak(step.to_string()), nth.parse().unwrap());
    let _ = move_folder(&cat, &trip_to_b(a, b));
    panic!("{at} was never reached");
}

/// Stopped once the catalog points at the copy; the next start is without
/// the copy's disk: the originals wait, untouched, and a scan of their disk
/// leaves them alone (no duplicates). With the disk back, the move finishes,
/// checking every byte again first.
#[test]
fn a_move_cut_short_waits_for_its_disk() {
    let after = done_state("wait-done", |cat, a, b| drop(move_folder(cat, &trip_to_b(a, b)).unwrap()));
    let w = World::new("wait");
    let (cat, a, b) = w.open(&["a", "b"]);
    killed("copy:cataloged", 1, || move_folder(&cat, &trip_to_b(a, b)).map(drop));
    let cut = w.state(&cat);
    drop(cat);

    let (cat, ..) = w.open(&["a"]);
    let r = organize::resume(&cat).unwrap();
    assert_eq!((r.finished.len(), r.undone.len(), r.waiting.len()), (0, 0, 1));
    assert_eq!(w.state(&cat), cut);
    w.rescan_changes_nothing(&cat);
    assert_eq!(w.state(&cat), cut);
    drop(cat);

    // A copy that isn't the same as its original keeps that original.
    let changed = w.disk("b").join(r"Archive\Trip\IMG_0003.JPG");
    let mut bytes = std::fs::read(&changed).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(&changed, &bytes).unwrap();
    let (cat, ..) = w.open(&["a", "b"]);
    assert_eq!(organize::resume(&cat).unwrap().finished.len(), 1);
    let now = w.state(&cat);
    assert!(now.disks.contains_key(r"disk-a\Photos\Trip\IMG_0003.JPG"));
    assert_eq!(now.disks.keys().filter(|k| k.starts_with(r"disk-a\Photos\Trip")).count(), 2); // it and its folder
    w.settled(&cat);
    drop(cat);

    // Same again, every copy intact: the move ends as if never stopped.
    let w = World::new("wait-intact");
    let (cat, a, b) = w.open(&["a", "b"]);
    killed("copy:removed", 3, || move_folder(&cat, &trip_to_b(a, b)).map(drop));
    drop(cat);
    let (cat, ..) = w.open(&["b"]);
    assert_eq!(organize::resume(&cat).unwrap().waiting.len(), 1);
    drop(cat);
    let (cat, ..) = w.open(&["a", "b"]);
    assert_eq!(organize::resume(&cat).unwrap().finished.len(), 1);
    assert_eq!(w.state(&cat), after);
    w.settled(&cat);
}

/// While a move cut short waits for a disk, nothing else may touch its
/// folders, and scans leave them alone; once settled, all is as before.
#[test]
fn a_change_waiting_for_a_disk_keeps_others_off_its_folders() {
    let w = World::new("busy");
    let (cat, a, b) = w.open(&["a", "b"]);
    let before = w.state(&cat);
    killed("copy:copied", 3, || move_folder(&cat, &trip_to_b(a, b)).map(drop));
    drop(cat);

    let (cat, a, _) = w.open(&["a"]);
    assert_eq!(organize::resume(&cat).unwrap().waiting.len(), 1);
    let refused = organize::rename_folder(&cat, a, TRIP, "Trip 2").unwrap_err().to_string();
    assert!(refused.contains("hasn't finished"), "{refused}");
    let refused = organize::rename_folder(&cat, a, "Photos", "Pictures").unwrap_err().to_string();
    assert!(refused.contains("hasn't finished"), "{refused}");
    organize::rename_folder(&cat, a, r"Photos\2026", "2027").unwrap();
    organize::rename_folder(&cat, a, r"Photos\2027", "2026").unwrap();
    w.rescan_changes_nothing(&cat);
    drop(cat);

    let (cat, ..) = w.open(&["a", "b"]);
    assert_eq!(organize::resume(&cat).unwrap().undone.len(), 1);
    assert_eq!(w.state(&cat), before);
    w.settled(&cat);
}

/// A change still running - here or in another Shoebox - holds its lock:
/// a start in the meantime leaves it alone.
#[test]
fn a_running_change_is_left_alone() {
    let w = World::new("running");
    let (cat, a, b) = w.open(&["a", "b"]);
    let before = w.state(&cat);
    killed("copy:copied", 2, || move_folder(&cat, &trip_to_b(a, b)).map(drop));
    let cut = w.state(&cat);
    drop(cat);

    let (cat, ..) = w.open(&["a", "b"]);
    let id: String = cat.db().query_row("SELECT id FROM ops", [], |r| r.get(0)).unwrap();
    let running = w.data.try_op_lock(&id).unwrap().expect("free after the kill");
    assert!(organize::resume(&cat).unwrap().is_empty());
    assert_eq!(w.state(&cat), cut);
    drop(running);
    // A kill leaves its lock's file behind (the system only lets go of the
    // lock): one stopped before it was even written down is tidied away too.
    std::fs::write(w.data.root.join("ops").join("1-2-3.lock"), b"").unwrap();
    assert_eq!(organize::resume(&cat).unwrap().undone.len(), 1);
    assert_eq!(w.state(&cat), before);
    w.settled(&cat);
}

#[test]
fn a_rename_stopped_at_any_step_is_finished_or_undone() {
    let cases: [(&str, &str, &str); 3] = [
        ("rename", "Journey", "the folder renamed"),
        ("case", "TRIP", "only its case changed: one folder on Windows"),
        ("move", r"..\2026", "moved into another folder of the same disk"),
    ];
    for (name, new, what) in cases {
        let change = |cat: &Catalog, a: i64| -> anyhow::Result<()> {
            match new.strip_prefix(r"..\") {
                Some(into) => {
                    let m = FolderMove { from: a, rel: TRIP.into(), to: a, to_parent: format!(r"Photos\{into}") };
                    move_folder(cat, &m).map(drop)
                }
                None => organize::rename_folder(cat, a, TRIP, new).map(drop),
            }
        };
        let after = done_state(&format!("{name}-done"), |cat, a, _| change(cat, a).unwrap());
        for (step, taken) in [("rename:journaled", false), ("rename:renamed", true)] {
            let w = World::new(&format!("{name}-{step}").replace(':', "-"));
            let (cat, a, _) = w.open(&["a", "b"]);
            let before = w.state(&cat);
            killed(step, 1, || change(&cat, a));
            drop(cat);

            // Before the start settles it, the catalog may point at a folder
            // that isn't there: the start sets that right.
            let (cat, ..) = w.open(&["a", "b"]);
            let r = organize::resume(&cat).unwrap();
            assert_eq!((r.finished.len(), r.undone.len()), (taken as usize, !taken as usize), "{what}, {step}");
            assert_eq!(&w.state(&cat), if taken { &after } else { &before }, "{what}, {step}");
            w.settled(&cat);
            w.rescan_changes_nothing(&cat);
        }
    }
}

#[test]
fn a_delete_stopped_at_any_step_is_finished_or_undone() {
    let marked = |cat: &Catalog| -> Vec<i64> {
        let ids: Vec<i64> = cat
            .db()
            .prepare("SELECT photo_id FROM files WHERE name IN ('IMG_0001.JPG', 'IMG_0002.JPG') ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        organize::mark(cat, &ids, true).unwrap();
        ids
    };
    let delete = |cat: &Catalog| -> anyhow::Result<()> {
        let plan = organize::plan(cat)?;
        // A test disk has no Recycle Bin: the files go for good, never into the real bin.
        assert!(plan.disks.iter().all(|d| !d.recycle));
        let report = organize::delete_confirmed(cat, &plan.token, &[plan.disks[0].volume_id])?;
        assert_eq!((report.deleted, report.photos_removed), (2, 2));
        Ok(())
    };
    let after = done_state("delete-done", |cat, _, _| {
        marked(cat);
        delete(cat).unwrap();
    });
    assert_eq!((after.photos, after.marked), (4, 0));

    for (step, taken) in [("delete:journaled", false), ("delete:sent", true), ("delete:forgotten", true)] {
        let w = World::new(&step.replace(':', "-"));
        let (cat, ..) = w.open(&["a", "b"]);
        let photos = marked(&cat);
        let before = w.state(&cat);
        killed(step, 1, || delete(&cat));
        drop(cat);

        let (cat, ..) = w.open(&["a", "b"]);
        let r = organize::resume(&cat).unwrap();
        assert_eq!((r.finished.len(), r.undone.len()), (taken as usize, !taken as usize), "{step}");
        assert_eq!(&w.state(&cat), if taken { &after } else { &before }, "{step}");
        for pid in photos {
            assert_eq!(w.data.preview_path(pid).exists(), !taken, "{step}: preview of photo {pid}");
        }
        let plan = organize::plan(&cat).unwrap();
        assert_eq!((plan.marked, plan.waiting), if taken { (0, 0) } else { (2, 0) }, "{step}");
        w.settled(&cat);
        w.rescan_changes_nothing(&cat);
    }
}
