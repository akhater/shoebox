//! A folder holding every photo twice under two names (an import tool's copy:
//! `IMG_0001.JPG` = `2006-01-01_Camera_100-0001.JPG`). Each photo is
//! read once, its copy is only linked, and nothing fails - before, two copies
//! read at the same moment both wrote the preview through one temporary file
//! and one of them was marked unreadable.
//!
//! Lives here, not in src/, because it writes sample photos: no file under
//! src/ outside paths.rs may hold file-writing code (promise 1).
#![cfg(windows)]

use rusqlite::params;
use shoebox_lib::catalog::Catalog;
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::{Report, scan_root};
use std::path::Path;
use std::sync::atomic::AtomicBool;

const PHOTOS: u8 = 12;

fn jpeg(path: &Path, shade: u8) {
    image::RgbImage::from_fn(640, 480, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

fn scan(cat: &Catalog, root: i64) -> Report {
    scan_root(cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap()
}

#[test]
fn the_same_photo_twice_in_a_folder_is_read_once() {
    let base = std::env::temp_dir().join("shoebox-scan-copies");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    // Listed apart (2006-... first, IMG_... last), so the workers meet each
    // photo's two copies at the same time.
    for i in 0..PHOTOS {
        let a = photos.join(format!("2006-01-01_Camera_100-{i:04}.JPG"));
        jpeg(&a, i * 20);
        std::fs::copy(&a, photos.join(format!("IMG_{i:04}.JPG"))).unwrap();
    }
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();

    let r = scan(&cat, root);
    let n = PHOTOS as u64;
    assert_eq!((r.photos_added, r.duplicates, r.linked, r.reread, r.errors), (n, n, 0, 0, 0), "{r:?}");
    let (photos_ok, files): (i64, i64) = cat
        .db()
        .query_row(
            "SELECT (SELECT COUNT(*) FROM photos WHERE preview_state = 1), (SELECT COUNT(*) FROM files)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((photos_ok, files), (n as i64, 2 * n as i64));

    // A photo marked unreadable is tried again by the next scan (once, for both copies).
    cat.db().execute("UPDATE photos SET preview_state = 2, error = 'x' WHERE id = 1", params![]).unwrap();
    let r = scan(&cat, root);
    assert_eq!((r.photos_added, r.reread, r.duplicates, r.errors), (0, 1, 1, 0), "{r:?}");
    let state: i64 = cat.db().query_row("SELECT preview_state FROM photos WHERE id = 1", [], |row| row.get(0)).unwrap();
    assert_eq!(state, 1);

    // Nothing to do the time after.
    let r = scan(&cat, root);
    assert_eq!((r.photos_added, r.reread, r.duplicates, r.linked, r.errors), (0, 0, 0, 0, 0), "{r:?}");

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
