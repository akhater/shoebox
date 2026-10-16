//! Reading a big catalog: the memory each connection may use follows the
//! settings (automatic by default), a change applies at once, and
//! `shoebox-cli settings` / `shoebox-cli bench` set and time it from outside.
//! Word search and faces opened in the background answer as when opened first.
//!
//! Lives here, not in src/, because it makes a throwaway catalog folder: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use shoebox_lib::catalog::Catalog;
use shoebox_lib::db::Memory;
use shoebox_lib::faces::FaceIndex;
use shoebox_lib::paths::DataDir;
use shoebox_lib::search::SearchIndex;
use std::process::Command;
use std::sync::Arc;

fn in_use(cat: &Catalog) -> (i64, i64) {
    let r = cat.read().unwrap();
    (
        r.pragma_query_value(None, "cache_size", |v| v.get(0)).unwrap(),
        r.pragma_query_value(None, "mmap_size", |v| v.get(0)).unwrap(),
    )
}

#[test]
fn catalog_memory_follows_the_settings() {
    let base = std::env::temp_dir().join("shoebox-catalog-memory");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    {
        let cat = Catalog::open(data.clone()).unwrap();
        assert_eq!(cat.memory(), Memory { cache_mb: 16, map_mb: 256 });
        assert_eq!(in_use(&cat), (-16 * 1024, 256 << 20));
        // Changed in the settings: used at once, by the connections opened from then on.
        let mut s = cat.settings();
        s.catalog_map_mb = 512;
        cat.save_settings(s).unwrap();
        assert_eq!(cat.memory(), Memory { cache_mb: 16, map_mb: 512 });
        assert_eq!(in_use(&cat), (-16 * 1024, 512 << 20));
    }
    let cli = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_shoebox-cli"))
            .args(args)
            .env("SHOEBOX_DATA", &data.root)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
    };
    // Kept for the next start, and set from outside.
    assert_eq!(cli(&["settings"])["memory"]["mapMb"], 512);
    let set = cli(&["settings", "catalogMapMb=0", "catalogCacheMb=32"]);
    assert_eq!(set["settings"]["catalogMapMb"], 0);
    assert_eq!(set["memory"], serde_json::json!({ "cacheMb": 32, "mapMb": 256 }));
    // Timed with other memory for one run; the settings stay as they are.
    let bench = cli(&["bench", "--cache-mb", "2", "--map-mb", "off"]);
    assert_eq!(bench["memory"], serde_json::json!({ "cacheMb": 2, "mapMb": 0 }));
    assert!(bench["views"].as_array().unwrap().iter().any(|v| v["view"] == "all photos" && v["ms"].is_number()));
    assert_eq!(cli(&["settings"])["memory"]["cacheMb"], 32);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn indexes_opened_in_the_background_answer_as_when_opened_first() {
    let base = std::env::temp_dir().join("shoebox-indexes-background");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Arc::new(Catalog::open(data).unwrap());
    cat.db()
        .execute_batch(
            "INSERT INTO photos (id, fingerprint, kind, preview_state, added_at) VALUES (1, 'f1', 'image', 1, 0), (2, 'f2', 'image', 1, 0);
             INSERT INTO people (id, name, created_at) VALUES (1, 'Ana', 0);
             INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score) VALUES
                 (1, 1, 0.1, 0.1, 0.4, 0.4, '', 0.9), (2, 1, 0.1, 0.1, 0.4, 0.4, '', 0.9);
             INSERT INTO meta (key, value) VALUES ('face_grouping', ''), ('look_alikes_joined', '');",
        )
        .unwrap();
    let first: Vec<(i64, i64)> =
        FaceIndex::open(&cat).unwrap().people(&cat).unwrap().iter().map(|p| (p.id, p.photos)).collect();
    let later = FaceIndex::open_in_background(cat.clone()).unwrap();
    let background: Vec<(i64, i64)> = later.people(&cat).unwrap().iter().map(|p| (p.id, p.photos)).collect();
    assert_eq!(background, first);
    let search = SearchIndex::open_in_background(&cat).unwrap();
    assert_eq!(search.tidy(&cat).unwrap(), 0);
    assert_eq!(search.indexed(), 0);
    drop((later, search));
    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
