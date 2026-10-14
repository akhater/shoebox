//! "Who is this?": the user moves one photo's face to someone else (the split to
//! merge's join). The face is placed by hand and never regrouped back; a
//! person stays once per photo, so a face already theirs there makes room.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::{FaceIndex, FaceTarget};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use std::sync::atomic::AtomicBool;

const DIM: usize = 512;

/// A face of `person` in `photo` whose fingerprint points along `axis`.
fn face(cat: &Catalog, emb: &rusqlite::Connection, photo: i64, person: i64, axis: usize) -> i64 {
    cat.db()
        .execute(
            "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, seen) VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9, 1)",
            [photo, person],
        )
        .unwrap();
    let id = cat.db().last_insert_rowid();
    let mut v = vec![0f32; DIM];
    v[axis] = 1.0;
    let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    emb.execute("INSERT INTO emb (face_id, vec) VALUES (?1, ?2)", rusqlite::params![id, blob]).unwrap();
    id
}

/// (person, by hand, seen) of a face.
fn where_is(cat: &Catalog, face: i64) -> (i64, bool, bool) {
    cat.db()
        .query_row("SELECT person_id, by_hand, seen FROM faces WHERE id = ?1", [face], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get::<_, i64>(2)? != 0))
        })
        .unwrap()
}

#[test]
fn a_face_goes_where_ak_says_and_stays_there() {
    let base = std::env::temp_dir().join("shoebox-reassign-faces");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    for i in 0..5u8 {
        image::RgbImage::from_fn(320, 240, |x, y| image::Rgb([i * 50, (x % 256) as u8, (y % 256) as u8]))
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
    let (a, b, c, d, e) = (photo(0), photo(1), photo(2), photo(3), photo(4));
    // Named, so opening the face index doesn't regroup them.
    cat.db()
        .execute("INSERT INTO people (id, name, created_at) VALUES (1, 'Ana', 0), (2, 'Bob', 0), (3, 'Cy', 0)", [])
        .unwrap();
    let emb = rusqlite::Connection::open(data.root.join("faces.db")).unwrap();
    emb.execute_batch("CREATE TABLE IF NOT EXISTS emb (face_id INTEGER PRIMARY KEY, vec BLOB NOT NULL);").unwrap();
    // Photo A: two babies, swapped. x looks like Bob but is in Ana; y the other way.
    let x = face(&cat, &emb, a, 1, 1);
    let y = face(&cat, &emb, a, 2, 0);
    let ana_b = face(&cat, &emb, b, 1, 0);
    let bob_b = face(&cat, &emb, b, 2, 1);
    let ana_c = face(&cat, &emb, c, 1, 0);
    let cy_c = face(&cat, &emb, c, 3, 2);
    // Faces in Ana that are really Cy (z), and someone Shoebox has never seen (w).
    let z = face(&cat, &emb, d, 1, 2);
    let w = face(&cat, &emb, e, 1, 3);
    drop(emb);
    let faces = FaceIndex::open(&cat).unwrap();

    // Clean up offers the three faces unlike Ana; "all faces" shows her five.
    let mut strays: Vec<i64> = faces.strays(&cat, 1).unwrap().iter().map(|s| s.face_id).collect();
    strays.sort();
    assert_eq!(strays, [x, z, w]);
    let all = faces.person_faces(&cat, 1).unwrap();
    assert_eq!(all.len(), 5);
    assert!(all.windows(2).all(|p| p[0].similarity <= p[1].similarity), "least alike first");

    // z looks like Cy (Ana herself not suggested).
    let alike = faces.look_alikes(&[z], &[1], 6).unwrap();
    assert_eq!(alike.first().map(|l| l.person_id), Some(3));

    // "x is Bob": Bob was already in photo A (y), so y makes room - not back
    // into Bob, and it looks like Ana, who just left the photo: the swap.
    let moved = faces.reassign(&cat, &[x], &FaceTarget::Person { id: 2 }).unwrap();
    assert_eq!((moved.faces, moved.people.as_slice(), moved.bumped.as_slice()), (1, &[2][..], &[1][..]));
    assert_eq!(where_is(&cat, x), (2, true, true));
    assert_eq!(where_is(&cat, y), (1, false, false), "Shoebox's guess shows up as new, to check");

    // Undo puts both back exactly, and the averages follow: y still looks
    // like Ana, x like Bob, after a round trip.
    assert_eq!(faces.restore(&cat, &moved.undo).unwrap(), 2);
    assert_eq!(where_is(&cat, x), (1, false, true));
    assert_eq!(where_is(&cat, y), (2, false, true));
    let again = faces.reassign(&cat, &[x], &FaceTarget::Person { id: 2 }).unwrap();
    assert_eq!(again.bumped, [1]);
    assert_eq!(where_is(&cat, y), (1, false, false));

    // Already them: nothing moves, but it is the user's word now.
    let moved = faces.reassign(&cat, &[ana_b], &FaceTarget::Person { id: 1 }).unwrap();
    assert_eq!(moved.faces, 0);
    assert_eq!(where_is(&cat, ana_b), (1, true, true));

    // "Not Ana, don't know who": z joins Cy, whom it resembles; w becomes
    // someone new. Neither can land back in Ana.
    let moved = faces.reassign(&cat, &[z, w], &FaceTarget::Elsewhere).unwrap();
    assert_eq!(moved.faces, 2);
    assert_eq!(where_is(&cat, z), (3, true, false));
    let (w_person, w_hand, _) = where_is(&cat, w);
    assert!(w_hand && ![1, 2, 3].contains(&w_person), "someone new, not {w_person}");
    let no_name: Option<String> =
        cat.db().query_row("SELECT name FROM people WHERE id = ?1", [w_person], |r| r.get(0)).unwrap();
    assert_eq!(no_name, None);

    // Regrouping never undoes the user's word.
    faces.regroup(&cat).unwrap();
    assert_eq!(where_is(&cat, x).0, 2);
    assert_eq!(where_is(&cat, z).0, 3);
    assert_eq!(where_is(&cat, w).0, w_person);

    // Someone new, named: one person for every face moved.
    let moved = faces.reassign(&cat, &[cy_c, bob_b], &FaceTarget::New { name: Some("  Dee ".into()) }).unwrap();
    assert_eq!(moved.faces, 2);
    let dee = moved.people[0];
    assert_eq!(moved.people, [dee]);
    assert_eq!(where_is(&cat, cy_c), (dee, true, true));
    assert_eq!(where_is(&cat, bob_b), (dee, true, true));
    let name: Option<String> =
        cat.db().query_row("SELECT name FROM people WHERE id = ?1", [dee], |r| r.get(0)).unwrap();
    assert_eq!(name.as_deref(), Some("Dee"));

    // The photos' faces, and who each is.
    let in_a = faces.faces_in(&cat, &[a, c]).unwrap();
    let who: Vec<(i64, i64, Option<i64>)> = in_a.iter().map(|f| (f.id, f.photo_id, f.person_id)).collect();
    assert!(who.contains(&(x, a, Some(2))) && who.contains(&(y, a, Some(1))) && who.contains(&(ana_c, c, Some(1))));

    // Someone who isn't there.
    assert!(faces.reassign(&cat, &[x], &FaceTarget::Person { id: 999 }).is_err());
    assert_eq!(where_is(&cat, x).0, 2);

    drop(faces);
    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
