//! Photos edited in Argentum carry its mark: a scan reads each photo's
//! sidecars (`<file>.agdata`, `<file>.<copy>.agdata`), only those new or
//! changed since, and a sidecar gone clears the mark only after a complete
//! walk of a folder that could be read. An unplugged disk keeps its marks.
//!
//! Lives here, not in src/, because it writes sample photos and sidecars
//! (promise 1).

use crate::kill_safe_organize::jpeg;
use rusqlite::params;
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::edits;
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::{Report, scan_root};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const EDIT: &str = r#"{"version":1,"rating":0,"adjustments":{"exposure":0.5}}"#;
const FILM: &str = r#"{"version":1,"rating":0,"adjustments":{"lutPath":"provider-look",
    "agProviderLook":{"camera":"c","look":"l"}}}"#;
const NO_EDIT: &str = r#"{"version":1,"rating":3,"adjustments":{}}"#;

/// A disk ("disk") with a source folder, Photos, holding IMG_0..3.JPG.
struct World {
    base: PathBuf,
    data: DataDir,
}

impl World {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("shoebox-edits-{name}"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("disk").join("Photos")).unwrap();
        for i in 0..4u8 {
            jpeg(&base.join("disk").join("Photos").join(format!("IMG_{i}.JPG")), i);
        }
        let w = Self { data: DataDir { root: base.join("catalog") }, base };
        w.data.ensure().unwrap();
        let cat = w.open(true);
        let disk: i64 = cat.db().query_row("SELECT id FROM volumes", [], |r| r.get(0)).unwrap();
        cat.db()
            .execute("INSERT INTO roots (volume_id, rel_path, added_at) VALUES (?1, 'Photos', 0)", params![disk])
            .unwrap();
        w
    }

    fn photos(&self) -> PathBuf {
        self.base.join("disk").join("Photos")
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.photos().join(name), text).unwrap();
    }

    /// Opens the catalog as a start does, its disk plugged in or not.
    fn open(&self, plugged: bool) -> Catalog {
        let cat = Catalog::open(self.data.clone()).unwrap();
        cat.plug_in_test_disk("Disk", &self.base.join("disk")).unwrap();
        if !plugged {
            cat.refresh_volumes().unwrap(); // test disks are never found: unplugged
        }
        cat
    }
}

fn scan(cat: &Catalog) -> Report {
    let root = cat.root_ids().unwrap()[0];
    let report = scan_root(cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    assert_eq!(report.errors, 0, "{report:?}");
    report
}

/// The photo's mark (0 none, 1 edit, 2 film) and how many edited versions it has.
fn mark(cat: &Catalog, file: &str) -> (i64, i64) {
    cat.db()
        .query_row(
            "SELECT p.edit_kind, p.edit_versions FROM photos p JOIN files f ON f.photo_id = p.id
              WHERE f.name = ?1 AND f.state = 0",
            [file],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

/// Waits for the clock to move on a second, so a rewritten file's date differs.
fn tick() {
    std::thread::sleep(std::time::Duration::from_millis(1100));
}

#[test]
fn a_sidecar_marks_its_photo_and_each_edited_copy_counts() {
    let w = World::new("marks");
    w.write("IMG_0.JPG.agdata", EDIT);
    w.write("IMG_1.JPG.agdata", EDIT);
    w.write("IMG_1.JPG.a1b2c3.agdata", FILM);
    w.write("IMG_2.JPG.agdata", NO_EDIT);
    w.write("IMG_9.JPG.agdata", EDIT); // no such photo
    let cat = w.open(true);
    let report = scan(&cat);
    assert_eq!(report.edits_read, 4, "IMG_9 is not in the catalog: its sidecar is not read");
    assert_eq!(mark(&cat, "IMG_0.JPG"), (1, 1));
    assert_eq!(mark(&cat, "IMG_1.JPG"), (2, 2), "a film version makes it film");
    assert_eq!(mark(&cat, "IMG_2.JPG"), (0, 0), "a sidecar with no edit in it");
    assert_eq!(mark(&cat, "IMG_3.JPG"), (0, 0));
}

#[test]
fn a_rescan_reads_only_sidecars_new_or_changed_even_on_unchanged_photos() {
    let w = World::new("rescan");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    assert_eq!(scan(&cat).edits_read, 0, "nothing changed");
    tick();
    w.write("IMG_0.JPG.agdata", FILM);
    w.write("IMG_1.JPG.agdata", EDIT);
    let report = scan(&cat);
    assert_eq!((report.edits_read, report.unchanged), (2, 4), "the photos themselves are unchanged");
    assert_eq!(mark(&cat, "IMG_0.JPG"), (2, 1));
    assert_eq!(mark(&cat, "IMG_1.JPG"), (1, 1));
}

#[test]
fn a_sidecar_gone_clears_the_mark_after_a_complete_scan() {
    let w = World::new("gone");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    std::fs::remove_file(w.photos().join("IMG_0.JPG.agdata")).unwrap();
    assert_eq!(mark(&cat, "IMG_0.JPG"), (1, 1), "not until a scan");
    scan(&cat);
    assert_eq!(mark(&cat, "IMG_0.JPG"), (0, 0));
}

#[test]
fn an_unplugged_disk_keeps_its_marks() {
    let w = World::new("unplugged");
    w.write("IMG_0.JPG.agdata", EDIT);
    scan(&w.open(true));
    std::fs::remove_file(w.photos().join("IMG_0.JPG.agdata")).unwrap();
    let cat = w.open(false);
    let root = cat.root_ids().unwrap()[0];
    assert!(scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).is_err());
    assert_eq!(mark(&cat, "IMG_0.JPG"), (1, 1));
}

/// Makes `dir` unreadable until dropped.
struct Unreadable(PathBuf);

impl Unreadable {
    fn new(dir: &Path) -> Self {
        let ok = if cfg!(windows) {
            std::process::Command::new("icacls").arg(dir).args(["/deny", "*S-1-1-0:(OI)(CI)(RD)"]).output()
        } else {
            std::process::Command::new("chmod").arg("000").arg(dir).output()
        };
        assert!(ok.unwrap().status.success());
        Self(dir.to_path_buf())
    }
}

impl Drop for Unreadable {
    fn drop(&mut self) {
        let _ = if cfg!(windows) {
            std::process::Command::new("icacls").arg(&self.0).args(["/remove:d", "*S-1-1-0"]).output()
        } else {
            std::process::Command::new("chmod").arg("755").arg(&self.0).output()
        };
    }
}

#[test]
fn a_folder_that_cannot_be_read_keeps_its_marks() {
    let w = World::new("unread");
    let day = w.photos().join("Day 2");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::rename(w.photos().join("IMG_3.JPG"), day.join("IMG_3.JPG")).unwrap();
    std::fs::write(day.join("IMG_3.JPG.agdata"), EDIT).unwrap();
    let cat = w.open(true);
    scan(&cat);
    assert_eq!(mark(&cat, "IMG_3.JPG"), (1, 1));
    std::fs::remove_file(day.join("IMG_3.JPG.agdata")).unwrap();
    {
        let _locked = Unreadable::new(&day);
        let root = cat.root_ids().unwrap()[0];
        scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    }
    assert_eq!(mark(&cat, "IMG_3.JPG"), (1, 1));
}

#[test]
fn a_photo_moved_with_its_sidecar_keeps_its_mark() {
    let w = World::new("moved");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let elsewhere = w.photos().join("Elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    for name in ["IMG_0.JPG", "IMG_0.JPG.agdata"] {
        std::fs::rename(w.photos().join(name), elsewhere.join(name)).unwrap();
    }
    scan(&cat);
    assert_eq!(mark(&cat, "IMG_0.JPG"), (1, 1));
}

#[test]
fn copies_of_a_photo_on_two_disks_count_one_edit_once() {
    let w = World::new("copies");
    let copy = w.photos().join("Copy");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::copy(w.photos().join("IMG_0.JPG"), copy.join("IMG_0.JPG")).unwrap();
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    assert_eq!(mark(&cat, "IMG_0.JPG"), (1, 1), "edited if any copy is");
    std::fs::write(copy.join("IMG_0.JPG.agdata"), EDIT).unwrap();
    scan(&cat);
    assert_eq!(mark(&cat, "IMG_0.JPG"), (1, 1), "the same edit twice is one version");
}

#[cfg(windows)]
#[test]
fn a_sidecar_kept_only_in_the_cloud_is_never_read() {
    let w = World::new("cloud");
    w.write("IMG_0.JPG.agdata", EDIT);
    let attrib = std::process::Command::new("attrib").arg("+O").arg(w.photos().join("IMG_0.JPG.agdata")).output();
    assert!(attrib.unwrap().status.success());
    let cat = w.open(true);
    assert_eq!(scan(&cat).edits_read, 0);
    assert_eq!(mark(&cat, "IMG_0.JPG"), (0, 0));
}

#[test]
fn the_check_after_editing_reads_one_folder_and_says_what_changed() {
    let w = World::new("check");
    let cat = w.open(true);
    scan(&cat);
    let folder = w.photos();
    assert_eq!(edits::check_folder(&cat, &folder).unwrap(), edits::Checked { read: 0, changed: 0 });
    w.write("IMG_0.JPG.agdata", EDIT);
    assert_eq!(edits::check_folder(&cat, &folder).unwrap(), edits::Checked { read: 1, changed: 1 });
    assert_eq!(mark(&cat, "IMG_0.JPG"), (1, 1));
    assert_eq!(edits::check_folder(&cat, &folder).unwrap(), edits::Checked { read: 0, changed: 0 });
    tick();
    w.write("IMG_0.JPG.agdata", r#"{"version":1,"rating":5,"adjustments":{"exposure":0.5}}"#);
    assert_eq!(
        edits::check_folder(&cat, &folder).unwrap(),
        edits::Checked { read: 1, changed: 0 },
        "only its rating changed: the edit is the same"
    );
    std::fs::remove_file(folder.join("IMG_0.JPG.agdata")).unwrap();
    assert_eq!(edits::check_folder(&cat, &folder).unwrap(), edits::Checked { read: 0, changed: 1 });
    assert_eq!(mark(&cat, "IMG_0.JPG"), (0, 0));
}

#[test]
fn the_check_after_editing_looks_only_at_the_folders_handed_to_argentum() {
    let w = World::new("handed");
    let cat = w.open(true);
    scan(&cat);
    w.write("IMG_0.JPG.agdata", EDIT);
    assert_eq!(edits::check_handed_over(&cat).unwrap(), 0, "nothing handed over yet");
    edits::handed_over(&cat, &w.photos().join("IMG_0.JPG"));
    assert_eq!(edits::check_handed_over(&cat).unwrap(), 1);
    assert_eq!(mark(&cat, "IMG_0.JPG"), (1, 1));
}

#[test]
fn the_grid_filters_and_details_show_the_mark() {
    let w = World::new("views");
    w.write("IMG_0.JPG.agdata", EDIT);
    w.write("IMG_1.JPG.agdata", EDIT);
    w.write("IMG_1.JPG.a1b2c3.agdata", FILM);
    let cat = w.open(true);
    scan(&cat);
    let id = |file: &str| -> i64 {
        cat.db().query_row("SELECT photo_id FROM files WHERE name = ?1", [file], |r| r.get(0)).unwrap()
    };
    let only = |picked: &[&str]| -> Vec<i64> {
        let filters = Filters { edits: picked.iter().map(|s| s.to_string()).collect(), ..Filters::default() };
        let mut ids = cat.list_photos(&Scope::All, Sort::Name, &filters).unwrap().ids;
        ids.sort();
        ids
    };
    assert_eq!(only(&["edited"]), [id("IMG_0.JPG")]);
    assert_eq!(only(&["film"]), [id("IMG_1.JPG")]);
    let mut not_edited = vec![id("IMG_2.JPG"), id("IMG_3.JPG")];
    not_edited.sort();
    assert_eq!(only(&["notEdited"]), not_edited);
    assert_eq!(only(&["edited", "film"]).len(), 2);

    let list = cat.list_photos(&Scope::All, Sort::Name, &Filters::default()).unwrap();
    let at = list.ids.iter().position(|p| *p == id("IMG_1.JPG")).unwrap();
    assert_eq!((list.edit[at], list.edits[at]), (2, 2));

    let facets = cat.facets(&Scope::All, &Filters { edits: vec!["film".into()], ..Filters::default() }).unwrap();
    let counts: Vec<(String, i64)> = facets.edits.into_iter().map(|v| (v.value, v.count)).collect();
    assert_eq!(
        counts,
        [("edited".into(), 1), ("film".into(), 1), ("notEdited".into(), 2)],
        "counted without its own filter"
    );

    let files = cat.photo_detail(id("IMG_1.JPG")).unwrap().files;
    assert_eq!(
        files[0].edits,
        [edits::FileEdit { copy: None, film: false }, edits::FileEdit { copy: Some("a1b2c3".into()), film: true }]
    );
}

#[test]
fn a_photo_missing_loses_its_mark_and_gets_it_back_when_found_again() {
    let w = World::new("missing");
    w.write("IMG_0.JPG.agdata", EDIT);
    let cat = w.open(true);
    scan(&cat);
    let photo: i64 =
        cat.db().query_row("SELECT photo_id FROM files WHERE name = 'IMG_0.JPG'", [], |r| r.get(0)).unwrap();
    let kind =
        || -> i64 { cat.db().query_row("SELECT edit_kind FROM photos WHERE id = ?1", [photo], |r| r.get(0)).unwrap() };
    let away = w.base.join("IMG_0.JPG");
    std::fs::rename(w.photos().join("IMG_0.JPG"), &away).unwrap();
    scan(&cat);
    assert_eq!(kind(), 0, "its only file is gone: nothing of it is edited");
    std::fs::rename(&away, w.photos().join("IMG_0.JPG")).unwrap();
    scan(&cat);
    assert_eq!(kind(), 1);
}
