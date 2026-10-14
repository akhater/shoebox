//! Ignoring strangers headless: `shoebox-cli people-ignore` hides people from
//! People as the window's eye does, `--show` brings them back, and an unknown
//! id changes nobody.
//!
//! Lives here, not in src/, because it writes a test catalog (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::FaceIndex;
use shoebox_lib::paths::DataDir;
use std::process::Command;

fn ignored(cat: &Catalog) -> Vec<i64> {
    cat.db()
        .prepare("SELECT id FROM people WHERE ignored = 1 ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn strangers_are_ignored_and_shown_again_headless() {
    let base = std::env::temp_dir().join("shoebox-people-ignore");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    FaceIndex::open(&cat).unwrap();
    cat.db()
        .execute_batch(
            "INSERT INTO people (id, name, created_at) VALUES (1, NULL, 0), (2, NULL, 0), (3, 'Jane Smith', 0);",
        )
        .unwrap();
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_shoebox-cli")).args(args).env("SHOEBOX_DATA", &data.root).output().unwrap()
    };

    let out = cli(&["people-ignore", "1", "2"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let said: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(said["ignored"], true);
    assert_eq!(ignored(&cat), [1, 2]);

    assert!(cli(&["people-ignore", "2", "--show"]).status.success());
    assert_eq!(ignored(&cat), [1]);

    // An unknown id: nobody changes, not even the known one before it.
    assert!(!cli(&["people-ignore", "3", "99"]).status.success());
    assert_eq!(ignored(&cat), [1]);
    assert!(!cli(&["people-ignore"]).status.success());

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
