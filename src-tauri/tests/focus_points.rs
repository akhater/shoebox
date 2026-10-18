//! Where the camera focused: read from a photo's maker note when it is
//! scanned, kept in the catalog, given with the photo upright and turned as
//! the user turned it; a photo read before Shoebox knew focus points is read
//! again from its file (metadata only).
//!
//! Lives here, not in src/, because it writes a sample photo (promise 1).
#![cfg(windows)]

use image::RgbImage;
use shoebox_lib::catalog::Catalog;
use shoebox_lib::focus::{self, State};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::scan_root;
use shoebox_lib::turn;
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::AtomicBool;

/// EXIF as a little-endian TIFF: Make "Canon", orientation 6 (taken upright:
/// shown a quarter turn clockwise), and a Canon maker note whose AFInfo2 has
/// two points of a 6000 x 4000 frame, the one right of the middle in focus.
fn canon_exif() -> Vec<u8> {
    let mut af: Vec<u16> = vec![0, 2, 2, 2, 6000, 4000, 6000, 4000, 100, 100, 100, 100, 0, 1500, 0, 0, 0b10, 0b10];
    af[0] = (af.len() * 2) as u16;
    // IFD0 (3 entries) at 8, the EXIF IFD (1 entry) after it, then "Canon\0", then the note.
    let exif_at: u32 = 8 + 2 + 3 * 12 + 4;
    let make_at = exif_at + 2 + 12 + 4;
    let note_at = make_at + 6;
    let mut note = 1u16.to_le_bytes().to_vec();
    for v in [0x26u16, 3] {
        note.extend(v.to_le_bytes());
    }
    note.extend((af.len() as u32).to_le_bytes());
    note.extend((note_at + 18).to_le_bytes()); // Canon's offsets count from the TIFF's start
    note.extend(0u32.to_le_bytes());
    note.extend(af.iter().flat_map(|v| v.to_le_bytes()));

    let entry = |out: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: u32| {
        out.extend(tag.to_le_bytes());
        out.extend(typ.to_le_bytes());
        out.extend(count.to_le_bytes());
        out.extend(value.to_le_bytes());
    };
    let mut t = b"II*\0".to_vec();
    t.extend(8u32.to_le_bytes());
    t.extend(3u16.to_le_bytes());
    entry(&mut t, 0x010F, 2, 6, make_at);
    entry(&mut t, 0x0112, 3, 1, 6);
    entry(&mut t, 0x8769, 4, 1, exif_at);
    t.extend(0u32.to_le_bytes());
    t.extend(1u16.to_le_bytes());
    entry(&mut t, 0x927C, 7, note.len() as u32, note_at);
    t.extend(0u32.to_le_bytes());
    t.extend(b"Canon\0");
    t.extend(note);
    t
}

/// A 300 x 200 JPEG carrying that EXIF.
fn photo(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = RgbImage::from_fn(300, 200, |x, y| image::Rgb([(x % 250) as u8, (y % 250) as u8, 90]));
    let mut jpeg = Vec::new();
    img.write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg).unwrap();
    let exif = canon_exif();
    let mut app1 = vec![0xFF, 0xE1];
    app1.extend(((exif.len() + 8) as u16).to_be_bytes());
    app1.extend(b"Exif\0\0");
    app1.extend(exif);
    let mut file = jpeg[..2].to_vec();
    file.extend(app1);
    file.extend(&jpeg[2..]);
    std::fs::write(path, file).unwrap();
}

/// The middle of the photo's point in focus, as the photo is shown.
fn in_focus(cat: &Catalog, id: i64) -> (f32, f32) {
    let f = cat.photo_detail(id).unwrap().focus.expect("focus points");
    let p = f.points.iter().find(|p| p.state == State::InFocus).expect("a point in focus");
    (p.x + p.w / 2.0, p.y + p.h / 2.0)
}

fn near(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
}

#[test]
fn focus_points_follow_the_photo() {
    let base = std::env::temp_dir().join("shoebox-focus-points");
    let _ = std::fs::remove_dir_all(&base);
    let folder = base.join("photos");
    photo(&folder.join("IMG_0001.JPG"));
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&folder).unwrap();
    scan_root(&cat, root, &AtomicBool::new(false), false, &|_| {}).unwrap();
    let id: i64 = cat.db().query_row("SELECT id FROM photos", [], |r| r.get(0)).unwrap();

    // Read with the photo: right of the middle in the file's frame is below it upright.
    let f = cat.photo_detail(id).unwrap().focus.unwrap();
    assert_eq!(f.points.len(), 2);
    assert!(near(in_focus(&cat, id), (0.5, 0.75)));

    // Turned a quarter clockwise in Shoebox: the point turns with it.
    turn::turn(&cat, &[(id, 1)]).unwrap();
    assert!(near(in_focus(&cat, id), (0.25, 0.5)));

    // Read by an older Shoebox: read again from the file, metadata only.
    cat.db().execute("UPDATE photos SET focus = NULL, focus_read = 0", []).unwrap();
    assert!(cat.photo_detail(id).unwrap().focus.is_none());
    let r = focus::find(&cat, &AtomicBool::new(false)).unwrap();
    assert_eq!((r.read, r.found), (1, 1));
    assert!(near(in_focus(&cat, id), (0.25, 0.5)));
    // Once read, not again.
    assert_eq!(focus::find(&cat, &AtomicBool::new(false)).unwrap().read, 0);
}
