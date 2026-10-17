//! "Check" in People: likely mix-ups among named people, found from their
//! faces and the dates of their photos - someone twice in one photo, a face
//! from another age (a baby filed under an older child), faces much more like
//! someone else, one person under two names. Nothing moves until the user
//! acts on a finding, through the moves and merges Undo knows.
//!
//! Lives here, not in src/, because it writes sample photos (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::{FaceIndex, FaceTarget};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use std::sync::atomic::AtomicBool;

const DIM: usize = 512;
const DAY: i64 = 86_400;

/// A face of `person` in `photo` whose fingerprint is `axes` (axis, amount), made unit length.
fn face(
    cat: &Catalog,
    emb: &rusqlite::Connection,
    photo: i64,
    person: i64,
    by_hand: bool,
    axes: &[(usize, f32)],
) -> i64 {
    cat.db()
        .execute(
            "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, seen, by_hand)
             VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9, 1, ?3)",
            rusqlite::params![photo, person, by_hand as i64],
        )
        .unwrap();
    let id = cat.db().last_insert_rowid();
    let mut v = vec![0f32; DIM];
    for &(a, x) in axes {
        v[a] = x;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let blob: Vec<u8> = v.iter().flat_map(|x| (x / norm).to_le_bytes()).collect();
    emb.execute("INSERT INTO emb (face_id, vec) VALUES (?1, ?2)", rusqlite::params![id, blob]).unwrap();
    id
}

fn person_of(cat: &Catalog, face: i64) -> i64 {
    cat.db().query_row("SELECT person_id FROM faces WHERE id = ?1", [face], |r| r.get(0)).unwrap()
}

/// A throwaway catalog with `n` photos taken at `when[i]` (none: no date).
fn catalog(name: &str, when: &[Option<i64>]) -> (Catalog, Vec<i64>) {
    let base = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    for i in 0..when.len() {
        image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([(i * 8) as u8, (x * 4) as u8, (y * 5) as u8]))
            .save(photos.join(format!("IMG_{i}.JPG")))
            .unwrap();
    }
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let ids: Vec<i64> = (0..when.len())
        .map(|i| {
            cat.db()
                .query_row("SELECT photo_id FROM files WHERE name = ?1", [format!("IMG_{i}.JPG")], |r| r.get(0))
                .unwrap()
        })
        .collect();
    for (id, ts) in ids.iter().zip(when) {
        cat.db().execute("UPDATE photos SET taken_ts = ?1 WHERE id = ?2", rusqlite::params![ts, id]).unwrap();
    }
    (cat, ids)
}

fn fingerprints(cat: &Catalog) -> rusqlite::Connection {
    let emb = rusqlite::Connection::open(cat.data.root.join("faces.db")).unwrap();
    emb.execute_batch("CREATE TABLE IF NOT EXISTS emb (face_id INTEGER PRIMARY KEY, vec BLOB NOT NULL);").unwrap();
    emb
}

#[test]
fn likely_mix_ups_among_named_people_are_found_and_fixed() {
    let may_2017 = 1_493_596_800; // 2017-05-01
    let jun_2021 = 1_622_505_600; // 2021-06-01
    let mar_2022 = 1_646_092_800; // 2022-03-01
    let mut when: Vec<Option<i64>> = Vec::new();
    when.extend((0..6).map(|i| Some(may_2017 + i * DAY))); // 0-5: Ana as a baby
    when.extend((0..7).map(|i| Some(jun_2021 + i * DAY))); // 6-12: Ana at four
    when.push(Some(jun_2021 + 30 * DAY)); // 13: her baby brother, filed under her
    when.extend((0..3).map(|i| Some(mar_2022 + i * DAY))); // 14-16: Bea
    when.extend((0..3).map(|i| Some(mar_2022 + (20 + i) * DAY))); // 17-19: Cy
    when.push(Some(mar_2022 + 31 * DAY)); // 20: Cy filed under Bea, days after Cy's photos
    when.push(Some(mar_2022 + 900 * DAY)); // 21: someone like Cy filed under Bea, years later
    when.push(Some(mar_2022 + 24 * DAY)); // 22: Cy twice
    when.extend([Some(1_560_000_000), Some(1_560_100_000)]); // 23-24: "ANA"
    when.extend([None, None, None, None]); // 25-28: Eve, Evie (and Gil with each)
    when.push(None); // 29: someone unnamed twice
    let (cat, p) = catalog("shoebox-people-check", &when);
    // Named (and so never regrouped), but 9; 8 is ignored.
    cat.db()
        .execute(
            "INSERT INTO people (id, name, created_at) VALUES (1, 'Ana', 0), (2, 'Bea', 0), (3, 'Cy', 0),
               (4, 'ANA', 0), (5, 'Eve', 0), (6, 'Evie', 0), (7, 'Gil', 0), (8, 'Hal', 0), (9, NULL, 0)",
            [],
        )
        .unwrap();
    cat.db().execute("UPDATE people SET ignored = 1 WHERE id = 8", []).unwrap();
    let emb = fingerprints(&cat);
    // Axis 1: a baby's look; axis 0: Ana at four.
    for &ph in &p[0..6] {
        face(&cat, &emb, ph, 1, false, &[(1, 1.0)]);
    }
    for &ph in &p[6..13] {
        face(&cat, &emb, ph, 1, false, &[(0, 1.0)]);
    }
    let brother = face(&cat, &emb, p[13], 1, false, &[(1, 1.0), (11, 0.2)]);
    for &ph in &p[14..17] {
        face(&cat, &emb, ph, 2, false, &[(2, 1.0)]);
    }
    for &ph in &p[17..20] {
        face(&cat, &emb, ph, 3, false, &[(3, 1.0)]);
    }
    let cy_in_bea = face(&cat, &emb, p[20], 2, true, &[(3, 1.0)]);
    let like_cy = face(&cat, &emb, p[21], 2, false, &[(3, 1.0), (2, 0.3)]);
    let cy = face(&cat, &emb, p[22], 3, false, &[(3, 1.0)]);
    let not_cy = face(&cat, &emb, p[22], 3, false, &[(5, 1.0)]);
    face(&cat, &emb, p[23], 4, false, &[(6, 1.0)]);
    face(&cat, &emb, p[24], 4, false, &[(6, 1.0)]);
    for &ph in &p[25..27] {
        face(&cat, &emb, ph, 5, false, &[(8, 1.0)]);
    }
    for &ph in &p[27..29] {
        face(&cat, &emb, ph, 6, false, &[(8, 1.0), (9, 0.5)]);
    }
    // Gil looks like Eve and Evie, but is in a photo with each.
    face(&cat, &emb, p[25], 7, false, &[(8, 1.0), (10, 0.6)]);
    face(&cat, &emb, p[27], 7, false, &[(8, 1.0), (10, 0.6)]);
    // Ignored and unnamed people aren't checked: twice in a photo, unlike themselves.
    face(&cat, &emb, p[29], 8, false, &[(2, 1.0)]);
    face(&cat, &emb, p[29], 8, false, &[(7, 1.0)]);
    face(&cat, &emb, p[29], 9, true, &[(2, 1.0)]);
    face(&cat, &emb, p[29], 9, true, &[(7, 1.0)]);
    drop(emb);
    let faces = FaceIndex::open(&cat).unwrap();

    let check = faces.check_people(&cat).unwrap();
    assert_eq!((check.people, check.faces), (7, 32));

    // Cy twice in one photo: the face unlike Cy is the one to move out.
    assert_eq!(check.twice.len(), 1);
    let t = &check.twice[0];
    assert_eq!((t.person_id, t.photo_id, t.elsewhere.as_slice()), (3, p[22], &[not_cy][..]));
    assert_eq!(t.faces.iter().map(|f| f.face_id).collect::<Vec<_>>(), [cy, not_cy]);
    assert!(t.faces[0].similarity > 0.9 && t.faces[1].similarity < 0.1);

    // Ana at four, and a baby among her faces that year: like her as a baby, four years before.
    let jumps: Vec<(i64, i64, i32, i32)> =
        check.age_jumps.iter().map(|a| (a.person_id, a.face.face_id, a.year, a.like_year)).collect();
    assert_eq!(jumps, [(1, brother, 2021, 2017)]);
    let j = &check.age_jumps[0];
    assert!(j.own < 0.1 && j.like > 0.9, "{} {}", j.own, j.like);

    // Faces in Bea much more like Cy: days after Cy's photos (suggested), years later (unsure).
    let others: Vec<(i64, i64, i64, bool, bool)> =
        check.like_others.iter().map(|l| (l.person_id, l.face.face_id, l.other_id, l.near, l.face.by_hand)).collect();
    assert_eq!(others, [(2, cy_in_bea, 3, true, true), (2, like_cy, 3, false, false)]);
    assert!(check.like_others.iter().all(|l| l.other >= 0.5 && l.other >= l.face.similarity + 0.1));

    // Eve and Evie: alike, never together. Ana and ANA: one name. Gil is in a photo with each.
    let same: Vec<(i64, i64, bool, usize)> =
        check.same_people.iter().map(|s| (s.person_id, s.other_id, s.same_name, s.together)).collect();
    assert_eq!(same, [(5, 6, false, 0), (1, 4, true, 0)]);
    assert!(check.same_people[0].similarity > 0.85);

    // Each suggestion, as the window and the CLI make it.
    faces.reassign(&cat, &t.elsewhere, &FaceTarget::Elsewhere).unwrap();
    assert_ne!(person_of(&cat, not_cy), 3);
    faces.reassign(&cat, &[brother], &FaceTarget::Elsewhere).unwrap();
    assert_ne!(person_of(&cat, brother), 1);
    let moved = faces.reassign(&cat, &[cy_in_bea], &FaceTarget::Person { id: 3 }).unwrap();
    assert_eq!(person_of(&cat, cy_in_bea), 3);
    let merged = faces.merge(&cat, &[1, 4], 1).unwrap();
    assert_eq!((merged.into, merged.name.as_deref(), merged.joined.len()), (1, Some("Ana"), 1));
    assert_eq!(
        (merged.joined[0].id, merged.joined[0].name.as_deref(), merged.joined[0].faces.len()),
        (4, Some("ANA"), 2)
    );

    // Checked again: only what was left alone is still there.
    let again = faces.check_people(&cat).unwrap();
    assert!(again.twice.is_empty() && again.age_jumps.is_empty(), "{again:?}");
    assert_eq!(again.like_others.iter().map(|l| l.face.face_id).collect::<Vec<_>>(), [like_cy]);
    assert_eq!(again.same_people.iter().map(|s| (s.person_id, s.other_id)).collect::<Vec<_>>(), [(5, 6)]);

    // Undo of the move and of the merge: as they were, and found again.
    faces.restore(&cat, &moved.undo).unwrap();
    assert_eq!(faces.unmerge(&cat, &merged).unwrap(), 2);
    let undone = faces.check_people(&cat).unwrap();
    assert_eq!(undone.like_others.iter().map(|l| l.face.face_id).collect::<Vec<_>>(), [cy_in_bea, like_cy]);
    assert_eq!(undone.same_people.iter().map(|s| (s.person_id, s.other_id)).collect::<Vec<_>>(), [(5, 6), (1, 4)]);
    let ana: Option<String> = cat.db().query_row("SELECT name FROM people WHERE id = 4", [], |r| r.get(0)).unwrap();
    assert_eq!(ana.as_deref(), Some("ANA"));
}

#[test]
fn an_unnamed_person_merged_into_gets_its_name_back_on_undo() {
    let (cat, p) = catalog("shoebox-people-unmerge", &[None, None, None]);
    cat.db().execute("INSERT INTO people (id, name, created_at) VALUES (1, NULL, 5), (2, 'Bo', 7)", []).unwrap();
    cat.db().execute("UPDATE people SET ignored = 1 WHERE id = 2", []).unwrap();
    let emb = fingerprints(&cat);
    let a = face(&cat, &emb, p[0], 1, true, &[(0, 1.0)]);
    let b = face(&cat, &emb, p[1], 2, false, &[(0, 1.0), (1, 0.2)]);
    let c = face(&cat, &emb, p[2], 2, false, &[(0, 1.0), (1, 0.1)]);
    drop(emb);
    let faces = FaceIndex::open(&cat).unwrap();

    // The unnamed person takes Bo's name, and Bo's faces.
    let merged = faces.merge(&cat, &[1, 2], 1).unwrap();
    assert_eq!((merged.name.clone(), merged.joined[0].faces.clone()), (None, vec![b, c]));
    let name: Option<String> = cat.db().query_row("SELECT name FROM people WHERE id = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(name.as_deref(), Some("Bo"));
    // A face moved on since stays where it is now - with someone new, who
    // took the id Bo left free.
    faces.reassign(&cat, &[c], &FaceTarget::New { name: None }).unwrap();
    let c_now = person_of(&cat, c);
    assert_eq!(c_now, 2);

    assert_eq!(faces.unmerge(&cat, &merged).unwrap(), 1);
    // Bo is back as he was, under a new id.
    let bo = person_of(&cat, b);
    assert!(![1, 2].contains(&bo), "{bo}");
    let back: (Option<String>, i64, i64) = cat
        .db()
        .query_row("SELECT name, ignored, created_at FROM people WHERE id = ?1", [bo], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(back, (Some("Bo".into()), 1, 7));
    let name: Option<String> = cat.db().query_row("SELECT name FROM people WHERE id = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(name, None);
    assert_eq!((person_of(&cat, a), person_of(&cat, c)), (1, c_now));
    // The averages followed: Bo's remaining face looks like Bo again.
    let alike = faces.look_alikes(&[b], &[], 3).unwrap();
    assert_eq!(alike.first().map(|l| l.person_id), Some(bo));
    assert!(alike[0].similarity > 0.99);
}

#[test]
fn the_check_and_undoing_a_merge_work_headless() {
    let (cat, p) = catalog("shoebox-people-check-cli", &[None, None, None]);
    cat.db().execute("INSERT INTO people (id, name, created_at) VALUES (1, 'Jo', 0), (2, 'jo', 0)", []).unwrap();
    let emb = fingerprints(&cat);
    face(&cat, &emb, p[0], 1, false, &[(0, 1.0)]);
    face(&cat, &emb, p[1], 1, false, &[(0, 1.0)]);
    face(&cat, &emb, p[2], 2, false, &[(1, 1.0)]);
    drop(emb);
    let cli = |args: &[&str]| -> serde_json::Value {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_shoebox-cli"))
            .args(args)
            .env("SHOEBOX_DATA", &cat.data.root)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    };
    let same = |check: &serde_json::Value| -> Vec<(i64, i64, bool)> {
        check["samePeople"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (s["personId"].as_i64().unwrap(), s["otherId"].as_i64().unwrap(), s["sameName"].as_bool().unwrap())
            })
            .collect()
    };

    // One name, two people: the one with more faces keeps it.
    let check = cli(&["people-check"]);
    assert_eq!((check["people"].as_i64(), check["faces"].as_i64()), (Some(2), Some(3)));
    assert_eq!(same(&check), [(1, 2, true)]);

    let merged = cli(&["people-merge", "2", "--into", "1"]);
    assert_eq!(merged["into"], 1);
    assert!(same(&cli(&["people-check"])).is_empty());
    let back = cli(&["people-unmerge", &merged.to_string()]);
    assert_eq!(back["facesBack"], 1);
    assert_eq!(same(&cli(&["people-check"])), [(1, 2, true)]);
}
