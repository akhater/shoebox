//! Where the camera focused: the autofocus points a photo's maker note
//! records, drawn over the photo in the viewer (as the camera makers' own
//! software does) and given by `shoebox-cli photo`.
//!
//! Read with the rest of a photo's metadata when it is scanned (its disk is
//! usually unplugged later) and kept small in the catalog: a photo keeps the
//! points it used, a few bytes each, and which layout of points its camera
//! has; each layout is kept once, in `focus_grids`. Photos read before
//! Shoebox knew focus points are read again from their disks when plugged in
//! ([`find`]), metadata only.
//!
//! Points are fractions of the picture (0..1 of its width and height, from its
//! top-left corner) so they fit any size of it: the file's own frame as
//! decoded, upright once [`Focus::upright`] has turned them as the picture
//! is turned, then turned as the user turned the photo ([`Focus::turned`]).

use crate::catalog::Catalog;
use crate::db::FILE_PRESENT;
use anyhow::Result;
use focus_points::{Directory, Value};
use rawler::formats::tiff::{IFD, Value as TiffValue};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

// The reader itself is the `focus-points` crate; what the rest of Shoebox uses of it is named here.
pub use focus_points::{Bracket, Focus, MODES, OLYMPUS_CAMERA_SETTINGS, Point, State, from_crw};

/// Version of how focus points are read. Photos an older version read are
/// read again ([`find`]); raise it when Shoebox learns another maker's notes.
/// 1: Canon, Nikon, Sony, Fujifilm, Olympus / OM System, Panasonic / Leica,
/// Pentax / Ricoh, and any maker's EXIF SubjectArea (phones).
/// 2: Canon: a zone in a fine grid is one outline whatever its size, and a
/// photo focused by hand has no point drawn.
/// 3: Canon: a frame of a focus bracket says so ([`Bracket`]).
pub const FOCUS_READER: i64 = 3;

/// What a file's metadata holds, as its focus points are read from it.
pub struct Source<'a> {
    /// The maker and the model as the file names them.
    pub make: &'a str,
    pub model: &'a str,
    /// Its EXIF and main directories.
    pub ifds: &'a [&'a IFD],
    /// Its maker's note, when Shoebox could find it.
    pub note: Option<&'a IFD>,
}

/// A directory of rawler's, as `focus-points` reads tags.
#[derive(Clone, Copy)]
struct Tags<'a>(&'a IFD);

impl Directory for Tags<'_> {
    fn value(&self, tag: u16) -> Option<Value<'_>> {
        Some(match &self.0.get_entry(tag)?.value {
            TiffValue::Undefined(b) | TiffValue::Byte(b) => Value::Bytes(b),
            TiffValue::Short(v) => Value::Short(v),
            TiffValue::SShort(v) => Value::SShort(v),
            TiffValue::Long(v) => Value::Long(v),
            TiffValue::SLong(v) => Value::SLong(v),
            TiffValue::Rational(v) => Value::Rational(v.iter().map(|r| (r.n, r.d)).collect()),
            TiffValue::SRational(v) => Value::SRational(v.iter().map(|r| (r.n, r.d)).collect()),
            _ => return None,
        })
    }

    fn sub(&self, tag: u16) -> Option<Self> {
        self.0.get_sub_ifd(tag).map(Tags)
    }

    fn big_endian(&self) -> bool {
        matches!(self.0.endian, rawler::bits::Endian::Big)
    }
}

fn with_tags<'a, T>(src: &Source<'a>, f: impl FnOnce(&focus_points::Source<Tags<'a>>) -> T) -> T {
    let ifds: Vec<Tags> = src.ifds.iter().map(|i| Tags(i)).collect();
    f(&focus_points::Source { make: src.make, model: src.model, ifds: &ifds, note: src.note.map(Tags) })
}

/// Where the camera focused, in the file's own frame: not yet turned upright
/// (see [`focus_points::read`]). `picture`: the picture's size as the file
/// stores it, when it may have been edited since the camera made it (a JPEG).
pub fn read(src: &Source, picture: Option<(u32, u32)>) -> Option<Focus> {
    with_tags(src, |s| focus_points::read(s, picture))
}

/// Whether the file is a frame of a focus bracket, when its maker's note says.
pub fn bracket(src: &Source) -> Option<Bracket> {
    with_tags(src, focus_points::bracket)
}

// ---- Kept in the catalog --------------------------------------------------

const BLOB_VERSION: u8 = 1;

fn q16(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

fn f16(b: &[u8]) -> f32 {
    u16::from_le_bytes([b[0], b[1]]) as f32 / 65535.0
}

fn put_box(out: &mut Vec<u8>, p: &Point) {
    // A point running off an edge keeps what is on the picture.
    let (x0, y0) = (p.x.max(0.0), p.y.max(0.0));
    let (x1, y1) = ((p.x + p.w).min(1.0), (p.y + p.h).min(1.0));
    for v in [x0, y0, x1 - x0, y1 - y0] {
        out.extend_from_slice(&q16(v).to_le_bytes());
    }
}

fn get_box(b: &[u8], state: State) -> Point {
    Point { x: f16(&b[0..2]), y: f16(&b[2..4]), w: f16(&b[4..6]), h: f16(&b[6..8]), state }
}

fn state_code(s: State) -> u8 {
    match s {
        State::Other => 0,
        State::Chosen => 1,
        State::InFocus => 2,
        State::Area => 3,
    }
}

fn state_of(code: u8) -> State {
    match code {
        1 => State::Chosen,
        2 => State::InFocus,
        3 => State::Area,
        _ => State::Other,
    }
}

/// What the catalog keeps of a photo's focus (`photos.focus`): its mode, its
/// camera's layout of points (an id in `focus_grids`, saved there the first
/// time) with the points it used, else its points as they are.
pub fn stored(db: &Connection, focus: Option<&Focus>) -> rusqlite::Result<Option<Vec<u8>>> {
    let Some(f) = focus else { return Ok(None) };
    let code = f.mode.and_then(|m| MODES.iter().position(|x| *x == m)).map_or(0, |i| i as u8 + 1);
    let mut out = vec![BLOB_VERSION, code];
    let layout = f.points.len() > 1 && f.points.iter().any(|p| p.state == State::Other);
    if layout {
        let mut grid = Vec::with_capacity(f.points.len() * 8);
        for p in &f.points {
            put_box(&mut grid, p);
        }
        db.execute("INSERT INTO focus_grids (points) VALUES (?1) ON CONFLICT (points) DO NOTHING", [&grid])?;
        let id: i64 = db.query_row("SELECT id FROM focus_grids WHERE points = ?1", [&grid], |r| r.get(0))?;
        out.extend_from_slice(&(id as u32).to_le_bytes());
        let used: Vec<(usize, &Point)> = f.points.iter().enumerate().filter(|(_, p)| p.state != State::Other).collect();
        out.extend_from_slice(&(used.len() as u16).to_le_bytes());
        for (i, p) in used {
            out.extend_from_slice(&(i as u16).to_le_bytes());
            out.push(state_code(p.state));
        }
    } else {
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(f.points.len() as u16).to_le_bytes());
        for p in &f.points {
            put_box(&mut out, p);
            out.push(state_code(p.state));
        }
    }
    Ok(Some(out))
}

/// A photo's focus as the catalog keeps it (see [`stored`]); None if it
/// can't be read (a newer Shoebox's).
pub fn decode(db: &Connection, blob: &[u8]) -> rusqlite::Result<Option<Focus>> {
    if blob.len() < 8 || blob[0] != BLOB_VERSION {
        return Ok(None);
    }
    let mode = (blob[1] as usize).checked_sub(1).and_then(|i| MODES.get(i)).copied();
    let grid = u32::from_le_bytes([blob[2], blob[3], blob[4], blob[5]]);
    let count = u16::from_le_bytes([blob[6], blob[7]]) as usize;
    let rest = &blob[8..];
    let points = if grid != 0 {
        // Kept prepared: a shoot decodes thousands of photos, most of one layout.
        let layout: Option<Vec<u8>> = db
            .prepare_cached("SELECT points FROM focus_grids WHERE id = ?1")?
            .query_row([grid], |r| r.get(0))
            .optional()?;
        let Some(layout) = layout else { return Ok(None) };
        let mut points: Vec<Point> = layout.as_chunks::<8>().0.iter().map(|b| get_box(b, State::Other)).collect();
        for used in rest.as_chunks::<3>().0.iter().take(count) {
            if let Some(p) = points.get_mut(u16::from_le_bytes([used[0], used[1]]) as usize) {
                p.state = state_of(used[2]);
            }
        }
        points
    } else {
        rest.as_chunks::<9>().0.iter().take(count).map(|b| get_box(&b[..8], state_of(b[8]))).collect()
    };
    Ok(Some(Focus { mode, points }))
}

// ---- Photos read before ---------------------------------------------------

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FocusReport {
    /// Originals read again (metadata only).
    pub read: usize,
    /// Of those, with focus points.
    pub found: usize,
    /// Of those, frames of a focus bracket.
    pub brackets: usize,
}

/// Photos an older Shoebox read, before it knew focus points (or another
/// maker's, or focus brackets): their originals on the disks plugged in are
/// read again, metadata only, never the picture; the others wait for their
/// disk. The frames of focus brackets found are then gathered into stacks.
pub fn find(cat: &Catalog, cancel: &AtomicBool) -> Result<FocusReport> {
    // Reader 3 learned only Canon's brackets: other makers' photos read by 2
    // have nothing new to give.
    cat.db().execute(
        "UPDATE photos SET focus_read = 3 WHERE focus_read = 2 AND lower(COALESCE(make, '')) NOT LIKE 'canon%'",
        [],
    )?;
    let online = cat.online_ids();
    let mut report = FocusReport::default();
    if online.is_empty() {
        return Ok(report);
    }
    let volumes = online.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    let pending: Vec<i64> = cat
        .read()?
        .prepare(&format!(
            "SELECT p.id FROM photos p WHERE p.focus_read < ?1 AND EXISTS (
                 SELECT 1 FROM files f JOIN folders fo ON fo.id = f.folder_id
                  WHERE f.photo_id = p.id AND f.state = {FILE_PRESENT} AND fo.volume_id IN ({volumes}))
             ORDER BY p.id"
        ))?
        .query_map([FOCUS_READER], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for chunk in pending.chunks(200) {
        let mut read = Vec::with_capacity(chunk.len());
        for &id in chunk {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            // Unplugged meanwhile: it waits for its disk.
            let Ok((path, kind, _)) = cat.original(id) else { continue };
            let (focus, bracket) = crate::imaging::read_note(&path, kind);
            read.push((id, focus, bracket));
        }
        let db = cat.db();
        let tx = db.unchecked_transaction()?;
        for (id, focus, bracket) in &read {
            let blob = stored(&tx, focus.as_ref())?;
            tx.execute(
                "UPDATE photos SET focus = ?1, focus_read = ?2, bracket = ?3 WHERE id = ?4",
                params![blob, FOCUS_READER, crate::stacks::stored(bracket.as_ref()), id],
            )?;
        }
        tx.commit()?;
        report.read += read.len();
        report.found += read.iter().filter(|(_, f, _)| f.is_some()).count();
        report.brackets += read.iter().filter(|(_, _, b)| b.is_some()).count();
        if cancel.load(Ordering::Relaxed) {
            break;
        }
    }
    if report.read > 0 {
        log::info!("focus points: {report:?}");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// A 5D Mark II's AFInfo2 as its CR2 keeps it: nine points in a diamond,
    /// the right one in focus and chosen.
    fn five_d_mark_ii() -> Vec<u16> {
        let mut v = vec![96, 2, 9, 9, 5616, 3744, 5616, 3744];
        v.extend([84, 84, 101, 84, 84, 84, 101, 84, 123]);
        v.extend([101, 101, 74, 101, 101, 101, 74, 101, 131]);
        v.extend([-1173i16, -561, 0, 561, 1173, 561, 0, -561, 0].map(|x| x as u16));
        v.extend([0i16, 280, 501, 280, 0, -280, -501, -280, 0].map(|y| y as u16));
        v.extend([0b10000, 0b10000, 0, 0xFFFF]);
        v
    }

    /// An R6 Mark III's AFInfo3: a grid of 1,053 cells in a 6960 x 4640
    /// frame, of which a photo gives only its first `cells` (width, height,
    /// X, Y), the rest left at zero, and the cells in focus and chosen.
    fn r6_mark_iii(mode: u16, cells: &[(u16, u16, i16, i16)], in_focus: &[usize], chosen: &[usize]) -> Vec<u16> {
        let n = 1053;
        let mut v = vec![8838, mode, n as u16, cells.len() as u16, 6960, 4640, 6960, 4640];
        for k in 0..4 {
            v.extend((0..n).map(|i| cells.get(i).map_or(0, |c| [c.0, c.1, c.2 as u16, c.3 as u16][k])));
        }
        for set in [in_focus, chosen] {
            let mut mask = vec![0u16; n.div_ceil(16)];
            for &i in set {
                mask[i / 16] |= 1 << (i % 16);
            }
            v.extend(mask);
        }
        v
    }

    #[test]
    fn an_r6_mark_iii_tracking_frame_turned_upright() {
        // Whole area AF following an eye, the camera held upright
        // (orientation 8): one frame, in focus, in its AFInfo3.
        let v = r6_mark_iii(22, &[(532, 533, 1260, 252)], &[0], &[0]);
        let note = dir(&[short_tag(0x3c, &v)]);
        let src = Source { make: "Canon", model: "Canon EOS R6 Mark III", ifds: &[], note: Some(&note) };
        let f = read(&src, None).unwrap().upright(8);
        assert_eq!(f.mode, Some("Whole area"));
        // Right of the middle and above it, turned a quarter counter-clockwise:
        // above the middle and left of it.
        let (cx, cy, w, h, state) = only(&f);
        assert!(close(cx, 0.5 - 252.0 / 4640.0) && close(cy, 0.5 - 1260.0 / 6960.0));
        assert!(close(w, 533.0 / 4640.0) && close(h, 532.0 / 6960.0) && state == State::InFocus);
    }

    #[test]
    fn kept_small_and_read_back() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE focus_grids (id INTEGER PRIMARY KEY, points BLOB NOT NULL UNIQUE);").unwrap();
        let note = dir(&[short_tag(0x26, &five_d_mark_ii())]);
        let src = Source { make: "Canon", model: "Canon EOS 5D Mark II", ifds: &[], note: Some(&note) };
        let f = read(&src, None).unwrap();
        let blob = stored(&db, Some(&f)).unwrap().unwrap();
        // The layout once; the photo keeps one point used.
        assert_eq!(blob.len(), 8 + 3);
        let again = stored(&db, Some(&f)).unwrap().unwrap();
        assert_eq!(blob, again);
        let n: i64 = db.query_row("SELECT COUNT(*) FROM focus_grids", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let back = decode(&db, &blob).unwrap().unwrap();
        assert_eq!(back.mode, f.mode);
        for (a, b) in back.points.iter().zip(&f.points) {
            assert_eq!(a.state, b.state);
            assert!(close(a.x, b.x) && close(a.y, b.y) && close(a.w, b.w) && close(a.h, b.h));
        }
        // A single frame (a mirrorless camera following a face) is kept as it is.
        let one = Focus {
            mode: Some("Whole area"),
            points: vec![Point { x: 0.4, y: 0.3, w: 0.08, h: 0.12, state: State::InFocus }],
        };
        let blob = stored(&db, Some(&one)).unwrap().unwrap();
        assert_eq!(blob.len(), 8 + 9);
        assert_eq!(decode(&db, &blob).unwrap().unwrap().points[0].state, State::InFocus);
    }

    /// A directory as a little-endian TIFF keeps it: (tag, type, count, value bytes).
    fn dir(entries: &[(u16, u16, u32, Vec<u8>)]) -> IFD {
        use rawler::formats::tiff::reader::TiffReader;
        let mut entries = entries.to_vec();
        entries.sort_by_key(|e| e.0);
        let data_at = 8 + 2 + entries.len() * 12 + 4;
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        out.extend((entries.len() as u16).to_le_bytes());
        let mut data: Vec<u8> = Vec::new();
        for (tag, typ, count, bytes) in &entries {
            out.extend(tag.to_le_bytes());
            out.extend(typ.to_le_bytes());
            out.extend(count.to_le_bytes());
            if bytes.len() <= 4 {
                out.extend(bytes.iter().copied().chain(std::iter::repeat(0)).take(4));
            } else {
                out.extend(((data_at + data.len()) as u32).to_le_bytes());
                data.extend(bytes);
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(data);
        rawler::formats::tiff::GenericTiffReader::new_with_buffer(&out, 0, 0, Some(0)).unwrap().root_ifd().clone()
    }

    fn short_tag(tag: u16, v: &[u16]) -> (u16, u16, u32, Vec<u8>) {
        (tag, 3, v.len() as u32, v.iter().flat_map(|x| x.to_le_bytes()).collect())
    }

    fn rational_tag(tag: u16, v: &[(u32, u32)]) -> (u16, u16, u32, Vec<u8>) {
        (
            tag,
            5,
            v.len() as u32,
            v.iter().flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes())).collect(),
        )
    }

    fn read_one(make: &str, note: Option<&IFD>, ifds: &[&IFD]) -> Option<Focus> {
        read(&Source { make, model: "", ifds, note }, None)
    }

    /// The one point drawn: its centre, size and state.
    fn only(f: &Focus) -> (f32, f32, f32, f32, State) {
        assert_eq!(f.points.len(), 1, "{f:?}");
        let p = f.points[0];
        (p.x + p.w / 2.0, p.y + p.h / 2.0, p.w, p.h, p.state)
    }

    fn about(a: (f32, f32, f32, f32, State), b: (f32, f32, f32, f32, State)) -> bool {
        close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2) && close(a.3, b.3) && a.4 == b.4
    }

    #[test]
    fn a_compact_canon_counts_y_down_and_has_no_chosen_mask() {
        let mut v = five_d_mark_ii();
        // Nothing in focus; the word after the mask is not a chosen mask on a compact.
        let mask = 8 + 4 * 9;
        v[mask] = 0;
        v[mask + 1] = 0b100;
        let mut shot = vec![0u16; 30];
        shot[26] = 250;
        let note = dir(&[short_tag(0x26, &v), short_tag(0x04, &shot)]);
        let src = Source { make: "Canon", model: "Canon PowerShot S50", ifds: &[], note: Some(&note) };
        let f = read(&src, None).unwrap();
        assert!(f.points.iter().all(|p| p.state == State::Other));
        // The top point (Y +501) is below the middle there.
        assert!(close(f.points[2].y + f.points[2].h / 2.0, 0.5 + 501.0 / 3744.0));
    }

    #[test]
    fn nikon_z_cameras_place_their_area() {
        let mut b = vec![0u8; 0x50];
        b[..4].copy_from_slice(b"0400");
        b[5] = 193; // single point
        b[7] = 1; // coordinates available
        for (at, v) in [(0x3e, 8256u16), (0x40, 5504), (0x42, 2000), (0x44, 1000), (0x46, 400), (0x48, 300)] {
            b[at..at + 2].copy_from_slice(&v.to_le_bytes());
        }
        b[0x4a] = 1; // in focus
        let note = dir(&[(0xb7, 7, b.len() as u32, b.clone())]);
        let f = read_one("NIKON CORPORATION", Some(&note), &[]).unwrap();
        assert_eq!(f.mode, Some("Single point"));
        assert!(about(only(&f), (2000.0 / 8256.0, 1000.0 / 5504.0, 400.0 / 8256.0, 300.0 / 5504.0, State::InFocus)));
        // Through a DSLR's viewfinder (phase detection) its points are only named: nothing.
        b[..4].copy_from_slice(b"0100");
        b[4] = 0;
        assert!(read_one("NIKON CORPORATION", Some(&dir(&[(0xb7, 7, b.len() as u32, b)])), &[]).is_none());
    }

    #[test]
    fn sony_says_where_it_focused() {
        let note = dir(&[short_tag(0x2027, &[6000, 4000, 4500, 1000]), short_tag(0x2037, &[300, 200, 257])]);
        let f = read_one("SONY", Some(&note), &[]).unwrap();
        assert!(about(only(&f), (0.75, 0.25, 0.05, 0.05, State::InFocus)));
        // The middle and no frame: no focus location recorded.
        let note = dir(&[short_tag(0x2027, &[6000, 4000, 3000, 2000])]);
        assert!(read_one("SONY", Some(&note), &[]).is_none());
    }

    #[test]
    fn fujifilm_points_in_its_jpeg() {
        let exif = dir(&[short_tag(0xA002, &[6000]), short_tag(0xA003, &[4000])]);
        let note = dir(&[short_tag(0x1022, &[1]), short_tag(0x1023, &[3000, 1000])]);
        let f = read_one("FUJIFILM", Some(&note), &[&exif]).unwrap();
        assert_eq!(f.mode, Some("Single point"));
        let (cx, cy, _, _, state) = only(&f);
        assert!(close(cx, 0.5) && close(cy, 0.25) && state == State::Chosen);
        // Focused by hand: says so, nothing to draw.
        let manual = dir(&[short_tag(0x1021, &[1]), short_tag(0x1023, &[3000, 1000])]);
        let f = read_one("FUJIFILM", Some(&manual), &[&exif]).unwrap();
        assert_eq!(f, Focus { mode: Some("Manual focus"), points: vec![] });
    }

    #[test]
    fn olympus_and_om_cameras_place_their_af_area() {
        let settings = dir(&[short_tag(0x0303, &[1]), short_tag(0x030a, &[640, 480, 320, 120, 64, 48, 0, 0, 0, 0])]);
        let mut note = dir(&[short_tag(0x0001, &[0])]);
        note.sub.insert(OLYMPUS_CAMERA_SETTINGS, vec![settings]);
        let f = read_one("OM Digital Solutions", Some(&note), &[]).unwrap();
        assert!(about(only(&f), (0.55, 0.3, 0.1, 0.1, State::InFocus)));
        // Older ones: the point chosen, as fractions.
        let point: Vec<u8> = [(0i32, 1i32), (1, 4), (3, 4), (1, 4), (3, 4)]
            .iter()
            .flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes()))
            .collect();
        let settings = dir(&[(0x0305, 10, 5, point)]);
        let mut note = dir(&[short_tag(0x0001, &[0])]);
        note.sub.insert(OLYMPUS_CAMERA_SETTINGS, vec![settings]);
        let (cx, cy, _, _, state) = only(&read_one("OLYMPUS IMAGING CORP.", Some(&note), &[]).unwrap());
        assert!(close(cx, 0.25) && close(cy, 0.75) && state == State::Chosen);
    }

    #[test]
    fn panasonic_gives_its_af_area_as_fractions() {
        let note = dir(&[
            rational_tag(0x004d, &[(1, 4), (1, 2)]),
            rational_tag(0x00de, &[(1, 10), (1, 10)]),
            (0x000f, 1, 2, vec![16, 0]),
        ]);
        let f = read_one("Panasonic", Some(&note), &[]).unwrap();
        assert_eq!(f.mode, Some("Single point"));
        assert!(about(only(&f), (0.25, 0.5, 0.1, 0.1, State::Chosen)));
        // No AF point: nothing to draw.
        let none = dir(&[rational_tag(0x004d, &[(16777216, 1), (16777216, 1)])]);
        assert!(read_one("Panasonic", Some(&none), &[]).is_none());
    }

    #[test]
    fn pentax_live_view_area() {
        let note = dir(&[short_tag(0x0231, &[324, 216, 72, 48])]);
        let f = read_one("RICOH IMAGING COMPANY, LTD.", Some(&note), &[]).unwrap();
        assert!(about(only(&f), (0.5, 0.5, 0.1, 0.1, State::Chosen)));
    }

    #[test]
    fn a_phone_says_where_with_subject_area() {
        let exif =
            dir(&[short_tag(0xA002, &[4032]), short_tag(0xA003, &[3024]), short_tag(0x9214, &[1008, 756, 403, 302])]);
        let f = read_one("Apple", None, &[&exif]).unwrap();
        let (cx, cy, w, _, state) = only(&f);
        assert!(close(cx, 0.25) && close(cy, 0.25) && close(w, 403.0 / 4032.0) && state == State::Chosen);
        // The whole picture says nothing.
        let all = dir(&[
            short_tag(0xA002, &[4032]),
            short_tag(0xA003, &[3024]),
            short_tag(0x9214, &[2016, 1512, 4032, 3024]),
        ]);
        assert!(read_one("Apple", None, &[&all]).is_none());
    }
}
