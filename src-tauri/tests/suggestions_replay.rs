//! How well "Who is this" suggestions (`look_alikes`) and "Find more"
//! (`more_of`) find the person the user chose, on a copy of a real catalog.
//! The days holding faces placed by hand are taken out a fifth at a time
//! (whole days, so a face's own photo session doesn't give it away), then:
//!
//! - **Held out**: those faces are taken out of their people (nobody's, as a
//!   face too small to group) and each is asked "who is this".
//! - **Not them**: a child's faces placed by hand those days go to whoever
//!   else they look like most ("not them, don't know who"), and the child is
//!   asked "find more".
//! - **Grouped again**: those days' photos are found again as a scan of new
//!   photos does (as `grouping_replay`), then people found twice joined.
//!   Each face placed by hand that grouping put in someone else is asked
//!   "who is this", not counting that someone, as the window asks; and each
//!   child is asked "find more", counting the faces offered: theirs grouping
//!   missed, and anyone else's.
//!
//! The person the user gave a face is the answer. Suggestions are counted as
//! the window shows them: named people first, then groups seen in 2+ photos,
//! 5 at most.
//!
//! Not run with the other tests: it needs a catalog copy. Run it on one
//! (never the live catalog; it is copied again, so stays as it is):
//!
//!   SHOEBOX_REPLAY=<folder with catalog.db and faces.db> \
//!     cargo test --release --test suggestions_replay -- --ignored --nocapture
//!
//! Lives here, not in src/, because it writes catalog copies (promise 1).
#![cfg(windows)]

use image::{DynamicImage, RgbImage};
use shoebox_lib::catalog::Catalog;
use shoebox_lib::faces::{CandidateDay, Detected, FaceIndex, FaceTarget, Placement};
use shoebox_lib::paths::DataDir;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

const FOLDS: i64 = 5;
const DAY: i64 = 86_400;
const YEAR: i64 = 31_557_600;
/// Children with at least this many faces placed by hand are asked "find more".
const CHILD_FACES: usize = 10;

struct Face {
    id: i64,
    photo: i64,
    person: Option<i64>,
    by_hand: bool,
    seen: bool,
    score: f32,
    taken: Option<i64>,
}

fn day_of(f: &Face) -> Option<i64> {
    f.taken.filter(|t| *t > 0).map(|t| t.div_euclid(DAY))
}

fn fold_of(d: i64) -> i64 {
    d.wrapping_mul(2_654_435_761).rem_euclid(FOLDS)
}

/// Where the right person came among the suggestions.
#[derive(Default)]
struct Ranks {
    first: u32,
    top3: u32,
    shown: u32,
    missing: u32,
}

impl Ranks {
    fn add(&mut self, rank: Option<usize>) {
        match rank {
            Some(0) => self.first += 1,
            Some(1..=2) => self.top3 += 1,
            Some(_) => self.shown += 1,
            None => self.missing += 1,
        }
    }

    fn line(&self) -> String {
        let n = (self.first + self.top3 + self.shown + self.missing).max(1) as f64;
        let pc = |x: u32| 100.0 * x as f64 / n;
        format!(
            "first {:5.1}%  top 3 {:5.1}%  shown {:5.1}%  ({} faces)",
            pc(self.first),
            pc(self.first + self.top3),
            pc(self.first + self.top3 + self.shown),
            n as u32
        )
    }
}

/// "Find more" for one child, or all.
#[derive(Default)]
struct Offered {
    /// Their faces placed by hand that grouping put elsewhere.
    to_find: usize,
    found: usize,
    found_ticked: usize,
    /// Faces of someone else: placed by hand in someone else, or unknown.
    others_known: usize,
    others_unknown: usize,
    others_known_ticked: usize,
    others_unknown_ticked: usize,
}

impl Offered {
    fn add(&mut self, o: &Offered) {
        self.to_find += o.to_find;
        self.found += o.found;
        self.found_ticked += o.found_ticked;
        self.others_known += o.others_known;
        self.others_unknown += o.others_unknown;
        self.others_known_ticked += o.others_known_ticked;
        self.others_unknown_ticked += o.others_unknown_ticked;
    }

    fn line(&self) -> String {
        format!(
            "{} to find: offered {} ({} ticked); others offered {} known + {} unsure ({} + {} ticked)",
            self.to_find,
            self.found,
            self.found_ticked,
            self.others_known,
            self.others_unknown,
            self.others_known_ticked,
            self.others_unknown_ticked
        )
    }
}

fn read_faces(catalog: &Path, faces_db: &Path) -> Vec<Face> {
    let printed: HashSet<i64> = rusqlite::Connection::open(faces_db)
        .unwrap()
        .prepare("SELECT face_id FROM emb")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    rusqlite::Connection::open(catalog)
        .unwrap()
        .prepare(
            "SELECT f.id, f.photo_id, f.person_id, f.by_hand, f.seen, f.score, p.taken_ts
               FROM faces f JOIN photos p ON p.id = f.photo_id",
        )
        .unwrap()
        .query_map([], |r| {
            Ok(Face {
                id: r.get(0)?,
                photo: r.get(1)?,
                person: r.get(2)?,
                by_hand: r.get::<_, i64>(3)? != 0,
                seen: r.get::<_, i64>(4)? != 0,
                score: r.get::<_, f64>(5)? as f32,
                taken: r.get(6)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .filter(|f: &Face| printed.contains(&f.id))
        .collect()
}

fn copy_of(source: &Path, name: &str) -> (std::path::PathBuf, DataDir) {
    let base = std::env::temp_dir().join(format!("shoebox-suggestions-replay-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    for file in ["catalog.db", "faces.db"] {
        std::fs::copy(source.join(file), data.root.join(file)).unwrap();
    }
    (base, data)
}

fn ids(cat: &Catalog, sql: &str) -> HashSet<i64> {
    cat.db().prepare(sql).unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect()
}

/// The suggestions for some faces as the window shows them: named people
/// first, then groups seen in 2+ photos, ignored strangers left out, 5 at most.
fn shown(index: &FaceIndex, cat: &Catalog, faces: &[i64], except: &[i64], took: &mut (f64, u32)) -> Vec<i64> {
    let t = std::time::Instant::now();
    let alike = index.look_alikes(cat, faces, except, 12).unwrap();
    took.0 += t.elapsed().as_secs_f64();
    took.1 += 1;
    let db = cat.db();
    let mut who = db
        .prepare(
            "SELECT p.name IS NOT NULL, p.ignored, (SELECT COUNT(DISTINCT photo_id) FROM faces WHERE person_id = p.id)
               FROM people p WHERE p.id = ?1",
        )
        .unwrap();
    let (mut named, mut unnamed) = (Vec::new(), Vec::new());
    for l in alike {
        let (has_name, ignored, photos): (bool, bool, i64) =
            who.query_row([l.person_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        if ignored {
            continue;
        }
        if has_name {
            named.push(l.person_id);
        } else if photos > 1 {
            unnamed.push(l.person_id);
        }
    }
    named.extend(unnamed);
    named.truncate(5);
    named
}

#[test]
#[ignore]
fn replay_suggestions_on_a_catalog_copy() {
    let Some(source) = std::env::var_os("SHOEBOX_REPLAY").map(std::path::PathBuf::from) else {
        eprintln!("SHOEBOX_REPLAY not set: nothing to replay");
        return;
    };
    let faces = read_faces(&source.join("catalog.db"), &source.join("faces.db"));
    let named: HashSet<i64> = {
        let db = rusqlite::Connection::open(source.join("catalog.db")).unwrap();
        db.prepare("SELECT id FROM people WHERE name IS NOT NULL")
            .unwrap()
            .query_map([], |r| r.get(0))
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
    let age = |f: &Face| -> Option<f64> {
        let p = f.person?;
        let t = f.taken.filter(|t| *t > 0)?;
        (first[&p] > start + YEAR).then(|| (t - first[&p]) as f64 / YEAR as f64).filter(|a| *a < 10.0)
    };
    let groups = |f: &Face| -> Vec<&'static str> {
        match age(f) {
            Some(a) => vec![
                "all",
                "children under 10",
                if a < 1.0 {
                    "children under 1"
                } else if a < 3.0 {
                    "children 1-3"
                } else {
                    "children 3-10"
                },
            ],
            None => vec!["all", "everyone else"],
        }
    };
    let hand: Vec<&Face> = faces.iter().filter(|f| f.by_hand && f.person.is_some() && day_of(f).is_some()).collect();
    let hand_days: HashSet<i64> = hand.iter().filter_map(|f| day_of(f)).collect();
    let mut children: HashMap<i64, usize> = HashMap::new();
    for f in &hand {
        if age(f).is_some() && named.contains(&f.person.unwrap()) {
            *children.entry(f.person.unwrap()).or_default() += 1;
        }
    }
    children.retain(|_, n| *n >= CHILD_FACES);
    let started = std::time::Instant::now();
    let mut asking = (0f64, 0u32);
    let mut finding = (0f64, 0u32);
    let mut not_them: BTreeMap<i64, Offered> = BTreeMap::new();

    // Held out: each fold's faces placed by hand belong to nobody while asked.
    let mut held_out: BTreeMap<&'static str, Ranks> = BTreeMap::new();
    let mut left_out = 0;
    {
        let (base, data) = copy_of(&source, "held");
        let cat = Catalog::open(data.clone()).unwrap();
        let index = FaceIndex::open(&cat).unwrap();
        for k in 0..FOLDS {
            let held: Vec<&Face> =
                hand.iter().copied().filter(|f| day_of(f).is_some_and(|d| fold_of(d) == k)).collect();
            let nobody: Vec<Placement> =
                held.iter().map(|f| Placement { face_id: f.id, person_id: None, by_hand: true, seen: true }).collect();
            index.restore(&cat, &nobody).unwrap();
            let left = ids(&cat, "SELECT DISTINCT person_id FROM faces WHERE person_id IS NOT NULL");
            for f in &held {
                let truth = f.person.unwrap();
                if !left.contains(&truth) {
                    left_out += 1;
                    continue;
                }
                let rank = shown(&index, &cat, &[f.id], &[], &mut asking).iter().position(|&p| p == truth);
                for g in groups(f) {
                    held_out.entry(g).or_default().add(rank);
                }
            }
            let back: Vec<Placement> = held
                .iter()
                .map(|f| Placement { face_id: f.id, person_id: f.person, by_hand: f.by_hand, seen: f.seen })
                .collect();
            index.restore(&cat, &back).unwrap();
            eprintln!("held out: fold {} of {FOLDS} done at {:.0} s", k + 1, started.elapsed().as_secs_f64());
        }
        // Find more after "not them": each child's faces placed by hand on a
        // fold's days go to whoever else they look like most.
        let hand_now: HashMap<i64, i64> = hand.iter().map(|f| (f.id, f.person.unwrap())).collect();
        let mut kids: Vec<i64> = children.keys().copied().collect();
        kids.sort_unstable();
        for child in kids {
            let o = not_them.entry(child).or_default();
            for k in 0..FOLDS {
                let held: HashSet<i64> = hand
                    .iter()
                    .filter(|f| f.person == Some(child) && day_of(f).is_some_and(|d| fold_of(d) == k))
                    .map(|f| f.id)
                    .collect();
                if held.is_empty() {
                    continue;
                }
                let ids: Vec<i64> = held.iter().copied().collect();
                let moved = index.reassign(&cat, &ids, &FaceTarget::Elsewhere).unwrap();
                let t = std::time::Instant::now();
                let days = index.more_of(&cat, child).unwrap();
                finding.0 += t.elapsed().as_secs_f64();
                finding.1 += 1;
                o.to_find += held.len();
                count(o, &days, &held, &hand_now, child);
                index.restore(&cat, &moved.undo).unwrap();
            }
        }
        drop((index, cat));
        let _ = std::fs::remove_dir_all(&base);
    }

    // Grouped again, fold by fold.
    let mut regrouped: BTreeMap<&'static str, Ranks> = BTreeMap::new();
    let mut grouped_right: BTreeMap<&'static str, u32> = BTreeMap::new();
    let mut more: BTreeMap<i64, Offered> = BTreeMap::new();
    for k in 0..FOLDS {
        let mut photos: BTreeMap<(i64, i64), Vec<&Face>> = BTreeMap::new();
        for f in &faces {
            if day_of(f).is_some_and(|d| hand_days.contains(&d) && fold_of(d) == k) {
                photos.entry((-f.taken.unwrap(), f.photo)).or_default().push(f);
            }
        }
        for fs in photos.values_mut() {
            fs.sort_by(|a, b| b.score.total_cmp(&a.score));
        }
        let held: HashSet<i64> = photos.values().flatten().map(|f| f.id).collect();
        let (base, data) = copy_of(&source, &format!("fold{k}"));
        // Their fingerprints, then the faces taken out as if never looked at.
        let mut prints: HashMap<i64, Vec<f32>> = HashMap::new();
        {
            let emb = rusqlite::Connection::open(data.root.join("faces.db")).unwrap();
            let mut stmt = emb.prepare("SELECT vec FROM emb WHERE face_id = ?1").unwrap();
            for &id in &held {
                let blob: Option<Vec<u8>> = stmt.query_row([id], |r| r.get(0)).ok();
                if let Some(b) = blob.filter(|b| b.len() == 512 * 4) {
                    prints.insert(id, b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect());
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
        let index = FaceIndex::open(&cat).unwrap();
        let known = ids(&cat, "SELECT DISTINCT person_id FROM faces WHERE person_id IS NOT NULL");

        // Found again. Faces that had nobody were too small to group: kept so.
        let img = DynamicImage::ImageRgb8(RgbImage::new(1000, 1000));
        let mut now_is: HashMap<i64, i64> = HashMap::new();
        for ((_, photo), fs) in &photos {
            let fs: Vec<&&Face> = fs.iter().filter(|f| prints.contains_key(&f.id)).collect();
            let found: Vec<(Detected, Vec<f32>)> = fs
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let side = if f.person.is_some() { 100.0 } else { 20.0 };
                    let (x, y) = ((i % 8) as f32 * 110.0, (i / 8 % 8) as f32 * 110.0);
                    (
                        Detected { bbox: [x, y, x + side, y + side], kps: [0.0; 10], score: f.score },
                        prints[&f.id].clone(),
                    )
                })
                .collect();
            index.store(&cat, *photo, &img, found).unwrap();
            let new_ids: Vec<i64> = cat
                .db()
                .prepare("SELECT id FROM faces WHERE photo_id = ?1 ORDER BY id")
                .unwrap()
                .query_map([photo], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(new_ids.len(), fs.len());
            now_is.extend(fs.iter().map(|f| f.id).zip(new_ids));
        }
        index.join_look_alikes(&cat).unwrap();
        let person_of = |face: i64| -> Option<i64> {
            cat.db().query_row("SELECT person_id FROM faces WHERE id = ?1", [face], |r| r.get(0)).unwrap()
        };

        // Who is this, for the faces grouping put in someone else.
        let fold_hand: Vec<&Face> = photos
            .values()
            .flatten()
            .copied()
            .filter(|f| f.by_hand && f.person.is_some() && now_is.contains_key(&f.id))
            .collect();
        for f in &fold_hand {
            let truth = f.person.unwrap();
            if !known.contains(&truth) {
                continue;
            }
            let face = now_is[&f.id];
            let now = person_of(face);
            if now == Some(truth) {
                for g in groups(f) {
                    *grouped_right.entry(g).or_default() += 1;
                }
                continue;
            }
            let except: Vec<i64> = now.into_iter().collect();
            let rank = shown(&index, &cat, &[face], &except, &mut asking).iter().position(|&p| p == truth);
            for g in groups(f) {
                regrouped.entry(g).or_default().add(rank);
            }
        }

        // Find more, for each child with faces placed by hand these days.
        // Faces placed by hand, as they are now: whose the user said they are.
        let mut hand_now: HashMap<i64, i64> = HashMap::new();
        for f in faces.iter().filter(|f| f.by_hand && f.person.is_some()) {
            let id = if held.contains(&f.id) { now_is.get(&f.id).copied() } else { Some(f.id) };
            if let Some(id) = id {
                hand_now.insert(id, f.person.unwrap());
            }
        }
        let mut asked: Vec<i64> =
            fold_hand.iter().map(|f| f.person.unwrap()).filter(|p| children.contains_key(p)).collect();
        asked.sort_unstable();
        asked.dedup();
        for child in asked {
            if !known.contains(&child) {
                continue;
            }
            let theirs: HashSet<i64> = fold_hand
                .iter()
                .filter(|f| f.person == Some(child))
                .map(|f| now_is[&f.id])
                .filter(|&id| person_of(id) != Some(child))
                .collect();
            let t = std::time::Instant::now();
            let days = index.more_of(&cat, child).unwrap();
            finding.0 += t.elapsed().as_secs_f64();
            finding.1 += 1;
            let o = more.entry(child).or_default();
            o.to_find += theirs.len();
            count(o, &days, &theirs, &hand_now, child);
        }
        drop((index, cat));
        let _ = std::fs::remove_dir_all(&base);
        eprintln!("grouped again: fold {} of {FOLDS} done at {:.0} s", k + 1, started.elapsed().as_secs_f64());
    }

    println!("\n{} days with faces placed by hand, in {FOLDS} folds", hand_days.len());
    println!("\nWho is this, faces placed by hand held out ({left_out} of people seen only those days left out):");
    for (g, r) in &held_out {
        println!("  {g:<20} {}", r.line());
    }
    println!("\nWho is this, faces placed by hand that grouping put in someone else:");
    for (g, r) in &regrouped {
        println!("  {g:<20} {}  ({} grouped right)", r.line(), grouped_right.get(g).copied().unwrap_or(0));
    }
    println!("  {:.1} ms a question", 1000.0 * asking.0 / asking.1.max(1) as f64);
    for (title, offered) in [
        ("Find more, after \"not them\" for a child's faces placed by hand those days", &not_them),
        ("Find more, after grouping those days again", &more),
    ] {
        println!("\n{title} (children with {CHILD_FACES}+ faces placed by hand):");
        let mut total = Offered::default();
        for (child, o) in offered {
            println!("  child {child:<6} {}", o.line());
            total.add(o);
        }
        println!("  all          {}", total.line());
    }
    println!("  {:.0} ms a question", 1000.0 * finding.0 / finding.1.max(1) as f64);
}

/// Counts what "find more" offered `child`: `theirs` (or theirs placed by
/// hand), faces placed by hand in someone else, and the rest.
fn count(o: &mut Offered, days: &[CandidateDay], theirs: &HashSet<i64>, hand: &HashMap<i64, i64>, child: i64) {
    for d in days {
        for c in &d.faces {
            if theirs.contains(&c.face_id) || hand.get(&c.face_id) == Some(&child) {
                o.found += 1;
                o.found_ticked += d.near as usize;
            } else if hand.contains_key(&c.face_id) {
                o.others_known += 1;
                o.others_known_ticked += d.near as usize;
            } else {
                o.others_unknown += 1;
                o.others_unknown_ticked += d.near as usize;
            }
        }
    }
}
