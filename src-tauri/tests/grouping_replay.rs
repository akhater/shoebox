//! How well grouping puts faces where the user would: on a copy of a real
//! catalog, the days holding faces placed by hand are taken out and found
//! again - photo by photo, newest first, then people found twice joined, as a
//! scan of new photos does - and each face placed by hand is checked against
//! the person the user gave it. A sample of other days checks faces Shoebox
//! placed in named people the same way.
//!
//! Not run with the other tests: it needs a catalog copy. Run it on one
//! (never the live catalog; it is copied again, so stays as it is):
//!
//!   SHOEBOX_REPLAY=<folder with catalog.db and faces.db> \
//!     cargo test --release --test grouping_replay -- --ignored --nocapture
//!
//! Lives here, not in src/, because it writes catalog copies (promise 1).
#![cfg(windows)]

use image::{DynamicImage, RgbImage};
use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::{Detected, FaceIndex};
use shoebox_lib::paths::DataDir;
use std::collections::{BTreeMap, HashMap, HashSet};

const FOLDS: i64 = 5;
const DAY: i64 = 86_400;
const YEAR: i64 = 31_557_600;

struct Face {
    id: i64,
    photo: i64,
    person: Option<i64>,
    by_hand: bool,
    named: bool,
    score: f32,
    taken: Option<i64>,
}

#[derive(Default, Clone, Copy)]
struct Tally {
    right: u32,
    wrong: u32,
    new: u32,
}

impl Tally {
    fn line(&self) -> String {
        let n = (self.right + self.wrong + self.new).max(1) as f64;
        let pc = |x: u32| 100.0 * x as f64 / n;
        format!(
            "right {:5.1}%  wrong person {:5.1}%  someone new {:5.1}%  ({} faces)",
            pc(self.right),
            pc(self.wrong),
            pc(self.new),
            self.right + self.wrong + self.new
        )
    }
}

fn day_of(f: &Face) -> Option<i64> {
    f.taken.filter(|t| *t > 0).map(|t| t.div_euclid(DAY))
}

#[test]
#[ignore]
fn replay_grouping_on_a_catalog_copy() {
    let Some(source) = std::env::var_os("SHOEBOX_REPLAY").map(std::path::PathBuf::from) else {
        eprintln!("SHOEBOX_REPLAY not set: nothing to replay");
        return;
    };
    let faces: Vec<Face> = {
        let db = rusqlite::Connection::open(source.join("catalog.db")).unwrap();
        db.prepare(
            "SELECT f.id, f.photo_id, f.person_id, f.by_hand, pe.name IS NOT NULL, f.score, p.taken_ts
               FROM faces f JOIN photos p ON p.id = f.photo_id LEFT JOIN people pe ON pe.id = f.person_id",
        )
        .unwrap()
        .query_map([], |r| {
            Ok(Face {
                id: r.get(0)?,
                photo: r.get(1)?,
                person: r.get(2)?,
                by_hand: r.get::<_, i64>(3)? != 0,
                named: r.get::<_, i64>(4)? != 0,
                score: r.get::<_, f64>(5)? as f32,
                taken: r.get(6)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };
    // Each person's first photo: under 10 years after it, someone first seen
    // after the catalog's first year counts as a child.
    let start = faces.iter().filter_map(|f| f.taken.filter(|t| *t > 0)).min().unwrap_or(0);
    let mut first: HashMap<i64, i64> = HashMap::new();
    for f in &faces {
        if let (Some(p), Some(t)) = (f.person, f.taken.filter(|t| *t > 0)) {
            let e = first.entry(p).or_insert(t);
            *e = (*e).min(t);
        }
    }
    let hand_days: HashSet<i64> = faces.iter().filter(|f| f.by_hand && f.person.is_some()).filter_map(day_of).collect();
    let sample_days: HashSet<i64> = faces
        .iter()
        .filter(|f| f.named && !f.by_hand)
        .filter_map(day_of)
        .filter(|d| !hand_days.contains(d) && d.rem_euclid(10) == 0)
        .collect();
    let fold_of = |d: i64| d.wrapping_mul(2_654_435_761).rem_euclid(FOLDS);
    let groups = |f: &Face| -> Vec<&'static str> {
        let p = f.person.unwrap();
        if !f.by_hand {
            return if f.named && day_of(f).is_some_and(|d| sample_days.contains(&d)) {
                vec!["named, placed by Shoebox (sample days)"]
            } else {
                vec![]
            };
        }
        let age = f.taken.map(|t| (t - first[&p]) as f64 / YEAR as f64);
        match age {
            Some(a) if first[&p] > start + YEAR && a < 10.0 => vec![
                "placed by hand: all",
                "placed by hand: children under 10",
                if a < 1.0 {
                    "placed by hand: children under 1"
                } else if a < 3.0 {
                    "placed by hand: children 1-3"
                } else {
                    "placed by hand: children 3-10"
                },
            ],
            _ => vec!["placed by hand: all", "placed by hand: everyone else"],
        }
    };

    let mut totals: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut left_out = 0;
    #[derive(Default)]
    struct Timing {
        open: f64,
        store: f64,
        photos: u32,
        join: f64,
    }
    let mut timing = Timing::default();
    let started = std::time::Instant::now();
    for k in 0..FOLDS {
        // Every face of the fold's days, by photo, newest first.
        let mut photos: BTreeMap<(i64, i64), Vec<&Face>> = BTreeMap::new();
        for f in &faces {
            if day_of(f).is_some_and(|d| (hand_days.contains(&d) || sample_days.contains(&d)) && fold_of(d) == k) {
                photos.entry((-f.taken.unwrap(), f.photo)).or_default().push(f);
            }
        }
        for fs in photos.values_mut() {
            fs.sort_by(|a, b| b.score.total_cmp(&a.score));
        }
        let held: HashSet<i64> = photos.values().flatten().map(|f| f.id).collect();

        let base = std::env::temp_dir().join(format!("shoebox-grouping-replay-{k}"));
        let _ = std::fs::remove_dir_all(&base);
        let data = DataDir { root: base.join("catalog") };
        data.ensure().unwrap();
        for file in ["catalog.db", "faces.db"] {
            std::fs::copy(source.join(file), data.root.join(file)).unwrap();
        }
        // Their fingerprints, then the faces taken out as if never looked at.
        let mut prints: HashMap<i64, Vec<f32>> = HashMap::new();
        {
            let emb = rusqlite::Connection::open(data.root.join("faces.db")).unwrap();
            let mut stmt = emb.prepare("SELECT vec FROM emb WHERE face_id = ?1").unwrap();
            for &id in &held {
                let blob: Option<Vec<u8>> = stmt.query_row([id], |r| r.get(0)).ok();
                if let Some(b) = blob.filter(|b| b.len() == 512 * 4) {
                    prints.insert(id, b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect());
                }
            }
            drop(stmt);
            let cat = rusqlite::Connection::open(data.root.join("catalog.db")).unwrap();
            let (tx, etx) = (cat.unchecked_transaction().unwrap(), emb.unchecked_transaction().unwrap());
            for &id in &held {
                tx.execute("DELETE FROM faces WHERE id = ?1", [id]).unwrap();
                etx.execute("DELETE FROM emb WHERE face_id = ?1", [id]).unwrap();
            }
            for (_, photo) in photos.keys() {
                tx.execute("DELETE FROM face_scan WHERE photo_id = ?1", [photo]).unwrap();
            }
            tx.commit().unwrap();
            etx.commit().unwrap();
        }
        let cat = Catalog::open(data.clone()).unwrap();
        let t = std::time::Instant::now();
        let index = FaceIndex::open(&cat).unwrap();
        timing.open += t.elapsed().as_secs_f64();
        let known: HashSet<i64> = cat
            .db()
            .prepare("SELECT DISTINCT person_id FROM faces WHERE person_id IS NOT NULL")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let last_person: i64 = cat.db().query_row("SELECT COALESCE(MAX(id), 0) FROM people", [], |r| r.get(0)).unwrap();

        // Found again. Faces that had nobody were too small to group: kept so.
        let img = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
        let mut now_is: HashMap<i64, i64> = HashMap::new();
        for ((_, photo), fs) in &photos {
            let found: Vec<(Detected, Vec<f32>)> = fs
                .iter()
                .filter(|f| prints.contains_key(&f.id))
                .enumerate()
                .map(|(i, f)| {
                    let side = if f.person.is_some() { 100.0 } else { 20.0 };
                    let (x, y) = ((i % 8) as f32 * 110.0, (i / 8 % 8) as f32 * 110.0);
                    (Detected { bbox: [x, y, x + side, y + side], kps: [0.0; 10], score: f.score }, prints[&f.id].clone())
                })
                .collect();
            let ids: Vec<i64> = fs.iter().filter(|f| prints.contains_key(&f.id)).map(|f| f.id).collect();
            let t = std::time::Instant::now();
            index.store(&cat, *photo, &img, found).unwrap();
            timing.store += t.elapsed().as_secs_f64();
            timing.photos += 1;
            let new_ids: Vec<i64> = cat
                .db()
                .prepare("SELECT id FROM faces WHERE photo_id = ?1 ORDER BY id")
                .unwrap()
                .query_map([photo], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(new_ids.len(), ids.len());
            now_is.extend(ids.into_iter().zip(new_ids));
        }
        let t = std::time::Instant::now();
        index.join_look_alikes(&cat).unwrap();
        timing.join += t.elapsed().as_secs_f64();

        for f in photos.values().flatten() {
            let (Some(truth), Some(&now)) = (f.person, now_is.get(&f.id)) else { continue };
            let names = groups(f);
            if names.is_empty() {
                continue;
            }
            if !known.contains(&truth) {
                left_out += 1;
                continue;
            }
            let person: Option<i64> =
                cat.db().query_row("SELECT person_id FROM faces WHERE id = ?1", [now], |r| r.get(0)).unwrap();
            for name in names {
                let t = totals.entry(name).or_default();
                match person {
                    Some(p) if p == truth => t.right += 1,
                    Some(p) if p > last_person => t.new += 1,
                    _ => t.wrong += 1,
                }
            }
        }
        drop((index, cat));
        let _ = std::fs::remove_dir_all(&base);
        eprintln!("fold {} of {FOLDS} done at {:.0} s", k + 1, started.elapsed().as_secs_f64());
    }
    println!(
        "\n{} days with faces placed by hand, {} sample days ({left_out} faces of people only seen those days left out)",
        hand_days.len(),
        sample_days.len()
    );
    for (name, t) in &totals {
        println!("{name:<42} {}", t.line());
    }
    println!(
        "faces read at open {:.2} s, a photo's faces stored in {:.1} ms, people found twice joined in {:.1} s (means of {FOLDS} runs)",
        timing.open / FOLDS as f64,
        1000.0 * timing.store / timing.photos.max(1) as f64,
        timing.join / FOLDS as f64
    );
}
