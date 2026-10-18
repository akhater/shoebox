//! Word search keeps a photo's description only while that photo is there. A
//! photo id is used again when the newest photo leaves the catalog and another
//! comes in: the newcomer gets its own description, never the one of the
//! photo that left.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use shoebox_lib::ai::{CLIP_DIM, CLIP_MODEL};
use shoebox_lib::catalog::Catalog;
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use shoebox_lib::search::SearchIndex;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn jpeg(path: &Path, shade: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(640, 480, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

fn add(cat: &Catalog, folder: &Path) -> i64 {
    let root = cat.add_root(folder).unwrap();
    scan_root(cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    root
}

fn photo(cat: &Catalog, name: &str) -> (i64, String) {
    cat.db()
        .query_row(
            "SELECT p.id, p.fingerprint FROM files f JOIN photos p ON p.id = f.photo_id WHERE f.name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

fn setup(name: &str) -> (PathBuf, Catalog) {
    let base = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&base);
    jpeg(&base.join("photos").join("a").join("IMG_A.JPG"), 30);
    jpeg(&base.join("photos").join("b").join("IMG_B.JPG"), 200);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    (base, Catalog::open(data).unwrap())
}

fn search_db(cat: &Catalog) -> rusqlite::Connection {
    rusqlite::Connection::open(cat.data.root.join("search.db")).unwrap()
}

#[test]
fn a_reused_photo_id_is_described_again() {
    let (base, cat) = setup("shoebox-search-reused-id");
    let root_a = add(&cat, &base.join("photos").join("a"));
    let (a, fp_a) = photo(&cat, "IMG_A.JPG");
    drop(SearchIndex::open(&cat).unwrap());
    // Described, as the AI does it.
    search_db(&cat)
        .execute(
            "INSERT INTO clip (photo_id, model, vec, fingerprint) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![a, CLIP_MODEL, vec![0u8; CLIP_DIM * 4], fp_a],
        )
        .unwrap();
    let index = SearchIndex::open(&cat).unwrap();
    assert_eq!((index.tidy(&cat).unwrap(), index.indexed()), (0, 1), "still its photo");

    // A leaves, B comes in before the AI has had a look, and takes A's id.
    cat.remove_root(root_a).unwrap();
    add(&cat, &base.join("photos").join("b"));
    let (b, _) = photo(&cat, "IMG_B.JPG");
    assert_eq!(b, a, "the id is used again");
    assert_eq!(index.tidy(&cat).unwrap(), 1);
    assert_eq!(index.indexed(), 0, "B will be described");
    let left: i64 = search_db(&cat).query_row("SELECT COUNT(*) FROM clip", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 0);

    drop((index, cat));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn descriptions_made_before_keep_their_photo() {
    let (base, cat) = setup("shoebox-search-older-index");
    add(&cat, &base.join("photos").join("a"));
    let (a, _) = photo(&cat, "IMG_A.JPG");
    // An index from before descriptions carried their photo's fingerprint,
    // also holding photos that have left since, listed before and after it.
    let db = search_db(&cat);
    db.execute_batch("CREATE TABLE clip (photo_id INTEGER PRIMARY KEY, model TEXT NOT NULL, vec BLOB NOT NULL);")
        .unwrap();
    for (id, v) in [(0, 1.0f32), (a, 2.0), (a + 1, 3.0)] {
        let vec: Vec<u8> = std::iter::repeat_n(v.to_le_bytes(), CLIP_DIM).flatten().collect();
        db.execute(
            "INSERT INTO clip (photo_id, model, vec) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, CLIP_MODEL, vec],
        )
        .unwrap();
    }
    drop(db);

    let index = SearchIndex::open(&cat).unwrap();
    assert_eq!(index.indexed(), 3);
    assert_eq!(index.tidy(&cat).unwrap(), 2);
    assert_eq!(index.indexed(), 1, "the photo still here keeps its description");
    assert_eq!(index.vectors(&[a])[&a], vec![2.0; CLIP_DIM], "its own");

    drop((index, cat));
    let _ = std::fs::remove_dir_all(&base);
}
