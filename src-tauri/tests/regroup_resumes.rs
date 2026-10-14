//! Regrouping faces (new grouping rules, or asked for in People) takes every
//! untouched face out of its person, then sorts them again photo by photo.
//! Stopped halfway, it is done again at the next start, rather than leaving
//! faces with nobody.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use image::{DynamicImage, RgbImage};
use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::{Detected, FaceIndex};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use std::sync::atomic::AtomicBool;

/// One face, big enough to be grouped, with the same fingerprint every time.
fn one_face() -> Vec<(Detected, Vec<f32>)> {
    let mut v = vec![0.0; 512];
    v[0] = 1.0;
    vec![(Detected { bbox: [100.0, 60.0, 200.0, 180.0], kps: [0.0; 10], score: 0.9 }, v)]
}

fn count(cat: &Catalog, sql: &str) -> i64 {
    cat.db().query_row(sql, [], |r| r.get(0)).unwrap()
}

fn grouping(cat: &Catalog) -> Option<String> {
    cat.db().query_row("SELECT value FROM meta WHERE key = 'face_grouping'", [], |r| r.get(0)).ok()
}

#[test]
fn a_regroup_stopped_halfway_is_done_again_at_start() {
    let base = std::env::temp_dir().join("shoebox-regroup-resumes");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    let shot = |shade: u8| RgbImage::from_fn(320, 240, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]));
    for i in 0..2u8 {
        shot(40 + 120 * i).save(photos.join(format!("IMG_{i}.JPG"))).unwrap();
    }
    let img = shot(40);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let ids: Vec<i64> = cat
        .db()
        .prepare("SELECT id FROM photos ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    let faces = FaceIndex::open(&cat).unwrap();
    let preview = DynamicImage::ImageRgb8(img);
    for &photo in &ids {
        faces.store(&cat, photo, &preview, one_face()).unwrap();
    }
    assert_eq!(count(&cat, "SELECT COUNT(*) FROM faces WHERE person_id IS NULL"), 0);
    let sorted = grouping(&cat);
    assert!(sorted.is_some());

    // Stopped after the first photo is sorted again (as if Shoebox were killed):
    // the second photo's face can't join anyone.
    cat.db()
        .execute_batch(&format!(
            "CREATE TRIGGER stop BEFORE UPDATE OF person_id ON faces
               WHEN NEW.person_id IS NOT NULL AND NEW.photo_id = {}
             BEGIN SELECT RAISE(ABORT, 'stopped'); END;",
            ids[1]
        ))
        .unwrap();
    assert!(faces.regroup(&cat).is_err());
    assert_eq!(count(&cat, "SELECT COUNT(*) FROM faces WHERE person_id IS NULL"), 1);
    // Not counted as sorted: the next start sorts again.
    assert_eq!(grouping(&cat), None);

    cat.db().execute_batch("DROP TRIGGER stop;").unwrap();
    drop(faces);
    FaceIndex::open(&cat).unwrap();
    assert_eq!(count(&cat, "SELECT COUNT(*) FROM faces WHERE person_id IS NULL"), 0);
    assert_eq!(count(&cat, "SELECT COUNT(DISTINCT person_id) FROM faces"), 1);
    assert_eq!(grouping(&cat), sorted);

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
