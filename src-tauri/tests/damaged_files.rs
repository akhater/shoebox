//! Previews, thumbnails and face pictures a PC that stopped mid-write left at
//! their full size but all zeros (or zeros at their end) are found: thumbnail
//! and face picture are made again from the preview, a photo whose preview is
//! damaged is read again from its disk. Only files written since the last
//! check are looked into; a missing one is noticed whatever its date.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::paths::DataDir;
use shoebox_lib::repair::check_files;
use shoebox_lib::scan::{failed_count, retry_failed, scan_root};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

fn state(cat: &Catalog, id: i64) -> i64 {
    cat.db().query_row("SELECT preview_state FROM photos WHERE id = ?1", [id], |r| r.get(0)).unwrap()
}

fn rev(cat: &Catalog, id: i64) -> i64 {
    cat.db().query_row("SELECT preview_rev FROM photos WHERE id = ?1", [id], |r| r.get(0)).unwrap()
}

fn photo(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

/// What a crash leaves: the size is right, the content never reached the disk.
fn zero(path: &Path, from: usize) {
    let mut bytes = std::fs::read(path).unwrap();
    bytes[from..].iter_mut().for_each(|b| *b = 0);
    std::fs::write(path, bytes).unwrap();
}

fn whole(path: &Path) -> bool {
    image::open(path).is_ok()
}

#[test]
fn damaged_files_are_made_again() {
    let base = std::env::temp_dir().join("shoebox-damaged-files");
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
    // A face found in IMG_2, its picture not saved (yet).
    cat.db()
        .execute(
            "INSERT INTO faces (photo_id, x1, y1, x2, y2, kps, score) VALUES (?1, 0.25, 0.25, 0.5, 0.6, '', 0.9)",
            [p2],
        )
        .unwrap();
    let face: i64 = cat.db().query_row("SELECT id FROM faces", [], |r| r.get(0)).unwrap();

    // The first check looks at everything: all fine, the face picture cut.
    let c = check_files(&cat, false).unwrap();
    assert_eq!(c.checked, 6);
    assert!(c.to_read_again.is_empty(), "{c:?}");
    assert_eq!((c.faces_cut_again, c.faces_waiting), (1, 0));
    assert_eq!(image::image_dimensions(cat.data.face_path(face)).unwrap(), (160, 160));

    // A crash: IMG_0's preview all zeros, IMG_1's thumbnail zeros at its end, the face picture zeros.
    zero(&cat.data.preview_path(p0), 0);
    let thumb1 = cat.data.thumb_path(p1);
    zero(&thumb1, std::fs::metadata(&thumb1).unwrap().len() as usize / 2);
    zero(&cat.data.face_path(face), 0);
    let rev1 = rev(&cat, p1);
    let c = check_files(&cat, false).unwrap();
    // The thumbnail comes from IMG_1's preview, which is fine: no disk needed.
    assert_eq!(c.to_read_again, [p0], "{c:?}");
    assert_eq!(c.disks, [volume]);
    assert_eq!(c.thumbs_made_again, 1);
    assert!(whole(&thumb1) && rev(&cat, p1) == rev1 + 1);
    assert_eq!((c.faces_cut_again, c.faces_waiting), (1, 0));
    assert!(whole(&cat.data.face_path(face)));
    assert_eq!((state(&cat, p0), state(&cat, p1), state(&cat, p2)), (0, 1, 1));

    // Its disk is plugged in: read again, preview whole.
    assert_eq!(failed_count(&cat, volume).unwrap(), 1);
    let r = retry_failed(&cat, volume, &no, &|_| {}).unwrap();
    assert_eq!((r.found, r.reread, r.errors), (1, 1, 0), "{r:?}");
    assert_eq!(state(&cat, p0), 1);
    assert!(whole(&cat.data.preview_path(p0)));

    // Damaged long before the last check: not looked into again, unless asked for all.
    let preview2 = cat.data.preview_path(p2);
    zero(&preview2, 0);
    let day_ago = SystemTime::now() - Duration::from_secs(24 * 3600);
    std::fs::File::options().write(true).open(&preview2).unwrap().set_modified(day_ago).unwrap();
    let c = check_files(&cat, false).unwrap();
    assert!(c.to_read_again.is_empty(), "{c:?}");
    let c = check_files(&cat, true).unwrap();
    assert_eq!(c.to_read_again, [p2]);
    // Its face picture waits for the new preview, then is cut from it.
    zero(&cat.data.face_path(face), 0);
    let c = check_files(&cat, false).unwrap();
    assert_eq!((c.faces_cut_again, c.faces_waiting), (0, 1));
    assert!(!cat.data.face_path(face).exists());
    retry_failed(&cat, volume, &no, &|_| {}).unwrap();
    let c = check_files(&cat, false).unwrap();
    assert_eq!((c.faces_cut_again, c.faces_waiting), (1, 0));
    assert!(whole(&cat.data.face_path(face)));

    // A thumbnail gone, whatever when: made again from the preview. A preview
    // gone: its photo is read again.
    std::fs::remove_file(cat.data.thumb_path(p1)).unwrap();
    std::fs::remove_file(cat.data.preview_path(p0)).unwrap();
    let c = check_files(&cat, false).unwrap();
    assert_eq!((c.to_read_again.as_slice(), c.thumbs_made_again), ([p0].as_slice(), 1), "{c:?}");
    assert!(whole(&cat.data.thumb_path(p1)));

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
