//! Photoshop files are read by their content, whatever their name: the
//! flattened picture inside (RGB, CMYK, grey, indexed colour; 8 or 16 bits;
//! uncompressed, PackBits or zip; .psd or the large .psb) and the original
//! photo's EXIF. A Photoshop file given up on by an older Shoebox is read at
//! the next plug-in, and folders listed by an older one are listed again.
//!
//! Lives here, not in src/, because it writes sample files (promise 1).
#![cfg(windows)]

use shoebox_lib::catalog::Catalog;
use shoebox_lib::formats::Kind;
use shoebox_lib::imaging::{PICTURE_READER, load};
use shoebox_lib::paths::DataDir;
use shoebox_lib::scan::{failed_count, retry_failed, roots_to_list_again, scan_root};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;

struct Doc<'a> {
    w: u32,
    h: u32,
    mode: u16,
    depth: u16,
    /// One plane of samples per channel, row after row.
    planes: Vec<Vec<u16>>,
    compression: u16,
    big: bool,
    exif: Option<&'a [u8]>,
    palette: &'a [u8],
    /// Photoshop's note on the flattened picture: real or a placeholder; None: no note.
    merged: Option<bool>,
    /// How many layers it lists (their records left out: only counted), and
    /// whether in a tagged block of their own, as a 16-bit document does.
    layers: (u16, bool),
}

/// A Photoshop file as Photoshop writes one.
fn write(d: &Doc) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(b"8BPS");
    b.extend((if d.big { 2u16 } else { 1 }).to_be_bytes());
    b.extend([0; 6]);
    b.extend((d.planes.len() as u16).to_be_bytes());
    b.extend(d.h.to_be_bytes());
    b.extend(d.w.to_be_bytes());
    b.extend(d.depth.to_be_bytes());
    b.extend(d.mode.to_be_bytes());
    b.extend((d.palette.len() as u32).to_be_bytes());
    b.extend(d.palette);
    let mut res = Vec::new();
    let mut resource = |id: u16, data: &[u8]| {
        res.extend(b"8BIM");
        res.extend(id.to_be_bytes());
        res.extend([0, 0]); // no name, padded
        res.extend((data.len() as u32).to_be_bytes());
        res.extend(data);
        if data.len() % 2 == 1 {
            res.push(0);
        }
    };
    if let Some(merged) = d.merged {
        resource(1057, &[0, 0, 0, 1, merged as u8]);
    }
    if let Some(e) = d.exif {
        resource(1058, e);
    }
    b.extend((res.len() as u32).to_be_bytes());
    b.extend(&res);
    let len = |n: usize| if d.big { (n as u64).to_be_bytes().to_vec() } else { (n as u32).to_be_bytes().to_vec() };
    let section: Vec<u8> = match d.layers {
        (0, _) => Vec::new(),
        // An empty list, no global mask, then the block listing them.
        (n, true) => [len(0), vec![0; 4], b"8BIMLr16".to_vec(), len(2), n.to_be_bytes().to_vec()].concat(),
        (n, false) => [len(2), n.to_be_bytes().to_vec(), vec![0; 4]].concat(),
    };
    b.extend(len(section.len()));
    b.extend(section);
    b.extend(d.compression.to_be_bytes());
    let rows: Vec<Vec<u8>> = d
        .planes
        .iter()
        .flat_map(|p| {
            p.chunks(d.w as usize).map(|r| match d.depth {
                8 => r.iter().map(|&s| s as u8).collect(),
                _ => r.iter().flat_map(|&s| s.to_be_bytes()).collect::<Vec<u8>>(),
            })
        })
        .collect();
    match d.compression {
        0 => rows.iter().for_each(|r| b.extend(r)),
        1 => {
            // PackBits, literal runs only.
            let packed: Vec<Vec<u8>> = rows
                .iter()
                .map(|r| {
                    r.chunks(128).flat_map(|c| std::iter::once(c.len() as u8 - 1).chain(c.iter().copied())).collect()
                })
                .collect();
            for p in &packed {
                if d.big {
                    b.extend((p.len() as u32).to_be_bytes());
                } else {
                    b.extend((p.len() as u16).to_be_bytes());
                }
            }
            packed.iter().for_each(|p| b.extend(p));
        }
        _ => {
            // Zip with prediction: each sample as its difference from the one before.
            let mut all = Vec::new();
            for r in &rows {
                if d.depth == 8 {
                    all.extend(r.iter().enumerate().map(|(i, &s)| if i == 0 { s } else { s.wrapping_sub(r[i - 1]) }));
                } else {
                    let s: Vec<u16> = r.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                    all.extend(
                        s.iter()
                            .enumerate()
                            .flat_map(|(i, &v)| (if i == 0 { v } else { v.wrapping_sub(s[i - 1]) }).to_be_bytes()),
                    );
                }
            }
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(&all).unwrap();
            b.extend(z.finish().unwrap());
        }
    }
    b
}

fn doc(w: u32, h: u32, mode: u16, depth: u16, planes: Vec<Vec<u16>>, compression: u16) -> Doc<'static> {
    Doc {
        w,
        h,
        mode,
        depth,
        planes,
        compression,
        big: false,
        exif: None,
        palette: &[],
        merged: Some(true),
        layers: (0, false),
    }
}

fn exif_bytes() -> Vec<u8> {
    use exif::{Field, In, Tag, Value};
    let fields = [(Tag::Make, "Canon"), (Tag::Model, "Canon EOS 40D"), (Tag::DateTimeOriginal, "2008:09:25 14:30:00")]
        .map(|(tag, v)| Field { tag, ifd_num: In::PRIMARY, value: Value::Ascii(vec![v.as_bytes().to_vec()]) });
    let mut w = exif::experimental::Writer::new();
    fields.iter().for_each(|f| w.push_field(f));
    let mut out = std::io::Cursor::new(Vec::new());
    w.write(&mut out, false).unwrap();
    out.into_inner()
}

fn pixels(path: &Path, kind: Kind, long: u32) -> (u32, u32, Vec<[u8; 3]>) {
    let img = load(path, kind, long).image.unwrap_or_else(|e| panic!("{}: {e}", path.display())).to_rgb8();
    (img.width(), img.height(), img.pixels().map(|p| p.0).collect())
}

#[test]
fn photoshop_files_are_read_whatever_their_name() {
    let base = std::env::temp_dir().join("shoebox-photoshop");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let put = |name: &str, d: &Doc| {
        let p = base.join(name);
        std::fs::write(&p, write(d)).unwrap();
        p
    };

    // RGB, PackBits, named like a camera RAW, with the original photo's EXIF.
    let exif = exif_bytes();
    let rgb = Doc {
        exif: Some(&exif),
        ..doc(
            3,
            2,
            3,
            8,
            vec![vec![255, 0, 0, 10, 20, 30], vec![0, 255, 0, 40, 50, 60], vec![0, 0, 255, 70, 80, 90]],
            1,
        )
    };
    let raw = put("IMG_0077.raw", &rgb);
    let loaded = load(&raw, Kind::Raw, 2048);
    assert_eq!(loaded.meta.model.as_deref(), Some("Canon EOS 40D"));
    assert_eq!(loaded.meta.taken.unwrap().to_string(), "2008-09-25 14:30:00");
    assert_eq!((loaded.meta.width, loaded.meta.height), (Some(3), Some(2)));
    assert_eq!(
        pixels(&raw, Kind::Raw, 2048),
        (3, 2, vec![[255, 0, 0], [0, 255, 0], [0, 0, 255], [10, 40, 70], [20, 50, 80], [30, 60, 90]])
    );

    // 16 bits, uncompressed: the high byte of each sample.
    let p = put("deep.psd", &doc(2, 1, 3, 16, vec![vec![0x1234, 0xFFFF], vec![0x5600, 0], vec![0x9A00, 0x0100]], 0));
    assert_eq!(pixels(&p, Kind::Image, 2048).2, [[0x12, 0x56, 0x9A], [0xFF, 0, 1]]);

    // CMYK in a large document (.psb), PackBits: stored inverted, 255 = no ink.
    let cmyk =
        Doc { big: true, ..doc(2, 1, 4, 8, vec![vec![255, 0], vec![255, 255], vec![0, 255], vec![255, 255]], 1) };
    let p = put("print.psb", &cmyk);
    assert_eq!(pixels(&p, Kind::Image, 2048).2, [[255, 255, 0], [0, 255, 255]]);

    // Grey, 16 bits, zip with prediction.
    let p = put("grey.psd", &doc(3, 1, 1, 16, vec![vec![0x1000, 0x8000, 0x0100]], 3));
    assert_eq!(pixels(&p, Kind::Image, 2048).2, [[0x10; 3], [0x80; 3], [0x01; 3]]);

    // Indexed colour: through its palette.
    let mut palette = vec![0u8; 768];
    (palette[7], palette[256 + 7], palette[512 + 7]) = (1, 2, 3);
    let p = put("gif-like.psd", &Doc { palette: &palette, ..doc(1, 1, 2, 8, vec![vec![7]], 0) });
    assert_eq!(pixels(&p, Kind::Image, 2048).2, [[1, 2, 3]]);

    // A big document is read every few rows and columns for a small preview.
    let plane: Vec<u16> = (0..40 * 20).map(|i| (i % 40) as u16).collect();
    let p = put("wide.psd", &doc(40, 20, 1, 8, vec![plane], 1));
    let (w, h, px) = pixels(&p, Kind::Image, 5);
    assert_eq!((w, h), (10, 5));
    assert_eq!(px[1], [4; 3], "column 4 of the document");

    // Saved without "Maximize compatibility": a white placeholder stands in
    // for the picture. Said, not shown white - whether Photoshop says so, or
    // leaves its note out of a layered document (layers listed either way,
    // in a large document too).
    let white = || doc(4, 3, 3, 8, vec![vec![255; 12]; 3], 1);
    for (merged, layers, big) in
        [(Some(false), (0, false), false), (None, (4, true), false), (None, (2, false), false), (None, (3, true), true)]
    {
        let p = put("layers.psd", &Doc { merged, layers, big, ..white() });
        let loaded = load(&p, Kind::Image, 2048);
        let e = loaded.image.unwrap_err();
        assert!(e.contains("Maximize compatibility") && e.contains("re-save"), "{merged:?} {layers:?}: {e}");
        assert_eq!(loaded.meta.width, Some(4), "its size is still known");
    }
    // White Photoshop vouches for, or a document without layers, is white.
    for (merged, layers) in [(Some(true), (4, true)), (None, (0, false))] {
        let p = put("white.psd", &Doc { merged, layers, ..white() });
        assert_eq!(pixels(&p, Kind::Image, 2048).2, [[255; 3]; 12], "{merged:?} {layers:?}");
    }

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn photoshop_files_join_the_catalog() {
    let base = std::env::temp_dir().join("shoebox-photoshop-catalog");
    let _ = std::fs::remove_dir_all(&base);
    let photos = base.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    let plane = |v: u16| vec![v; 64 * 48];
    std::fs::write(photos.join("edit.psd"), write(&doc(64, 48, 3, 8, vec![plane(200), plane(100), plane(50)], 1)))
        .unwrap();
    std::fs::write(photos.join("IMG_0077.raw"), write(&doc(48, 64, 1, 8, vec![plane(90)], 0))).unwrap();
    let data = DataDir { root: base.join("catalog") };
    data.ensure().unwrap();
    let cat = Catalog::open(data).unwrap();
    let root = cat.add_root(&photos).unwrap();
    let no = AtomicBool::new(false);
    let r = scan_root(&cat, root, &no, false, &|_| {}).unwrap();
    assert_eq!((r.photos_added, r.errors), (2, 0), "{r:?}");
    let ready: i64 =
        cat.db().query_row("SELECT COUNT(*) FROM photos WHERE preview_state = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(ready, 2);
    let volume: i64 = cat.db().query_row("SELECT volume_id FROM roots WHERE id = ?1", [root], |r| r.get(0)).unwrap();

    // Read by an older Shoebox, which could show the white placeholder: a .psd
    // is read again once, at the next plug-in.
    cat.db().execute_batch("PRAGMA user_version = 11;").unwrap();
    drop(cat);
    let cat = Catalog::open(DataDir { root: base.join("catalog") }).unwrap();
    assert_eq!(failed_count(&cat, volume).unwrap(), 1);
    let r = retry_failed(&cat, volume, &no, &|_| {}).unwrap();
    assert_eq!((r.reread, r.errors), (1, 0), "{r:?}");

    // Given up on by an older Shoebox, which couldn't read it: read at the next plug-in.
    let pid: i64 =
        cat.db().query_row("SELECT photo_id FROM files WHERE name = 'IMG_0077.raw'", [], |r| r.get(0)).unwrap();
    cat.db()
        .execute(
            "UPDATE photos SET preview_state = 2, error = 'no decoder', given_up = ?1 WHERE id = ?2",
            [PICTURE_READER - 1, pid],
        )
        .unwrap();
    assert_eq!(failed_count(&cat, volume).unwrap(), 1);
    let r = retry_failed(&cat, volume, &no, &|_| {}).unwrap();
    assert_eq!((r.reread, r.errors), (1, 0), "{r:?}");

    // A folder an older Shoebox listed is listed again, once.
    assert!(roots_to_list_again(&cat, volume).unwrap().is_empty());
    cat.db().execute("UPDATE roots SET reader = ?1", [PICTURE_READER - 1]).unwrap();
    assert_eq!(roots_to_list_again(&cat, volume).unwrap(), [root]);
    scan_root(&cat, root, &no, false, &|_| {}).unwrap();
    assert!(roots_to_list_again(&cat, volume).unwrap().is_empty());

    drop(cat);
    let _ = std::fs::remove_dir_all(&base);
}
