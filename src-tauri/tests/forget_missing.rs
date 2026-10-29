//! Photos deleted outside Shoebox: after a scan they are missing, and stay so
//! until they are taken out of the catalog by hand ("Remove from catalog"),
//! or by "Delete marked files" when they were marked. Before a scan, a file
//! gone from its folder on a plugged-in disk counts as gone for both. Only
//! photos with no copy left anywhere leave; nothing on disk is touched.
//!
//! Lives here, not in src/, because it writes and deletes sample photos: no
//! file under src/ outside paths.rs may hold file-writing code (promise 1).

use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use shoebox_lib::{organize, tags};
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn jpeg(path: &Path, shade: u8) {
    image::RgbImage::from_fn(320, 240, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

fn scan(cat: &Catalog, root: i64) {
    scan_root(cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
}

fn photo_of(cat: &Catalog, name: &str) -> i64 {
    cat.db()
        .query_row("SELECT photo_id FROM files WHERE name = ?1", [name], |r| r.get(0))
        .unwrap_or_else(|_| panic!("{name} isn't in the catalog"))
}

fn exists(cat: &Catalog, id: i64) -> bool {
    cat.db().query_row("SELECT EXISTS (SELECT 1 FROM photos WHERE id = ?1)", [id], |r| r.get(0)).unwrap()
}

fn missing_ids(cat: &Catalog) -> Vec<i64> {
    let mut ids = cat.list_photos(&Scope::Missing, Sort::default(), &Filters::default()).unwrap().ids;
    ids.sort();
    ids
}

#[test]
fn only_photos_gone_from_every_disk_leave_and_no_file_is_touched() {
    let base = std::env::temp_dir().join("shoebox-forget-missing");
    let _ = std::fs::remove_dir_all(&base);
    let (shoot, backup) = (base.join("shoot"), base.join("backup"));
    std::fs::create_dir_all(&shoot).unwrap();
    std::fs::create_dir_all(&backup).unwrap();
    for (i, name) in ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].iter().enumerate() {
        jpeg(&shoot.join(name), i as u8 * 60);
    }
    // c has a second copy elsewhere.
    std::fs::copy(shoot.join("c.jpg"), backup.join("c-copy.jpg")).unwrap();

    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let (shoot_root, backup_root) = (cat.add_root(&shoot).unwrap(), cat.add_root(&backup).unwrap());
    scan(&cat, shoot_root);
    scan(&cat, backup_root);
    let [a, b, c, d] = ["a.jpg", "b.jpg", "c.jpg", "d.jpg"].map(|n| photo_of(&cat, n));
    tags::add(&cat, &[a], &["Lost".to_string()]).unwrap();
    organize::mark(&cat, &[d], true).unwrap();
    assert!(cat.data.preview_path(a).is_file() && cat.data.thumb_path(a).is_file());

    // Deleted outside Shoebox, then a complete scan: missing, still in the catalog.
    for name in ["a.jpg", "c.jpg", "d.jpg"] {
        std::fs::remove_file(shoot.join(name)).unwrap();
    }
    scan(&cat, shoot_root);
    assert_eq!(
        missing_ids(&cat),
        {
            let mut gone = vec![a, d];
            gone.sort();
            gone
        },
        "c still has its copy"
    );
    let report = cat.missing(&[a, b, c, d]).unwrap();
    assert_eq!(report.photos.iter().map(|p| p.id).collect::<Vec<_>>(), vec![a, d]);
    assert!(report.photos.iter().all(|p| !p.in_bin), "deleted for good, not sent to the bin");
    // Where the file was, as the system writes a path.
    assert!(
        Path::new(&report.photos[0].paths[0]).ends_with(Path::new("shoot").join("a.jpg")),
        "{:?}",
        report.photos[0].paths
    );

    // Marked and already gone: the delete only takes it out of the catalog.
    let plan = organize::plan(&cat).unwrap();
    assert_eq!((plan.gone.clone(), plan.waiting, plan.files.len()), (vec![d], 0, 0));
    let deleted = organize::delete_confirmed(&cat, &plan.token, &[]).unwrap();
    assert_eq!((deleted.deleted, deleted.forgotten, deleted.photos_removed), (0, 1, 1));
    assert!(!exists(&cat, d));

    // Remove from catalog: a leaves with its tag and previews; b and c are on a disk and stay.
    let forgotten = cat.forget_missing(&[a, b, c]).unwrap();
    assert_eq!((forgotten.photos, forgotten.files), (1, 1));
    assert!(!exists(&cat, a) && exists(&cat, b) && exists(&cat, c));
    assert!(!cat.data.preview_path(a).exists() && !cat.data.thumb_path(a).exists());
    assert!(cat.data.preview_path(b).is_file());
    let tagged: i64 = cat.db().query_row("SELECT COUNT(*) FROM photo_tags", [], |r| r.get(0)).unwrap();
    assert_eq!(tagged, 0);
    assert!(missing_ids(&cat).is_empty());
    // c stays whole: one of its copies is still on a disk. Every file left is untouched.
    assert_eq!(cat.missing(&[c]).unwrap().photos.len(), 0);
    assert!(shoot.join("b.jpg").is_file() && backup.join("c-copy.jpg").is_file());

    // Asked again, nothing more leaves.
    assert_eq!(cat.forget_missing(&[a, b, c, d]).unwrap().photos, 0);
    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn deleted_outside_shoebox_since_the_last_scan_can_leave_too() {
    let base = std::env::temp_dir().join("shoebox-forget-unscanned");
    let _ = std::fs::remove_dir_all(&base);
    let (shoot, backup, sub) = (base.join("shoot"), base.join("backup"), base.join("shoot").join("sub"));
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(&backup).unwrap();
    for (i, name) in ["e.jpg", "f.jpg", "g.jpg"].iter().enumerate() {
        jpeg(&shoot.join(name), i as u8 * 60);
    }
    jpeg(&sub.join("h.jpg"), 200);
    std::fs::copy(shoot.join("g.jpg"), backup.join("g-copy.jpg")).unwrap();

    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let (shoot_root, backup_root) = (cat.add_root(&shoot).unwrap(), cat.add_root(&backup).unwrap());
    scan(&cat, shoot_root);
    scan(&cat, backup_root);
    let [e, f, g, h] = ["e.jpg", "f.jpg", "g.jpg", "h.jpg"].map(|n| photo_of(&cat, n));
    organize::mark(&cat, &[e, g, h], true).unwrap();

    // Deleted outside Shoebox, no scan since: the catalog still lists them.
    // h's whole folder went away: a folder that can't be read proves nothing.
    for name in ["e.jpg", "f.jpg", "g.jpg"] {
        std::fs::remove_file(shoot.join(name)).unwrap();
    }
    std::fs::rename(&sub, base.join("elsewhere")).unwrap();
    assert!(missing_ids(&cat).is_empty());
    let report = cat.missing(&[e, f, g, h]).unwrap();
    assert_eq!(
        report.photos.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![e, f],
        "g has its copy, h can't be checked"
    );

    // Delete marked files: e only leaves the catalog, g's copy is offered, h waits.
    let plan = organize::plan(&cat).unwrap();
    assert_eq!((plan.gone.clone(), plan.photos, plan.waiting), (vec![e], 1, 1));
    assert_eq!(plan.files.len(), 1);
    assert!(Path::new(&plan.files[0].path).ends_with(Path::new("backup").join("g-copy.jpg")));
    let deleted = organize::delete_confirmed(&cat, &plan.token, &[]).unwrap();
    assert_eq!((deleted.deleted, deleted.forgotten), (0, 1));
    assert!(!exists(&cat, e));

    // Remove from catalog: f leaves; g (a copy on a disk) and h (folder not found) stay.
    assert_eq!(cat.forget_missing(&[f, g, h]).unwrap().photos, 1);
    assert!(!exists(&cat, f) && exists(&cat, g) && exists(&cat, h));
    assert!(backup.join("g-copy.jpg").is_file() && base.join("elsewhere").join("h.jpg").is_file());
    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
