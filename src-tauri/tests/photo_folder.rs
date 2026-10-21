//! "Go to folder": each copy of a photo says which folder it is in, and that
//! folder, shown as the window and `shoebox-cli list` show one, holds the photo.
//!
//! Lives here, not in src/, because it writes sample photos: no file under
//! src/ outside paths.rs may hold file-writing code (promise 1).

use shoebox_lib::catalog::{Catalog, Filters, Scope, Sort};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use shoebox_lib::volumes::SEP;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn jpeg(path: &Path, shade: u8) {
    image::RgbImage::from_fn(320, 240, |x, y| image::Rgb([shade, (x % 256) as u8, (y % 256) as u8]))
        .save(path)
        .unwrap();
}

fn folder_list(cat: &Catalog, volume_id: i64, rel_path: &str) -> Vec<i64> {
    let scope = Scope::Folder { volume_id, rel_path: rel_path.into() };
    cat.list_photos(&scope, Sort::default(), &Filters::default()).unwrap().ids
}

#[test]
fn each_copy_leads_to_its_folder() {
    let base = std::env::temp_dir().join("shoebox-photo-folder");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    let wedding = photos.join("2007").join("wedding");
    let backup = photos.join("backup");
    std::fs::create_dir_all(&wedding).unwrap();
    std::fs::create_dir_all(&backup).unwrap();
    jpeg(&wedding.join("a.jpg"), 10);
    std::fs::copy(wedding.join("a.jpg"), backup.join("a copy.jpg")).unwrap();
    jpeg(&photos.join("2007").join("b.jpg"), 200);

    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();

    let a: i64 = cat.db().query_row("SELECT photo_id FROM files WHERE name = 'a.jpg'", [], |r| r.get(0)).unwrap();
    let b: i64 = cat.db().query_row("SELECT photo_id FROM files WHERE name = 'b.jpg'", [], |r| r.get(0)).unwrap();
    let d = cat.photo_detail(a).unwrap();
    let mut folders: Vec<&str> = d.files.iter().map(|f| f.folder.as_str()).collect();
    folders.sort();
    assert_eq!(folders.len(), 2, "{folders:?}");
    // A folder is kept as its names joined by backslashes, on every system.
    assert!(folders[0].ends_with(&format!("photos{SEP}2007{SEP}wedding")), "{folders:?}");
    assert!(folders[1].ends_with(&format!("photos{SEP}backup")), "{folders:?}");

    for f in &d.files {
        // The folder is where the file is: its path is that folder plus the name,
        // as the system writes a path.
        let on_disk: PathBuf = f.folder.split(SEP).chain(std::iter::once(f.name.as_str())).collect();
        assert!(Path::new(&f.path).ends_with(&on_disk), "{} / {}", f.path, f.folder);
        // ...and showing it shows the photo (and only what is in it).
        assert_eq!(folder_list(&cat, f.volume_id, &f.folder), vec![a], "{}", f.folder);
    }
    // The folder above holds both photos.
    let parent = folders[0].rsplit_once(SEP).unwrap().0;
    let (mut above, mut both) = (folder_list(&cat, d.files[0].volume_id, parent), vec![a, b]);
    above.sort();
    both.sort();
    assert_eq!(above, both);

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
