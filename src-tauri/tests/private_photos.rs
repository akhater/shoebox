//! Private photos: while locked they are in no view, count, folder, date,
//! tag, person or answer by id, and Shoebox's copies of them are sealed on
//! disk. Every password is its own space: never refused, it shows its own
//! photos and no other's. A stop halfway leaves nothing in the clear.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::faces::FaceIndex;
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::{retry_failed, scan_root};
use shoebox_lib::{organize, private, tags};
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn jpeg(path: &Path, shade: u8) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_fn(320, 240, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

fn photo_of(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

fn listed(cat: &Catalog, scope: &Scope) -> Vec<i64> {
    let mut ids = cat.list_photos(scope, Sort::default(), &Filters::default()).unwrap().ids;
    ids.sort();
    ids
}

fn sorted(mut ids: Vec<i64>) -> Vec<i64> {
    ids.sort();
    ids
}

/// Every folder name in the sidebar's tree.
fn folder_names(cat: &Catalog) -> Vec<String> {
    fn walk(nodes: &[shoebox_lib::catalog::FolderNode], out: &mut Vec<String>) {
        for n in nodes {
            out.push(n.name.clone());
            walk(&n.children, out);
        }
    }
    let mut out = Vec::new();
    for t in cat.browse().unwrap().folders {
        walk(&t.children, &mut out);
    }
    out
}

#[test]
fn private_photos_are_hidden_and_sealed_until_unlocked() {
    let base = std::env::temp_dir().join("shoebox-private-photos");
    let _ = std::fs::remove_dir_all(&base);
    let (pics, vault) = (base.join("Pictures"), base.join("Vault"));
    jpeg(&pics.join("Secret").join("a.jpg"), 10);
    jpeg(&pics.join("Secret").join("b.jpg"), 60);
    jpeg(&pics.join("Trip").join("c.jpg"), 110);
    jpeg(&pics.join("Trip").join("d.jpg"), 160);
    jpeg(&vault.join("e.jpg"), 210);

    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    let faces = FaceIndex::open(&cat).unwrap();
    let (pics_root, vault_root) = (cat.add_root(&pics).unwrap(), cat.add_root(&vault).unwrap());
    for root in [pics_root, vault_root] {
        scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    }
    let [a, b, c, d, e] = ["a.jpg", "b.jpg", "c.jpg", "d.jpg", "e.jpg"].map(|n| photo_of(&cat, n));

    // Ana is only in a (private soon), Bob in a and c. Their face pictures.
    let (ana_face, bob_private, bob_face) = {
        let db = cat.db();
        db.execute_batch("INSERT INTO people (id, name, created_at) VALUES (1, 'Ana', 0), (2, 'Bob', 0);").unwrap();
        let add = |photo: i64, person: i64, score: f64| {
            db.execute(
                "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score) VALUES (?1, ?2, 0.1, 0.1, 0.5, 0.5, '', ?3)",
                rusqlite::params![photo, person, score],
            )
            .unwrap();
            db.last_insert_rowid()
        };
        // Bob's best face is in the private photo: it must not be his cover while locked.
        (add(a, 1, 0.9), add(a, 2, 0.99), add(c, 2, 0.8))
    };
    for f in [ana_face, bob_private, bob_face] {
        data.write(&data.face_path(f), &std::fs::read(data.thumb_path(a)).unwrap()).unwrap();
    }
    tags::add(&cat, &[a, b], &["Boudoir".to_string()]).unwrap();
    tags::add(&cat, &[c], &["Beach".to_string()]).unwrap();
    let preview_a = std::fs::read(data.preview_path(a)).unwrap();

    // Locked, nothing can be made private: a space must be open first.
    assert!(private::hide(&cat, &[a], None).is_err());
    assert_eq!(private::status(&cat), private::Status { unlocked: false });
    assert!(private::unlock(&cat, "short").is_err(), "too short");

    // Any password opens a space, never refused; a new one is empty and its
    // first photos need the password again (a typo would start a space).
    assert!(private::unlock(&cat, "correct horse").unwrap().unlocked);
    assert_eq!(listed(&cat, &Scope::Private), Vec::<i64>::new());
    assert!(private::hide(&cat, &[a, b, e], None).unwrap_err().is::<private::NeedsConfirm>());
    assert!(private::hide(&cat, &[a, b, e], Some("correct hrose")).is_err(), "a typo");
    assert_eq!(private::hide(&cat, &[a, b, e], Some("correct horse")).unwrap(), 3);
    assert_eq!(private::hide(&cat, &[a], None).unwrap(), 0, "already private");
    assert_eq!(listed(&cat, &Scope::Private), sorted(vec![a, b, e]));
    private::lock(&cat);

    // Locked: in no view, count, folder, date, tag or person.
    assert_eq!(listed(&cat, &Scope::All), sorted(vec![c, d]));
    // No Private filter to offer, and one asked for anyway (an agent) changes nothing.
    let private_only = Filters { private: vec!["private".into()], ..Filters::default() };
    assert!(cat.facets(&Scope::All, &Filters::default()).unwrap().private.is_empty());
    assert_eq!(sorted(cat.list_photos(&Scope::All, Sort::default(), &private_only).unwrap().ids), sorted(vec![c, d]));
    assert_eq!(listed(&cat, &Scope::Private), Vec::<i64>::new());
    assert_eq!(listed(&cat, &Scope::Person { id: 2, new_only: false }), vec![c]);
    let o = cat.overview().unwrap();
    assert_eq!((o.total, o.private), (2, 0));
    let roots: Vec<String> = o.volumes.iter().flat_map(|v| v.roots.iter().map(|r| r.name.clone())).collect();
    assert_eq!(roots, ["Pictures"], "a source holding only private photos isn't listed");
    assert_eq!(o.volumes.iter().flat_map(|v| &v.roots).map(|r| r.photos).sum::<i64>(), 2);
    assert_eq!(folder_names(&cat), ["Trip"], "nor a folder holding only private photos");
    let browse = cat.browse().unwrap();
    assert_eq!(browse.years.iter().map(|y| y.count).sum::<i64>() + browse.unknown_date, 2);
    let days = cat.days(&Scope::All, &Filters::default()).unwrap();
    assert_eq!(days.days.iter().map(|d| d.photos).sum::<i64>() + days.undated, 2);
    assert_eq!(cat.stats().unwrap().photos, 2);
    let facets = cat.facets(&Scope::All, &Filters::default()).unwrap();
    assert_eq!(facets.tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Beach"]);
    assert_eq!(facets.people.iter().map(|p| (p.id, p.cover, p.count)).collect::<Vec<_>>(), [(2, bob_face, 1)]);
    assert_eq!(tags::all(&cat).unwrap().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Beach"]);
    let people = faces.people(&cat).unwrap();
    assert_eq!(people.iter().map(|p| (p.id, p.photos, p.cover)).collect::<Vec<_>>(), [(2, 1, bob_face)]);
    assert_eq!(faces.person_faces(&cat, 2).unwrap().len(), 0, "no fingerprints here, and never a's face");
    assert!(faces.faces_in(&cat, &[a, c]).unwrap().iter().all(|f| f.photo_id == c));
    // A face found in a private photo afterwards (faces looked for again) is hidden with it.
    cat.db()
        .execute(
            "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score) VALUES (?1, 2, 0.6, 0.6, 0.9, 0.9, '', 1.0)",
            [b],
        )
        .unwrap();
    cat.forget_kept();
    faces.forget_kept();
    let people = faces.people(&cat).unwrap();
    assert_eq!(people.iter().map(|p| (p.id, p.faces, p.cover)).collect::<Vec<_>>(), [(2, 1, bob_face)]);

    // Asked for by id: as if it didn't exist.
    assert_eq!(cat.photo_detail(a).err().map(|e| e.to_string()), Some(format!("Unknown photo {a}")));
    assert!(cat.original(a).is_err());
    assert!(tags::on(&cat, &[a]).unwrap().is_empty());
    assert_eq!(organize::mark(&cat, &[a, c], true).unwrap(), 1, "only c");
    assert!(cat.photo_detail(c).is_ok());

    // Shoebox's copies: sealed, nothing readable left in the clear.
    for path in [data.preview_path(a), data.thumb_path(a), data.face_path(ana_face), data.face_path(bob_private)] {
        let sealed = DataDir::sealed_path(&path);
        assert!(!path.exists() && sealed.is_file(), "{}", path.display());
        let bytes = std::fs::read(&sealed).unwrap();
        assert!(image::load_from_memory(&bytes).is_err());
    }
    assert!(data.face_path(bob_face).is_file(), "c is not private");
    assert!(cat.read_copy(&data.preview_path(a)).unwrap_err().is::<private::Locked>());

    // Another password: no error, an empty space, with photos of its own the
    // first never sees.
    assert!(private::unlock(&cat, "battery staple").unwrap().unlocked);
    assert_eq!(listed(&cat, &Scope::All), sorted(vec![c, d]));
    assert_eq!(listed(&cat, &Scope::Private), Vec::<i64>::new());
    assert_eq!(cat.overview().unwrap().private, 0);
    assert!(cat.read_copy(&data.preview_path(a)).unwrap_err().is::<private::Locked>());
    assert_eq!(private::hide(&cat, &[d], Some("battery staple")).unwrap(), 1);
    assert_eq!(listed(&cat, &Scope::All), sorted(vec![c, d]));
    assert_eq!(listed(&cat, &Scope::Private), vec![d]);
    assert_eq!(private::show(&cat, &[a]).unwrap(), 0, "not one of its photos");
    private::lock(&cat);
    assert_eq!(listed(&cat, &Scope::All), vec![c]);

    // The first password again: its photos, not the other's.
    assert!(private::unlock(&cat, "correct horse").unwrap().unlocked);
    assert_eq!(listed(&cat, &Scope::All), sorted(vec![a, b, c, e]));
    assert_eq!(listed(&cat, &Scope::Private), sorted(vec![a, b, e]));
    let list = cat.list_photos(&Scope::Private, Sort::default(), &Filters::default()).unwrap();
    assert!(list.private.iter().all(|p| *p == 1));
    assert_eq!(cat.overview().unwrap().private, 3);
    // The Private filter: private or not, counted with the other filters.
    let only = |v: &str| Filters { private: vec![v.to_string()], ..Filters::default() };
    let filtered = |f: &Filters| sorted(cat.list_photos(&Scope::All, Sort::default(), f).unwrap().ids);
    assert_eq!(filtered(&only("private")), sorted(vec![a, b, e]));
    assert_eq!(filtered(&only("notPrivate")), vec![c]);
    let facet = |f: &Filters| {
        cat.facets(&Scope::All, f).unwrap().private.iter().map(|v| (v.value.clone(), v.count)).collect::<Vec<_>>()
    };
    let both = vec![("private".to_string(), 3), ("notPrivate".to_string(), 1)];
    assert_eq!(facet(&Filters::default()), both);
    assert_eq!(facet(&only("private")), both, "not narrowed by itself");
    assert!(folder_names(&cat).contains(&"Secret".to_string()));
    assert!(cat.photo_detail(a).unwrap().private);
    assert!(cat.photo_detail(d).is_err(), "the other space's");
    assert_eq!(cat.read_copy(&data.preview_path(a)).unwrap(), preview_a, "the same preview, unsealed");
    assert_eq!(faces.people(&cat).unwrap().len(), 2, "Ana is back");
    // Each photo's key is its own: nothing in the catalog groups them by space.
    let keys: std::collections::HashSet<Vec<u8>> = cat
        .db()
        .prepare("SELECT seal_to FROM photos WHERE private = 1")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(keys.len(), 4);
    let meta: Vec<String> =
        cat.db().prepare("SELECT key FROM meta").unwrap().query_map([], |r| r.get(0)).unwrap().flatten().collect();
    assert!(meta.iter().all(|k| !k.contains("key") && !k.contains("password")), "{meta:?}");

    // A stop while making private left a copy in the clear: sealed at the next start.
    data.write(&data.preview_path(a), &preview_a).unwrap();
    assert_eq!(private::settle(&cat).unwrap(), 1);
    assert!(!data.preview_path(a).exists() && DataDir::sealed_path(&data.preview_path(a)).is_file());
    assert_eq!(cat.read_copy(&data.preview_path(a)).unwrap(), preview_a);

    // Made ordinary again: its copies back in the clear.
    assert_eq!(private::show(&cat, &[b]).unwrap(), 1);
    assert!(data.preview_path(b).is_file() && !DataDir::sealed_path(&data.preview_path(b)).exists());
    private::lock(&cat);
    assert!(private::show(&cat, &[a]).is_err(), "only while open");
    assert_eq!(listed(&cat, &Scope::All), sorted(vec![b, c]));

    // Each process starts locked.
    let other = Catalog::open(data.clone()).unwrap();
    assert_eq!(listed(&other, &Scope::All), sorted(vec![b, c]));
    drop(other);

    // A preview made again while locked (its disk plugged in) is sealed to its
    // photo's key: no password needed.
    cat.db().execute("UPDATE photos SET preview_state = 0 WHERE id = ?1", [a]).unwrap();
    for path in [data.preview_path(a), data.thumb_path(a)] {
        std::fs::remove_file(DataDir::sealed_path(&path)).unwrap();
    }
    let volume: i64 =
        cat.db().query_row("SELECT volume_id FROM roots WHERE id = ?1", [pics_root], |r| r.get(0)).unwrap();
    retry_failed(&cat, volume, &AtomicBool::new(false), &|_| {}).unwrap();
    assert!(!data.preview_path(a).exists() && DataDir::sealed_path(&data.preview_path(a)).is_file());
    private::unlock(&cat, "correct horse").unwrap();
    assert!(image::load_from_memory(&cat.read_copy(&data.preview_path(a)).unwrap()).is_ok());
    // A face picture lost while locked is cut again once its space is open.
    std::fs::remove_file(DataDir::sealed_path(&data.face_path(ana_face))).unwrap();
    assert_eq!(private::after_unlock(&cat).unwrap(), 1);
    assert!(cat.has_copy(&data.face_path(ana_face)) && !data.face_path(ana_face).exists());

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}

/// Agents make photos private headless (with a password on stdin), and never
/// see them: nothing the CLI lists or counts shows one.
#[test]
fn agents_hide_photos_but_never_see_them() {
    let base = std::env::temp_dir().join("shoebox-private-cli");
    let _ = std::fs::remove_dir_all(&base);
    let pics = base.join("Pictures");
    jpeg(&pics.join("a.jpg"), 30);
    jpeg(&pics.join("b.jpg"), 90);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    let root = cat.add_root(&pics).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let (a, b) = (photo_of(&cat, "a.jpg"), photo_of(&cat, "b.jpg"));
    let cli = |args: &[&str], stdin: &str| {
        use std::io::Write;
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_shoebox-cli"))
            .args(args)
            .env("SHOEBOX_DATA", &data.root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
    };

    let a_ = a.to_string();
    assert!(!cli(&["private", &a_], "short\n").0, "too short");
    assert!(!cli(&["private", &a_], "correct horse\n").0, "a password with no photos yet needs --new");
    let (ok, out) = cli(&["private", "--new", &a_], "correct horse\n");
    assert!(ok, "{out}");
    assert!(out.contains("\"madePrivate\":1"), "{out}");

    let (_, list) = cli(&["list"], "");
    assert!(list.contains(&format!("\"id\": {b}")) && !list.contains(&format!("\"id\": {a}")), "{list}");
    assert!(!cli(&["photo", &a_], "").0);
    let (_, stats) = cli(&["stats"], "");
    assert!(stats.contains("\"total\": 1") && stats.contains("photos listed: 1"), "{stats}");
    // The window, with that password, sees it.
    private::unlock(&cat, "correct horse").unwrap();
    assert_eq!(listed(&cat, &Scope::All), sorted(vec![a, b]));
    assert_eq!(listed(&cat, &Scope::Private), vec![a]);

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
