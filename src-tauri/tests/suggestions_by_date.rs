//! "Who is this" suggestions and "Find more" compare a face with each person
//! as they looked around the date of its photo, as grouping does: to the
//! model a child's baby and school-age faces are far apart and all babies
//! look alike, so an average over a whole childhood suggests the wrong child.
//!
//! Lives here, not in src/, because it writes a test catalog (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::FaceIndex;
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

/// How alike `v` is to the average of `faces`.
fn likeness(faces: &[Vec<f32>], v: &[f32]) -> f32 {
    let mut s = vec![0f32; DIM];
    for f in faces {
        s.iter_mut().zip(f).for_each(|(a, b)| *a += b);
    }
    let n = s.iter().map(|x| x * x).sum::<f32>().sqrt();
    s.iter().zip(v).map(|(a, b)| a / n * b).sum()
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
        // Opened empty first, so it never regroups or joins the people made below.
        FaceIndex::open(&cat).unwrap();
        let emb = rusqlite::Connection::open(data.root.join("faces.db")).unwrap();
        Self { cat, emb, next_photo: 1 }
    }

    /// Someone, named or not.
    fn person(&self, name: Option<&str>) -> i64 {
        self.cat.db().execute("INSERT INTO people (name, created_at) VALUES (?1, 0)", [name]).unwrap();
        self.cat.db().last_insert_rowid()
    }

    /// A face of `person` in a photo of its own taken on `taken` (none: date
    /// unknown).
    fn face(&mut self, person: i64, taken: Option<i64>, v: &[f32]) -> i64 {
        let photo = self.next_photo;
        self.next_photo += 1;
        let db = self.cat.db();
        db.execute(
            "INSERT INTO photos (id, fingerprint, kind, taken_ts, preview_state, added_at) VALUES (?1, ?2, 'image', ?3, 1, 0)",
            rusqlite::params![photo, format!("f{photo}"), taken],
        )
        .unwrap();
        db.execute(
            "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, seen)
             VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9, 1)",
            rusqlite::params![photo, person],
        )
        .unwrap();
        let id = db.last_insert_rowid();
        let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.emb.execute("INSERT INTO emb (face_id, vec) VALUES (?1, ?2)", rusqlite::params![id, blob]).unwrap();
        id
    }

    /// Faces of `person`, one a day from `first`, each `look(i)`.
    fn faces(&mut self, person: i64, n: usize, first: (i32, u32), look: impl Fn(usize) -> Vec<f32>) -> Vec<Vec<f32>> {
        (0..n)
            .map(|i| {
                let v = look(i);
                self.face(person, Some(day(first.0, first.1, 1 + i as u32)), &v);
                v
            })
            .collect()
    }
}

// Axes: 0 = how babies look, 1 = the older brother at three, 2 = the younger
// one's own features, 3 = the younger one at three, 5 and 6 = Ana, 10.. =
// small differences between photos.
fn baby(i: usize) -> Vec<f32> {
    print(&[(0, 1.0), (10 + i, 0.25)])
}

fn newborn_younger(i: usize) -> Vec<f32> {
    print(&[(0, 1.0), (2, 0.5), (100 + i, 0.2)])
}

fn younger_at_three(i: usize) -> Vec<f32> {
    print(&[(3, 1.0), (2, 0.3), (150 + i, 0.2)])
}

/// The older brother: ten photos as a baby (2015), five at three (2018).
/// The younger one: five as a newborn (April 2018), twelve at three (2021).
fn brothers(t: &mut Test) -> (i64, i64, Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let older = t.person(Some("Older brother"));
    let mut o = t.faces(older, 10, (2015, 3), baby);
    o.extend(t.faces(older, 5, (2018, 3), |i| print(&[(1, 1.0), (30 + i, 0.25)])));
    let younger = t.person(Some("Younger brother"));
    let mut y = t.faces(younger, 5, (2018, 4), newborn_younger);
    y.extend(t.faces(younger, 12, (2021, 4), younger_at_three));
    (older, younger, o, y)
}

#[test]
fn who_is_this_suggests_the_child_as_they_looked_then() {
    let mut t = Test::new("shoebox-suggestions-by-date-who");
    let (older, younger, o, y) = brothers(&mut t);
    // The younger one as a newborn, in a group of his own.
    let face = newborn_younger(20);
    let alone = t.person(None);
    let fid = t.face(alone, Some(day(2018, 5, 10)), &face);
    // Over their whole lives, the older brother (mostly baby photos) looks
    // more like him than he does himself (mostly at three).
    assert!(likeness(&o, &face) > likeness(&y, &face) + 0.2);
    let faces = FaceIndex::open(&t.cat).unwrap();

    // Around May 2018 the older brother was three, the younger one a newborn.
    let alike = faces.look_alikes(&t.cat, &[fid], &[alone], 6).unwrap();
    assert_eq!(alike.first().map(|l| l.person_id), Some(younger));
    assert!(alike[0].similarity > 0.9);
    assert!(alike.iter().all(|l| l.person_id != older), "the older brother at three doesn't look like a newborn");

    // Several faces at once: compared over the months of their photos.
    let later = t.face(alone, Some(day(2021, 4, 20)), &younger_at_three(20));
    let alike = faces.look_alikes(&t.cat, &[fid, later], &[alone], 6).unwrap();
    assert_eq!(alike.first().map(|l| l.person_id), Some(younger));
}

#[test]
fn who_is_this_compares_a_photo_without_a_date_with_whole_lives() {
    let mut t = Test::new("shoebox-suggestions-by-date-undated");
    let (older, younger, ..) = brothers(&mut t);
    let alone = t.person(None);
    let fid = t.face(alone, None, &newborn_younger(20));
    let faces = FaceIndex::open(&t.cat).unwrap();
    // Nothing tells when it was taken: whole-life averages decide, as before.
    let alike: Vec<i64> = faces.look_alikes(&t.cat, &[fid], &[alone], 6).unwrap().iter().map(|l| l.person_id).collect();
    assert_eq!(alike, [older, younger]);
}

#[test]
fn find_more_finds_a_baby_by_the_babys_own_months() {
    let mut t = Test::new("shoebox-suggestions-by-date-more");
    // Ana, the older sister: six baby photos (2016), twelve at five (July 2021).
    let ana = t.person(Some("Ana"));
    let ana_baby = t.faces(ana, 6, (2016, 5), |i| print(&[(0, 1.0), (6, 0.3), (200 + i, 0.2)]));
    t.faces(ana, 12, (2021, 7), |i| print(&[(5, 1.0), (250 + i, 0.25)]));
    // Bea: six photos as a newborn (June 2021), fifteen at three (2024).
    let bea = t.person(Some("Bea"));
    let mut b = t.faces(bea, 6, (2021, 6), newborn_younger);
    b.extend(t.faces(bea, 15, (2024, 6), younger_at_three));
    // Two of Bea's newborn faces went to Ana, two weeks later.
    let wrong: Vec<i64> =
        (0..2).map(|i| t.face(ana, Some(day(2021, 6, 20 + i)), &newborn_younger(20 + i as usize))).collect();
    // Over her whole life Bea is mostly three: her newborn faces don't look like her.
    assert!(likeness(&b, &newborn_younger(20)) < 0.5);
    let faces = FaceIndex::open(&t.cat).unwrap();

    // Around June 2021 she was a newborn: both are offered, ticked (near her
    // own photos).
    let days = faces.more_of(&t.cat, bea).unwrap();
    let shown: Vec<(Option<&str>, bool, Vec<i64>)> =
        days.iter().map(|d| (d.day.as_deref(), d.near, d.faces.iter().map(|f| f.face_id).collect())).collect();
    assert_eq!(shown, [(Some("2021-06-20"), true, vec![wrong[0]]), (Some("2021-06-21"), true, vec![wrong[1]])]);
    assert!(days.iter().all(|d| d.faces[0].similarity > 0.9));
    // Ana's own baby faces, from years before Bea, are compared with Bea's
    // whole life: not offered.
    assert!(likeness(&b, &ana_baby[0]) < 0.5);
}
