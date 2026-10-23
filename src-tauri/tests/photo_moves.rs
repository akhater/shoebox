//! Photos moved into another folder, one or many, on the disk and in the
//! catalog in one step: renamed on their own disk, or copied to another one,
//! checked byte for byte and the original removed. Never over a file already
//! there, never a photo without its edits, and a move stopped dead at any step
//! is settled by the next start: finished if the catalog had taken it,
//! undone otherwise.
//!
//! The world is kill_safe_organize.rs's: disk A holds Photos (a source) with
//! Trip (IMG_0000-0003, Argentum's edits of IMG_0000, and Day 2 with
//! IMG_0004-0005) and an empty 2026; disk B holds Archive (a source), empty.

use crate::kill_safe_organize::{TRIP, World, done_state, jpeg, killed, native};
use shoebox_lib::catalog::Catalog;
use shoebox_lib::organize::{self, PhotoMove, PhotoMoveResult};
use shoebox_lib::scan::scan_root;
use std::sync::atomic::AtomicBool;

const Y2026: &str = r"Photos\2026";

fn photo(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

fn photos(cat: &Catalog, names: &[&str]) -> Vec<i64> {
    names.iter().map(|n| photo(cat, n)).collect()
}

fn into(cat: &Catalog, names: &[&str], to: i64, to_rel: &str) -> PhotoMove {
    PhotoMove { photo_ids: photos(cat, names), from: None, to, to_rel: to_rel.into(), number_clashes: false }
}

fn go(cat: &Catalog, m: &PhotoMove) -> anyhow::Result<PhotoMoveResult> {
    organize::move_photos(cat, m, None, &AtomicBool::new(false), &|_| {})
}

fn back(cat: &Catalog, r: &PhotoMoveResult) -> PhotoMoveResult {
    organize::move_back(cat, &r.moved, &AtomicBool::new(false), &|_| {}).unwrap()
}

/// The folder (as the catalog keeps it) and name of each file the catalog knows
/// on a disk, sorted.
fn cataloged(w: &World, cat: &Catalog, disk: i64) -> Vec<String> {
    w.state(cat).files.iter().filter(|f| f.0 == disk).map(|f| format!(r"{}\{}", f.1, f.2)).collect()
}

fn rescan(cat: &Catalog) {
    for id in cat.root_ids().unwrap() {
        if cat.mount_of(cat.root(id).unwrap().volume_id).is_some() {
            scan_root(cat, id, &AtomicBool::new(false), false, &|_| {}).unwrap();
        }
    }
}

#[test]
fn photos_move_on_their_disk_with_their_edits_and_come_back() {
    let w = World::new("photos-same");
    let (cat, a, _) = w.open(&["a", "b"]);
    let before = w.state(&cat);
    // Seen in Trip, which holds Day 2: both come from there.
    let m = PhotoMove { from: Some((a, TRIP.into())), ..into(&cat, &["IMG_0000.JPG", "IMG_0004.JPG"], a, Y2026) };
    let plan = organize::plan_photo_move(&cat, &m).unwrap();
    assert_eq!((plan.photos, plan.files.len(), plan.copy_bytes, plan.left.len()), (2, 3, 0, 0));
    assert_eq!(plan.files.iter().filter(|f| f.role == "edits").count(), 1);
    let r = organize::move_photos(&cat, &m, Some(&plan.token), &AtomicBool::new(false), &|_| {}).unwrap();
    assert_eq!((r.photos, r.files, r.copied, r.numbered, r.failed.len()), (2, 3, 0, 0, 0));

    let now = w.state(&cat);
    for f in ["IMG_0000.JPG", "IMG_0000.JPG.agdata", "IMG_0004.JPG"] {
        assert!(now.disks.contains_key(&format!(r"disk-a\Photos\2026\{f}")), "{f}");
    }
    assert!(!now.disks.contains_key(r"disk-a\Photos\Trip\IMG_0000.JPG"));
    assert!(!now.disks.contains_key(r"disk-a\Photos\Trip\IMG_0000.JPG.agdata"));
    assert!(!now.disks.contains_key(r"disk-a\Photos\Trip\Day 2\IMG_0004.JPG"));
    let files = cataloged(&w, &cat, a);
    assert!(files.contains(&r"Photos\2026\IMG_0000.JPG".to_string()), "{files:?}");
    assert!(files.contains(&r"Photos\2026\IMG_0004.JPG".to_string()), "{files:?}");
    // Same photos, same count: nothing read again, nothing new.
    assert_eq!(now.photos, before.photos);
    w.settled(&cat);
    w.rescan_changes_nothing(&cat);

    // Undo. The catalog now knows 2026 (empty again, as "New folder" leaves one).
    let undone = back(&cat, &r);
    assert_eq!((undone.files, undone.failed.len()), (3, 0));
    let now = w.state(&cat);
    assert_eq!((&now.files, &now.disks, now.photos), (&before.files, &before.disks, before.photos));
    w.settled(&cat);
}

#[test]
fn photos_already_there_or_away_are_left_and_said_why() {
    let w = World::new("photos-left");
    let (cat, a, b) = w.open(&["a", "b"]);
    let m = into(&cat, &["IMG_0001.JPG"], a, TRIP);
    let plan = organize::plan_photo_move(&cat, &m).unwrap();
    assert_eq!((plan.files.len(), plan.left[0].why.as_str()), (0, "here"));
    assert_eq!(go(&cat, &m).unwrap().files, 0);
    drop(cat);

    // Disk A unplugged: its photos are browsable, not movable.
    let (cat, ..) = w.open(&["b"]);
    let plan = organize::plan_photo_move(&cat, &into(&cat, &["IMG_0001.JPG"], b, "Archive")).unwrap();
    assert_eq!((plan.files.len(), plan.left[0].why.as_str()), (0, "unplugged"));
    // Nor can anything go to a folder on it.
    assert!(organize::plan_photo_move(&cat, &into(&cat, &["IMG_0001.JPG"], a, Y2026)).is_err());
}

#[test]
fn a_name_taken_is_never_overwritten() {
    let w = World::new("photos-taken");
    let (cat, a, _) = w.open(&["a", "b"]);
    // Another picture called IMG_0000.JPG in 2026, not in the catalog yet.
    let other = native(&w.disk("a"), r"Photos\2026\IMG_0000.JPG");
    jpeg(&other, 9);
    let theirs = std::fs::read(&other).unwrap();
    let before = w.state(&cat);

    let m = into(&cat, &["IMG_0000.JPG", "IMG_0001.JPG"], a, Y2026);
    let plan = organize::plan_photo_move(&cat, &m).unwrap();
    assert_eq!((plan.photos, plan.left.len(), plan.left[0].why.as_str()), (1, 1, "taken"));
    // Asked by one plan, done by another: refused, nothing moved.
    std::fs::write(native(&w.disk("a"), r"Photos\2026\IMG_0001.JPG"), b"arrived meanwhile").unwrap();
    let refused =
        organize::move_photos(&cat, &m, Some(&plan.token), &AtomicBool::new(false), &|_| {}).unwrap_err().to_string();
    assert!(refused.contains("changed since you looked"), "{refused}");
    std::fs::remove_file(native(&w.disk("a"), r"Photos\2026\IMG_0001.JPG")).unwrap();
    assert_eq!(w.state(&cat), before);

    // Keep both: the one coming in is numbered, with its edits.
    let r = go(&cat, &PhotoMove { number_clashes: true, ..m }).unwrap();
    assert_eq!((r.photos, r.files, r.numbered), (2, 3, 2));
    let now = w.state(&cat);
    for f in ["IMG_0000 (2).JPG", "IMG_0000 (2).JPG.agdata", "IMG_0001.JPG"] {
        assert!(now.disks.contains_key(&format!(r"disk-a\Photos\2026\{f}")), "{f}");
    }
    assert_eq!(std::fs::read(&other).unwrap(), theirs);
    w.settled(&cat);
    // The other picture is new to the catalog; the moved ones aren't.
    rescan(&cat);
    w.rescan_changes_nothing(&cat);
    let undone = back(&cat, &r);
    assert_eq!(undone.files, 3);
    assert!(w.state(&cat).disks.contains_key(r"disk-a\Photos\Trip\IMG_0000.JPG.agdata"));
}

#[test]
fn a_copy_already_there_leaves_the_photo() {
    let w = World::new("photos-copy-there");
    let (cat, a, _) = w.open(&["a", "b"]);
    let trip = native(&w.disk("a"), TRIP);
    std::fs::copy(trip.join("IMG_0002.JPG"), native(&w.disk("a"), r"Photos\2026\IMG_0002.JPG")).unwrap();
    rescan(&cat);
    // The photo is in 2026 already.
    let plan = organize::plan_photo_move(&cat, &into(&cat, &["IMG_0002.JPG"], a, Y2026)).unwrap();
    assert_eq!((plan.files.len(), plan.left[0].why.as_str()), (0, "here"));
    // Seen in Trip: its copy there would go, but one is in 2026 already.
    let m = PhotoMove { from: Some((a, TRIP.into())), ..into(&cat, &["IMG_0002.JPG"], a, Y2026) };
    assert_eq!(organize::plan_photo_move(&cat, &m).unwrap().left[0].why, "copyThere");
}

#[test]
fn photos_moved_to_another_disk_stopped_at_any_step_are_finished_or_undone() {
    let names = ["IMG_0000.JPG", "IMG_0001.JPG", "IMG_0005.JPG"];
    let after = done_state("photos-copy-done", |cat, _, b| {
        let r = go(cat, &into(cat, &names, b, "Archive")).unwrap();
        assert_eq!((r.photos, r.files, r.copied, r.left_behind.len(), r.failed.len()), (3, 4, 4, 0, 0));
    });
    for f in ["IMG_0000.JPG", "IMG_0000.JPG.agdata", "IMG_0001.JPG", "IMG_0005.JPG"] {
        assert!(after.disks.contains_key(&format!(r"disk-b\Archive\{f}")), "{f}");
    }
    assert!(!after.disks.contains_key(r"disk-a\Photos\Trip\IMG_0000.JPG"));
    assert!(!after.disks.contains_key(r"disk-a\Photos\Trip\Day 2\IMG_0005.JPG"));

    // (step, nth time, whether the catalog had taken the move)
    let steps = [
        ("files:journaled", 1, false),
        ("files:copied", 1, false),
        ("files:copied", 4, false),
        ("files:cataloged", 1, true),
        ("files:removed", 1, true),
        ("files:removed", 4, true),
    ];
    for (i, (step, nth, taken)) in steps.into_iter().enumerate() {
        let w = World::new(&format!("photos-copy-{i}"));
        let (cat, _, b) = w.open(&["a", "b"]);
        let before = w.state(&cat);
        let m = into(&cat, &names, b, "Archive");
        killed(step, nth, || go(&cat, &m).map(drop));
        assert_eq!(w.state(&cat).ops, 1, "{step} #{nth}: written down");
        drop(cat);

        let (cat, _, b) = w.open(&["a", "b"]);
        let r = organize::resume(&cat).unwrap();
        assert_eq!((r.finished.len(), r.undone.len(), r.waiting.len()), (taken as usize, !taken as usize, 0), "{step} #{nth}");
        assert_eq!(&w.state(&cat), if taken { &after } else { &before }, "{step} #{nth}");
        w.settled(&cat);
        w.rescan_changes_nothing(&cat);
        if !taken {
            go(&cat, &into(&cat, &names, b, "Archive")).unwrap();
            assert_eq!(w.state(&cat), after, "{step} #{nth}: moved again");
        }
    }
}

#[test]
fn photos_moved_on_their_disk_stopped_at_any_step_are_finished_or_undone() {
    let names = ["IMG_0000.JPG", "IMG_0004.JPG"];
    let after = done_state("photos-rename-done", |cat, a, _| {
        assert_eq!(go(cat, &into(cat, &names, a, Y2026)).unwrap().files, 3);
    });
    // (step, nth time, whether the catalog had taken the move). Once it has,
    // nothing is left to do: the same transaction ended the journal.
    let steps =
        [("files:journaled", 1, false), ("files:renamed", 1, false), ("files:renamed", 3, false), ("files:cataloged", 1, true)];
    for (i, (step, nth, taken)) in steps.into_iter().enumerate() {
        let w = World::new(&format!("photos-rename-{i}"));
        let (cat, a, _) = w.open(&["a", "b"]);
        let before = w.state(&cat);
        let m = into(&cat, &names, a, Y2026);
        killed(step, nth, || go(&cat, &m).map(drop));
        assert_eq!(w.state(&cat).ops, !taken as i64, "{step} #{nth}");
        drop(cat);

        let (cat, ..) = w.open(&["a", "b"]);
        let r = organize::resume(&cat).unwrap();
        assert_eq!((r.finished.len(), r.undone.len()), (0, !taken as usize), "{step} #{nth}");
        assert_eq!(&w.state(&cat), if taken { &after } else { &before }, "{step} #{nth}");
        w.settled(&cat);
        w.rescan_changes_nothing(&cat);
    }
}

/// Cut short while copying, the next start is without the other disk: the
/// move waits, the originals stay where they are and are left alone; with
/// the disk back the copies are taken back.
#[test]
fn a_photo_move_cut_short_waits_for_its_disk() {
    let w = World::new("photos-wait");
    let (cat, a, b) = w.open(&["a", "b"]);
    let before = w.state(&cat);
    killed("files:copied", 2, || go(&cat, &into(&cat, &["IMG_0001.JPG", "IMG_0002.JPG", "IMG_0003.JPG"], b, "Archive")).map(drop));
    drop(cat);

    let (cat, ..) = w.open(&["a"]);
    assert_eq!(organize::resume(&cat).unwrap().waiting.len(), 1);
    // Its folders are off limits meanwhile.
    let refused = go(&cat, &into(&cat, &["IMG_0001.JPG"], a, Y2026)).unwrap_err().to_string();
    assert!(refused.contains("hasn't finished"), "{refused}");
    drop(cat);

    let (cat, ..) = w.open(&["a", "b"]);
    assert_eq!(organize::resume(&cat).unwrap().undone.len(), 1);
    assert_eq!(w.state(&cat), before);
    w.settled(&cat);
}
