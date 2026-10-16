//! Each photo's camera and lens as shown are stored with it (for quick
//! views) and always match the names worked out from its files and the
//! names grouped, renamed or split by hand: when a photo is added, when its
//! lens is found later, when names are grouped, renamed or split, and after
//! something left them behind.
//!
//! Lives here, not in src/, because it makes a throwaway catalog folder: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::gear::{self, Kind};
use shoebox_lib::paths::DataDir;

/// Photos whose stored names differ from the names worked out now.
fn behind(cat: &Catalog) -> i64 {
    cat.db()
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM photos p WHERE p.camera_shown IS NOT {} OR p.lens_shown IS NOT {}",
                Kind::Camera.shown_sql(),
                Kind::Lens.shown_sql()
            ),
            [],
            |r| r.get(0),
        )
        .unwrap()
}

fn on_page(cat: &Catalog, scope: Scope) -> usize {
    cat.list_photos(&scope, Sort::default(), &Filters::default()).unwrap().ids.len()
}

#[test]
fn stored_names_follow_every_change() {
    let base = std::env::temp_dir().join("shoebox-stored-gear-names");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let add = |fp: &str, make: Option<&str>, model: Option<&str>, lens: Option<&str>| {
        cat.db()
            .execute(
                "INSERT INTO photos (fingerprint, kind, make, model, lens, taken_ts, taken_exif, added_at)
                 VALUES (?1, 'raw', ?2, ?3, ?4, 1136073600, 1, 0)",
                rusqlite::params![fp, make, model, lens],
            )
            .unwrap();
    };
    // Added: named at once, including photos that don't say.
    add("a", Some("Canon"), Some("Canon EOS 5D Mark II"), Some("EF70-200mm f/4L IS USM"));
    add("b", Some("Canon"), Some("Canon EOS 5D Mark II"), Some("Canon EF 70-200mm f/4L IS USM"));
    add("c", Some("Canon"), Some("Canon EOS 5D Mark II"), None);
    add("d", None, None, None);
    assert_eq!(behind(&cat), 0);
    assert_eq!(on_page(&cat, Scope::Camera { name: "Canon EOS 5D Mark II".into() }), 3);
    assert_eq!(on_page(&cat, Scope::Camera { name: "Unknown camera".into() }), 1);
    assert_eq!(on_page(&cat, Scope::Lens { name: "Unknown lens".into() }), 2);

    // Two spellings of one lens grouped (as after a scan).
    gear::tidy(&cat.db()).unwrap();
    assert_eq!(behind(&cat), 0);
    let lenses = cat.facets(&Scope::All, &Filters::default()).unwrap().lenses;
    assert_eq!(lenses.iter().find(|l| l.value != "Unknown lens").map(|l| l.count), Some(2));

    // Renamed by hand, then split back.
    let shown = lenses.iter().find(|l| l.value != "Unknown lens").unwrap().value.clone();
    gear::rename(&cat.db(), Kind::Lens, &shown, "My 70-200").unwrap();
    assert_eq!(behind(&cat), 0);
    assert_eq!(on_page(&cat, Scope::Lens { name: "My 70-200".into() }), 2);
    let lens_filter = Filters { lenses: vec!["My 70-200".into()], ..Default::default() };
    assert_eq!(cat.list_photos(&Scope::All, Sort::default(), &lens_filter).unwrap().ids.len(), 2);
    gear::split(&cat.db(), Kind::Lens, "My 70-200").unwrap();
    assert_eq!(behind(&cat), 0);
    assert_eq!(on_page(&cat, Scope::Lens { name: "My 70-200".into() }), 0);

    // A lens found later (read again, or lent by the same shot's other file).
    cat.db().execute("UPDATE photos SET lens = 'EF70-200mm f/4L IS USM' WHERE fingerprint = 'c'", []).unwrap();
    assert_eq!(behind(&cat), 0);
    assert_eq!(on_page(&cat, Scope::Lens { name: "Unknown lens".into() }), 1);
    let detail = cat
        .photo_detail(cat.db().query_row("SELECT id FROM photos WHERE fingerprint = 'c'", [], |r| r.get(0)).unwrap())
        .unwrap();
    assert_eq!(detail.lens.as_deref(), Some("EF70-200mm f/4L IS USM"));

    // Left behind (an older Shoebox renaming without storing): caught up.
    cat.db().execute("UPDATE photos SET camera_shown = 'Old name' WHERE fingerprint = 'a'", []).unwrap();
    assert_eq!(behind(&cat), 1);
    assert_eq!(gear::store_shown(&cat.db()).unwrap(), 1);
    assert_eq!(behind(&cat), 0);

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
