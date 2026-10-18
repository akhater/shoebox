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
use rawler::formats::tiff::{IFD, Value};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};

/// Version of how focus points are read. Photos an older version read are
/// read again ([`find`]); raise it when Shoebox learns another maker's notes.
/// 1: Canon, Nikon, Sony, Fujifilm, Olympus / OM System, Panasonic / Leica,
/// Pentax / Ricoh, and any maker's EXIF SubjectArea (phones).
pub const FOCUS_READER: i64 = 1;

/// What a point was in this photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// One of the camera's points, not used for this photo.
    Other,
    /// Chosen (by the photographer, or the camera following a subject), the
    /// camera not saying focus was found there.
    Chosen,
    /// Focus was found there.
    InFocus,
    /// The zone the camera was given to look in: an outline, not a point.
    Area,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Point {
    /// Top-left corner and size, fractions of the picture.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Focus {
    /// How the camera was set to focus, in [`MODES`]' words.
    pub mode: Option<&'static str>,
    pub points: Vec<Point>,
}

/// The autofocus modes, in plain words, whatever each maker calls them.
/// Kept by position in the catalog: only ever add at the end.
pub const MODES: &[&str] = &[
    "Manual focus",
    "Single point",
    "Spot",
    "Expanded point",
    "Zone",
    "Large zone",
    "Flexible zone",
    "Whole area",
    "Face detection",
    "Face tracking",
    "Automatic",
    "Tracking",
    "Wide",
    "Dynamic area",
    "Pinpoint",
    "Multi-point",
    "Centre",
];

fn mode(name: &str) -> Option<&'static str> {
    MODES.iter().copied().find(|m| *m == name)
}

/// A layout with more points than this is a mirrorless camera's fine grid of
/// cells (Canon R: 1,053), not points to draw one by one: only the cells it
/// used are kept, the zone it was given as one outline.
const LAYOUT_MAX: usize = 100;

impl Point {
    /// A point given by its centre and size in a frame `fw` x `fh` whose
    /// origin is its centre (Y up when `up`).
    fn centred(cx: f32, cy: f32, w: f32, h: f32, (fw, fh): (f32, f32), up: bool, state: State) -> Option<Self> {
        if fw <= 0.0 || fh <= 0.0 || w <= 0.0 || h <= 0.0 {
            return None;
        }
        let (x, y) = (0.5 + cx / fw, if up { 0.5 - cy / fh } else { 0.5 + cy / fh });
        Self::boxed(x - w / fw / 2.0, y - h / fh / 2.0, w / fw, h / fh, state)
    }

    /// A point given by its top-left corner and size, fractions of the
    /// picture; None when it misses the picture.
    fn boxed(x: f32, y: f32, w: f32, h: f32, state: State) -> Option<Self> {
        let ok = [x, y, w, h].iter().all(|v| v.is_finite()) && w > 0.0 && h > 0.0;
        (ok && x < 1.0 && y < 1.0 && x + w > 0.0 && y + h > 0.0).then_some(Self { x, y, w, h, state })
    }

    /// The point once the picture is turned a quarter turn clockwise.
    fn quarter(self) -> Self {
        Self { x: 1.0 - (self.y + self.h), y: self.x, w: self.h, h: self.w, ..self }
    }

    /// The point once the picture is mirrored left to right.
    fn mirrored(self) -> Self {
        Self { x: 1.0 - (self.x + self.w), ..self }
    }

    fn area(&self) -> f32 {
        self.w * self.h
    }
}

impl Focus {
    /// Turned `quarters` quarter turns clockwise.
    pub fn turned(mut self, quarters: i64) -> Self {
        for _ in 0..quarters.rem_euclid(4) {
            self.points = self.points.into_iter().map(Point::quarter).collect();
        }
        self
    }

    /// As the picture stands once turned upright by its EXIF orientation, as
    /// [`crate::imaging::orient`] turns it: quarter turns, then a mirror.
    pub fn upright(self, orientation: u16) -> Self {
        let (quarters, mirror) = match orientation {
            2 => (0, true),
            3 => (2, false),
            4 => (2, true),
            5 => (1, true),
            6 => (1, false),
            7 => (3, true),
            8 => (3, false),
            _ => (0, false),
        };
        let mut f = self.turned(quarters);
        if mirror {
            f.points = f.points.into_iter().map(Point::mirrored).collect();
        }
        f
    }

    /// What is worth keeping: a fine grid of cells (a mirrorless camera)
    /// keeps the cells it used, and the zone it was given as one outline,
    /// unless that is the whole picture.
    fn tidy(mut self) -> Self {
        if self.points.len() <= LAYOUT_MAX {
            return self;
        }
        let chosen: Vec<Point> = self.points.iter().copied().filter(|p| p.state == State::Chosen).collect();
        self.points.retain(|p| p.state == State::InFocus);
        if chosen.len() == 1 {
            self.points.insert(0, chosen[0]);
        } else if let Some(zone) = outline(&chosen)
            && zone.area() < 0.9
        {
            self.points.insert(0, zone);
        }
        self
    }

    /// Nothing to draw and no mode to tell: not worth keeping.
    fn is_empty(&self) -> bool {
        self.mode.is_none() && self.points.is_empty()
    }
}

/// The box around `points`, as a zone.
fn outline(points: &[Point]) -> Option<Point> {
    let first = points.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x + first.w, first.y + first.h);
    for p in points {
        (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x + p.w), y1.max(p.y + p.h));
    }
    Point::boxed(x0, y0, x1 - x0, y1 - y0, State::Area)
}

/// What a file says about focus, and the size of the frame its points are
/// measured in (when it says): to tell a picture cropped since.
type Found = (Focus, Option<(f32, f32)>);

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

/// Where the camera focused, in the file's own frame: not yet turned
/// upright. Read from the maker's note of the makers whose notes place their
/// points (see each below), else from the standard EXIF SubjectArea (phones).
/// Makers that only name their points ("upper left") without placing them
/// (older Nikon, Pentax and Sony A-mount DSLRs), or that encipher them, say
/// nothing here. `picture`: the picture's size as the file stores it, when
/// it may have been edited since the camera made it (a JPEG): a picture of
/// another shape was cropped, and its points would land in the wrong place.
pub fn read(src: &Source, picture: Option<(u32, u32)>) -> Option<Focus> {
    let make = src.make.trim().to_ascii_lowercase();
    let from_note = src.note.and_then(|note| {
        if make.starts_with("canon") {
            canon(note, src.model)
        } else if make.starts_with("nikon") {
            nikon(note)
        } else if make.starts_with("sony") {
            sony(note)
        } else if make.starts_with("fujifilm") {
            fujifilm(note, src.ifds)
        } else if make.starts_with("olympus") || make.starts_with("om digital") {
            olympus(note, src.ifds)
        } else if make.starts_with("panasonic") || make.starts_with("leica") {
            panasonic(note)
        } else if make.starts_with("pentax") || make.starts_with("ricoh") {
            pentax(note)
        } else {
            None
        }
    });
    let (focus, frame) = from_note.or_else(|| subject_area(src.ifds))?;
    if let (Some((pw, ph)), Some((fw, fh))) = (picture, frame)
        && pw > 0
        && ph > 0
        && ((pw as f32 / ph as f32) / (fw / fh)).ln().abs() > 0.02
    {
        return None;
    }
    Some(focus.tidy()).filter(|f| !f.is_empty())
}

fn shorts(ifd: &IFD, tag: u16) -> Option<Vec<u16>> {
    match &ifd.get_entry(tag)?.value {
        Value::Short(v) => Some(v.clone()),
        Value::SShort(v) => Some(v.iter().map(|&s| s as u16).collect()),
        Value::Long(v) => Some(v.iter().map(|&l| l.min(u16::MAX as u32) as u16).collect()),
        _ => None,
    }
}

fn first(ifd: &IFD, tag: u16) -> Option<u16> {
    shorts(ifd, tag)?.first().copied()
}

fn rationals(ifd: &IFD, tag: u16) -> Option<Vec<f64>> {
    let ratio = |n: f64, d: f64| if d == 0.0 { f64::NAN } else { n / d };
    match &ifd.get_entry(tag)?.value {
        Value::Rational(v) => Some(v.iter().map(|r| ratio(r.n as f64, r.d as f64)).collect()),
        Value::SRational(v) => Some(v.iter().map(|r| ratio(r.n as f64, r.d as f64)).collect()),
        _ => None,
    }
}

fn bytes(ifd: &IFD, tag: u16) -> Option<&[u8]> {
    match &ifd.get_entry(tag)?.value {
        Value::Undefined(b) | Value::Byte(b) => Some(b),
        _ => None,
    }
}

/// The picture's size as its EXIF gives it (PixelXDimension, PixelYDimension).
fn exif_size(ifds: &[&IFD]) -> Option<(f32, f32)> {
    ifds.iter()
        .find_map(|i| Some((first(i, 0xA002)? as f32, first(i, 0xA003)? as f32)))
        .filter(|&(w, h)| w > 0.0 && h > 0.0)
}

/// Bit `i` of a mask kept as 16-bit words, lowest bit first.
fn bit(words: &[u16], i: usize) -> bool {
    words.get(i / 16).is_some_and(|w| w >> (i % 16) & 1 == 1)
}

impl Point {
    /// A point given by its centre and size in a frame `fw` x `fh` whose
    /// origin is its top-left corner (Y down).
    fn at(cx: f32, cy: f32, w: f32, h: f32, (fw, fh): (f32, f32), state: State) -> Option<Self> {
        if fw <= 0.0 || fh <= 0.0 || w <= 0.0 || h <= 0.0 {
            return None;
        }
        Self::boxed((cx - w / 2.0) / fw, (cy - h / 2.0) / fh, w / fw, h / fh, state)
    }

    /// A point the file gives without a size, `x`, `y` fractions of a frame
    /// of shape `aspect` (width / height): a small square around it.
    fn marker(x: f32, y: f32, aspect: f32, state: State) -> Option<Self> {
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) || aspect <= 0.0 {
            return None;
        }
        let side = 0.04;
        let (w, h) = if aspect >= 1.0 { (side / aspect, side) } else { (side, side * aspect) };
        Self::boxed(x - w / 2.0, y - h / 2.0, w, h, state)
    }
}

// ---- Canon ----------------------------------------------------------------

/// Canon's AF records: AFInfo2 (0x26; cameras since about 2004) and AFInfo3
/// (0x3c, the same layout), else the older AFInfo (0x12; also in CRWs).
const CANON_AF_INFO: u16 = 0x0012;
const CANON_AF_INFO2: u16 = 0x0026;
const CANON_AF_INFO3: u16 = 0x003c;
/// Shot info: index 26 is the kind of camera (250: a compact, its Y down).
const CANON_SHOT_INFO: u16 = 0x0004;

fn canon(note: &IFD, model: &str) -> Option<Found> {
    let compact = shorts(note, CANON_SHOT_INFO).and_then(|v| v.get(26).copied()) == Some(250);
    let eos = model.to_ascii_uppercase().contains("EOS");
    if let Some(v) = shorts(note, CANON_AF_INFO2).or_else(|| shorts(note, CANON_AF_INFO3)) {
        // The original 5D gives a wrong AF frame: its image's size is the right one.
        let use_image = model.trim() == "Canon EOS 5D";
        return canon_af_info2(&v, !compact, eos, use_image);
    }
    canon_af_info(&shorts(note, CANON_AF_INFO)?, !compact)
}

/// AFInfo2: size, area mode, points, valid points, the image's size, the AF
/// frame's size, then each point's width, height, X and Y (centres, from the
/// frame's centre; Y up but on compacts), the in-focus mask, and on EOS
/// cameras the chosen mask. The first `valid` points are the ones that count.
fn canon_af_info2(v: &[u16], up: bool, eos: bool, use_image: bool) -> Option<Found> {
    // Four zero bytes first: not filled in.
    if *v.first()? == 0 {
        return None;
    }
    let n = *v.get(2)? as usize;
    let valid = (*v.get(3)? as usize).min(n);
    let frame = match (v.get(6), v.get(7)) {
        (Some(&w), Some(&h)) if w > 0 && h > 0 && !use_image => (w as f32, h as f32),
        _ => (*v.get(4)? as f32, *v.get(5)? as f32),
    };
    let at = |k: usize, i: usize| v.get(8 + k * n + i).copied();
    let words = n.div_ceil(16);
    let focus = v.get(8 + 4 * n..8 + 4 * n + words).unwrap_or(&[]);
    let chosen = if eos { v.get(8 + 4 * n + words..8 + 4 * n + 2 * words).unwrap_or(&[]) } else { &[] };
    let points = (0..valid)
        .filter_map(|i| {
            let (w, h, x, y) = (at(0, i)?, at(1, i)?, at(2, i)? as i16, at(3, i)? as i16);
            let state = if bit(focus, i) {
                State::InFocus
            } else if bit(chosen, i) {
                State::Chosen
            } else {
                State::Other
            };
            Point::centred(x as f32, y as f32, w as f32, h as f32, frame, up, state)
        })
        .collect();
    Some((Focus { mode: canon_mode(*v.get(1)?), points }, Some(frame)))
}

/// The older AFInfo (10D, 20D, 30D, 1D Mark II, PowerShots...): points, valid
/// points, the image's size, the AF frame's size, one width and height for
/// every point, each point's X and Y, the in-focus mask.
fn canon_af_info(v: &[u16], up: bool) -> Option<Found> {
    let n = *v.first()? as usize;
    if ![1, 5, 7, 9, 15, 45, 53].contains(&n) {
        return None;
    }
    let valid = (*v.get(1)? as usize).min(n);
    let frame = match (v.get(4), v.get(5)) {
        (Some(&w), Some(&h)) if w > 0 && h > 0 => (w as f32, h as f32),
        _ => (*v.get(2)? as f32, *v.get(3)? as f32),
    };
    let (w, h) = (*v.get(6)? as f32, *v.get(7)? as f32);
    let focus = v.get(8 + 2 * n..8 + 2 * n + n.div_ceil(16)).unwrap_or(&[]);
    let points = (0..valid)
        .filter_map(|i| {
            let (x, y) = (*v.get(8 + i)? as i16, *v.get(8 + n + i)? as i16);
            let state = if bit(focus, i) { State::InFocus } else { State::Other };
            Point::centred(x as f32, y as f32, w, h, frame, up, state)
        })
        .collect();
    Some((Focus { mode: None, points }, Some(frame)))
}

/// Where a CRW's camera focused: its AFInfo, kept in its heap (as
/// [`canon_af_info`]), in the file's own frame.
pub fn from_crw(af_info: &[u16]) -> Option<Focus> {
    let (focus, _) = canon_af_info(af_info, true)?;
    Some(focus.tidy()).filter(|f| !f.is_empty())
}

fn canon_mode(code: u16) -> Option<&'static str> {
    mode(match code {
        0 => "Manual focus",
        1 | 8 | 10 => "Expanded point",
        2 | 13 => "Single point",
        4 => "Automatic",
        5 => "Face detection",
        6 => "Face tracking",
        7 => "Zone",
        9 => "Spot",
        11 | 12 => "Multi-point",
        14 | 16 | 17 => "Large zone",
        19..=21 => "Flexible zone",
        22 => "Whole area",
        _ => return None,
    })
}

// ---- Nikon ----------------------------------------------------------------

/// Nikon's AFInfo2 (0xb7, not enciphered): a version, then where its
/// coordinates are when it has them: the mirrorless Zs (versions 0300 and
/// 0400, when "coordinates available"), and the DSLRs in live view
/// (contrast detection, 0100 and 0101). Their AF area's centre and size in
/// an AF frame, from its top-left corner. Through the viewfinder the DSLRs
/// only name their points: nothing here.
const NIKON_AF_INFO2: u16 = 0x00b7;

fn nikon(note: &IFD) -> Option<Found> {
    let b = bytes(note, NIKON_AF_INFO2)?;
    let big = matches!(note.endian, rawler::bits::Endian::Big);
    let u = |i: usize| {
        b.get(i..i + 2).map(|x| if big { u16::from_be_bytes([x[0], x[1]]) } else { u16::from_le_bytes([x[0], x[1]]) })
    };
    // (AF frame, centre, size, in focus) at these offsets.
    let (frame, centre, size, in_focus) = match b.get(..4)? {
        b"0100" if *b.get(4)? == 1 => (0x10, 0x14, 0x18, Some(0x1c)),
        b"0101" if *b.get(4)? == 1 => (0x46, 0x4a, 0x4e, Some(0x52)),
        b"0300" | b"0301" if *b.get(7)? == 1 => (0x2a, 0x2e, 0x32, None),
        b"0400" | b"0401" | b"0402" if *b.get(7)? == 1 => (0x3e, 0x42, 0x46, Some(0x4a)),
        _ => return None,
    };
    let (fw, fh) = (u(frame)? as f32, u(frame + 2)? as f32);
    let (cx, cy) = (u(centre)? as f32, u(centre + 2)? as f32);
    let (w, h) = (u(size)? as f32, u(size + 2)? as f32);
    let state = match in_focus.and_then(|i| b.get(i)) {
        Some(&f) if f != 0 => State::InFocus,
        _ => State::Chosen,
    };
    let point = if w > 0.0 && h > 0.0 {
        Point::at(cx, cy, w, h, (fw, fh), state)
    } else if fw > 0.0 && fh > 0.0 {
        Point::marker(cx / fw, cy / fh, fw / fh, state)
    } else {
        None
    };
    Some((Focus { mode: nikon_mode(*b.get(5)?), points: point.into_iter().collect() }, Some((fw, fh))))
}

fn nikon_mode(code: u8) -> Option<&'static str> {
    mode(match code {
        128 | 193 => "Single point",
        129 | 197..=199 => "Automatic",
        130 | 203 | 207 => "Tracking",
        131 | 202 => "Face detection",
        192 => "Pinpoint",
        194 | 204..=206 => "Dynamic area",
        195 | 196 | 200 | 201 | 208 => "Wide",
        _ => return None,
    })
}

// ---- Sony -----------------------------------------------------------------

/// Sony: where the camera focused (FocusLocation: the picture's width and
/// height, then X, Y from its top-left corner; 0x204a a copy), and the frame
/// it showed there (FocusFrameSize: width, height, 257 when given). With no
/// focus location the camera writes the middle: that says nothing.
const SONY_FOCUS_LOCATION: u16 = 0x2027;
const SONY_FOCUS_LOCATION2: u16 = 0x204a;
const SONY_FOCUS_FRAME: u16 = 0x2037;

fn sony(note: &IFD) -> Option<Found> {
    let loc = shorts(note, SONY_FOCUS_LOCATION).or_else(|| shorts(note, SONY_FOCUS_LOCATION2))?;
    let &[w, h, x, y, ..] = loc.as_slice() else { return None };
    if w == 0 || h == 0 {
        return None;
    }
    let frame = shorts(note, SONY_FOCUS_FRAME).filter(|f| f.len() >= 3 && f[2] == 257 && f[0] > 0 && f[1] > 0);
    if frame.is_none() && x == w / 2 && y == h / 2 {
        return None;
    }
    let size = (w as f32, h as f32);
    let point = match frame {
        Some(f) => Point::at(x as f32, y as f32, f[0] as f32, f[1] as f32, size, State::InFocus),
        None => Point::marker(x as f32 / size.0, y as f32 / size.1, size.0 / size.1, State::InFocus),
    };
    Some((Focus { mode: None, points: point.into_iter().collect() }, Some(size)))
}

// ---- Fujifilm -------------------------------------------------------------

/// Fujifilm: the focus point (FocusPixel: X, Y in the camera's JPEG, from its
/// top-left corner), how it focused (FocusMode, 1 manual) and the AF mode.
const FUJI_FOCUS_MODE: u16 = 0x1021;
const FUJI_AF_MODE: u16 = 0x1022;
const FUJI_FOCUS_PIXEL: u16 = 0x1023;

fn fujifilm(note: &IFD, ifds: &[&IFD]) -> Option<Found> {
    if first(note, FUJI_FOCUS_MODE) == Some(1) {
        return Some((Focus { mode: mode("Manual focus"), points: vec![] }, None));
    }
    let m = match first(note, FUJI_AF_MODE) {
        Some(1) => mode("Single point"),
        Some(256) => mode("Zone"),
        Some(512) => mode("Wide"),
        _ => None,
    };
    let px = shorts(note, FUJI_FOCUS_PIXEL)?;
    let &[x, y, ..] = px.as_slice() else { return None };
    let size = exif_size(ifds)?;
    let point =
        (x > 0 || y > 0).then(|| Point::marker(x as f32 / size.0, y as f32 / size.1, size.0 / size.1, State::Chosen));
    Some((Focus { mode: m, points: point.flatten().into_iter().collect() }, Some(size)))
}

// ---- Olympus / OM System --------------------------------------------------

/// Olympus and OM System: in the camera settings (a directory of its own,
/// 0x2020), the AF area the camera focused in (AFTargetInfo, OM cameras: its
/// frame's size, then X, Y, width, height from the top-left corner), else
/// the point chosen (AFPointSelected: fractions of the picture), and whether
/// focus was found (AFSearch, 1: ready).
pub(crate) const OLYMPUS_CAMERA_SETTINGS: u16 = 0x2020;
const OLYMPUS_FOCUS_MODE: u16 = 0x0301;
const OLYMPUS_AF_SEARCH: u16 = 0x0303;
const OLYMPUS_AF_POINT: u16 = 0x0305;
const OLYMPUS_AF_TARGET: u16 = 0x030a;

fn olympus(note: &IFD, ifds: &[&IFD]) -> Option<Found> {
    let cs = note.get_sub_ifd(OLYMPUS_CAMERA_SETTINGS)?;
    let m = match first(cs, OLYMPUS_FOCUS_MODE) {
        Some(10) => return Some((Focus { mode: mode("Manual focus"), points: vec![] }, None)),
        Some(4) => mode("Face detection"),
        _ => None,
    };
    let state = if first(cs, OLYMPUS_AF_SEARCH) == Some(1) { State::InFocus } else { State::Chosen };
    if let Some(t) = shorts(cs, OLYMPUS_AF_TARGET)
        && let &[fw, fh, x, y, w, h, ..] = t.as_slice()
        && fw > 0
        && fh > 0
        && w > 0
        && h > 0
    {
        let (fw, fh) = (fw as f32, fh as f32);
        let point = Point::boxed(x as f32 / fw, y as f32 / fh, w as f32 / fw, h as f32 / fh, state);
        return Some((Focus { mode: m, points: point.into_iter().collect() }, Some((fw, fh))));
    }
    let r = rationals(cs, OLYMPUS_AF_POINT)?;
    let (x, y) = (*r.get(1)? as f32, *r.get(2)? as f32);
    if x == 0.0 && y == 0.0 {
        return None;
    }
    // Olympus sensors are 4:3.
    let aspect = exif_size(ifds).map_or(4.0 / 3.0, |(w, h)| w / h);
    Some((Focus { mode: m, points: Point::marker(x, y, aspect, state).into_iter().collect() }, None))
}

// ---- Panasonic (and Leica's Panasonic-made cameras) -----------------------

/// Panasonic: the middle of the AF area used (AFPointPosition: fractions of
/// the picture; above 1 when there is none), its size (AFAreaSize, fractions)
/// and the AF area mode (two bytes).
const PANASONIC_FOCUS_MODE: u16 = 0x0007;
const PANASONIC_AF_AREA_MODE: u16 = 0x000f;
const PANASONIC_AF_POINT: u16 = 0x004d;
const PANASONIC_AF_AREA_SIZE: u16 = 0x00de;

fn panasonic(note: &IFD) -> Option<Found> {
    if first(note, PANASONIC_FOCUS_MODE) == Some(2) {
        return Some((Focus { mode: mode("Manual focus"), points: vec![] }, None));
    }
    let m = match bytes(note, PANASONIC_AF_AREA_MODE).and_then(|b| b.get(..2)) {
        Some(&[0, n]) if n > 0 => mode("Multi-point"),
        Some(&[1, 0]) => mode("Spot"),
        Some(&[16, _]) => mode("Single point"),
        Some(&[32, 0] | &[240, 0]) => mode("Tracking"),
        Some(&[64, _]) => mode("Face detection"),
        Some(&[128, 0]) => mode("Pinpoint"),
        _ => None,
    };
    let p = rationals(note, PANASONIC_AF_POINT)?;
    let (x, y) = (*p.first()? as f32, *p.get(1)? as f32);
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Some((Focus { mode: m, points: vec![] }, None));
    }
    let size = rationals(note, PANASONIC_AF_AREA_SIZE)
        .and_then(|s| Some((*s.first()? as f32, *s.get(1)? as f32)))
        .filter(|&(w, h)| w > 0.0 && w <= 1.0 && h > 0.0 && h <= 1.0);
    let point = match size {
        Some((w, h)) => Point::boxed(x - w / 2.0, y - h / 2.0, w, h, State::Chosen),
        None => Point::marker(x, y, 4.0 / 3.0, State::Chosen),
    };
    Some((Focus { mode: m, points: point.into_iter().collect() }, None))
}

// ---- Pentax / Ricoh -------------------------------------------------------

/// Pentax: in live view, the contrast-detection AF area (left, top, width,
/// height in a 720 x 480 frame). Through the viewfinder the DSLRs only name
/// their points: nothing here.
const PENTAX_CONTRAST_AF_AREA: u16 = 0x0231;

fn pentax(note: &IFD) -> Option<Found> {
    let a = shorts(note, PENTAX_CONTRAST_AF_AREA)?;
    let &[x, y, w, h, ..] = a.as_slice() else { return None };
    if w == 0 || h == 0 {
        return None;
    }
    let (fw, fh) = (720.0, 480.0);
    let point = Point::boxed(x as f32 / fw, y as f32 / fh, w as f32 / fw, h as f32 / fh, State::Chosen);
    Some((Focus { mode: None, points: point.into_iter().collect() }, Some((fw, fh))))
}

// ---- Any maker: EXIF SubjectArea -------------------------------------------

/// The standard EXIF SubjectArea (phones write where they focused there): its
/// centre in the picture, then a diameter, or a width and height. A centre
/// alone at the very middle says nothing; nor does an area as big as the picture.
const SUBJECT_AREA: u16 = 0x9214;

fn subject_area(ifds: &[&IFD]) -> Option<Found> {
    let a = ifds.iter().find_map(|i| shorts(i, SUBJECT_AREA))?;
    let size = exif_size(ifds)?;
    let (x, y) = (*a.first()? as f32, *a.get(1)? as f32);
    let point = match a.len() {
        2 if (x - size.0 / 2.0).abs() >= 1.0 || (y - size.1 / 2.0).abs() >= 1.0 => {
            Point::marker(x / size.0, y / size.1, size.0 / size.1, State::Chosen)
        }
        3 => Point::at(x, y, a[2] as f32, a[2] as f32, size, State::Chosen),
        4.. => Point::at(x, y, a[2] as f32, a[3] as f32, size, State::Chosen),
        _ => None,
    }
    .filter(|p| p.area() < 0.9)?;
    Some((Focus { mode: None, points: vec![point] }, Some(size)))
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
        let layout: Option<Vec<u8>> =
            db.query_row("SELECT points FROM focus_grids WHERE id = ?1", [grid], |r| r.get(0)).optional()?;
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
}

/// Photos an older Shoebox read, before it knew focus points (or another
/// maker's): their originals on the disks plugged in are read again,
/// metadata only, never the picture; the others wait for their disk.
pub fn find(cat: &Catalog, cancel: &AtomicBool) -> Result<FocusReport> {
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
            read.push((id, crate::imaging::read_focus(&path, kind)));
        }
        let db = cat.db();
        let tx = db.unchecked_transaction()?;
        for (id, focus) in &read {
            let blob = stored(&tx, focus.as_ref())?;
            tx.execute("UPDATE photos SET focus = ?1, focus_read = ?2 WHERE id = ?3", params![blob, FOCUS_READER, id])?;
        }
        tx.commit()?;
        report.read += read.len();
        report.found += read.iter().filter(|(_, f)| f.is_some()).count();
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

    #[test]
    fn canon_points_from_the_centre_y_up() {
        let f = canon_af_info2(&five_d_mark_ii(), true, true, false).unwrap().0;
        assert_eq!(f.mode, Some("Single point"));
        assert_eq!(f.points.len(), 9);
        // The top point (Y +501) is above the middle; the right one is the one used.
        let top = f.points[2];
        assert!(close(top.y + top.h / 2.0, 0.5 - 501.0 / 3744.0));
        let right = f.points[4];
        assert_eq!(right.state, State::InFocus);
        assert!(close(right.x + right.w / 2.0, 0.5 + 1173.0 / 5616.0));
        assert!(f.points.iter().enumerate().all(|(i, p)| i == 4 || p.state == State::Other));
    }

    #[test]
    fn a_fine_grid_keeps_its_zone_and_the_cells_in_focus() {
        // 1,053 cells of 153 px; 81 of them the zone, two in focus.
        let n = 1053usize;
        let mut v = vec![8838, 19, n as u16, n as u16, 6960, 4640, 6960, 4640];
        let (mut w, mut h, mut x, mut y) = (vec![], vec![], vec![], vec![]);
        for i in 0..n {
            let (c, r) = ((i % 39) as i16, (i / 39) as i16);
            w.push(153);
            h.push(153);
            x.push(((c - 19) * 170) as u16);
            y.push(((13 - r) * 170) as u16);
        }
        v.extend(w);
        v.extend(h);
        v.extend(x);
        v.extend(y);
        let words = n.div_ceil(16);
        let mut focus = vec![0u16; words];
        let mut chosen = vec![0u16; words];
        let zone: Vec<usize> = (0..9).flat_map(|r| (0..9).map(move |c| (r + 9) * 39 + c + 15)).collect();
        for &i in &zone {
            chosen[i / 16] |= 1 << (i % 16);
        }
        for &i in &zone[40..42] {
            focus[i / 16] |= 1 << (i % 16);
        }
        v.extend(focus);
        v.extend(chosen);
        let f = canon_af_info2(&v, true, true, false).unwrap().0.tidy();
        assert_eq!(f.mode, Some("Flexible zone"));
        assert_eq!(f.points.iter().map(|p| p.state).collect::<Vec<_>>(), [State::Area, State::InFocus, State::InFocus]);
        let area = f.points[0];
        assert!(close(area.w, (8.0 * 170.0 + 153.0) / 6960.0));
    }

    #[test]
    fn manual_focus_says_so_with_nothing_to_draw() {
        let mut v = vec![96, 0, 9, 0, 5616, 3744, 5616, 3744];
        v.extend([0; 36 + 3]);
        let f = canon_af_info2(&v, true, true, false).unwrap().0.tidy();
        assert_eq!(f, Focus { mode: Some("Manual focus"), points: vec![] });
    }

    #[test]
    fn turned_upright_like_the_picture() {
        let p = Point { x: 0.1, y: 0.2, w: 0.3, h: 0.1, state: State::InFocus };
        let f = |o| Focus { mode: None, points: vec![p] }.upright(o).points[0];
        // A quarter turn clockwise (6): the top-left corner goes to the top right.
        let q = f(6);
        assert!(close(q.x, 1.0 - 0.3) && close(q.y, 0.1) && close(q.w, 0.1) && close(q.h, 0.3));
        // Four quarter turns: back where it was.
        let back = Focus { mode: None, points: vec![p] }.turned(4).points[0];
        assert_eq!(back, p);
        // 3 = upside down; 2 = mirrored.
        let u = f(3);
        assert!(close(u.x, 0.6) && close(u.y, 0.7));
        let m = f(2);
        assert!(close(m.x, 0.6) && close(m.y, 0.2));
        // 8 = a quarter turn counter-clockwise, the opposite of 6.
        let l = Focus { mode: None, points: vec![f(6)] }.upright(8).points[0];
        assert!(close(l.x, p.x) && close(l.y, p.y));
    }

    #[test]
    fn kept_small_and_read_back() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE focus_grids (id INTEGER PRIMARY KEY, points BLOB NOT NULL UNIQUE);").unwrap();
        let f = canon_af_info2(&five_d_mark_ii(), true, true, false).unwrap().0;
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
