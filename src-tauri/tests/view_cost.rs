//! A view costs what it shows, not the size of the catalog: a small folder, a
//! month or a person's page take a fraction of the time of "All photos", for
//! the photos and for the filters' counts. Compared with each other on the same
//! machine, so it holds on a slow one too.
//!
//! Lives here, not in src/, because it makes a throwaway catalog folder: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use rusqlite::params;
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::paths::DataDir;
use std::time::{Duration, Instant};

const PHOTOS: i64 = 20_000;

/// The quickest of a few runs, so a pause of the machine doesn't count. Each
/// is worked out: the answers kept from the last one are forgotten first.
fn quickest(cat: &Catalog, f: impl Fn()) -> Duration {
    (0..5)
        .map(|_| {
            cat.forget_kept();
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
fn a_view_costs_what_it_shows() {
    let base = std::env::temp_dir().join("shoebox-view-cost");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let disk = cat.plug_in_test_disk("Disk", &base).unwrap();
    // 20,000 photos an hour apart, one file each: 20 in a small folder, the
    // rest in 200 folders; a person in 1,000 of them.
    let person = {
        let mut db = cat.db();
        let tx = db.transaction().unwrap();
        let mut folders = Vec::new();
        for i in 0..=200 {
            let rel = if i == 200 { "Pictures\\Small".to_string() } else { format!("Pictures\\F{i}") };
            tx.execute("INSERT INTO folders (volume_id, rel_path) VALUES (?1, ?2)", params![disk, rel]).unwrap();
            folders.push(tx.last_insert_rowid());
        }
        tx.execute("INSERT INTO people (name, created_at) VALUES ('Ana', 0)", []).unwrap();
        let person = tx.last_insert_rowid();
        {
            let mut photo = tx
                .prepare(
                    "INSERT INTO photos (fingerprint, kind, taken_ts, make, model, lens, preview_state, added_at)
                     VALUES (?1, ?2, ?3, 'Canon', ?4, 'RF24-105mm F4 L IS USM', 1, 0)",
                )
                .unwrap();
            let mut file = tx
                .prepare("INSERT INTO files (photo_id, folder_id, name, size, mtime) VALUES (?1, ?2, ?3, 1, 0)")
                .unwrap();
            let mut face = tx
                .prepare(
                    "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, seen)
                     VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9, ?3)",
                )
                .unwrap();
            for i in 0..PHOTOS {
                let raw = i % 3 == 0;
                let model = if i % 2 == 0 { "EOS R5" } else { "EOS 5D Mark IV" };
                photo
                    .execute(params![
                        format!("fp{i}"),
                        if raw { "raw" } else { "image" },
                        1_600_000_000 + i * 3600,
                        model
                    ])
                    .unwrap();
                let id = tx.last_insert_rowid();
                let folder = if i < 20 { folders[200] } else { folders[(i % 200) as usize] };
                let name = format!("IMG_{i}.{}", if raw { "CR3" } else { "JPG" });
                file.execute(params![id, folder, name]).unwrap();
                if (5_000..6_000).contains(&i) {
                    face.execute(params![id, person, i % 2]).unwrap();
                }
            }
        }
        // 50 photos the upright model would turn, the rest found upright.
        tx.execute("UPDATE photos SET sideways = CASE WHEN id % 400 = 7 THEN 1 ELSE 0 END", []).unwrap();
        tx.commit().unwrap();
        person
    };

    let none = Filters::default();
    let all_list =
        quickest(&cat, || assert_eq!(cat.list_photos(&Scope::All, Sort::Oldest, &none).unwrap().ids.len(), 20_000));
    let all_counts = quickest(&cat, || {
        cat.facets(&Scope::All, &none).unwrap();
    });
    let views = [
        ("a folder of 20 photos", Scope::Folder { volume_id: disk, rel_path: "Pictures\\Small".into() }, 20),
        // 1,600,000,000 + 3,600 * i: 2020-09-13 onwards, a month is 720-744 photos.
        ("a month", Scope::Date { key: "2021-03".into() }, 744),
        ("a person in 1,000 photos", Scope::Person { id: person, new_only: false }, 1_000),
        ("50 sideways photos", Scope::Sideways, 50),
    ];
    let mut slow = Vec::new();
    for (what, view, photos) in views {
        let list =
            quickest(&cat, || assert_eq!(cat.list_photos(&view, Sort::Oldest, &none).unwrap().ids.len(), photos));
        let counts = quickest(&cat, || {
            cat.facets(&view, &none).unwrap();
        });
        // A twentieth of the photos or less: a quarter of the time at most.
        if list * 4 > all_list {
            slow.push(format!("{what}: its photos took {list:?}, all photos {all_list:?}"));
        }
        if counts * 4 > all_counts {
            slow.push(format!("{what}: its filters took {counts:?}, all photos' {all_counts:?}"));
        }
    }
    assert!(slow.is_empty(), "views that cost as much as the whole catalog:\n{}", slow.join("\n"));
}
