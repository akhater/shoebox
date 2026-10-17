//! Cull's assistant on a scanned folder: photos are measured from their
//! previews, the blurry and the empty frame are found, frames a second apart
//! make one stack whose sharpest frame is kept, a closer look at the
//! originals finds the frame only soft at 100%, and the user's decisions are
//! picks or marks - a pick is never also marked. Measures are kept apart
//! from the catalog and read again when it opens.
//!
//! Lives here, not in src/, because it writes sample photos: no file under
//! src/ outside paths.rs may hold file-writing code (promise 1).
#![cfg(windows)]

use image::{Rgb, RgbImage};
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::cull::{self, Problem, QualityIndex, Suggest};
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

fn id_of(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

#[test]
fn a_shoot_is_measured_stacked_and_decided() {
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
        [vec![Suggest::Reject, Suggest::Pick], vec![Suggest::Reject], vec![Suggest::Reject], vec![Suggest::Pick]],
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

    // What Cull suggests, done: the keepers picked, the rest marked.
    let (picks, rejects) = shoot.suggested();
    assert_eq!((picks.as_slice(), rejects.as_slice()), (&[ids[1], ids[4]][..], &[ids[0], ids[2], ids[3]][..]));
    cull::decide(&cat, &picks, &rejects, &[]).unwrap();
    let picked = |f: &[&str]| {
        let filters = Filters { cull: f.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        cat.list_photos(&scope, Sort::Oldest, &filters).unwrap().ids
    };
    assert_eq!(picked(&["picked"]), [ids[1], ids[4]]);
    assert_eq!(picked(&["rejected"]), [ids[0], ids[2], ids[3]]);
    assert!(picked(&["undecided"]).is_empty());
    // Decided frames get no new suggestion to accept.
    let shoot = quality.shoot(&cat, &search, &scope, &Filters::default()).unwrap();
    assert_eq!(shoot.suggested(), (vec![], vec![]));
    let facets = cat.facets(&scope, &Filters::default()).unwrap();
    let counts: Vec<(String, i64)> = facets.cull.iter().map(|v| (v.value.clone(), v.count)).collect();
    assert_eq!(counts, [("picked".to_string(), 2), ("rejected".to_string(), 3)]);

    // A mark takes the pick away; a pick takes the mark away.
    organize::mark(&cat, &[ids[1]], true).unwrap();
    assert_eq!(picked(&["picked"]), [ids[4]]);
    cull::decide(&cat, &[ids[0]], &[], &[]).unwrap();
    assert_eq!(picked(&["picked"]), [ids[0], ids[4]]);
    assert_eq!(picked(&["rejected"]), [ids[1], ids[2], ids[3]]);
    cull::unpick(&cat, &[ids[0]]).unwrap();
    assert_eq!(picked(&["undecided"]), [ids[0]]);

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
