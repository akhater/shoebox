//! Turning a photo changes only the catalog and Shoebox's own copies: the
//! preview and thumbnail turn (no disk needed), its faces keep their names
//! with their boxes turned, the photo file stays byte for byte as it was. A
//! turn cut short is finished from the files as they are, never turned twice;
//! a photo read again from its original keeps its turn. The upright model's
//! suggestions wait in Sideways until the user turns them or leaves them.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use image::{DynamicImage, GenericImageView, RgbImage};
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::faces::{Detected, FaceIndex};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use shoebox_lib::{imaging, turn, upright};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// 320x240, red in its top-left corner: where the red is says how it stands.
fn photo(path: &Path, shade: u8) -> RgbImage {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = RgbImage::from_fn(320, 240, |x, y| {
        if x < 80 && y < 60 { image::Rgb([230, 20, 20]) } else { image::Rgb([shade, (x % 200) as u8, (y % 200) as u8]) }
    });
    img.save(path).unwrap();
    img
}

fn catalog(name: &str, photos: usize) -> (Catalog, PathBuf, i64) {
    let base = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("photos");
    for i in 0..photos {
        photo(&folder.join(format!("IMG_{i}.JPG")), 40 + i as u8 * 50);
    }
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&folder).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    (cat, folder, root)
}

fn photo_of(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

fn int(cat: &Catalog, sql: &str, id: i64) -> i64 {
    cat.db().query_row(sql, [id], |r| r.get(0)).unwrap()
}

fn red(img: &DynamicImage, x: u32, y: u32) -> bool {
    let p = img.get_pixel(x, y);
    p[0] > 180 && p[1] < 80 && p[2] < 80
}

fn mark_of(path: &Path) -> u8 {
    imaging::turn_mark(&std::fs::read(path).unwrap())
}

/// The face's box, 0..1 of the preview.
fn face_box(cat: &Catalog, face: i64) -> [f64; 4] {
    cat.db()
        .query_row("SELECT x1, y1, x2, y2 FROM faces WHERE id = ?1", [face], |r| {
            Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?])
        })
        .unwrap()
}

fn near(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
}

#[test]
fn a_turn_changes_shoebox_copies_and_never_the_file() {
    let (cat, folder, _) = catalog("shoebox-turns", 1);
    let file = folder.join("IMG_0.JPG");
    let original = std::fs::read(&file).unwrap();
    let id = photo_of(&cat, "IMG_0.JPG");
    let (preview, thumb) = (cat.data.preview_path(id), cat.data.thumb_path(id));
    let rev = int(&cat, "SELECT preview_rev FROM photos WHERE id = ?1", id);

    // A named face, and a finding of the people check left as is about it.
    let faces = FaceIndex::open(&cat).unwrap();
    let mut v = vec![0.0; 512];
    v[0] = 1.0;
    let kps = [120.0, 90.0, 180.0, 90.0, 150.0, 120.0, 125.0, 150.0, 175.0, 150.0];
    let found = vec![(Detected { bbox: [100.0, 60.0, 200.0, 180.0], kps, score: 0.9 }, v)];
    faces.store(&cat, id, &image::open(&preview).unwrap(), found).unwrap();
    let face: i64 = int(&cat, "SELECT id FROM faces WHERE photo_id = ?1", id);
    let person = int(&cat, "SELECT person_id FROM faces WHERE id = ?1", face);
    faces.rename(&cat, person, "Ann").unwrap();
    cat.db()
        .execute("INSERT INTO check_left (kind, id, id2, seen, left_at) VALUES ('ageJump', ?1, 0, '{}', 0)", [face])
        .unwrap();
    let before = face_box(&cat, face);

    // A quarter turn clockwise.
    let r = turn::turn(&cat, &[(id, 1)]).unwrap();
    assert_eq!((r.turned, r.faces), (1, 1), "{r:?}");
    let t = &r.photos[0];
    assert_eq!((t.id, t.by, t.turn, t.w, t.h), (id, 1, 1, 240, 320), "the window shows it as it stands now");
    assert!(t.rev > rev);
    assert_eq!(std::fs::read(&file).unwrap(), original, "the photo file is never touched");
    let p = image::open(&preview).unwrap();
    assert_eq!(p.dimensions(), (240, 320));
    assert!(red(&p, 230, 10) && !red(&p, 10, 10), "its top-left corner is now top-right");
    assert_eq!(image::image_dimensions(&thumb).unwrap(), (240, 320));
    assert_eq!((mark_of(&preview), mark_of(&thumb)), (1, 1));
    let row: (i64, i64, i64, i64, i64, i64) = cat
        .db()
        .query_row(
            "SELECT turn, turning, sideways, thumb_w, thumb_h, preview_rev FROM photos WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!((row.0, row.1, row.2, row.3, row.4), (1, 0, -1, 240, 320), "turned, done, decided by the user");
    assert!(row.5 > rev, "a new revision: the window asks for the new pictures");
    assert_eq!(cat.photo_detail(id).unwrap().turn, 1);

    // The face: a new one (no window keeps its old picture), same person, box turned.
    let new_face = int(&cat, "SELECT id FROM faces WHERE photo_id = ?1", id);
    assert_ne!(new_face, face);
    assert_eq!(int(&cat, "SELECT person_id FROM faces WHERE id = ?1", new_face), person);
    assert!(near(face_box(&cat, new_face), [1.0 - before[3], before[0], 1.0 - before[1], before[2]]));
    assert!(cat.data.face_path(new_face).is_file() && !cat.data.face_path(face).is_file());
    let emb = rusqlite::Connection::open(cat.data.root.join("faces.db")).unwrap();
    let fingerprint =
        |f: i64| -> i64 { emb.query_row("SELECT COUNT(*) FROM emb WHERE face_id = ?1", [f], |r| r.get(0)).unwrap() };
    assert_eq!((fingerprint(new_face), fingerprint(face)), (1, 0));
    assert_eq!(int(&cat, "SELECT COUNT(*) FROM check_left WHERE id = ?1", new_face), 1, "left as is follows the face");
    assert_eq!(int(&cat, "SELECT COUNT(*) FROM face_scan WHERE photo_id = ?1", id), 0, "looked at again for faces");
    let ann = faces.people(&cat).unwrap().into_iter().find(|p| p.id == person).unwrap();
    assert_eq!((ann.name.as_deref(), ann.faces), (Some("Ann"), 1));

    // What word search and Cull keep is worked out again for the turned picture.
    let fp: String = cat.db().query_row("SELECT fingerprint FROM photos WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert!(cat.preview_ready().unwrap().contains(&(id, format!("{fp}@1"))));

    // Back the other way: as it was, the files without a turn comment again.
    turn::turn(&cat, &[(id, -1)]).unwrap();
    let p = image::open(&preview).unwrap();
    assert_eq!(p.dimensions(), (320, 240));
    assert!(red(&p, 10, 10));
    assert_eq!((mark_of(&preview), mark_of(&thumb)), (0, 0));
    let face = int(&cat, "SELECT id FROM faces WHERE photo_id = ?1", id);
    assert!(near(face_box(&cat, face), before));
    assert!(cat.preview_ready().unwrap().contains(&(id, fp.clone())));

    // Upside down, then back to how the file shows it.
    turn::turn(&cat, &[(id, 2)]).unwrap();
    assert!(red(&image::open(&preview).unwrap(), 310, 230));
    let r = turn::back(&cat, &[id]).unwrap();
    assert_eq!(r.turned, 1);
    assert_eq!(int(&cat, "SELECT turn FROM photos WHERE id = ?1", id), 0);
    assert!(red(&image::open(&preview).unwrap(), 10, 10));
    assert_eq!(std::fs::read(&file).unwrap(), original);
}

#[test]
fn a_turn_cut_short_is_finished_once() {
    let (cat, _, _) = catalog("shoebox-turn-cut-short", 2);
    let (a, b) = (photo_of(&cat, "IMG_0.JPG"), photo_of(&cat, "IMG_1.JPG"));

    // Stopped right after the catalog took the turn: neither file turned yet.
    cat.db().execute("UPDATE photos SET turn = 1, turning = 1 WHERE id = ?1", [a]).unwrap();
    // Stopped after the preview was turned, before the thumbnail.
    let preview = std::fs::read(cat.data.preview_path(b)).unwrap();
    let (turned, ..) = imaging::turn_jpeg(&preview, 1, 84).unwrap().unwrap();
    cat.data.write(&cat.data.preview_path(b), &turned).unwrap();
    cat.db().execute("UPDATE photos SET turn = 1, turning = 1 WHERE id = ?1", [b]).unwrap();

    assert_eq!(turn::settle(&cat).unwrap(), 2);
    for id in [a, b] {
        let p = image::open(cat.data.preview_path(id)).unwrap();
        assert_eq!(p.dimensions(), (240, 320), "photo {id}: turned once, not twice");
        assert!(red(&p, 230, 10));
        assert_eq!(image::image_dimensions(cat.data.thumb_path(id)).unwrap(), (240, 320));
        assert_eq!(int(&cat, "SELECT turning FROM photos WHERE id = ?1", id), 0);
        assert_eq!(int(&cat, "SELECT thumb_h FROM photos WHERE id = ?1", id), 320);
    }
    assert_eq!(turn::settle(&cat).unwrap(), 0, "nothing left to finish");
}

#[test]
fn a_photo_read_again_keeps_its_turn() {
    let (cat, _, root) = catalog("shoebox-turn-read-again", 1);
    let id = photo_of(&cat, "IMG_0.JPG");
    turn::turn(&cat, &[(id, 3)]).unwrap();
    // Previews rebuilt from the originals (Settings > Rebuild previews).
    scan_root(&cat, root, &AtomicBool::new(false), true, &|_| {}).unwrap();
    let p = image::open(cat.data.preview_path(id)).unwrap();
    assert_eq!(p.dimensions(), (240, 320));
    assert!(red(&p, 10, 310), "a quarter turn left: its top-left corner is bottom-left");
    assert_eq!((mark_of(&cat.data.preview_path(id)), mark_of(&cat.data.thumb_path(id))), (3, 3));
    assert_eq!(int(&cat, "SELECT turn FROM photos WHERE id = ?1", id), 3);
}

#[test]
fn sideways_photos_wait_for_the_user() {
    let (cat, _, _) = catalog("shoebox-sideways", 3);
    let [a, b, c] = ["IMG_0.JPG", "IMG_1.JPG", "IMG_2.JPG"].map(|n| photo_of(&cat, n));
    // What the model found: a sideways (a quarter turn right fixes it), b upright, c upside down.
    for (id, turn, sure) in [(a, 1, 0.92), (b, 0, 0.91), (c, 2, 0.93)] {
        cat.db()
            .execute(
                "UPDATE photos SET sideways = ?1, sideways_p = ?2 WHERE id = ?3",
                rusqlite::params![turn, sure, id],
            )
            .unwrap();
    }
    let sideways = |cat: &Catalog| {
        let mut ids = cat.list_photos(&Scope::Sideways, Sort::default(), &Filters::default()).unwrap().ids;
        ids.sort();
        ids
    };
    let mut both = vec![a, c];
    both.sort();
    assert_eq!(sideways(&cat), both);
    assert_eq!(cat.overview().unwrap().sideways, 2);
    let list = cat.list_photos(&Scope::All, Sort::default(), &Filters::default()).unwrap();
    let suggested = |id: i64| list.sideways[list.ids.iter().position(|x| *x == id).unwrap()];
    assert_eq!((suggested(a), suggested(b), suggested(c)), (1, 0, 2));
    assert_eq!(upright::suggestions(&cat).unwrap().first().map(|s| s.id), Some(c), "the surest first");

    // Turned as suggested: it leaves Sideways, turned, never suggested again.
    upright::accept(&cat, &[a]).unwrap();
    assert_eq!(sideways(&cat), vec![c]);
    assert_eq!(int(&cat, "SELECT turn FROM photos WHERE id = ?1", a), 1);
    assert!(red(&image::open(cat.data.preview_path(a)).unwrap(), 230, 10));
    // Fine as it is: it leaves Sideways, not turned.
    assert_eq!(upright::leave(&cat, &[c]).unwrap(), 1);
    assert!(sideways(&cat).is_empty());
    assert_eq!(int(&cat, "SELECT turn FROM photos WHERE id = ?1", c), 0);

    // Looked at again (the model changed): only what nobody decided about.
    assert_eq!(upright::look_again(&cat).unwrap(), 1);
    let unchecked: i64 =
        cat.db().query_row("SELECT COUNT(*) FROM photos WHERE sideways IS NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(unchecked, 1, "b only: a and c were decided by the user");
}
