//! Photos marked "couldn't be read" fix themselves: one whose preview is in
//! fact there is marked ready without its disk (faces and word search only
//! need the preview); one whose original really failed is read again when
//! its disk is plugged in - only that file, and nothing is marked missing.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::{failed_count, heal_saved_previews, retry_failed, scan_root};
use std::sync::atomic::AtomicBool;

fn state(cat: &Catalog, id: i64) -> i64 {
    cat.db().query_row("SELECT preview_state FROM photos WHERE id = ?1", [id], |r| r.get(0)).unwrap()
}

/// Photo ids follow reading order, which is parallel: find them by file.
fn photo(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

fn fail(cat: &Catalog, id: i64, error: &str) {
    cat.db().execute("UPDATE photos SET preview_state = 2, error = ?1 WHERE id = ?2", (error, id)).unwrap();
}

#[test]
fn failed_photos_fix_themselves() {
    let base = std::env::temp_dir().join("shoebox-failed-photos");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    for i in 0..3u8 {
        image::RgbImage::from_fn(640, 480, |x, y| image::Rgb([i * 80, (x % 256) as u8, (y % 256) as u8]))
            .save(photos.join(format!("IMG_{i}.JPG")))
            .unwrap();
    }
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();
    let no = AtomicBool::new(false);
    let r = scan_root(&cat, root, &no, false, &|_| {}).unwrap();
    assert_eq!((r.photos_added, r.errors), (3, 0));
    let volume: i64 = cat.db().query_row("SELECT volume_id FROM roots WHERE id = ?1", [root], |r| r.get(0)).unwrap();
    let (p0, p1, p2) = (photo(&cat, "IMG_0.JPG"), photo(&cat, "IMG_1.JPG"), photo(&cat, "IMG_2.JPG"));

    // Marked unreadable while saving its preview, but the preview is there: fine, no disk needed.
    fail(&cat, p0, "saving preview: The system cannot find the file specified. (os error 2)");
    // Same, but its preview really is gone: left for a re-read.
    fail(&cat, p1, "saving preview: disk full");
    std::fs::remove_file(cat.data.preview_path(p1)).unwrap();
    assert_eq!(heal_saved_previews(&cat).unwrap(), 1);
    assert_eq!((state(&cat, p0), state(&cat, p1)), (1, 2));

    // The original of IMG_2 couldn't be read; its file is away while the disk is "plugged in".
    fail(&cat, p2, "decode error");
    let img2 = photos.join("IMG_2.JPG");
    let away = base.join("IMG_2.away");
    std::fs::rename(&img2, &away).unwrap();
    assert_eq!(failed_count(&cat, volume).unwrap(), 2);
    let r = retry_failed(&cat, volume, &no, &|_| {}).unwrap();
    // Only IMG_1 is there to read; IMG_2 being away marks nothing missing.
    assert!(r.retry);
    assert_eq!((r.found, r.reread, r.errors, r.missing), (1, 1, 0, 0), "{r:?}");
    assert_eq!((state(&cat, p1), state(&cat, p2)), (1, 2));
    let missing: i64 = cat.db().query_row("SELECT COUNT(*) FROM files WHERE state = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(missing, 0);

    // Back in its place: the next retry reads it.
    std::fs::rename(&away, &img2).unwrap();
    let r = retry_failed(&cat, volume, &no, &|_| {}).unwrap();
    assert_eq!((r.found, r.reread, r.errors), (1, 1, 0), "{r:?}");
    assert_eq!(state(&cat, p2), 1);
    assert_eq!(failed_count(&cat, volume).unwrap(), 0);

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
