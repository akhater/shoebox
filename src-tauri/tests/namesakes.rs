//! Two people can share a name. Naming someone, or saying who a face is by a
//! name already taken, never joins them to the person called that: only
//! "Same person" (a merge) does. Headless too: `shoebox-cli person-rename`
//! and `people-merge`.
//!
//! Lives here, not in src/, because it writes a test catalog (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::{FaceIndex, FaceTarget};
use shoebox_lib::paths::DataDir;
use std::process::Command;

/// Everyone called `name`, by id.
fn called(faces: &FaceIndex, cat: &Catalog, name: &str) -> Vec<i64> {
    let mut ids: Vec<i64> =
        faces.people(cat).unwrap().into_iter().filter(|p| p.name.as_deref() == Some(name)).map(|p| p.id).collect();
    ids.sort();
    ids
}

fn person_of(cat: &Catalog, face: i64) -> i64 {
    cat.db().query_row("SELECT person_id FROM faces WHERE id = ?1", [face], |r| r.get(0)).unwrap()
}

#[test]
fn people_sharing_a_name_stay_apart_until_merged() {
    let base = std::env::temp_dir().join("shoebox-namesakes");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    // Opened empty first, so it never regroups the people made below.
    FaceIndex::open(&cat).unwrap();
    cat.db()
        .execute_batch(
            "INSERT INTO photos (id, fingerprint, kind, added_at) VALUES
                (1, 'f1', 'image', 0), (2, 'f2', 'image', 0), (3, 'f3', 'image', 0), (4, 'f4', 'image', 0);
             INSERT INTO people (id, name, created_at) VALUES (1, 'Jane Smith', 0), (2, NULL, 0), (3, NULL, 0);
             INSERT INTO faces (id, photo_id, person_id, x1, y1, x2, y2, kps, score, seen) VALUES
                (11, 1, 1, 0, 0, 1, 1, '', 0.9, 1),
                (12, 2, 2, 0, 0, 1, 1, '', 0.9, 1),
                (13, 3, 3, 0, 0, 1, 1, '', 0.9, 1),
                (14, 4, 3, 0, 0, 1, 1, '', 0.9, 1);",
        )
        .unwrap();
    let faces = FaceIndex::open(&cat).unwrap();

    // Naming someone with a name already taken: two people called Jane Smith.
    faces.rename(&cat, 2, "Jane Smith").unwrap();
    assert_eq!(called(&faces, &cat, "Jane Smith"), [1, 2]);
    assert_eq!(faces.find_people(&cat, "jane smith").unwrap().len(), 2, "both found by their name");

    // "Who is this?" by that name: someone new, a third Jane Smith.
    let moved = faces.reassign(&cat, &[14], &FaceTarget::New { name: Some("Jane Smith".into()) }).unwrap();
    let third = moved.people[0];
    assert!(![1, 2, 3].contains(&third));
    assert_eq!(called(&faces, &cat, "Jane Smith"), [1, 2, third]);
    assert_eq!((person_of(&cat, 11), person_of(&cat, 12)), (1, 2), "nobody else's faces moved");

    // Headless naming keeps them apart too.
    let cli = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_shoebox-cli"))
            .args(args)
            .env("SHOEBOX_DATA", &data.root)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    cli(&["person-rename", "3", "Jane Smith"]);
    let faces = FaceIndex::open(&cat).unwrap();
    assert_eq!(called(&faces, &cat, "Jane Smith"), [1, 2, 3, third]);

    // "Same person" is what makes two of them one.
    cli(&["people-merge", "2", "--into", "1"]);
    let faces = FaceIndex::open(&cat).unwrap();
    assert_eq!(called(&faces, &cat, "Jane Smith"), [1, 3, third]);
    assert_eq!(person_of(&cat, 12), 1);
    faces.merge(&cat, &[3], third).unwrap();
    assert_eq!(called(&faces, &cat, "Jane Smith"), [1, third]);
    assert_eq!(person_of(&cat, 13), third);

    // Nobody by that id: nothing named, nothing merged into thin air.
    assert!(faces.rename(&cat, 99, "Jane Smith").is_err());
    assert!(faces.merge(&cat, &[1], 99).is_err());
    assert_eq!(person_of(&cat, 11), 1);

    drop(faces);
    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
