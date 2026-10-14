//! `shoebox-cli faces-reset` (test catalogs only) forgets every face, person
//! and "looked at" mark together, or none of them.
//!
//! Lives here, not in src/, because it writes a test catalog (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::FaceIndex;
use shoebox_lib::paths::DataDir;
use std::process::Command;

fn counts(data: &DataDir) -> [i64; 3] {
    let cat = Catalog::open(data.clone()).unwrap();
    ["faces", "people", "face_scan"]
        .map(|t| cat.db().query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).unwrap())
}

#[test]
fn faces_reset_is_all_or_nothing() {
    let base = std::env::temp_dir().join("shoebox-cli-faces-reset");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    {
        let cat = Catalog::open(data.clone()).unwrap();
        FaceIndex::open(&cat).unwrap();
        cat.db()
            .execute_batch(
                "INSERT INTO photos (id, fingerprint, kind, added_at) VALUES (1, 'f1', 'image', 0);
                 INSERT INTO people (id, name, created_at) VALUES (1, 'Someone', 0);
                 INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score) VALUES (1, 1, 0, 0, 1, 1, '', 0.9);
                 INSERT INTO face_scan (photo_id, model, faces) VALUES (1, 'buffalo_l', 1);
                 -- The last deletion fails, as if Shoebox were stopped there.
                 CREATE TRIGGER stop BEFORE DELETE ON face_scan BEGIN SELECT RAISE(ABORT, 'stopped'); END;",
            )
            .unwrap();
    }
    let reset = || {
        Command::new(env!("CARGO_BIN_EXE_shoebox-cli"))
            .arg("faces-reset")
            .env("SHOEBOX_DATA", &data.root)
            .output()
            .unwrap()
            .status
            .success()
    };

    assert!(!reset());
    assert_eq!(counts(&data), [1, 1, 1], "nothing forgotten when it can't all be");

    Catalog::open(data.clone()).unwrap().db().execute_batch("DROP TRIGGER stop;").unwrap();
    assert!(reset());
    assert_eq!(counts(&data), [0, 0, 0]);

    let _ = std::fs::remove_dir_all(&base);
}
