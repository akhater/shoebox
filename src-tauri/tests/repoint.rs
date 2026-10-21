//! A source whose photos live somewhere else now: its disk was replaced (a new computer, a disk copied onto a
//! new one). Shoebox finds it at the same path on a disk plugged in and offers it; repointing changes the
//! catalog only, keeps the source and its photos, and leaves "missing" to the scan that follows.
//!
//! Two folders stand for the old and the new disk (`Catalog::plug_in_test_disk`).
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).

use rusqlite::params;
use shoebox_lib::catalog::Catalog;
use shoebox_lib::paths::DataDir;
use shoebox_lib::repoint;
use shoebox_lib::scan::{Report, scan_root};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Where the source is on each disk: a path no real disk has, as disks the catalog doesn't know are looked at too.
const PICTURES: &str = "shoebox-repoint-test\\Pictures";

fn jpeg(path: &Path, shade: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(160, 120, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

/// Below a disk's folder, the native path of a catalog path.
fn on(disk: &Path, rel: &str) -> PathBuf {
    rel.split('\\').fold(disk.to_path_buf(), |p, name| p.join(name))
}

struct World {
    base: PathBuf,
    cat: Catalog,
    old: i64,
    root: i64,
}

impl World {
    /// A catalog with a source on the old disk (three photos, two folders), scanned.
    fn new(name: &str) -> World {
        let base = std::env::temp_dir().join(format!("shoebox-repoint-{name}"));
        let _ = std::fs::remove_dir_all(&base);
        for d in ["old", "new"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        let data = DataDir { root: base.join("catalog") };
        data.ensure().unwrap();
        let cat = Catalog::open(data).unwrap();
        let old = cat.plug_in_test_disk("Old laptop", &base.join("old")).unwrap();
        let pictures = on(&base.join("old"), PICTURES);
        jpeg(&pictures.join("IMG_0.JPG"), 10);
        jpeg(&pictures.join("2021").join("IMG_1.JPG"), 60);
        jpeg(&pictures.join("2021").join("May").join("IMG_2.JPG"), 120);
        let root = {
            let db = cat.db();
            db.execute("INSERT INTO roots (volume_id, rel_path, added_at) VALUES (?1, ?2, 0)", params![old, PICTURES])
                .unwrap();
            db.last_insert_rowid()
        };
        scan(&cat, root);
        World { base, cat, old, root }
    }

    /// Copies the old disk's photos onto the new disk, below `rel`: all of them, or all but `IMG_2.JPG`.
    fn copy_to_new(&self, rel: &str, all: bool) {
        let from = on(&self.base.join("old"), PICTURES);
        let to = on(&self.base.join("new"), rel);
        for f in ["IMG_0.JPG", "2021\\IMG_1.JPG", "2021\\May\\IMG_2.JPG"] {
            if !all && f.ends_with("IMG_2.JPG") {
                continue;
            }
            let dst = on(&to, f);
            std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
            std::fs::copy(on(&from, f), dst).unwrap();
        }
    }

    /// The old laptop is gone; the new one is plugged in. Returns the new disk's id.
    fn switch_laptops(&self) -> i64 {
        // Looking at the disks plugged in forgets the test disks: only the new one comes back.
        self.cat.refresh_volumes().unwrap();
        self.cat.plug_in_test_disk("New laptop", &self.base.join("new")).unwrap()
    }

    /// The source's disk and path.
    fn source(&self) -> (i64, String) {
        let r = self.cat.root(self.root).unwrap();
        (r.volume_id, r.rel_path)
    }

    /// Every photo, and each of its files' disk, folder, name and state.
    fn files(&self) -> Vec<(i64, i64, String, String, i64)> {
        let db = self.cat.db();
        let mut stmt = db
            .prepare(
                "SELECT f.photo_id, fo.volume_id, fo.rel_path, f.name, f.state FROM files f
                   JOIN folders fo ON fo.id = f.folder_id ORDER BY f.name",
            )
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn finish(self) {
        drop(self.cat);
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn scan(cat: &Catalog, root: i64) -> Report {
    scan_root(cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap()
}

#[test]
fn a_source_on_a_replaced_disk_is_offered_where_it_is_now_and_keeps_its_photos() {
    let w = World::new("replaced");
    let before = w.files();
    assert_eq!(before.len(), 3);
    w.copy_to_new(PICTURES, true);
    let new = w.switch_laptops();
    assert!(!w.cat.root_is_here(w.old, PICTURES), "the old laptop's disk is gone");

    // Found at the same path on the new disk, and offered in the window.
    assert!(repoint::look_elsewhere(&w.cat).unwrap());
    let offer = w.cat.found_elsewhere().remove(&w.root).expect("found on the new disk");
    assert_eq!((offer.checked, offer.found, offer.problem.as_deref()), (3, 3, None));
    assert_eq!(offer.disk, "New laptop");
    let shown = w.cat.overview().unwrap();
    let root = shown.volumes.iter().flat_map(|v| &v.roots).find(|r| r.id == w.root).unwrap();
    assert_eq!(root.elsewhere.as_ref(), Some(&offer));

    // Pointed there: the same source, the same photos, their folders on the new disk. Nothing is missing.
    let done = repoint::repoint_on(&w.cat, w.root, new, PICTURES).unwrap();
    assert_eq!((done.checked, done.found), (3, 3));
    assert_eq!(w.source(), (new, PICTURES.to_string()));
    assert!(w.cat.root_is_here(new, PICTURES));
    assert!(w.cat.found_elsewhere().is_empty(), "no longer offered");
    let after = w.files();
    assert_eq!(
        after.iter().map(|f| (f.0, f.3.clone(), f.4)).collect::<Vec<_>>(),
        before.iter().map(|f| (f.0, f.3.clone(), f.4)).collect::<Vec<_>>(),
        "same photos, same files, none missing"
    );
    assert!(after.iter().all(|f| f.1 == new && f.2.starts_with(PICTURES)), "{after:?}");
    let shown = w.cat.overview().unwrap();
    assert!(shown.volumes.iter().all(|v| v.id != w.old), "the old disk holds nothing any more: not listed");
    assert_eq!(shown.missing, 0);
    assert_eq!(shown.offline, 0, "every photo is on a disk plugged in");

    // The scan that follows finds them all there, as the same photos.
    let r = scan(&w.cat, w.root);
    assert_eq!((r.found, r.missing, r.photos_added), (3, 0, 0), "{r:?}");
    assert_eq!(w.files().iter().map(|f| f.0).collect::<Vec<_>>(), before.iter().map(|f| f.0).collect::<Vec<_>>());
    w.finish();
}

#[test]
fn a_source_can_go_to_another_path_and_a_whole_disk() {
    let w = World::new("other-path");
    w.copy_to_new("", true);
    let new = w.switch_laptops();
    // Not at the same path: nothing to offer, the user picks the folder.
    assert!(!repoint::look_elsewhere(&w.cat).unwrap());
    let done = repoint::repoint_on(&w.cat, w.root, new, "").unwrap();
    assert_eq!((done.checked, done.found), (3, 3));
    assert_eq!(w.source(), (new, String::new()));
    let folders: Vec<String> = w.files().into_iter().map(|f| f.2).collect();
    assert_eq!(folders, ["", "2021", "2021\\May"]);
    let r = scan(&w.cat, w.root);
    assert_eq!((r.found, r.missing, r.photos_added), (3, 0, 0), "{r:?}");
    w.finish();
}

#[test]
fn a_folder_without_its_photos_is_refused_and_nothing_changes() {
    let w = World::new("refused");
    let other = on(&w.base.join("new"), PICTURES);
    jpeg(&other.join("IMG_0.JPG"), 200); // same name, other photo
    jpeg(&other.join("2021").join("IMG_1.JPG"), 220);
    let new = w.switch_laptops();
    let before = w.files();
    assert!(!repoint::look_elsewhere(&w.cat).unwrap(), "not offered");
    let check = repoint::check_on(&w.cat, w.root, new, PICTURES).unwrap();
    assert_eq!((check.checked, check.found), (3, 0));
    assert!(check.problem.as_deref().unwrap().contains("isn't the same folder"), "{check:?}");
    let refused = repoint::repoint_on(&w.cat, w.root, new, PICTURES).unwrap_err().to_string();
    assert!(refused.contains("Nothing was changed"), "{refused}");
    assert_eq!(w.source(), (w.old, PICTURES.to_string()));
    assert_eq!(w.files(), before);
    w.finish();
}

#[test]
fn a_copy_already_in_the_catalog_is_not_offered() {
    let w = World::new("already");
    w.copy_to_new(PICTURES, true);
    let new = w.switch_laptops();
    // The copy on the new disk is a source of its own (a backup, or added again by hand).
    w.cat
        .db()
        .execute("INSERT INTO roots (volume_id, rel_path, added_at) VALUES (?1, ?2, 0)", params![new, PICTURES])
        .unwrap();
    assert!(!repoint::look_elsewhere(&w.cat).unwrap());
    let check = repoint::check_on(&w.cat, w.root, new, PICTURES).unwrap();
    assert!(check.problem.as_deref().unwrap().contains("already in the catalog"), "{check:?}");
    // Inside it, or holding it: refused too.
    for rel in ["shoebox-repoint-test", "shoebox-repoint-test\\Pictures\\2021"] {
        assert!(repoint::check_on(&w.cat, w.root, new, rel).unwrap().problem.is_some(), "{rel}");
    }
    w.finish();
}

#[test]
fn photos_not_in_the_new_place_go_missing_only_after_the_scan_and_stay() {
    let w = World::new("partial");
    w.copy_to_new(PICTURES, false);
    let new = w.switch_laptops();
    // Two of three is too few to offer by itself; the user can still choose it.
    assert!(!repoint::look_elsewhere(&w.cat).unwrap());
    let check = repoint::check_on(&w.cat, w.root, new, PICTURES).unwrap();
    assert_eq!((check.checked, check.found, check.problem.as_deref()), (3, 2, None));
    repoint::repoint_on(&w.cat, w.root, new, PICTURES).unwrap();
    assert_eq!(w.cat.overview().unwrap().missing, 0, "nothing is missing before a complete scan");
    let r = scan(&w.cat, w.root);
    assert_eq!((r.found, r.missing), (2, 1), "{r:?}");
    let files = w.files();
    assert_eq!(files.len(), 3, "the missing photo stays in the catalog");
    assert_eq!(files.iter().filter(|f| f.4 == 1).map(|f| f.3.as_str()).collect::<Vec<_>>(), ["IMG_2.JPG"]);
    w.finish();
}
