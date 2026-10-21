//! Folder paths on every system. The catalog keeps a folder as its names
//! joined by backslashes, whatever the system, so one catalog reads the same
//! everywhere; the files are found, read, renamed and moved through the
//! system's own separator. Names that can't be kept in those paths (a
//! backslash in a name on macOS and Linux, a name that is not Unicode) are
//! left out of the catalog, and never taken for photos that are gone.
//!
//! Two folders stand for two disks (`Catalog::plug_in_test_disk`).
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).

use rusqlite::params;
use shoebox_lib::catalog::Catalog;
use shoebox_lib::organize::{self, FolderMove};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::{Report, scan_root};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn jpeg(path: &Path, shade: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(160, 120, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

/// A catalog, and the folders standing for disks A and B, plugged in.
fn world(name: &str) -> (PathBuf, Catalog, i64, i64) {
    let base = std::env::temp_dir().join(format!("shoebox-folder-paths-{name}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("disk-a")).unwrap();
    std::fs::create_dir_all(base.join("disk-b")).unwrap();
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let a = cat.plug_in_test_disk("Disk A", &base.join("disk-a")).unwrap();
    let b = cat.plug_in_test_disk("Disk B", &base.join("disk-b")).unwrap();
    (base, cat, a, b)
}

fn finish(base: PathBuf, cat: Catalog) {
    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}

/// Makes `rel` (names joined by backslashes) a source of `disk`.
fn source(cat: &Catalog, disk: i64, rel: &str) -> i64 {
    let db = cat.db();
    db.execute("INSERT INTO roots (volume_id, rel_path, added_at) VALUES (?1, ?2, 0)", params![disk, rel]).unwrap();
    db.last_insert_rowid()
}

fn scan(cat: &Catalog, root: i64) -> Report {
    scan_root(cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap()
}

/// The folders the catalog knows on a disk, as it keeps them.
fn folders(cat: &Catalog, disk: i64) -> Vec<String> {
    let db = cat.db();
    let mut stmt = db.prepare("SELECT rel_path FROM folders WHERE volume_id = ?1 ORDER BY rel_path").unwrap();
    let rows: Vec<String> = stmt.query_map([disk], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    rows
}

#[cfg(unix)]
fn file_names(cat: &Catalog) -> Vec<String> {
    let db = cat.db();
    let mut stmt = db.prepare("SELECT name FROM files ORDER BY name").unwrap();
    let rows: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    rows
}

/// Photos on disk A: `Photos` with `Trip` (and its `Day 2`) and one at the top.
fn trip(base: &Path) -> PathBuf {
    let photos = base.join("disk-a").join("Photos");
    jpeg(&photos.join("IMG_0.JPG"), 10);
    jpeg(&photos.join("Trip").join("IMG_1.JPG"), 60);
    jpeg(&photos.join("Trip").join("Day 2").join("IMG_2.JPG"), 120);
    photos
}

#[test]
fn a_scan_keeps_folders_the_same_on_every_system_and_finds_their_files_again() {
    let (base, cat, a, _) = world("scan");
    let photos = trip(&base);
    let root = source(&cat, a, "Photos");

    let r = scan(&cat, root);
    assert_eq!((r.found, r.new_files, r.photos_added, r.errors, r.skipped), (3, 3, 3, 0, 0), "{r:?}");
    assert_eq!(folders(&cat, a), ["Photos", "Photos\\Trip", "Photos\\Trip\\Day 2"]);

    // From what the catalog keeps, the files and folders are found on disk.
    let id: i64 = cat.db().query_row("SELECT id FROM files WHERE name = 'IMG_2.JPG'", [], |r| r.get(0)).unwrap();
    let file = PathBuf::from(cat.file_path(id).unwrap());
    assert_eq!(file, photos.join("Trip").join("Day 2").join("IMG_2.JPG"));
    assert!(file.is_file());
    assert_eq!(PathBuf::from(cat.folder_path(a, "Photos\\Trip\\Day 2").unwrap()), photos.join("Trip").join("Day 2"));

    // A second scan changes nothing.
    let r = scan(&cat, root);
    assert_eq!((r.new_files, r.changed, r.unchanged, r.missing, r.errors), (0, 0, 3, 0, 0), "{r:?}");

    // A photo deleted outside Shoebox is found gone.
    std::fs::remove_file(&file).unwrap();
    let r = scan(&cat, root);
    assert_eq!((r.found, r.missing), (2, 1), "{r:?}");
    finish(base, cat);
}

#[test]
fn a_folder_is_renamed_and_moved_to_another_disk_through_the_systems_own_paths() {
    let (base, cat, a, b) = world("organize");
    let photos = trip(&base);
    let archive = base.join("disk-b").join("Archive");
    std::fs::create_dir_all(&archive).unwrap();
    let (photos_root, archive_root) = (source(&cat, a, "Photos"), source(&cat, b, "Archive"));
    scan(&cat, photos_root);
    scan(&cat, archive_root);

    let renamed = organize::rename_folder(&cat, a, "Photos\\Trip", "Journey").unwrap();
    assert_eq!(renamed, "Photos\\Journey");
    assert!(photos.join("Journey").join("Day 2").join("IMG_2.JPG").is_file());
    assert!(!photos.join("Trip").exists());
    assert_eq!(folders(&cat, a), ["Photos", "Photos\\Journey", "Photos\\Journey\\Day 2"]);
    let r = scan(&cat, photos_root);
    assert_eq!((r.new_files, r.changed, r.unchanged, r.missing, r.errors), (0, 0, 3, 0, 0), "{r:?}");

    // To the other disk: copied, checked, the original removed, nothing read again.
    let m = FolderMove { from: a, rel: "Photos\\Journey".into(), to: b, to_parent: "Archive".into() };
    let moved = organize::move_folder(&cat, &m, &AtomicBool::new(false), &|_| {}).unwrap();
    assert_eq!(
        (moved.copied, moved.files, moved.rel.as_str(), moved.left_behind.len()),
        (true, 2, "Archive\\Journey", 0)
    );
    assert!(archive.join("Journey").join("Day 2").join("IMG_2.JPG").is_file());
    assert!(archive.join("Journey").join("IMG_1.JPG").is_file());
    assert!(!photos.join("Journey").exists());
    assert_eq!(folders(&cat, b), ["Archive\\Journey", "Archive\\Journey\\Day 2"]);

    // The copies are as the originals were, so a scan finds nothing to change on either disk.
    let r = scan(&cat, archive_root);
    assert_eq!((r.found, r.new_files, r.changed, r.unchanged, r.missing, r.errors), (2, 0, 0, 2, 0, 0), "{r:?}");
    let r = scan(&cat, photos_root);
    assert_eq!((r.found, r.unchanged, r.missing, r.errors), (1, 1, 0, 0), "{r:?}");
    finish(base, cat);
}

/// A backslash is a name on macOS and Linux (and a separator in the paths the
/// catalog keeps), and a name there can be no Unicode at all (Linux). Such
/// photos and folders are left out: not catalogued, counted, and the rest of
/// the catalog is left as it is.
#[cfg(unix)]
#[test]
fn names_the_catalog_cannot_keep_are_left_out_and_never_taken_for_gone() {
    let (base, cat, a, _) = world("weird");
    let photos = base.join("disk-a").join("Photos");
    jpeg(&photos.join("ok").join("a.JPG"), 10);
    jpeg(&photos.join("ok").join("z.JPG"), 20);
    let root = source(&cat, a, "Photos");
    let r = scan(&cat, root);
    assert_eq!((r.found, r.photos_added, r.skipped), (2, 2, 0), "{r:?}");

    // A photo and a folder with a backslash in their names...
    jpeg(&photos.join("ok").join("c\\d.JPG"), 30);
    jpeg(&photos.join("we\\ird").join("b.JPG"), 40);
    // ...and, where the system allows it (macOS doesn't), one that is not Unicode.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        jpeg(&photos.join("ok").join(std::ffi::OsStr::from_bytes(b"caf\xe9.JPG")), 50);
    }
    let left_out = if cfg!(target_os = "linux") { 3 } else { 2 };

    let r = scan(&cat, root);
    assert_eq!((r.found, r.new_files, r.unchanged, r.missing, r.errors), (2, 0, 2, 0, 0), "{r:?}");
    assert_eq!(r.skipped, left_out, "{r:?}");
    assert_eq!(file_names(&cat), ["a.JPG", "z.JPG"]);
    assert_eq!(folders(&cat, a), ["Photos\\ok"]);

    // What is catalogued is still found gone when it goes.
    std::fs::remove_file(photos.join("ok").join("z.JPG")).unwrap();
    let r = scan(&cat, root);
    assert_eq!((r.found, r.missing), (1, 1), "{r:?}");
    assert_eq!(r.skipped, left_out, "{r:?}");
    finish(base, cat);
}

#[cfg(unix)]
#[test]
fn a_folder_with_a_backslash_in_its_name_is_not_added() {
    let (base, cat, ..) = world("weird-root");
    let weird = base.join("disk-a").join("Photos\\2026");
    std::fs::create_dir_all(&weird).unwrap();
    let refused = cat.add_root(&weird).unwrap_err().to_string();
    assert!(refused.contains("can't be added"), "{refused}");
    finish(base, cat);
}

/// A catalog made on one system is opened on another, and shows an unplugged
/// disk where it was last seen.
#[test]
fn a_disk_last_seen_on_another_system_is_shown_as_it_was() {
    let base = std::env::temp_dir().join("shoebox-folder-paths-shown");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    {
        let db = cat.db();
        for (serial, label, mount, rel) in [
            (1, "Windows disk", "D:\\", "Photos\\Trip"),
            (2, "Mac disk", "/Volumes/Photos/", "Pictures\\Trip"),
            (3, "Linux disk", "/", "home\\me\\Pictures"),
        ] {
            db.execute(
                "INSERT INTO volumes (serial, fs, size_bytes, label, last_mount) VALUES (?1, 'test', 1, ?2, ?3)",
                params![serial, label, mount],
            )
            .unwrap();
            let id = db.last_insert_rowid();
            db.execute("INSERT INTO roots (volume_id, rel_path, added_at) VALUES (?1, ?2, 0)", params![id, rel])
                .unwrap();
        }
    }
    let overview = cat.overview().unwrap();
    let path_of = |label: &str| overview.volumes.iter().find(|v| v.label == label).unwrap().roots[0].path.clone();
    assert_eq!(path_of("Windows disk"), "D:\\Photos\\Trip");
    assert_eq!(path_of("Mac disk"), "/Volumes/Photos/Pictures/Trip");
    assert_eq!(path_of("Linux disk"), "/home/me/Pictures");
    finish(base, cat);
}
