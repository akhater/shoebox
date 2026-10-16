//! A new face is compared with each person as they looked around the date of
//! its photo, not only with their whole-life average. To the model a child's
//! baby and school-age faces are far apart and all babies look alike: one
//! average over a childhood draws in a newborn brother, the faces from the
//! same months don't. In a photo with several faces, the most alike pairs of
//! face and person go first.
//!
//! Lives here, not in src/, because it writes a test catalog (promise 1).
#![cfg(windows)]

use image::{DynamicImage, RgbImage};
use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::{Detected, FaceIndex};
use shoebox_lib::paths::DataDir;

const DIM: usize = 512;

/// A unit fingerprint from (axis, weight) pairs.
fn print(parts: &[(usize, f32)]) -> Vec<f32> {
    let mut v = vec![0f32; DIM];
    for &(axis, w) in parts {
        v[axis] += w;
    }
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / n).collect()
}

/// Seconds since 1970 of a day, read as UTC (as photos' dates are kept).
fn day(y: i32, m: u32, d: u32) -> i64 {
    chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp()
}

struct Test {
    cat: Catalog,
    emb: rusqlite::Connection,
    next_photo: i64,
}

impl Test {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&base);
        let data = DataDir { root: base.join("catalog") };
        data.ensure().unwrap();
        let cat = Catalog::open(data.clone()).unwrap();
        // Opened empty first, so it never regroups the people made below.
        FaceIndex::open(&cat).unwrap();
        let emb = rusqlite::Connection::open(data.root.join("faces.db")).unwrap();
        Self { cat, emb, next_photo: 1 }
    }

    /// A photo taken on `taken` (none: date unknown).
    fn photo(&mut self, taken: Option<i64>) -> i64 {
        let id = self.next_photo;
        self.next_photo += 1;
        self.cat
            .db()
            .execute(
                "INSERT INTO photos (id, fingerprint, kind, taken_ts, preview_state, added_at) VALUES (?1, ?2, 'image', ?3, 1, 0)",
                rusqlite::params![id, format!("f{id}"), taken],
            )
            .unwrap();
        id
    }

    fn person(&self, name: &str) -> i64 {
        self.cat.db().execute("INSERT INTO people (name, created_at) VALUES (?1, 0)", [name]).unwrap();
        self.cat.db().last_insert_rowid()
    }

    /// A face of `person` already in the catalog, in a photo of its own.
    fn known(&mut self, person: i64, taken: i64, v: &[f32]) {
        let photo = self.photo(Some(taken));
        self.cat
            .db()
            .execute(
                "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, seen)
                 VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9, 1)",
                rusqlite::params![photo, person],
            )
            .unwrap();
        let id = self.cat.db().last_insert_rowid();
        let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.emb.execute("INSERT INTO emb (face_id, vec) VALUES (?1, ?2)", rusqlite::params![id, blob]).unwrap();
    }

    /// A new photo's faces, found and grouped as a scan does: returns each
    /// face's person.
    fn found(&mut self, faces: &FaceIndex, taken: Option<i64>, prints: &[Vec<f32>]) -> Vec<i64> {
        let photo = self.photo(taken);
        let img = DynamicImage::ImageRgb8(RgbImage::new(640, 480));
        let found: Vec<(Detected, Vec<f32>)> = prints
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = 20.0 + 150.0 * i as f32;
                (Detected { bbox: [x, 60.0, x + 100.0, 180.0], kps: [0.0; 10], score: 0.9 - 0.1 * i as f32 }, v.clone())
            })
            .collect();
        faces.store(&self.cat, photo, &img, found).unwrap();
        self.people_in(photo)
    }

    /// Who each face of a photo is, left to right.
    fn people_in(&self, photo: i64) -> Vec<i64> {
        self.cat
            .db()
            .prepare("SELECT person_id FROM faces WHERE photo_id = ?1 ORDER BY x1")
            .unwrap()
            .query_map([photo], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn faces_of(&self, person: i64) -> i64 {
        self.cat.db().query_row("SELECT COUNT(*) FROM faces WHERE person_id = ?1", [person], |r| r.get(0)).unwrap()
    }
}

// Axes: 0 = how babies look, 1 = the older brother at three, 2 = the younger
// one's own features, 3.. = small differences between photos.
fn baby(extra: usize) -> Vec<f32> {
    print(&[(0, 1.0), (10 + extra, 0.25)])
}

fn brother_at_three(extra: usize) -> Vec<f32> {
    print(&[(1, 1.0), (30 + extra, 0.25)])
}

fn newborn_brother() -> Vec<f32> {
    print(&[(0, 1.0), (2, 0.5)])
}

/// The older brother: six photos as a baby (2015), six at three (2018).
fn older_brother(t: &mut Test) -> i64 {
    let brother = t.person("Older brother");
    for i in 0..6 {
        t.known(brother, day(2015, 3, 1 + i as u32), &baby(i));
        t.known(brother, day(2018, 3, 1 + i as u32), &brother_at_three(i));
    }
    brother
}

#[test]
fn a_newborn_does_not_join_his_older_brother() {
    let mut t = Test::new("shoebox-faces-by-date-newborn");
    let brother = older_brother(&mut t);
    let faces = FaceIndex::open(&t.cat).unwrap();

    // His whole-life average is half baby: the newborn would look like him.
    let newborn = newborn_brother();
    let life = {
        let mut s = vec![0f32; DIM];
        for i in 0..6 {
            s.iter_mut().zip(baby(i)).for_each(|(a, b)| *a += b);
            s.iter_mut().zip(brother_at_three(i)).for_each(|(a, b)| *a += b);
        }
        let n = s.iter().map(|x| x * x).sum::<f32>().sqrt();
        s.iter().zip(&newborn).map(|(a, b)| a / n * b).sum::<f32>()
    };
    assert!(life >= 0.5, "the newborn is {life} like the brother's whole-life average");

    // A month after the brother's photos at three: the brother joins himself,
    // the newborn is someone new.
    let got = t.found(&faces, Some(day(2018, 4, 15)), &[newborn, brother_at_three(7)]);
    let photo = t.next_photo - 1;
    let newborn_is = got[0];
    assert_eq!(got[1], brother);
    assert_ne!(newborn_is, brother);

    // Joining people found twice doesn't bring him back to his brother...
    assert_eq!(faces.join_look_alikes(&t.cat).unwrap(), 0);
    assert_eq!(t.people_in(photo), got);

    // ...and more photos of the newborn that month join him, not the brother.
    let next = t.found(&faces, Some(day(2018, 4, 20)), &[print(&[(0, 1.0), (2, 0.45), (40, 0.2)])]);
    assert_eq!(next, [newborn_is]);

    // Sorted again (regrouping), the brother still has only his own 13 faces.
    faces.regroup(&t.cat).unwrap();
    assert_eq!(t.faces_of(brother), 13);
    assert_ne!(t.people_in(photo)[0], brother);
}

#[test]
fn in_a_photo_the_most_alike_face_and_person_go_together_first() {
    let mut t = Test::new("shoebox-faces-by-date-pairs");
    let (p, q) = (t.person("P"), t.person("Q"));
    for i in 0..6 {
        t.known(p, day(2020, 6, 1 + i), &print(&[(3, 1.0), (50 + i as usize, 0.1)]));
        t.known(q, day(2020, 6, 1 + i), &print(&[(4, 1.0), (60 + i as usize, 0.1)]));
    }
    let faces = FaceIndex::open(&t.cat).unwrap();
    // The first face is 0.7 like P and 0.6 like Q; the second, 0.9 like P.
    // First come, first served would give P the first face and leave the
    // second with nobody.
    let first = print(&[(3, 0.7), (4, 0.6), (5, 0.387)]);
    let second = print(&[(3, 0.9), (4, 0.1), (6, 0.424)]);
    let got = t.found(&faces, Some(day(2020, 6, 20)), &[first, second]);
    assert_eq!(got, [q, p]);
}

#[test]
fn a_photo_without_a_date_is_compared_with_whole_lives() {
    let mut t = Test::new("shoebox-faces-by-date-undated");
    let brother = older_brother(&mut t);
    let faces = FaceIndex::open(&t.cat).unwrap();
    // Nothing tells when it was taken: the whole-life average decides, as before.
    let got = t.found(&faces, None, &[newborn_brother()]);
    assert_eq!(got, [brother]);
}

#[test]
fn someone_not_photographed_for_years_is_still_found() {
    let mut t = Test::new("shoebox-faces-by-date-years-later");
    let friend = t.person("Friend");
    for i in 0..6 {
        t.known(friend, day(2010, 5, 1 + i), &print(&[(7, 1.0), (70 + i as usize, 0.3)]));
    }
    let faces = FaceIndex::open(&t.cat).unwrap();
    // Nothing of them within a year of the photo: their whole-life average decides.
    let got = t.found(&faces, Some(day(2018, 9, 1)), &[print(&[(7, 1.0), (80, 0.4)])]);
    assert_eq!(got, [friend]);
}

#[test]
fn a_child_is_compared_with_the_nearest_months_holding_enough_faces() {
    let mut t = Test::new("shoebox-faces-by-date-nearest");
    let child = t.person("Child");
    // Six photos as a baby, then six at two, nine months before the new photo.
    for i in 0..6 {
        t.known(child, day(2016, 1, 1 + i as u32), &baby(i));
        t.known(child, day(2017, 12, 1 + i as u32), &print(&[(8, 1.0), (90 + i, 0.25)]));
    }
    let faces = FaceIndex::open(&t.cat).unwrap();
    // A face like the child at two joins them; a baby face, though like half
    // of the child's whole life, doesn't.
    let at_two = t.found(&faces, Some(day(2018, 9, 1)), &[print(&[(8, 1.0), (99, 0.3)])]);
    assert_eq!(at_two, [child]);
    let baby_then = t.found(&faces, Some(day(2018, 9, 2)), &[print(&[(0, 1.0), (2, 0.5)])]);
    assert_ne!(baby_then, [child]);
}
