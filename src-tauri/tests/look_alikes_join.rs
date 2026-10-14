//! Someone found twice (another angle, other light) ends up as one person:
//! an unnamed person whose average face is the same person's joins them -
//! someone ignored stays hidden, a name wins, the smaller of two unnamed
//! joins the bigger. Named and ignored people and faces placed by hand never
//! move; two people in one photo never join. Runs after each face scan, after
//! regrouping, and once at start (headless too: any `shoebox-cli` people
//! command).
//!
//! Lives here, not in src/, because it writes a test catalog (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::FaceIndex;
use shoebox_lib::paths::DataDir;
use std::process::Command;

const DIM: usize = 512;

/// A face of `person` in `photo` whose fingerprint is `axis`, turned towards
/// `axis + 1` by `turn` (0.9: alike enough on average, 0.74, not identical).
fn face(
    cat: &Catalog,
    emb: &rusqlite::Connection,
    photo: i64,
    person: i64,
    by_hand: bool,
    axis: usize,
    turn: f32,
) -> i64 {
    cat.db()
        .execute(
            "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, seen, by_hand)
             VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9, 1, ?3)",
            rusqlite::params![photo, person, by_hand as i64],
        )
        .unwrap();
    let id = cat.db().last_insert_rowid();
    let mut v = vec![0f32; DIM];
    v[axis] = 1.0;
    v[axis + 1] = turn;
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let blob: Vec<u8> = v.iter().flat_map(|x| (x / n).to_le_bytes()).collect();
    emb.execute("INSERT INTO emb (face_id, vec) VALUES (?1, ?2)", rusqlite::params![id, blob]).unwrap();
    id
}

fn person_of(cat: &Catalog, face: i64) -> i64 {
    cat.db().query_row("SELECT person_id FROM faces WHERE id = ?1", [face], |r| r.get(0)).unwrap()
}

/// Who People shows (not ignored), by id.
fn shown(faces: &FaceIndex, cat: &Catalog) -> Vec<i64> {
    let mut ids: Vec<i64> = faces.people(cat).unwrap().into_iter().filter(|p| !p.ignored).map(|p| p.id).collect();
    ids.sort();
    ids
}

#[test]
fn someone_found_twice_becomes_one_person() {
    let base = std::env::temp_dir().join("shoebox-look-alikes-join");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    // Opened empty first, so it never regroups (or joins) the people made below.
    FaceIndex::open(&cat).unwrap();
    let emb = rusqlite::Connection::open(data.root.join("faces.db")).unwrap();
    {
        let db = cat.db();
        for photo in 1..=21 {
            db.execute(
                "INSERT INTO photos (id, fingerprint, kind, added_at) VALUES (?1, ?2, 'image', 0)",
                rusqlite::params![photo, format!("f{photo}")],
            )
            .unwrap();
        }
        db.execute_batch(
            "INSERT INTO people (id, name, ignored, created_at) VALUES
                (1, NULL, 1, 0), (2, NULL, 0, 0),
                (3, 'Jane Smith', 0, 0), (4, NULL, 0, 0),
                (5, NULL, 0, 0), (6, NULL, 0, 0),
                (7, 'Jane Doe', 0, 0), (12, NULL, 0, 0),
                (8, NULL, 0, 0), (11, NULL, 0, 0),
                (9, NULL, 0, 0), (10, NULL, 0, 0),
                (13, NULL, 0, 0);",
        )
        .unwrap();
    }
    // A stranger, ignored (1), and the same stranger from the side (2).
    let stranger = [face(&cat, &emb, 1, 1, false, 0, 0.0), face(&cat, &emb, 2, 1, false, 0, 0.0)];
    let side = [face(&cat, &emb, 3, 2, false, 0, 0.9), face(&cat, &emb, 4, 2, false, 0, 0.9)];
    // Someone named (3), and them again, unnamed (4).
    let jane = [face(&cat, &emb, 5, 3, false, 10, 0.0), face(&cat, &emb, 6, 3, false, 10, 0.0)];
    let jane_again = face(&cat, &emb, 7, 4, false, 10, 0.9);
    // Two unnamed: one face (5) and two (6).
    let small = face(&cat, &emb, 8, 5, false, 20, 0.9);
    let big = [face(&cat, &emb, 9, 6, false, 20, 0.0), face(&cat, &emb, 10, 6, false, 20, 0.0)];
    // A name wins even over more faces: one named face (7), two unnamed (12).
    let doe = face(&cat, &emb, 11, 7, false, 50, 0.0);
    let doe_again = [face(&cat, &emb, 18, 12, false, 50, 0.9), face(&cat, &emb, 19, 12, false, 50, 0.9)];
    // A face placed by hand (8) never moves; others can join it (11).
    let placed = face(&cat, &emb, 15, 8, true, 40, 0.9);
    let near_placed = [face(&cat, &emb, 16, 11, false, 40, 0.0), face(&cat, &emb, 17, 11, false, 40, 0.0)];
    // Alike, but in the same photo: two people (9, 10).
    let one = face(&cat, &emb, 13, 9, false, 30, 0.0);
    let two = [face(&cat, &emb, 13, 10, false, 30, 0.0), face(&cat, &emb, 14, 10, false, 30, 0.0)];
    // Someone else entirely (13).
    let other = face(&cat, &emb, 20, 13, false, 60, 0.0);

    let faces = FaceIndex::open(&cat).unwrap();
    assert_eq!(shown(&faces, &cat), [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
    assert_eq!(faces.join_look_alikes(&cat).unwrap(), 5);
    assert!(
        stranger.iter().chain(&side).all(|f| person_of(&cat, *f) == 1),
        "the side view is hidden with the stranger"
    );
    assert!(jane.iter().chain([&jane_again]).all(|f| person_of(&cat, *f) == 3), "found again under their name");
    assert!(big.iter().chain([&small]).all(|f| person_of(&cat, *f) == 6), "the smaller joins the bigger");
    assert!(doe_again.iter().chain([&doe]).all(|f| person_of(&cat, *f) == 7), "the name wins");
    assert!(near_placed.iter().chain([&placed]).all(|f| person_of(&cat, *f) == 8), "a hand placement stays put");
    assert_eq!([one, two[0], two[1], other].map(|f| person_of(&cat, f)), [9, 10, 10, 13]);
    assert_eq!(shown(&faces, &cat), [3, 6, 7, 8, 9, 10, 13]);
    let names: Vec<_> = faces.people(&cat).unwrap().into_iter().filter_map(|p| p.name).collect();
    assert_eq!(names.len(), 2, "names untouched");
    assert_eq!(faces.join_look_alikes(&cat).unwrap(), 0, "nothing left to join");
    drop(faces);

    // A catalog grouped before this existed: joined once, at start - headless too.
    cat.db()
        .execute_batch(
            "INSERT INTO people (id, name, created_at) VALUES (14, NULL, 0);
             DELETE FROM meta WHERE key = 'look_alikes_joined';",
        )
        .unwrap();
    let later = face(&cat, &emb, 21, 14, false, 0, 0.5);
    let out =
        Command::new(env!("CARGO_BIN_EXE_shoebox-cli")).arg("people").env("SHOEBOX_DATA", &data.root).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(person_of(&cat, later), 1, "the stranger found again stays hidden");
    let people: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(people.as_array().unwrap().iter().all(|p| p["id"] != 14));

    drop(emb);
    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
