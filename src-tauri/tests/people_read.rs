//! "New" in People, and read: a person is new until the user has been on their
//! page; a known person shows "+N" for photos the user hasn't seen them in, and
//! those photos carry a "New" tag on their page. Leaving the page (or "Mark
//! all as read") clears it. A catalog from before keeps the badges it showed.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::faces::{FaceIndex, PeopleGroup};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use std::sync::atomic::AtomicBool;

fn face(cat: &Catalog, photo: i64, person: i64) -> i64 {
    cat.db()
        .execute(
            "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score) VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9)",
            [photo, person],
        )
        .unwrap();
    cat.db().last_insert_rowid()
}

/// (new, new photos) of a person, as People shows them.
fn badge(faces: &FaceIndex, cat: &Catalog, person: i64) -> (bool, i64) {
    let p = faces.people(cat).unwrap().into_iter().find(|p| p.id == person).unwrap();
    (p.is_new, p.new_photos)
}

#[test]
fn people_are_new_until_their_page_is_seen() {
    let base = std::env::temp_dir().join("shoebox-people-read");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    for i in 0..2u8 {
        image::RgbImage::from_fn(320, 240, |x, y| image::Rgb([i * 120, (x % 256) as u8, (y % 256) as u8]))
            .save(photos.join(format!("IMG_{i}.JPG")))
            .unwrap();
    }
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let photo = |name: &str| -> i64 {
        cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
    };
    let (a, b) = (photo("IMG_0.JPG"), photo("IMG_1.JPG"));
    // Named, so opening the face index doesn't regroup them.
    cat.db().execute("INSERT INTO people (id, name, created_at) VALUES (1, 'Ana', 0), (2, 'Bob', 0)", []).unwrap();
    let old = face(&cat, a, 1);
    cat.db().execute("UPDATE faces SET seen = 1 WHERE id = ?1", [old]).unwrap();
    face(&cat, b, 1);
    face(&cat, b, 2);

    let faces = FaceIndex::open(&cat).unwrap();
    // Ana was known, and is in one photo the user hasn't seen her in; Bob is new.
    assert_eq!(badge(&faces, &cat, 1), (false, 1));
    assert_eq!(badge(&faces, &cat, 2), (true, 1));
    // On Ana's page, that photo carries the New tag.
    let ana = |new_only| Scope::Person { id: 1, new_only };
    let list = cat.list_photos(&ana(false), Sort::default(), &Filters::default()).unwrap();
    let fresh: Vec<(i64, i64)> = list.ids.iter().copied().zip(list.fresh.iter().copied()).collect();
    assert!(fresh.contains(&(a, 0)) && fresh.contains(&(b, 1)), "{fresh:?}");
    // "Only new" on her page: just that photo.
    let new = cat.list_photos(&ana(true), Sort::default(), &Filters::default()).unwrap();
    assert_eq!((new.ids, new.fresh), (vec![b], vec![1]));
    // Not on other pages.
    let all = cat.list_photos(&Scope::All, Sort::default(), &Filters::default()).unwrap();
    assert!(all.fresh.iter().all(|&f| f == 0));

    // The user leaves Ana's page: she's read, Bob isn't.
    assert_eq!(faces.mark_read(&cat, &[1]).unwrap(), 1);
    assert_eq!(badge(&faces, &cat, 1), (false, 0));
    assert_eq!(badge(&faces, &cat, 2), (true, 1));
    assert_eq!(faces.mark_all_read(&cat).unwrap(), 1);
    assert_eq!(badge(&faces, &cat, 2), (false, 0));

    // "Mark as read" on one group leaves the others new: Cleo (named), someone
    // to name (two photos), someone seen once.
    cat.db()
        .execute("INSERT INTO people (id, name, created_at) VALUES (3, 'Cleo', 0), (4, NULL, 0), (5, NULL, 0)", [])
        .unwrap();
    for (photo, person) in [(a, 3), (a, 4), (b, 4), (b, 5)] {
        face(&cat, photo, person);
    }
    let group = |person| faces.people(&cat).unwrap().into_iter().find(|p| p.id == person).unwrap().group();
    assert_eq!([group(3), group(4), group(5)], [PeopleGroup::Named, PeopleGroup::ToName, PeopleGroup::Once]);
    assert_eq!(faces.mark_group_read(&cat, PeopleGroup::ToName).unwrap(), 2);
    assert_eq!(badge(&faces, &cat, 4), (false, 0));
    assert_eq!(badge(&faces, &cat, 3), (true, 1));
    assert_eq!(badge(&faces, &cat, 5), (true, 1));
    assert_eq!(faces.mark_group_read(&cat, PeopleGroup::Named).unwrap(), 1);
    assert_eq!(badge(&faces, &cat, 3), (false, 0));
    assert_eq!(badge(&faces, &cat, 5), (true, 1));
    assert_eq!(faces.mark_all_read(&cat).unwrap(), 1);
    // Back to Ana and Bob for the upgrade below.
    cat.db().execute_batch("DELETE FROM faces WHERE person_id > 2; DELETE FROM people WHERE id > 2;").unwrap();

    // A catalog from before this: badges came from "faces after the last visit".
    // Faces after that point stay new, the rest are seen.
    drop(faces);
    cat.db()
        .execute_batch(
            "ALTER TABLE faces DROP COLUMN seen; PRAGMA user_version = 6;
             INSERT INTO meta (key, value) VALUES ('people_new_after', '1'), ('people_seen_upto', '3');",
        )
        .unwrap();
    drop(cat);
    let cat = Catalog::open(data).unwrap();
    let seen: Vec<i64> = cat
        .db()
        .prepare("SELECT seen FROM faces ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(seen, vec![1, 0, 0]);
    let leftovers: i64 =
        cat.db().query_row("SELECT COUNT(*) FROM meta WHERE key LIKE 'people_%'", [], |r| r.get(0)).unwrap();
    assert_eq!(leftovers, 0);

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
