//! A Shoebox stopped mid-scan leaves nothing half-done for good: a photo saved
//! but stopped before its preview was written is read again at the next start
//! (with the files that couldn't be read), and a file found again in another
//! folder is linked to its photo in one step, never half.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).

use shoebox_lib::catalog::Catalog;
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::{Report, failed_count, retry_failed, scan_root};
use shoebox_lib::volumes::SEP;
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn jpeg(path: &Path, shade: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(640, 480, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

fn scan(cat: &Catalog, root: i64) -> Report {
    scan_root(cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap()
}

/// Photo ids follow reading order, which is parallel: find them by file.
fn photo_of(cat: &Catalog, name: &str) -> Option<i64> {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

#[test]
fn a_photo_left_half_read_is_read_again() {
    let base = std::env::temp_dir().join("shoebox-half-read");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    jpeg(&photos.join("IMG_0.JPG"), 40);
    jpeg(&photos.join("IMG_1.JPG"), 160);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    let root = cat.add_root(&photos).unwrap();
    assert_eq!(scan(&cat, root).photos_added, 2);
    let volume: i64 = cat.db().query_row("SELECT volume_id FROM roots WHERE id = ?1", [root], |r| r.get(0)).unwrap();
    let p0 = photo_of(&cat, "IMG_0.JPG").unwrap();

    // Stopped after the photo was saved, before its preview and thumbnail were.
    cat.db().execute("UPDATE photos SET preview_state = 0 WHERE id = ?1", [p0]).unwrap();
    std::fs::remove_file(data.preview_path(p0)).unwrap();
    std::fs::remove_file(data.thumb_path(p0)).unwrap();

    // At the next start its disk is there: it is read again, and only it.
    assert_eq!(failed_count(&cat, volume).unwrap(), 1);
    let r = retry_failed(&cat, volume, &AtomicBool::new(false), &|_| {}).unwrap();
    assert_eq!((r.found, r.reread, r.errors, r.missing), (1, 1, 0, 0), "{r:?}");
    let state: i64 = cat.db().query_row("SELECT preview_state FROM photos WHERE id = ?1", [p0], |r| r.get(0)).unwrap();
    assert_eq!(state, 1);
    assert!(data.preview_path(p0).is_file() && data.thumb_path(p0).is_file());
    assert_eq!(failed_count(&cat, volume).unwrap(), 0);

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_file_found_elsewhere_is_linked_in_one_step() {
    let base = std::env::temp_dir().join("shoebox-link-once");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    jpeg(&photos.join("a").join("IMG_0.JPG"), 90);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();
    assert_eq!(scan(&cat, root).photos_added, 1);
    let photo = photo_of(&cat, "IMG_0.JPG").unwrap();
    std::fs::create_dir_all(photos.join("b")).unwrap();
    std::fs::rename(photos.join("a").join("IMG_0.JPG"), photos.join("b").join("IMG_0.JPG")).unwrap();
    let files = |cat: &Catalog| -> Vec<(String, Option<i64>, i64)> {
        cat.db()
            .prepare(
                "SELECT fo.rel_path, f.photo_id, f.state FROM files f JOIN folders fo ON fo.id = f.folder_id
                  ORDER BY fo.rel_path",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .map(|(rel, pid, state)| (rel.rsplit(SEP).next().unwrap().to_string(), pid, state))
            .collect()
    };

    // Linking stops halfway, as if Shoebox were killed there: dropping the old,
    // missing location fails. The new file isn't left linked on its own.
    cat.db()
        .execute_batch("CREATE TRIGGER stop BEFORE DELETE ON files BEGIN SELECT RAISE(ABORT, 'stopped'); END;")
        .unwrap();
    let r = scan(&cat, root);
    assert_eq!((r.missing, r.linked, r.errors), (1, 0, 1), "{r:?}");
    assert_eq!(files(&cat), [("a".to_string(), Some(photo), 1), ("b".to_string(), None, 0)]);

    // The next scan links it, and the old location goes in the same step.
    cat.db().execute_batch("DROP TRIGGER stop;").unwrap();
    let r = scan(&cat, root);
    assert_eq!((r.linked, r.errors), (1, 0), "{r:?}");
    assert_eq!(files(&cat), [("b".to_string(), Some(photo), 0)]);

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
