//! Handing a photo to Argentum: which program, and which of its files.
//!
//! Lives here, not in src/, because it writes sample files (promise 1). The
//! world is kill_safe_organize.rs's: disk A holds Photos with Trip
//! (IMG_0000-0003, and Day 2 with IMG_0004-0005), disk B holds Archive.

use crate::kill_safe_organize::{TRIP, World, native};
use shoebox_lib::argentum::{self, Handoff};
use shoebox_lib::catalog::Catalog;
use shoebox_lib::scan::scan_root;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shoebox-argentum-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn photo(cat: &Catalog, name: &str) -> i64 {
    cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0)).unwrap()
}

fn rescan(cat: &Catalog) {
    for id in cat.root_ids().unwrap() {
        if cat.mount_of(cat.root(id).unwrap().volume_id).is_some() {
            scan_root(cat, id, &AtomicBool::new(false), false, &|_| {}).unwrap();
        }
    }
}

fn opened_on(cat: &Catalog, photo: i64, disk: &str) -> bool {
    match argentum::file_to_edit(cat, photo).unwrap() {
        Handoff::Opened { path } => path.contains(&format!("disk-{disk}")),
        other => panic!("not opened: {other:?}"),
    }
}

#[test]
fn only_a_program_named_argentum_is_ever_started() {
    let dir = folder("named");
    let exe = dir.join("Argentum.exe");
    let other = dir.join("cmd.exe");
    std::fs::write(&exe, b"").unwrap();
    std::fs::write(&other, b"").unwrap();
    assert_eq!(argentum::program(&exe), Some(exe.clone()));
    assert_eq!(argentum::program(&other), None);
    assert_eq!(argentum::find(other.to_str().unwrap()), argentum::find(""), "chosen, but not Argentum");
    assert_eq!(argentum::program(&dir.join("argentum-gone.exe")), None, "not there");
    assert_eq!(argentum::program(&dir), None, "a folder");
}

#[test]
fn a_macos_app_is_the_program_inside_it() {
    let dir = folder("app");
    let app = dir.join("Argentum.app");
    let inside = app.join("Contents").join("MacOS");
    std::fs::create_dir_all(&inside).unwrap();
    assert_eq!(argentum::program(&app), None, "an empty folder of that name");
    std::fs::write(inside.join("Argentum"), b"").unwrap();
    assert_eq!(argentum::program(&app), Some(app));
}

#[test]
fn the_one_chosen_comes_first_and_a_moved_one_falls_back() {
    let dir = folder("chosen");
    let exe = dir.join("argentum-portable.exe");
    std::fs::write(&exe, b"").unwrap();
    assert_eq!(argentum::find(exe.to_str().unwrap()), Some(exe.clone()));
    std::fs::remove_file(&exe).unwrap();
    assert_eq!(argentum::find(exe.to_str().unwrap()), argentum::find(""));
}

#[test]
fn a_photo_on_a_disk_plugged_in_opens_from_it() {
    let w = World::new("argentum-here");
    let (cat, ..) = w.open(&["a", "b"]);
    assert!(opened_on(&cat, photo(&cat, "IMG_0001.JPG"), "a"));
}

#[test]
fn an_unplugged_disk_is_named_and_the_catalog_stays_as_it_was() {
    let w = World::new("argentum-unplugged");
    let (cat, ..) = w.open(&["b"]);
    let before = w.state(&cat);
    let found = argentum::file_to_edit(&cat, photo(&cat, "IMG_0001.JPG")).unwrap();
    assert_eq!(found, Handoff::Unplugged { disks: vec!["Disk A".into()] });
    assert_eq!(w.state(&cat), before);
}

#[test]
fn a_copy_goes_on_from_the_one_argentum_edited() {
    let w = World::new("argentum-copies");
    let (cat, ..) = w.open(&["a", "b"]);
    let trip = native(&w.disk("a"), TRIP);
    let archive = w.disk("b").join("Archive");
    std::fs::copy(trip.join("IMG_0001.JPG"), archive.join("IMG_0001.JPG")).unwrap();
    rescan(&cat);
    let id = photo(&cat, "IMG_0001.JPG");
    assert_eq!(cat.photo_detail(id).unwrap().files.len(), 2);

    std::fs::write(archive.join("IMG_0001.JPG.agdata"), b"Argentum's edits").unwrap();
    assert!(opened_on(&cat, id, "b"), "the edits are on disk B");

    // Disk B gone: the copy left still opens.
    let (cat, ..) = w.open(&["a"]);
    assert!(opened_on(&cat, id, "a"));
}

#[test]
fn a_photo_gone_from_its_disk_says_so() {
    let w = World::new("argentum-gone");
    let (cat, ..) = w.open(&["a", "b"]);
    let id = photo(&cat, "IMG_0001.JPG");
    std::fs::remove_file(native(&w.disk("a"), TRIP).join("IMG_0001.JPG")).unwrap();
    rescan(&cat);
    assert_eq!(argentum::file_to_edit(&cat, id).unwrap(), Handoff::Gone);
}
