//! A photo counts as looked at for faces only once its faces, their
//! fingerprints and their pictures are all saved: a Shoebox stopped halfway
//! looks at it again, rather than leaving faces with no picture or fingerprint.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use image::{DynamicImage, RgbImage};
use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::{Detected, FaceIndex};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use std::sync::atomic::AtomicBool;

fn count(cat: &Catalog, sql: &str, photo: i64) -> i64 {
    cat.db().query_row(sql, [photo], |r| r.get(0)).unwrap()
}

/// One face, big enough to be grouped into a person.
fn one_face() -> Vec<(Detected, Vec<f32>)> {
    let mut v = vec![0.0; 512];
    v[0] = 1.0;
    vec![(Detected { bbox: [100.0, 60.0, 200.0, 180.0], kps: [0.0; 10], score: 0.9 }, v)]
}

#[test]
fn a_photo_is_looked_at_only_once_its_faces_are_saved() {
    let base = std::env::temp_dir().join("shoebox-faces-saved");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    let img = RgbImage::from_fn(320, 240, |x, y| image::Rgb([90, (x % 256) as u8, (y % 256) as u8]));
    img.save(photos.join("IMG_0.JPG")).unwrap();
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let photo: i64 = cat.db().query_row("SELECT id FROM photos", [], |r| r.get(0)).unwrap();
    let faces = FaceIndex::open(&cat).unwrap();
    let preview = DynamicImage::ImageRgb8(img);

    // Face pictures can't be saved (a file sits where their folder goes): the
    // photo isn't looked at, and none of its faces or people are left behind.
    std::fs::write(data.root.join("faces"), b"not a folder").unwrap();
    assert!(faces.store(&cat, photo, &preview, one_face()).is_err());
    assert_eq!(count(&cat, "SELECT COUNT(*) FROM face_scan WHERE photo_id = ?1", photo), 0);
    assert_eq!(count(&cat, "SELECT COUNT(*) FROM faces WHERE photo_id = ?1", photo), 0);
    assert!(faces.people(&cat).unwrap().is_empty());

    // Looked at again: the face, its fingerprint and its picture are all there.
    std::fs::remove_file(data.root.join("faces")).unwrap();
    faces.store(&cat, photo, &preview, one_face()).unwrap();
    assert_eq!(count(&cat, "SELECT COUNT(*) FROM face_scan WHERE photo_id = ?1", photo), 1);
    let face: i64 = cat.db().query_row("SELECT id FROM faces WHERE photo_id = ?1", [photo], |r| r.get(0)).unwrap();
    let fingerprints: i64 = rusqlite::Connection::open(data.root.join("faces.db"))
        .unwrap()
        .query_row("SELECT COUNT(*) FROM emb WHERE face_id = ?1", [face], |r| r.get(0))
        .unwrap();
    assert_eq!(fingerprints, 1);
    assert!(data.face_path(face).is_file());
    assert_eq!(faces.people(&cat).unwrap().len(), 1);
}
