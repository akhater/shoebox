//! What the window asks is kept until the catalog changes: asked again, it
//! comes back at once; after any change - made by Shoebox, by another program
//! such as shoebox-cli, or a catalog folder or disk coming or going - it is
//! worked out again. A kept answer never outlives a change.
//!
//! Lives here, not in src/, because it makes a throwaway catalog folder: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use rusqlite::params;
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::faces::FaceIndex;
use shoebox_lib::paths::DataDir;
use std::time::{Duration, Instant};

fn quickest(f: impl Fn()) -> Duration {
    (0..3)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
fn answers_are_kept_until_the_catalog_changes() {
    let base = std::env::temp_dir().join("shoebox-answers-kept");
    let _ = std::fs::remove_dir_all(&base);
    let photos_dir = base.join("Pictures");
    std::fs::create_dir_all(&photos_dir).unwrap();
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    let faces = FaceIndex::open(&cat).unwrap();
    let disk = cat.plug_in_test_disk("Disk", &base).unwrap();
    let all = || cat.list_photos(&Scope::All, Sort::default(), &Filters::default()).unwrap();
    let total = || cat.overview().unwrap().total;
    // 10,000 photos in one folder that is a source.
    {
        let mut db = cat.db();
        let tx = db.transaction().unwrap();
        tx.execute("INSERT INTO roots (volume_id, rel_path, added_at) VALUES (?1, 'Pictures', 0)", [disk]).unwrap();
        tx.execute("INSERT INTO folders (volume_id, rel_path) VALUES (?1, 'Pictures')", [disk]).unwrap();
        let folder = tx.last_insert_rowid();
        for i in 0..10_000 {
            tx.execute(
                "INSERT INTO photos (fingerprint, kind, taken_ts, make, model, preview_state, added_at) VALUES (?1, 'image', ?2, 'Canon', 'EOS R5', 1, 0)",
                params![format!("fp{i}"), 1_600_000_000 + i * 60],
            )
            .unwrap();
            let id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO files (photo_id, folder_id, name, size, mtime) VALUES (?1, ?2, ?3, 1, 0)",
                params![id, folder, format!("IMG_{i}.JPG")],
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    cat.refresh_roots().unwrap();
    assert_eq!(all().ids.len(), 10_000);
    assert_eq!(total(), 10_000);

    // Asked again with nothing changed: kept, so far quicker than worked out.
    // Worked out each time: something is written first.
    let round = std::cell::Cell::new(0);
    let fresh = quickest(|| {
        round.set(round.get() + 1);
        cat.db().execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('touch', ?1)", [round.get()]).unwrap();
        cat.facets(&Scope::All, &Filters::default()).unwrap();
    });
    let kept = quickest(|| {
        cat.facets(&Scope::All, &Filters::default()).unwrap();
    });
    assert!(kept * 4 < fresh, "kept {kept:?}, worked out {fresh:?}");

    // Changed by Shoebox: worked out again.
    cat.db()
        .execute("INSERT INTO photos (fingerprint, kind, preview_state, added_at) VALUES ('new', 'image', 1, 0)", [])
        .unwrap();
    assert_eq!(all().ids.len(), 10_001);
    assert_eq!(total(), 10_001);
    assert_eq!(cat.browse().unwrap().unknown_date, 1);

    // Changed by another program (shoebox-cli, a second Shoebox): noticed too.
    {
        let other = rusqlite::Connection::open(data.db_path()).unwrap();
        other.execute("UPDATE photos SET marked = 1 WHERE fingerprint = 'fp0'", []).unwrap();
        other.execute("INSERT INTO people (name, created_at) VALUES ('Ana', 0)", []).unwrap();
        let ana = other.last_insert_rowid();
        other
            .execute(
                "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score) SELECT id, ?1, 0.1, 0.1, 0.4, 0.4, '', 0.9 FROM photos WHERE fingerprint = 'fp1'",
                [ana],
            )
            .unwrap();
    }
    assert_eq!(cat.overview().unwrap().marked, 1);
    assert_eq!(all().marked.iter().filter(|&&m| m == 1).count(), 1);
    assert_eq!(faces.people(&cat).unwrap().len(), 1);

    // The folder taken away while its disk stays plugged in: its photos are no
    // longer reachable, with nothing written to the catalog.
    assert!(all().state.iter().take(10).all(|&s| s == 2));
    std::fs::remove_dir_all(&photos_dir).unwrap();
    assert!(cat.refresh_roots().unwrap());
    assert!(all().state.iter().take(10).all(|&s| s == 1));
    let status = cat.facets(&Scope::All, &Filters::default()).unwrap().status;
    assert!(status.iter().any(|s| s.value == "offline" && s.count == 10_000));

    drop((faces, cat));
    let _ = std::fs::remove_dir_all(&base);
}
