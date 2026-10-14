//! Moving the catalog is undone whole when it doesn't finish: stopped
//! halfway, the next start finds the old catalog as it was - databases,
//! previews and thumbnails - and nothing half-made where it was going. The
//! target counts as a catalog only once all of it is there.
//!
//! The last step, remembering the new folder in `shoebox.json`, is left out:
//! that file is the real one beside the app.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::paths::{DataDir, snapshot_db};
use shoebox_lib::scan::scan_root;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

/// A catalog with one photo, and a folder to move it to.
fn catalog(name: &str) -> (PathBuf, Catalog, i64, DataDir) {
    let base = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    image::RgbImage::from_fn(640, 480, |x, y| image::Rgb([70, (x % 256) as u8, (y % 256) as u8]))
        .save(photos.join("IMG_0.JPG"))
        .unwrap();
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let photo = cat.db().query_row("SELECT id FROM photos", [], |r| r.get(0)).unwrap();
    let to = DataDir { root: base.join("moved") };
    (base, cat, photo, to)
}

/// What a start does before opening the catalog (see `shoebox_lib::run`).
fn start(data: &DataDir) -> Option<PathBuf> {
    data.ensure().unwrap();
    data.undo_move().unwrap()
}

fn photos(data: &DataDir) -> i64 {
    Catalog::open(data.clone()).unwrap().db().query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0)).unwrap()
}

#[test]
fn a_move_stopped_after_the_folders_moved_is_undone() {
    let (base, cat, photo, to) = catalog("shoebox-move-stopped");
    let old = cat.data.clone();
    let preview = std::fs::read(old.preview_path(photo)).unwrap();

    let lock = old.begin_move(&to).unwrap();
    snapshot_db(&cat.db(), &to).unwrap();
    old.move_files_to(&to, &|_, _| {}).unwrap();
    assert!(!old.preview_path(photo).exists(), "moved whole, not copied");
    assert!(!to.has_catalog(), "not a catalog until all of it is there");
    // Stopped here: Shoebox ends, and the system lets go of the move.
    drop(cat);
    drop(lock);

    assert_eq!(start(&old), Some(to.root.clone()));
    assert_eq!(std::fs::read(old.preview_path(photo)).unwrap(), preview);
    assert!(old.thumb_path(photo).is_file());
    assert!(!to.root.exists(), "nothing half-made left where it was going");
    assert_eq!(photos(&old), 1);
    // Undone once: the next start finds nothing to do.
    assert_eq!(start(&old), None);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_move_stopped_before_shoebox_json_is_undone() {
    let (base, cat, photo, to) = catalog("shoebox-move-not-remembered");
    let old = cat.data.clone();

    let lock = old.begin_move(&to).unwrap();
    snapshot_db(&cat.db(), &to).unwrap();
    old.move_files_to(&to, &|_, _| {}).unwrap();
    to.place_databases().unwrap();
    assert!(to.has_catalog() && to.preview_path(photo).is_file());
    // Stopped before shoebox.json was told: the next start is on the old catalog.
    drop(cat);
    drop(lock);

    assert_eq!(start(&old), Some(to.root.clone()));
    assert!(old.preview_path(photo).is_file() && old.thumb_path(photo).is_file());
    assert!(!to.root.exists());
    assert_eq!(photos(&old), 1);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_move_under_way_is_left_alone() {
    let (base, cat, photo, to) = catalog("shoebox-move-under-way");
    let old = cat.data.clone();

    let lock = old.begin_move(&to).unwrap();
    old.move_files_to(&to, &|_, _| {}).unwrap();
    // Another Shoebox starting meanwhile (the CLI) leaves it be, and can't start one.
    assert_eq!(old.undo_move().unwrap(), None);
    assert!(to.preview_path(photo).is_file());
    assert!(old.begin_move(&DataDir { root: base.join("elsewhere") }).is_err());

    // The move fails: everything goes back at once.
    lock.undo().unwrap();
    assert!(old.preview_path(photo).is_file());
    assert!(!to.root.exists());
    drop(cat);
    assert_eq!(start(&old), None);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_face_saved_as_the_move_began_is_kept_when_undone() {
    let (base, cat, photo, to) = catalog("shoebox-move-merge-back");
    let old = cat.data.clone();
    old.write(&old.face_path(1), b"face 1").unwrap();

    let lock = old.begin_move(&to).unwrap();
    old.move_files_to(&to, &|_, _| {}).unwrap();
    // Saved in the old place just after its folder moved.
    old.write(&old.face_path(2), b"face 2").unwrap();
    drop(cat);
    drop(lock);

    assert_eq!(start(&old), Some(to.root.clone()));
    assert_eq!(std::fs::read(old.face_path(1)).unwrap(), b"face 1");
    assert_eq!(std::fs::read(old.face_path(2)).unwrap(), b"face 2");
    assert!(old.preview_path(photo).is_file());
    assert!(!to.root.exists());

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_finished_move_leaves_a_whole_catalog() {
    let (base, cat, photo, to) = catalog("shoebox-move-whole");
    let old = cat.data.clone();

    let lock = old.begin_move(&to).unwrap();
    snapshot_db(&cat.db(), &to).unwrap();
    old.move_files_to(&to, &|_, _| {}).unwrap();
    to.place_databases().unwrap();
    lock.finish();
    drop(cat);

    // Remembered as done, nothing is undone at the next start on either side.
    assert_eq!(start(&old), None);
    assert_eq!(start(&to), None);
    assert!(to.preview_path(photo).is_file() && to.thumb_path(photo).is_file());
    assert_eq!(photos(&to), 1);

    let _ = std::fs::remove_dir_all(&base);
}
