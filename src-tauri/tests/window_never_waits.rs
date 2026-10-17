//! What the window asks is answered while the catalog is being written (a
//! scan's batch, the AI saving faces, a rename): it reads beside the writer,
//! never behind it, so the window never stalls on a busy catalog.
//!
//! Lives here, not in src/, because it makes a throwaway catalog folder: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use rusqlite::params;
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::faces::FaceIndex;
use shoebox_lib::paths::DataDir;
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[test]
fn questions_are_answered_while_the_catalog_is_written() {
    let base = std::env::temp_dir().join("shoebox-window-never-waits");
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let disk = cat.plug_in_test_disk("Disk", &base).unwrap();
    let (photo, file, person, face) = {
        let db = cat.db();
        db.execute("INSERT INTO folders (volume_id, rel_path) VALUES (?1, 'Pictures')", [disk]).unwrap();
        let folder = db.last_insert_rowid();
        db.execute(
            "INSERT INTO photos (fingerprint, kind, taken_ts, preview_state, added_at) VALUES ('fp', 'raw', 1136073600, 1, 0)",
            [],
        )
        .unwrap();
        let photo = db.last_insert_rowid();
        db.execute(
            "INSERT INTO files (photo_id, folder_id, name, size, mtime) VALUES (?1, ?2, 'IMG_1.CR3', 1, 0)",
            params![photo, folder],
        )
        .unwrap();
        let file = db.last_insert_rowid();
        db.execute("INSERT INTO people (name, created_at) VALUES ('Ana', 0)", []).unwrap();
        let person = db.last_insert_rowid();
        db.execute(
            "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score) VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9)",
            params![photo, person],
        )
        .unwrap();
        let face = db.last_insert_rowid();
        db.execute("INSERT INTO tags (name, created_at) VALUES ('beach', 0)", []).unwrap();
        db.execute("INSERT INTO photo_tags (photo_id, tag_id, added_at) VALUES (?1, last_insert_rowid(), 0)", [photo])
            .unwrap();
        (photo, file, person, face)
    };
    // Her fingerprint, so suggestions and "find more" have someone to compare.
    let emb = rusqlite::Connection::open(cat.data.root.join("faces.db")).unwrap();
    emb.execute_batch("CREATE TABLE IF NOT EXISTS emb (face_id INTEGER PRIMARY KEY, vec BLOB NOT NULL);").unwrap();
    let print: Vec<u8> = (0..512).flat_map(|i| (if i == 0 { 1f32 } else { 0.0 }).to_le_bytes()).collect();
    emb.execute("INSERT INTO emb (face_id, vec) VALUES (?1, ?2)", params![face, print]).unwrap();
    drop(emb);
    let faces = Arc::new(FaceIndex::open(&cat).unwrap());
    let cat = Arc::new(cat);

    // A write under way: the writer is held, inside a transaction.
    let writer = cat.db();
    writer.execute_batch("BEGIN IMMEDIATE; UPDATE photos SET marked = marked;").unwrap();

    let (done, answered) = mpsc::channel();
    let (c, f) = (cat.clone(), faces.clone());
    std::thread::spawn(move || {
        let none = Filters::default();
        let folder = Scope::Folder { volume_id: disk, rel_path: "Pictures".into() };
        let views = [
            Scope::All,
            Scope::Missing,
            folder,
            Scope::Date { key: "2006".into() },
            Scope::Person { id: person, new_only: false },
        ];
        c.overview().unwrap();
        c.browse().unwrap();
        for view in &views {
            c.list_photos(view, Sort::Oldest, &none).unwrap();
            c.facets(view, &none).unwrap();
            c.days(view, &none).unwrap();
            c.ranked_in_scope(&[photo], view, &none).unwrap();
        }
        c.photo_detail(photo).unwrap();
        c.file_path(file).unwrap();
        f.people(&c).unwrap();
        f.find_people(&c, "an").unwrap();
        f.photo_faces(&c, photo).unwrap();
        f.faces_in(&c, &[photo]).unwrap();
        f.person_faces(&c, person).unwrap();
        f.look_alikes(&c, &[face], &[], 6).unwrap();
        f.more_of(&c, person).unwrap();
        shoebox_lib::tags::all(&c).unwrap();
        shoebox_lib::tags::on(&c, &[photo]).unwrap();
        let _ = done.send(());
    });
    let in_time = answered.recv_timeout(Duration::from_secs(20)).is_ok();
    writer.execute_batch("ROLLBACK").unwrap();
    drop(writer);
    assert!(in_time, "the window's questions waited for the catalog's writer");
}
