//! A folder deleted outside Shoebox after its photos went elsewhere: the
//! catalog holds nothing in it any more, so a complete scan takes it out of the
//! sidebar. Folders still on the disk stay, even empty, and so do folders
//! still holding missing photos (those leave only by hand).
//!
//! Lives here, not in src/, because it writes and deletes sample photos: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use shoebox_lib::catalog::Catalog;
use shoebox_lib::organize;
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn jpeg(path: &Path, shade: u8) {
    image::RgbImage::from_fn(320, 240, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

/// Catalog folders, relative to the scanned folder, sorted.
fn folders(cat: &Catalog, root_rel: &str) -> Vec<String> {
    let mut out: Vec<String> = cat
        .db()
        .prepare("SELECT rel_path FROM folders")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .filter_map(|rel| rel.strip_prefix(root_rel).map(|s| s.trim_start_matches(['\\', '/']).replace('\\', "/")))
        .collect();
    out.sort();
    out
}

#[test]
fn a_scan_drops_folders_gone_from_the_disk_with_nothing_left_in_them() {
    let base = std::env::temp_dir().join("shoebox-empty-folders");
    let _ = std::fs::remove_dir_all(&base);
    let shoot = base.join("shoot");
    let (day, deeper, lost) = (shoot.join("day"), shoot.join("day").join("roll"), shoot.join("lost"));
    for d in [&deeper, &lost] {
        std::fs::create_dir_all(d).unwrap();
    }
    jpeg(&day.join("a.jpg"), 10);
    jpeg(&deeper.join("b.jpg"), 90);
    jpeg(&lost.join("c.jpg"), 170);

    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root_id = cat.add_root(&shoot).unwrap();
    let root = cat.root(root_id).unwrap();
    let scan = || scan_root(&cat, root_id, &AtomicBool::new(false), false, &|_| {}).unwrap();
    scan();
    // An empty folder made in Shoebox: on the disk, so it stays.
    organize::create_folder(&cat, root.volume_id, &root.rel_path, "kept").unwrap();
    assert_eq!(folders(&cat, &root.rel_path), ["day", "day/roll", "kept", "lost"]);

    // Outside Shoebox: day's photos moved to "sorted" and day deleted; lost
    // deleted with its photo.
    let sorted = shoot.join("sorted");
    std::fs::create_dir_all(&sorted).unwrap();
    std::fs::rename(day.join("a.jpg"), sorted.join("a.jpg")).unwrap();
    std::fs::rename(deeper.join("b.jpg"), sorted.join("b.jpg")).unwrap();
    std::fs::remove_dir_all(&day).unwrap();
    std::fs::remove_dir_all(&lost).unwrap();

    // A scan that doesn't finish changes nothing.
    scan_root(&cat, root_id, &AtomicBool::new(true), false, &|_| {}).unwrap();
    assert_eq!(folders(&cat, &root.rel_path), ["day", "day/roll", "kept", "lost"]);

    let report = scan();
    assert_eq!((report.linked, report.missing), (2, 3), "a and b found again, then c missing");
    // day and day/roll held only a and b, now in sorted: gone. lost still holds
    // c, missing: stays until c is taken out by hand. kept is on the disk.
    assert_eq!(folders(&cat, &root.rel_path), ["kept", "lost", "sorted"]);
    let missing: i64 = cat.db().query_row("SELECT COUNT(*) FROM files WHERE state <> 0", [], |r| r.get(0)).unwrap();
    assert_eq!(missing, 1, "only c, missing");

    // c taken out by hand: the next scan drops its folder too.
    let c: i64 = cat.db().query_row("SELECT photo_id FROM files WHERE name = 'c.jpg'", [], |r| r.get(0)).unwrap();
    assert_eq!(cat.forget_missing(&[c]).unwrap().photos, 1);
    scan();
    assert_eq!(folders(&cat, &root.rel_path), ["kept", "sorted"]);
    assert!(shoot.join("kept").is_dir() && sorted.join("a.jpg").is_file() && sorted.join("b.jpg").is_file());
    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
