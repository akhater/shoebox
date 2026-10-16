//! "Find more": faces in other people that look more like someone than like
//! the person they're in - placed by hand too, since nothing moves until the
//! user says so. Grouped by day, the days near the person's own photos first
//! and ticked: to the model babies look alike, a person looks like themselves
//! around the same time.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::{FaceIndex, FaceTarget};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use std::sync::atomic::AtomicBool;

const DIM: usize = 512;
const DAY: i64 = 86_400;

/// A face of `person` in `photo` whose fingerprint is `a` along axis 0 (the
/// older child) and `b` along axis 1 (the newborn), `c` along axis 2.
fn face(cat: &Catalog, emb: &rusqlite::Connection, photo: i64, person: i64, by_hand: bool, abc: [f32; 3]) -> i64 {
    cat.db()
        .execute(
            "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, seen, by_hand)
             VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9, 1, ?3)",
            rusqlite::params![photo, person, by_hand as i64],
        )
        .unwrap();
    let id = cat.db().last_insert_rowid();
    let norm = abc.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mut v = vec![0f32; DIM];
    v[..3].copy_from_slice(&abc.map(|x| x / norm));
    let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    emb.execute("INSERT INTO emb (face_id, vec) VALUES (?1, ?2)", rusqlite::params![id, blob]).unwrap();
    id
}

#[test]
fn faces_that_look_more_like_someone_are_offered_by_day() {
    let base = std::env::temp_dir().join("shoebox-more-faces");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    for i in 0..9u8 {
        image::RgbImage::from_fn(320, 240, |x, y| image::Rgb([i * 25, (x % 256) as u8, (y % 256) as u8]))
            .save(photos.join(format!("IMG_{i}.JPG")))
            .unwrap();
    }
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let photo = |i: u8| -> i64 {
        cat.db()
            .query_row("SELECT photo_id FROM files WHERE name = ?1", [format!("IMG_{i}.JPG")], |r| r.get(0))
            .unwrap()
    };
    let p: Vec<i64> = (0..9).map(photo).collect();
    // The newborn's day, two days later, five years before (the older child's birth), no date.
    let born = 1_623_283_200; // 2021-06-10
    for (i, ts) in [born, born, born + 2 * DAY, born + 2 * DAY, born + 2 * DAY, born - 1_555 * DAY, born + 400 * DAY]
        .into_iter()
        .enumerate()
    {
        cat.db().execute("UPDATE photos SET taken_ts = ?1 WHERE id = ?2", [ts, p[i]]).unwrap();
    }
    cat.db().execute("UPDATE photos SET taken_ts = NULL WHERE id = ?1", [p[7]]).unwrap();
    // Named, so opening the face index doesn't regroup them; 4 has one face only.
    cat.db()
        .execute(
            "INSERT INTO people (id, name, created_at) VALUES (1, 'Ana', 0), (2, 'Bea', 0), (3, 'Cy', 0), (4, NULL, 0)",
            [],
        )
        .unwrap();
    let emb = rusqlite::Connection::open(data.root.join("faces.db")).unwrap();
    emb.execute_batch("CREATE TABLE IF NOT EXISTS emb (face_id INTEGER PRIMARY KEY, vec BLOB NOT NULL);").unwrap();
    // Bea, on the day she was born.
    face(&cat, &emb, p[0], 2, true, [0.0, 1.0, 0.0]);
    face(&cat, &emb, p[1], 2, true, [0.1, 1.0, 0.0]);
    // Ana, the older child, in many photos.
    for &ph in &p[1..=6] {
        face(&cat, &emb, ph, 1, false, [1.0, 0.0, 0.0]);
    }
    // Two days later: newborn faces put in Ana by hand - they are Bea.
    let wrong_a = face(&cat, &emb, p[2], 1, true, [0.3, 1.0, 0.0]);
    let wrong_b = face(&cat, &emb, p[3], 1, true, [0.2, 1.0, 0.0]);
    // A "not Ana" face left alone in its own person.
    let alone = face(&cat, &emb, p[4], 4, true, [0.0, 1.0, 0.05]);
    // Ana as a newborn, four years before: looks like Bea too.
    let baby_ana = face(&cat, &emb, p[5], 1, true, [0.4, 1.0, 0.0]);
    // Someone else entirely, and a face only a little like Bea.
    face(&cat, &emb, p[6], 3, false, [0.0, 0.0, 1.0]);
    let little = face(&cat, &emb, p[7], 3, false, [0.0, 0.4, 1.0]);
    // A newborn face in a photo with no date.
    let undated = face(&cat, &emb, p[7], 1, false, [0.1, 1.0, 0.0]);
    // In a photo Bea is already in: never offered, however alike.
    let with_her = face(&cat, &emb, p[1], 3, false, [0.0, 1.0, 0.3]);
    face(&cat, &emb, p[8], 3, false, [0.0, 0.0, 1.0]);
    drop(emb);
    let faces = FaceIndex::open(&cat).unwrap();

    let days = faces.more_of(&cat, 2).unwrap();
    let shown: Vec<(Option<&str>, bool, Vec<i64>)> =
        days.iter().map(|d| (d.day.as_deref(), d.near, d.faces.iter().map(|f| f.face_id).collect())).collect();
    assert_eq!(
        shown,
        [
            // Near her own photos: first, ticked, the most alike first.
            (Some("2021-06-12"), true, vec![alone, wrong_b, wrong_a]),
            // Far from them, or when unknown: offered, not ticked.
            (None, false, vec![undated]),
            (Some("2017-03-08"), false, vec![baby_ana]),
        ]
    );
    assert!(![little, with_her].iter().any(|f| days.iter().any(|d| d.faces.iter().any(|c| c.face_id == *f))));
    let first = &days[0].faces[0];
    assert_eq!((first.person_id, first.by_hand, first.now), (4, true, None), "alone in its person: nothing to compare");
    let wa = days[0].faces.iter().find(|f| f.face_id == wrong_a).unwrap();
    assert!(wa.now.unwrap() < wa.similarity && wa.similarity >= 0.5);

    // Moved, as the window and `more-faces --apply` do: the near ones only.
    let ids: Vec<i64> = days.iter().filter(|d| d.near).flat_map(|d| d.faces.iter().map(|f| f.face_id)).collect();
    let moved = faces.reassign(&cat, &ids, &FaceTarget::Person { id: 2 }).unwrap();
    assert_eq!((moved.faces, moved.bumped.len()), (3, 0));
    let in_bea: i64 =
        cat.db().query_row("SELECT COUNT(*) FROM faces WHERE person_id = 2 AND by_hand = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(in_bea, 5);
    // Asked again: only the faces far off (or undated) are left.
    let again = faces.more_of(&cat, 2).unwrap();
    assert_eq!(again.iter().flat_map(|d| d.faces.iter().map(|f| f.face_id)).collect::<Vec<_>>(), [undated, baby_ana]);
    // Someone with no faces has nobody to look like.
    assert!(faces.more_of(&cat, 999).unwrap().is_empty());
}
