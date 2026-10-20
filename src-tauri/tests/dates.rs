//! Dates corrected in the catalog: a camera whose clock was wrong is found
//! by the date check (folders named with a date, photos from one camera days
//! away from it and from the other photos there), its photos' dates move by
//! a fixed amount or to the folder's day keeping the time of day, every move
//! has its undo, and the date the file gives is kept and given back. A
//! rescan keeps a correction while the file still says the same date. Faces
//! count for the month their photo was taken in now.
//!
//! Lives here, not in src/, because it writes a test catalog and sample
//! photos (promise 1).

use rusqlite::params;
use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::dates::{self, Change, DAYS, LeftDates};
use shoebox_lib::paths::DataDir;

const DAY: i64 = 86_400;

/// Seconds since 1970 of a day and time, read as UTC (as photos' dates are kept).
fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
    chrono::NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, 0).unwrap().and_utc().timestamp()
}

struct Test {
    cat: Catalog,
    disk: i64,
}

impl Test {
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&base);
        let data = DataDir { root: base.join("catalog") };
        data.ensure().unwrap();
        let cat = Catalog::open(data).unwrap();
        let disk = cat.plug_in_test_disk("Disk", &base).unwrap();
        Self { cat, disk }
    }

    fn folder(&self, rel: &str) -> i64 {
        let db = self.cat.db();
        db.execute("INSERT OR IGNORE INTO folders (volume_id, rel_path) VALUES (?1, ?2)", params![self.disk, rel])
            .unwrap();
        db.query_row("SELECT id FROM folders WHERE volume_id = ?1 AND rel_path = ?2", params![self.disk, rel], |r| {
            r.get(0)
        })
        .unwrap()
    }

    /// `n` photos in folder `rel` from `camera`, the first taken at `first`,
    /// then one every ten minutes. Their ids.
    fn photos(&self, rel: &str, camera: Option<&str>, first: i64, n: i64) -> Vec<i64> {
        let folder = self.folder(rel);
        let db = self.cat.db();
        (0..n)
            .map(|i| {
                db.execute(
                    "INSERT INTO photos (fingerprint, kind, taken_ts, taken_exif, make, model, preview_state, added_at)
                     VALUES (?1, 'raw', ?2, 1, ?3, ?4, 1, 0)",
                    params![format!("{rel}-{i}-{first}"), first + i * 600, camera.map(|_| "Canon"), camera],
                )
                .unwrap();
                let id = db.last_insert_rowid();
                db.execute(
                    "INSERT INTO files (photo_id, folder_id, name, size, mtime) VALUES (?1, ?2, ?3, 1, 0)",
                    params![id, folder, format!("IMG_{id:04}.CR2")],
                )
                .unwrap();
                id
            })
            .collect()
    }

    fn taken(&self, id: i64) -> (i64, Option<i64>) {
        self.cat
            .db()
            .query_row("SELECT taken_ts, taken_original FROM photos WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap()
    }

    fn check(&self, within: Option<&str>, all: bool) -> dates::DateCheck {
        dates::check(&self.cat, within.map(|w| (self.disk, w)), DAYS, all).unwrap()
    }
}

/// A birthday on 4 November 2006: the host's camera right, a guest's camera
/// (its photos in a folder of their own) eight months behind. A trip named
/// with its first and last day; a picnic with one photo two years off;
/// a folder named with no date.
fn birthday_and_more(t: &Test) -> (Vec<i64>, Vec<i64>, Vec<i64>) {
    let host = t.photos("Photos\\2006-11-04 Birthday", Some("EOS 30D"), at(2006, 11, 4, 15, 0), 20);
    let guest = t.photos("Photos\\2006-11-04 Birthday\\Guest", Some("EOS 350D"), at(2006, 2, 13, 17, 29), 10);
    for day in 0..7 {
        t.photos(
            &format!("Photos\\2005-02-26 03-04 Trip\\Day {}", day + 1),
            Some("EOS 10D"),
            at(2005, 2, 26, 9, 0) + day * DAY,
            5,
        );
    }
    t.photos("Photos\\2006-04-09 Picnic", Some("EOS 10D"), at(2006, 4, 9, 10, 0), 15);
    let stray = t.photos("Photos\\2006-04-09 Picnic", Some("EOS 10D"), at(2008, 8, 25, 16, 45), 1);
    t.photos("Photos\\Misc", Some("EOS 10D"), at(2001, 1, 1, 12, 0), 5);
    (host, guest, stray)
}

#[test]
fn a_camera_whose_clock_was_wrong_is_found() {
    let t = Test::new("shoebox-dates-check");
    let (_, guest, stray) = birthday_and_more(&t);

    let c = t.check(None, false);
    // Folders named with a date or in one (Misc isn't); every photo in them.
    assert_eq!((c.folders, c.photos, c.days, c.left), (10, 81, 3, 0));
    let found: Vec<(&str, Option<&str>, usize, i64)> =
        c.findings.iter().map(|f| (f.rel_path.as_str(), f.camera.as_deref(), f.photos.len(), f.days_off)).collect();
    assert_eq!(
        found,
        [
            // The most photos first: the guest's camera, then the stray photo.
            ("Photos\\2006-11-04 Birthday\\Guest", Some("Canon EOS 350D"), 10, -264),
            ("Photos\\2006-04-09 Picnic", Some("Canon EOS 10D"), 1, 869),
        ]
    );
    let g = &c.findings[0];
    assert_eq!(g.photos, guest);
    assert_eq!(
        (g.dated.as_str(), g.from.as_str(), g.to.as_str()),
        ("Photos\\2006-11-04 Birthday", "2006-11-04", "2006-11-04")
    );
    // Moves its first photo to the folder's day, the time of day kept.
    assert_eq!(g.shift, 264 * DAY);
    assert_eq!((g.first, g.last), (at(2006, 2, 13, 17, 29), at(2006, 2, 13, 17, 29) + 9 * 600));
    // The host's photos are those that agree.
    assert_eq!(
        (g.others, g.others_from.as_deref(), g.others_to.as_deref()),
        (20, Some("2006-11-04"), Some("2006-11-04"))
    );
    assert_eq!(c.findings[1].photos, stray);

    // In one folder: only its findings, still weighed against the folder it is in.
    let c = t.check(Some("Photos\\2006-11-04 Birthday\\Guest"), false);
    assert_eq!((c.folders, c.photos, c.findings.len()), (1, 10, 1));
    assert_eq!(c.findings[0].others, 20);
    assert!(t.check(Some("Photos\\2005-02-26 03-04 Trip"), false).findings.is_empty());

    // Further than the check looks: nothing in the trip either way, the trip is its range.
    let wide = dates::check(&t.cat, None, 1000, false).unwrap();
    assert!(wide.findings.is_empty(), "{:?}", wide.findings);

    // Private photos, hidden: never listed (agents never see them).
    t.cat
        .db()
        .execute(&format!("UPDATE photos SET private = 1, seal_to = zeroblob(32) WHERE id IN ({})", guest[0]), [])
        .unwrap();
    assert!(t.cat.hides_any());
    let c = t.check(None, false);
    assert_eq!(c.findings[0].photos, guest[1..]);
    assert_eq!(c.photos, 80);
}

#[test]
fn a_finding_left_as_is_stays_out_until_brought_back() {
    let t = Test::new("shoebox-dates-left");
    birthday_and_more(&t);
    let c = t.check(None, false);
    let stray: LeftDates = (&c.findings[1]).into();

    assert_eq!(dates::leave_as_is(&t.cat, std::slice::from_ref(&stray), true).unwrap(), 1);
    let c = t.check(None, false);
    assert_eq!((c.left, c.findings.len()), (1, 1));
    assert!(c.findings[0].rel_path.ends_with("Guest"));
    // Asked for all: listed, flagged.
    let c = t.check(None, true);
    assert_eq!(c.findings.iter().map(|f| f.left).collect::<Vec<_>>(), [false, true]);

    // Back in the check.
    assert_eq!(dates::leave_as_is(&t.cat, &[stray], false).unwrap(), 1);
    let c = t.check(None, false);
    assert_eq!((c.left, c.findings.len()), (0, 2));
}

#[test]
fn dates_move_keep_the_files_date_and_come_back() {
    let t = Test::new("shoebox-dates-change");
    let (host, guest, _) = birthday_and_more(&t);

    // What "Fix dates" shows first: the folder whose name dates them.
    let d = dates::of(&t.cat, &guest).unwrap();
    assert_eq!((d.photos, d.dated, d.corrected), (10, 10, 0));
    assert_eq!((d.folder.as_deref(), d.folder_date.as_deref()), (Some("2006-11-04 Birthday"), Some("2006-11-04")));
    assert_eq!((d.first, d.last), (Some(at(2006, 2, 13, 17, 29)), Some(at(2006, 2, 13, 18, 59))));

    // To the folder's day: the time of day and the minutes between them kept.
    let moved = dates::change(&t.cat, &guest, &Change::To { date: "2006-11-04".into() }).unwrap();
    assert_eq!((moved.photos.len(), moved.unchanged), (10, 0));
    assert!(moved.photos.iter().all(|p| p.by == 264 * DAY));
    assert_eq!(t.taken(guest[0]), (at(2006, 11, 4, 17, 29), Some(at(2006, 2, 13, 17, 29))));
    assert_eq!(t.taken(guest[9]).0, at(2006, 11, 4, 17, 29) + 9 * 600);
    assert!(t.check(None, false).findings.iter().all(|f| !f.rel_path.ends_with("Guest")));
    assert_eq!(t.check(None, false).corrected, 10);
    // Shown and sorted by the corrected date: November, among the host's photos.
    let november =
        t.cat.list_photos(&Scope::Date { key: "2006-11".into() }, Sort::Oldest, &Filters::default()).unwrap().ids;
    assert_eq!(november.len(), 30);
    // The host's photos run from 15:00 to 18:10: the guest's first (17:29) follows the host's 17:20.
    assert_eq!(november[..16], [&host[..15], &guest[..1]].concat()[..]);
    let detail = t.cat.photo_detail(guest[0]).unwrap();
    assert_eq!(
        (detail.taken_ts, detail.taken_original),
        (Some(at(2006, 11, 4, 17, 29)), Some(at(2006, 2, 13, 17, 29)))
    );

    // Moved again, by hours: the file's date is still the one kept.
    dates::change(&t.cat, &guest[..1], &Change::By { seconds: -2 * 3600 }).unwrap();
    assert_eq!(t.taken(guest[0]), (at(2006, 11, 4, 15, 29), Some(at(2006, 2, 13, 17, 29))));
    // Undo puts back exactly that change.
    let back = dates::undo(&t.cat, &moved).unwrap();
    assert_eq!(back.photos.len(), 10);
    assert_eq!(t.taken(guest[0]), (at(2006, 2, 13, 15, 29), Some(at(2006, 2, 13, 17, 29))));
    assert_eq!(t.taken(guest[1]), (at(2006, 2, 13, 17, 39), None), "back on its file's date: nothing kept");

    // Back to the files' dates; undone, the correction is there again.
    let back = dates::change(&t.cat, &guest, &Change::Back).unwrap();
    assert_eq!((back.photos.len(), back.unchanged), (1, 9));
    assert_eq!(t.taken(guest[0]), (at(2006, 2, 13, 17, 29), None));
    dates::undo(&t.cat, &back).unwrap();
    assert_eq!(t.taken(guest[0]), (at(2006, 2, 13, 15, 29), Some(at(2006, 2, 13, 17, 29))));

    // Never before 1970, never a photo without a date, never a hidden one.
    let early = dates::change(&t.cat, &guest[..2], &Change::By { seconds: -40 * 365 * DAY }).unwrap();
    assert_eq!((early.photos.len(), early.unchanged), (0, 2));
    t.cat.db().execute("UPDATE photos SET taken_ts = NULL WHERE id = ?1", [host[0]]).unwrap();
    let undated = dates::change(&t.cat, &host[..1], &Change::By { seconds: DAY }).unwrap();
    assert_eq!((undated.photos.len(), undated.unchanged), (0, 1));
    t.cat.db().execute("UPDATE photos SET private = 1, seal_to = zeroblob(32) WHERE id = ?1", [host[1]]).unwrap();
    let hidden = dates::change(&t.cat, &host[1..2], &Change::By { seconds: DAY }).unwrap();
    assert_eq!((hidden.photos.len(), hidden.unchanged), (0, 0));
    assert_eq!(t.taken(host[1]), (at(2006, 11, 4, 15, 10), None));
    assert!(dates::change(&t.cat, &guest, &Change::To { date: "2006-11-31".into() }).is_err());
}

/// A rescan reads a photo's date again: a correction stays while its file
/// gives the date it was made from, and goes once the file says otherwise.
#[cfg(windows)]
#[test]
fn a_rescan_keeps_a_correction_while_the_file_says_the_same() {
    use shoebox_lib::scan::scan_root;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, SystemTime};

    let base = std::env::temp_dir().join("shoebox-dates-rescan");
    let _ = std::fs::remove_dir_all(&base);
    let pics = base.join("Pictures").join("2006-11-04 Birthday");
    std::fs::create_dir_all(&pics).unwrap();
    let file = pics.join("a.jpg");
    let jpeg = |shade: u8, written: SystemTime| {
        image::RgbImage::from_fn(320, 240, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
            .save(&file)
            .unwrap();
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(written).unwrap();
    };
    let written = SystemTime::UNIX_EPOCH + Duration::from_secs(at(2006, 2, 13, 12, 0) as u64);
    jpeg(10, written);
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&base.join("Pictures")).unwrap();
    let scan = |again: bool| scan_root(&cat, root, &AtomicBool::new(false), again, &|_| {}).unwrap();
    let taken = || -> (i64, Option<i64>) {
        cat.db().query_row("SELECT taken_ts, taken_original FROM photos", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    };
    scan(false);
    let (file_date, _) = taken();
    let id: i64 = cat.db().query_row("SELECT id FROM photos", [], |r| r.get(0)).unwrap();

    dates::change(&cat, &[id], &Change::By { seconds: 264 * DAY }).unwrap();
    assert_eq!(taken(), (file_date + 264 * DAY, Some(file_date)));
    // Read again (previews rebuilt): same file, same date, the correction stays.
    let r = scan(true);
    assert_eq!(r.reread, 1, "{r:?}");
    assert_eq!(taken(), (file_date + 264 * DAY, Some(file_date)));

    // Edited elsewhere and written an hour later: the file's new date wins.
    jpeg(200, written + Duration::from_secs(3600));
    let r = scan(false);
    assert_eq!((r.photos_added, r.errors), (0, 0), "{r:?}");
    assert_eq!(taken(), (file_date + 3600, None));

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}

/// Faces count for the month their photo was taken in: a baby's photos
/// dated years too early by a camera, once corrected, make how he looked
/// that month at once, as reading the faces again from the start would.
#[test]
fn faces_follow_corrected_dates() {
    use shoebox_lib::faces::FaceIndex;
    const DIM: usize = 512;
    let print = |parts: &[(usize, f32)]| -> Vec<f32> {
        let mut v = vec![0f32; DIM];
        for &(axis, w) in parts {
            v[axis] += w;
        }
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    };
    let t = Test::new("shoebox-dates-faces");
    let data = t.cat.data.clone();
    // Opened empty first, so it never regroups the people made below.
    FaceIndex::open(&t.cat).unwrap();
    let emb = rusqlite::Connection::open(data.root.join("faces.db")).unwrap();
    let face = |photo: i64, person: Option<i64>, v: &[f32]| -> i64 {
        t.cat
            .db()
            .execute(
                "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, seen)
                 VALUES (?1, ?2, 0.1, 0.1, 0.4, 0.4, '', 0.9, 1)",
                params![photo, person],
            )
            .unwrap();
        let id = t.cat.db().last_insert_rowid();
        let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        emb.execute("INSERT INTO emb (face_id, vec) VALUES (?1, ?2)", params![id, blob]).unwrap();
        id
    };
    t.cat.db().execute("INSERT INTO people (name, created_at) VALUES ('Baby', 0)", []).unwrap();
    let baby = t.cat.db().last_insert_rowid();
    // Six photos of him as a newborn, which a camera dated March 2015 (he
    // was born in April 2018), and six at two, in 2020, dated right.
    let newborn = t.photos("2018-04-10 Home\\Guest", Some("EOS 350D"), at(2015, 3, 10, 10, 0), 6);
    for (i, p) in newborn.iter().enumerate() {
        face(*p, Some(baby), &print(&[(0, 1.0), (10 + i, 0.25)]));
    }
    for (i, p) in t.photos("2020-06-01 Garden", Some("EOS 30D"), at(2020, 6, 1, 10, 0), 6).iter().enumerate() {
        face(*p, Some(baby), &print(&[(1, 1.0), (20 + i, 0.25)]));
    }
    // A newborn face nobody is sure of, from April 2018.
    let probe_photo = t.photos("2018-04-20 Visit", Some("EOS 30D"), at(2018, 4, 20, 10, 0), 1)[0];
    let probe = face(probe_photo, None, &print(&[(0, 1.0), (30, 0.25)]));
    let like_baby = |faces: &FaceIndex| -> f32 {
        let alike = faces.look_alikes(&t.cat, &[probe], &[], 5).unwrap();
        alike.iter().find(|a| a.person_id == baby).map_or(0.0, |a| a.similarity)
    };

    let faces = FaceIndex::open(&t.cat).unwrap();
    // No face of his within a year of April 2018: his whole life decides.
    let before = like_baby(&faces);
    let moved = dates::change(&t.cat, &newborn, &Change::To { date: "2018-04-10".into() }).unwrap();
    faces.redated(&t.cat, &moved.photos).unwrap();
    // His newborn faces are from that month now: much more alike, exactly
    // as the faces read again from the start say.
    let after = like_baby(&faces);
    let fresh = like_baby(&FaceIndex::open(&t.cat).unwrap());
    assert!(after > before + 0.1, "{before} -> {after}");
    assert!((after - fresh).abs() < 1e-5, "{after} vs {fresh}");
    // Undone: as before.
    let back = dates::undo(&t.cat, &moved).unwrap();
    faces.redated(&t.cat, &back.photos).unwrap();
    assert!((like_baby(&faces) - before).abs() < 1e-5);
}
