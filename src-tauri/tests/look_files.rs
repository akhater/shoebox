//! Look files (`IMG_0001.CR3.<name>.lut.png`): generated beside a photo and
//! going wherever it goes, but never photos themselves, though they end in
//! `.png`. Moved photos taking theirs along is in photo_moves.rs.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1). The
//! world is kill_safe_organize.rs's: disk A holds Photos with Trip
//! (IMG_0000-0003, and Day 2 with IMG_0004-0005), disk B holds Archive.

use crate::kill_safe_organize::{TRIP, World, native};
use rusqlite::params;
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::organize;
use shoebox_lib::scan::scan_root;
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// A real picture, which only its name keeps out of the catalog.
fn look(path: &Path) {
    image::RgbImage::from_fn(64, 64, |x, y| image::Rgb([x as u8, y as u8, 128])).save(path).unwrap();
}

fn photo(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

fn cataloged(cat: &Catalog, name: &str) -> bool {
    cat.db().query_row("SELECT EXISTS (SELECT 1 FROM files WHERE name = ?1)", [name], |r| r.get(0)).unwrap()
}

#[test]
fn a_look_file_beside_a_photo_adds_no_photo() {
    let w = World::new("looks-scan");
    let (cat, ..) = w.open(&["a", "b"]);
    let trip = native(&w.disk("a"), TRIP);
    look(&trip.join("IMG_0001.JPG.warm.lut.png"));
    look(&trip.join("IMG_0001.JPG.COOL.LUT.PNG"));
    look(&trip.join("Day 2").join("no photo of its name.lut.png"));

    // The scan that meets them changes nothing, and neither does the next.
    w.rescan_changes_nothing(&cat);
    w.rescan_changes_nothing(&cat);
    assert!(!cataloged(&cat, "IMG_0001.JPG.warm.lut.png"));
}

#[test]
fn a_photos_look_files_are_deleted_with_it() {
    let w = World::new("looks-delete");
    let (cat, ..) = w.open(&["a", "b"]);
    let trip = native(&w.disk("a"), TRIP);
    for name in ["IMG_0001.JPG.warm.lut.png", "IMG_0001.JPG.cool.lut.png", "IMG_0002.JPG.warm.lut.png"] {
        look(&trip.join(name));
    }
    organize::mark(&cat, &[photo(&cat, "IMG_0001.JPG")], true).unwrap();

    let plan = organize::plan(&cat).unwrap();
    let roles: Vec<&str> = plan.files.iter().map(|f| f.role.as_str()).collect();
    assert_eq!(roles, ["photo", "look", "look"], "{:?}", plan.files);
    // A test disk has no Recycle Bin: the files go for good, never into the real bin.
    let r = organize::delete_confirmed(&cat, &plan.token, &[plan.disks[0].volume_id]).unwrap();
    assert_eq!((r.deleted, r.photos_removed), (3, 1));

    assert!(!trip.join("IMG_0001.JPG.warm.lut.png").exists());
    assert!(!trip.join("IMG_0001.JPG.cool.lut.png").exists());
    assert!(trip.join("IMG_0002.JPG.warm.lut.png").exists());
    w.rescan_changes_nothing(&cat);
}

/// What a Shoebox that took them for photos left behind: they leave as soon as
/// the catalog opens, by their name, so on an unplugged disk too, and never
/// turn into missing photos. No file is touched.
#[test]
fn a_look_file_cataloged_before_leaves_the_catalog() {
    let w = World::new("looks-leftover");
    let (cat, ..) = w.open(&["a", "b"]);
    let trip = native(&w.disk("a"), TRIP);

    // Listed on its own: its photo goes with it, preview and all.
    std::fs::rename(trip.join("IMG_0003.JPG"), trip.join("IMG_0000.JPG.warm.lut.png")).unwrap();
    let alone = photo(&cat, "IMG_0003.JPG");
    cat.db().execute("UPDATE files SET name = 'IMG_0000.JPG.warm.lut.png' WHERE name = 'IMG_0003.JPG'", []).unwrap();
    // The same picture as a real photo: that photo stays, with its own file.
    std::fs::copy(trip.join("IMG_0002.JPG"), trip.join("IMG_0002.JPG.LUT.PNG")).unwrap();
    let kept = photo(&cat, "IMG_0002.JPG");
    cat.db()
        .execute(
            "INSERT INTO files (folder_id, name, size, mtime, state, seen_scan, photo_id)
             SELECT folder_id, 'IMG_0002.JPG.LUT.PNG', size, mtime, state, seen_scan, photo_id
               FROM files WHERE photo_id = ?1",
            params![kept],
        )
        .unwrap();
    let before = w.state(&cat);
    drop(cat);

    // Disk A, which holds them, unplugged.
    let (cat, ..) = w.open(&["b"]);
    let now = w.state(&cat);
    assert!(!now.files.iter().any(|f| f.2.to_lowercase().ends_with(".lut.png")), "{:?}", now.files);
    assert_eq!(now.files.len(), before.files.len() - 2);
    assert_eq!(now.photos, before.photos - 1);
    assert_eq!(photo(&cat, "IMG_0002.JPG"), kept);
    assert!(cat.data.preview_path(kept).exists() && !cat.data.preview_path(alone).exists());
    drop(cat);

    // Back on the disk, as they were; a scan finds nothing to change, nothing missing.
    let (cat, ..) = w.open(&["a", "b"]);
    assert!(trip.join("IMG_0000.JPG.warm.lut.png").is_file() && trip.join("IMG_0002.JPG.LUT.PNG").is_file());
    for id in cat.root_ids().unwrap() {
        let r = scan_root(&cat, id, &AtomicBool::new(false), false, &|_| {}).unwrap();
        assert_eq!((r.missing, r.new_files, r.photos_added), (0, 0, 0), "{r:?}");
    }
    let missing = cat.list_photos(&Scope::Missing, Sort::default(), &Filters::default()).unwrap().ids;
    assert!(missing.is_empty(), "{missing:?}");
}
