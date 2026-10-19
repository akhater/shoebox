//! Cull's assistant on a scanned folder: photos are measured from their
//! previews, the blurry and the empty frame are found, frames a second apart
//! make one stack whose sharpest frame is kept, a closer look at the
//! originals finds the frame only soft at 100%, and accepting its
//! suggestions marks the rejects for deletion. Measures are kept apart from
//! the catalog and read again when it opens.
//!
//! Lives here, not in src/, because it writes sample photos: no file under
//! src/ outside paths.rs may hold file-writing code (promise 1).
#![cfg(windows)]

use image::{Rgb, RgbImage};
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::cull::{Problem, QualityIndex, Suggest};
use shoebox_lib::focus::{self, Focus, Point, State};
use shoebox_lib::organize;
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use shoebox_lib::search::SearchIndex;
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// Squares of two greys (edges in both directions), blurred by `sigma` px.
fn squares(path: &Path, w: u32, h: u32, sigma: f32) {
    squares_of(path, w, h, sigma, 60);
}

/// [`squares`] of `side` px.
fn squares_of(path: &Path, w: u32, h: u32, sigma: f32, side: u32) {
    let img =
        RgbImage::from_fn(
            w,
            h,
            |x, y| if (x / side + y / side).is_multiple_of(2) { Rgb([60; 3]) } else { Rgb([190; 3]) },
        );
    let img = if sigma > 0.0 { image::imageops::blur(&img, sigma) } else { img };
    img.save(path).unwrap();
}

/// Tiles of 60 px, each its own grey (edges everywhere, no pattern), as a
/// camera shaken during the exposure leaves them: copies added together,
/// each `(weight, dx, dy)`, `dx, dy` px away.
fn shaken_tiles(path: &Path, w: u32, h: u32, copies: &[(f32, i32, i32)]) {
    let grey = |x: i32, y: i32| {
        let (x, y) = (x.clamp(0, w as i32 - 1) as u32 / 60, y.clamp(0, h as i32 - 1) as u32 / 60);
        let n = x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663);
        (40 + n.wrapping_mul(2_654_435_761) % 170) as f32
    };
    RgbImage::from_fn(w, h, |x, y| {
        let v: f32 = copies.iter().map(|&(weight, dx, dy)| weight * grey(x as i32 - dx, y as i32 - dy)).sum();
        Rgb([v.round() as u8; 3])
    })
    .save(path)
    .unwrap();
}

fn id_of(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

#[test]
fn a_shoot_is_measured_stacked_and_its_rejects_marked() {
    let base = std::env::temp_dir().join("shoebox-cull-assistant");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    // A burst: soft at 100% (3 px), then sharp. Then a frame blurred far
    // beyond (shake), a black frame (lens cap), and a sharp single.
    squares(&photos.join("IMG_0001.JPG"), 4000, 2667, 3.0);
    squares(&photos.join("IMG_0002.JPG"), 4000, 2667, 0.0);
    squares(&photos.join("IMG_0003.JPG"), 4000, 2667, 12.0);
    RgbImage::from_pixel(1200, 800, Rgb([3; 3])).save(photos.join("IMG_0004.JPG")).unwrap();
    squares_of(&photos.join("IMG_0005.JPG"), 4000, 2667, 0.0, 50);

    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    let root = cat.add_root(&photos).unwrap();
    let r = scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    assert_eq!((r.photos_added, r.errors), (5, 0), "{r:?}");
    let ids: Vec<i64> = (1..=5).map(|n| id_of(&cat, &format!("IMG_000{n}.JPG"))).collect();
    // The burst a second apart; the others a minute apart each.
    for (i, at) in [0, 1, 60, 120, 180].iter().enumerate() {
        cat.db().execute("UPDATE photos SET taken_ts = ?1 WHERE id = ?2", [1_700_000_000 + at, ids[i]]).unwrap();
    }

    let quality = QualityIndex::open(&cat).unwrap();
    quality.index_pending(&cat, &AtomicBool::new(false), &|_| {}).unwrap();
    assert_eq!(quality.measured(), 5);
    let search = SearchIndex::open(&cat).unwrap(); // no descriptions: bursts by time alone
    let scope = Scope::Root { id: root };
    let shoot = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap();
    let shape: Vec<Vec<i64>> = shoot.stacks.iter().map(|s| s.frames.iter().map(|f| f.id).collect()).collect();
    assert_eq!(shape, [vec![ids[0], ids[1]], vec![ids[2]], vec![ids[3]], vec![ids[4]]]);
    let suggested: Vec<Vec<Suggest>> =
        shoot.stacks.iter().map(|s| s.frames.iter().map(|f| f.suggest).collect()).collect();
    assert_eq!(
        suggested,
        [vec![Suggest::Reject, Suggest::Keep], vec![Suggest::Reject], vec![Suggest::Reject], vec![Suggest::Keep]],
        "{:#?}",
        shoot.stacks
    );
    assert_eq!(shoot.stacks[0].keeper, 1);
    assert_eq!(shoot.stacks[1].frames[0].problems, [Problem::Blurry]);
    assert_eq!(shoot.stacks[2].frames[0].problems, [Problem::Empty]);
    // On screen, the burst's softer frame looks fine: only a closer look says otherwise.
    assert!(shoot.stacks[0].frames[0].problems.is_empty(), "{:?}", shoot.stacks[0].frames[0]);
    // The grid's badges.
    let flags = quality.flags(&ids);
    assert_eq!(flags[2], Problem::Blurry.bit());
    assert_eq!(flags[1], 0);
    // And a photo's details (the info panel), with Cull's words.
    let shaken = quality.photo_detail(&cat, ids[2]).unwrap().problems;
    assert_eq!(shaken.iter().map(|f| f.problem).collect::<Vec<_>>(), [Problem::Blurry]);
    assert!(shaken[0].text.starts_with("Blurry everywhere"), "{shaken:?}");
    assert!(quality.photo_detail(&cat, ids[1]).unwrap().problems.is_empty());

    // The disk is here: every original can be looked at closer.
    assert_eq!(shoot.can_look_closer.len(), 5);
    let looked = quality.look_closer(&cat, &shoot.can_look_closer, &AtomicBool::new(false), &|_, _| {}).unwrap();
    assert_eq!(looked, 5);
    let shoot = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap();
    assert_eq!(shoot.closer, 5);
    assert!(shoot.can_look_closer.is_empty());
    let burst = &shoot.stacks[0];
    assert_eq!(burst.keeper, 1);
    assert!(burst.frames.iter().all(|f| f.closer));
    assert_eq!(burst.frames[0].problems, [Problem::Soft], "{:?}", quality.report(&cat, ids[0]).unwrap().quality);
    assert!(burst.frames[1].problems.is_empty(), "{:?}", quality.report(&cat, ids[1]).unwrap().quality);

    // What Cull suggests, done: the rejects marked for deletion, nothing else.
    let rejects = shoot.suggested();
    assert_eq!(rejects, [ids[0], ids[2], ids[3]]);
    organize::mark(&cat, &rejects, true).unwrap();
    let marked = cat.list_photos(&Scope::Marked, Sort::Oldest, &Filters::default()).unwrap().ids;
    assert_eq!(marked, [ids[0], ids[2], ids[3]]);
    // Marked frames get no new suggestion to accept; one taken back is suggested again.
    let shoot = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap();
    assert!(shoot.suggested().is_empty());
    organize::mark(&cat, &[ids[2]], false).unwrap();
    let shoot = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap();
    assert_eq!(shoot.suggested(), [ids[2]]);

    // The measures are kept: opened again, nothing to measure, closer looks still there.
    drop(quality);
    let quality = QualityIndex::open(&cat).unwrap();
    assert_eq!(quality.measured(), 5);
    assert!(quality.get(ids[0]).unwrap().close.is_some());
    let asked = std::cell::Cell::new(0);
    quality.index_pending(&cat, &AtomicBool::new(false), &|_| asked.set(asked.get() + 1)).unwrap();
    assert_eq!(asked.get(), 0, "nothing new to measure");

    drop((quality, search, cat));
    let _ = std::fs::remove_dir_all(&base);
}

/// A camera shaken during a long exposure leaves copies of the picture, each
/// with crisp edges: on screen the frame looks as sharp as the steady one,
/// and Cull already keeps the steady one of the burst; at 100% the copies
/// make it soft, and a shutter too fast for the hand to shake makes them a
/// pattern instead.
#[test]
fn a_frame_shaken_into_copies_is_found_at_100_percent() {
    let base = std::env::temp_dir().join("shoebox-cull-shake");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    // A burst at 1 s: steady, then twice the picture, the second copy
    // fainter, 26 px away. Then a picture doubled as much, taken at 1/500.
    let doubled = [(0.7, 0, 0), (0.3, 24, 10)];
    shaken_tiles(&photos.join("IMG_0001.JPG"), 4000, 2667, &[(1.0, 0, 0)]);
    shaken_tiles(&photos.join("IMG_0002.JPG"), 4000, 2667, &doubled);
    shaken_tiles(&photos.join("IMG_0003.JPG"), 4000, 2667, &[(0.7, 0, 0), (0.3, -20, 16)]);

    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let ids: Vec<i64> = (1..=3).map(|n| id_of(&cat, &format!("IMG_000{n}.JPG"))).collect();
    for (id, at, exposure) in [(ids[0], 0, "1s"), (ids[1], 1, "1s"), (ids[2], 60, "1/500")] {
        cat.db()
            .execute(
                "UPDATE photos SET taken_ts = ?1, exposure = ?2, focal = 35 WHERE id = ?3",
                rusqlite::params![1_700_000_000 + at, exposure, id],
            )
            .unwrap();
    }

    let quality = QualityIndex::open(&cat).unwrap();
    quality.index_pending(&cat, &AtomicBool::new(false), &|_| {}).unwrap();
    let search = SearchIndex::open(&cat).unwrap();
    let scope = Scope::Root { id: root };
    let shoot = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap();
    let shape: Vec<Vec<i64>> = shoot.stacks.iter().map(|s| s.frames.iter().map(|f| f.id).collect()).collect();
    assert_eq!(shape, [vec![ids[0], ids[1]], vec![ids[2]]]);
    // On screen nothing is wrong, but of the burst the steady frame is kept.
    let burst = &shoot.stacks[0];
    assert!(shoot.stacks.iter().flat_map(|s| &s.frames).all(|f| f.problems.is_empty()), "{:#?}", shoot.stacks);
    assert_eq!(burst.keeper, 0, "{:?}", quality.report(&cat, ids[1]).unwrap().quality);

    let looked = quality.look_closer(&cat, &shoot.can_look_closer, &AtomicBool::new(false), &|_, _| {}).unwrap();
    assert_eq!(looked, 3);
    let shoot = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap();
    let burst = &shoot.stacks[0];
    let report = quality.report(&cat, ids[1]).unwrap();
    assert_eq!(burst.frames[1].problems, [Problem::Soft], "{:?}", report.quality);
    assert_eq!(
        report.problems[0].text,
        "Shaken at 100%: the picture is doubled (1 s at 35 mm is slow for a steady hand)"
    );
    assert_eq!((burst.keeper, burst.frames[0].suggest, burst.frames[1].suggest), (0, Suggest::Keep, Suggest::Reject));
    assert_eq!(quality.flags(&ids), [0, Problem::Soft.bit(), 0]);
    // Copies at 1/500: not shake.
    assert!(shoot.stacks[1].frames[0].problems.is_empty(), "{:?}", quality.report(&cat, ids[2]).unwrap().quality);
    assert_eq!(shoot.stacks[1].frames[0].suggest, Suggest::Keep);

    drop((quality, search, cat));
    let _ = std::fs::remove_dir_all(&base);
}

/// An original that can't be read for a closer look (gone from the disk since
/// the last scan) is named with why, and no longer offered.
#[test]
fn an_original_that_cant_be_read_is_named_and_not_offered_again() {
    let base = std::env::temp_dir().join("shoebox-cull-unreadable");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    squares(&photos.join("IMG_0001.JPG"), 2000, 1333, 0.0);
    squares_of(&photos.join("IMG_0002.JPG"), 2000, 1333, 0.0, 50);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let (one, two) = (id_of(&cat, "IMG_0001.JPG"), id_of(&cat, "IMG_0002.JPG"));
    std::fs::remove_file(photos.join("IMG_0002.JPG")).unwrap();

    let quality = QualityIndex::open(&cat).unwrap();
    quality.index_pending(&cat, &AtomicBool::new(false), &|_| {}).unwrap();
    let search = SearchIndex::open(&cat).unwrap();
    let scope = Scope::Root { id: root };
    let shoot = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap();
    assert_eq!(shoot.can_look_closer, [one, two]);
    let looked = quality.look_closer(&cat, &shoot.can_look_closer, &AtomicBool::new(false), &|_, _| {}).unwrap();
    assert_eq!(looked, 1);
    let shoot = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap();
    assert!(shoot.can_look_closer.is_empty(), "{:?}", shoot.can_look_closer);
    assert_eq!(shoot.unreadable.len(), 1);
    assert_eq!((shoot.unreadable[0].id, shoot.unreadable[0].name.as_deref()), (two, Some("IMG_0002.JPG")));
    assert!(!shoot.unreadable[0].reason.is_empty());

    drop((quality, search, cat));
    let _ = std::fs::remove_dir_all(&base);
}

/// Where the camera focused leads Cull: its first look at 100% goes there,
/// turned with the photo, and a frame soft there while sharp elsewhere is
/// found. Measures kept from before Cull knew focus points still open, and a
/// photo whose points reach the catalog after it was measured is measured again.
#[test]
fn the_camera_focus_point_leads_cull() {
    let base = std::env::temp_dir().join("shoebox-cull-focus");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    // Sharp, but for a patch on the right whose own edges are blurred.
    let tile = |x: u32, y: u32| if (x / 30 + y / 30).is_multiple_of(2) { Rgb([60; 3]) } else { Rgb([190; 3]) };
    let sharp = RgbImage::from_fn(1200, 800, tile);
    let soft = image::imageops::blur(&sharp, 6.0);
    let in_patch = |x: u32, y: u32| (720..1080).contains(&x) && (240..560).contains(&y);
    RgbImage::from_fn(1200, 800, |x, y| if in_patch(x, y) { *soft.get_pixel(x, y) } else { *sharp.get_pixel(x, y) })
        .save(photos.join("IMG_0001.JPG"))
        .unwrap();

    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data.clone()).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let id = id_of(&cat, "IMG_0001.JPG");
    // Measures kept by a Shoebox from before focus points.
    rusqlite::Connection::open(data.root.join("quality.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE quality (photo_id INTEGER PRIMARY KEY, fingerprint TEXT NOT NULL, version INTEGER NOT NULL,
                                   faces INTEGER NOT NULL, preview TEXT NOT NULL, close TEXT);",
        )
        .unwrap();

    let quality = QualityIndex::open(&cat).unwrap();
    quality.index_pending(&cat, &AtomicBool::new(false), &|_| {}).unwrap();
    let search = SearchIndex::open(&cat).unwrap();
    let scope = Scope::Root { id: root };
    let frame = || quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap().stacks[0].frames[0].clone();
    let f = frame();
    assert!(f.problems.is_empty() && f.focus.is_none() && f.focus_points.is_empty(), "{f:?}");

    // Its file's focus points read since (as when its disk is plugged in):
    // the camera found focus in the middle of the soft patch.
    let point = Point { x: 0.73, y: 0.47, w: 0.04, h: 0.06, state: State::InFocus };
    let said = Focus { mode: Some("Single point"), points: vec![point] };
    {
        let db = cat.db();
        let blob = focus::stored(&db, Some(&said)).unwrap();
        db.execute("UPDATE photos SET focus = ?1 WHERE id = ?2", rusqlite::params![blob, id]).unwrap();
    }
    let asked = std::cell::Cell::new(0);
    quality.index_pending(&cat, &AtomicBool::new(false), &|_| asked.set(asked.get() + 1)).unwrap();
    assert!(asked.get() > 0, "measured again");
    let f = frame();
    assert_eq!(f.problems, [Problem::FocusMissed], "{:?}", quality.report(&cat, id).unwrap().quality);
    assert_eq!(f.suggest, Suggest::None);
    assert!(f.reasons[0].starts_with("Focus missed"), "{:?}", f.reasons);
    let near = |got: Option<[f32; 2]>, (x, y): (f32, f32)| {
        got.is_some_and(|g| (g[0] - x).abs() < 0.005 && (g[1] - y).abs() < 0.005)
    };
    assert!(near(f.focus, (0.75, 0.5)), "{:?}", f.focus);
    assert_eq!(f.focus_points.len(), 1);
    assert_eq!(quality.flags(&[id]), [Problem::FocusMissed.bit()]);

    // Opened again: nothing to measure.
    drop(quality);
    let quality = QualityIndex::open(&cat).unwrap();
    asked.set(0);
    quality.index_pending(&cat, &AtomicBool::new(false), &|_| asked.set(asked.get() + 1)).unwrap();
    assert_eq!(asked.get(), 0, "nothing new to measure");

    // Turned a quarter clockwise: the point goes with the photo.
    cat.db().execute("UPDATE photos SET turn = 1 WHERE id = ?1", [id]).unwrap();
    let f = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap().stacks[0].frames[0].clone();
    assert!(near(f.focus, (0.5, 0.75)), "{:?}", f.focus);

    drop((quality, search, cat));
    let _ = std::fs::remove_dir_all(&base);
}
