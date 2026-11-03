//! Where the camera focused: the autofocus points a photo's maker note
//! records, as fractions of the picture, ready to draw over it.
//!
//! [`read`] takes a file's metadata through the small [`Directory`] interface
//! (implement it over whatever TIFF / EXIF reader you use) and gives a
//! [`Focus`]: the camera's autofocus mode in plain words ([`MODES`]) and the
//! points it used. Canon, Nikon, Sony, Fujifilm, Olympus / OM System,
//! Panasonic / Leica and Pentax / Ricoh maker notes are read, and the EXIF
//! SubjectArea of any maker (phones). [`bracket`] says whether a Canon frame
//! belongs to a focus bracket.
//!
//! The formats are ExifTool's research, by Phil Harvey: see Credits in the README.
//!
//! Points are fractions of the picture (0..1 of its width and height, from its
//! top-left corner) so they fit any size of it: the file's own frame as
//! decoded, upright once [`Focus::upright`] has turned them as the picture
//! is turned, then turned as the user turned the photo ([`Focus::turned`]).

use serde::{Deserialize, Serialize};

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
/// An application may keep a mode by its position here: only ever add at the end.
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
/// used are kept, the zone it was given as one outline. Counted on the
/// camera's layout, not on the cells a photo gives: an R6 Mark III gives
/// only its zone's 81 cells.
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

    /// As the picture stands once turned upright by its EXIF orientation:
    /// quarter turns, then a mirror.
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

    /// What is worth keeping of a fine grid of cells (a mirrorless camera):
    /// the cells it used, and the zone it was given as one outline, unless
    /// that is the whole picture.
    fn tidy(mut self) -> Self {
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

/// A directory of tags in a TIFF-structured file, as the focus points are read
/// from it: the EXIF and main directories, or a maker's note.
pub trait Directory: Sized {
    /// The value of `tag` here, when it has one of the types in [`Value`].
    fn value(&self, tag: u16) -> Option<Value<'_>>;
    /// The directory nested under `tag` (Olympus keeps its camera settings
    /// in one: [`OLYMPUS_CAMERA_SETTINGS`]).
    fn sub(&self, tag: u16) -> Option<Self>;
    /// The directory's values are stored big-endian (a Nikon note's bytes).
    fn big_endian(&self) -> bool;
}

/// A tag's value, by its TIFF type.
pub enum Value<'a> {
    /// BYTE and UNDEFINED.
    Bytes(&'a [u8]),
    Short(&'a [u16]),
    SShort(&'a [i16]),
    Long(&'a [u32]),
    SLong(&'a [i32]),
    /// Numerator and denominator of each.
    Rational(Vec<(u32, u32)>),
    SRational(Vec<(i32, i32)>),
}

/// What a file's metadata holds, as its focus points are read from it.
pub struct Source<'a, D> {
    /// The maker and the model as the file names them.
    pub make: &'a str,
    pub model: &'a str,
    /// Its EXIF and main directories.
    pub ifds: &'a [D],
    /// Its maker's note, when it has one.
    pub note: Option<D>,
}

/// Where the camera focused, in the file's own frame: not yet turned
/// upright. Read from the maker's note of the makers whose notes place their
/// points (see each below), else from the standard EXIF SubjectArea (phones).
/// Makers that only name their points ("upper left") without placing them
/// (older Nikon, Pentax and Sony A-mount DSLRs), or that encipher them, say
/// nothing here. `picture`: the picture's size as the file stores it, when
/// it may have been edited since the camera made it (a JPEG): a picture of
/// another shape was cropped, and its points would land in the wrong place.
pub fn read<D: Directory>(src: &Source<D>, picture: Option<(u32, u32)>) -> Option<Focus> {
    let make = src.make.trim().to_ascii_lowercase();
    let from_note = src.note.as_ref().and_then(|note| {
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
    Some(focus).filter(|f| !f.is_empty())
}

/// A frame of a focus bracket: the camera took a series of frames, focus a
/// step further each time, to be merged into one picture sharp from front to
/// back. Read from the maker's note of the makers that say so (Canon so
/// far); which frames make one stack is for the application to work out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bracket {
    /// Frames the camera was set to take: at most this many (a Canon stops
    /// sooner once focus reaches infinity). 0: not said.
    pub shots: u16,
    /// The focus step between frames, in the camera's own units. 0: not said.
    pub step: u16,
    /// Seconds the camera waits between frames (for a flash to recharge).
    pub interval: u16,
    /// How far this frame is focused: the near and far ends of its range, in
    /// the camera's units, when it says. Focus only moves away during a
    /// bracket: a frame focused nearer than the one before starts another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<[u16; 2]>,
}

impl Bracket {
    /// The camera was set the same way for both frames.
    pub fn same_settings(&self, other: &Bracket) -> bool {
        (self.shots, self.step, self.interval) == (other.shots, other.step, other.interval)
    }

    /// Focused nearer than `before` was: focus went back, a new bracket.
    pub fn refocused_after(&self, before: &Bracket) -> bool {
        matches!((self.distance, before.distance), (Some([_, far]), Some([near, _])) if far < near)
    }
}

/// Whether the file is a frame of a focus bracket, when its maker's note says.
pub fn bracket<D: Directory>(src: &Source<D>) -> Option<Bracket> {
    let note = src.note.as_ref()?;
    if src.make.trim().to_ascii_lowercase().starts_with("canon") { canon_bracket(note) } else { None }
}

fn shorts<D: Directory>(ifd: &D, tag: u16) -> Option<Vec<u16>> {
    match ifd.value(tag)? {
        Value::Short(v) => Some(v.to_vec()),
        Value::SShort(v) => Some(v.iter().map(|&s| s as u16).collect()),
        Value::Long(v) => Some(v.iter().map(|&l| l.min(u16::MAX as u32) as u16).collect()),
        _ => None,
    }
}

fn first<D: Directory>(ifd: &D, tag: u16) -> Option<u16> {
    shorts(ifd, tag)?.first().copied()
}

/// 32-bit values (signed or not), and 16-bit ones, as they are.
fn longs<D: Directory>(ifd: &D, tag: u16) -> Option<Vec<i64>> {
    match ifd.value(tag)? {
        Value::Long(v) => Some(v.iter().map(|&l| l as i64).collect()),
        Value::SLong(v) => Some(v.iter().map(|&l| l as i64).collect()),
        Value::Short(v) => Some(v.iter().map(|&s| s as i64).collect()),
        Value::SShort(v) => Some(v.iter().map(|&s| s as i64).collect()),
        _ => None,
    }
}

fn rationals<D: Directory>(ifd: &D, tag: u16) -> Option<Vec<f64>> {
    let ratio = |n: f64, d: f64| if d == 0.0 { f64::NAN } else { n / d };
    match ifd.value(tag)? {
        Value::Rational(v) => Some(v.iter().map(|&(n, d)| ratio(n as f64, d as f64)).collect()),
        Value::SRational(v) => Some(v.iter().map(|&(n, d)| ratio(n as f64, d as f64)).collect()),
        _ => None,
    }
}

fn bytes<D: Directory>(ifd: &D, tag: u16) -> Option<&[u8]> {
    match ifd.value(tag)? {
        Value::Bytes(b) => Some(b),
        _ => None,
    }
}

/// The picture's size as its EXIF gives it (PixelXDimension, PixelYDimension).
fn exif_size<D: Directory>(ifds: &[D]) -> Option<(f32, f32)> {
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
/// Focus bracketing (cameras since about 2018): its size in bytes, then on
/// (1) or off, the number of shots, the focus step, exposure smoothing,
/// merged in camera, that merge cropped, and the flash interval (s). Every
/// frame of a bracket says the same: none says which frame it is.
const CANON_FOCUS_BRACKETING: u16 = 0x4053;
/// File info: the focus distance, far end at 20 and near end at 21 (cm;
/// 65535 at infinity), on the cameras that give it.
const CANON_FILE_INFO: u16 = 0x0093;

fn canon_bracket<D: Directory>(note: &D) -> Option<Bracket> {
    let v = longs(note, CANON_FOCUS_BRACKETING)?;
    let at = |i: usize| v.get(i).map_or(0, |&n| n.clamp(0, u16::MAX as i64) as u16);
    let distance = shorts(note, CANON_FILE_INFO)
        .and_then(|f| Some([*f.get(21)?, *f.get(20)?]))
        .filter(|&[near, far]| near > 0 && far >= near);
    (v.get(1) == Some(&1)).then(|| Bracket { shots: at(2), step: at(3), interval: at(7), distance })
}

fn canon<D: Directory>(note: &D, model: &str) -> Option<Found> {
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
    let mode = canon_mode(*v.get(1)?);
    // Focused by hand: a point still chosen on the camera (a 5D Mark II
    // keeps it) is not where it focused.
    if *v.get(1)? == 0 {
        return Some((Focus { mode, points: vec![] }, None));
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
    let focus = Focus { mode, points };
    Some((if n > LAYOUT_MAX { focus.tidy() } else { focus }, Some(frame)))
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
/// `canon_af_info`), in the file's own frame.
pub fn from_crw(af_info: &[u16]) -> Option<Focus> {
    let (focus, _) = canon_af_info(af_info, true)?;
    Some(focus).filter(|f| !f.is_empty())
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

fn nikon<D: Directory>(note: &D) -> Option<Found> {
    let b = bytes(note, NIKON_AF_INFO2)?;
    let big = note.big_endian();
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

fn sony<D: Directory>(note: &D) -> Option<Found> {
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

fn fujifilm<D: Directory>(note: &D, ifds: &[D]) -> Option<Found> {
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
pub const OLYMPUS_CAMERA_SETTINGS: u16 = 0x2020;
const OLYMPUS_FOCUS_MODE: u16 = 0x0301;
const OLYMPUS_AF_SEARCH: u16 = 0x0303;
const OLYMPUS_AF_POINT: u16 = 0x0305;
const OLYMPUS_AF_TARGET: u16 = 0x030a;

fn olympus<D: Directory>(note: &D, ifds: &[D]) -> Option<Found> {
    let cs = &note.sub(OLYMPUS_CAMERA_SETTINGS)?;
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

fn panasonic<D: Directory>(note: &D) -> Option<Found> {
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

fn pentax<D: Directory>(note: &D) -> Option<Found> {
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

fn subject_area<D: Directory>(ifds: &[D]) -> Option<Found> {
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
        let f = canon_af_info2(&v, true, true, false).unwrap().0;
        assert_eq!(f.mode, Some("Flexible zone"));
        assert_eq!(f.points.iter().map(|p| p.state).collect::<Vec<_>>(), [State::Area, State::InFocus, State::InFocus]);
        let area = f.points[0];
        assert!(close(area.w, (8.0 * 170.0 + 153.0) / 6960.0));
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
    fn a_zone_of_a_fine_grid_is_one_outline() {
        // An R6 Mark III in flexible zone AF gives only its zone's 81 cells
        // (9 x 9 of 153 px every 170 px, left of the middle), all chosen,
        // none in focus: one zone, not 81 points.
        let cells: Vec<_> =
            (0..81).map(|i| (153, 153, -2024 + 170 * (i % 9) as i16, 680 - 170 * (i / 9) as i16)).collect();
        let v = r6_mark_iii(19, &cells, &[], &(0..81).collect::<Vec<_>>());
        let f = canon_af_info2(&v, true, true, false).unwrap().0;
        assert_eq!(f.mode, Some("Flexible zone"));
        let side = 8.0 * 170.0 + 153.0;
        assert!(about(only(&f), (0.5 - 1344.0 / 6960.0, 0.5, side / 6960.0, side / 4640.0, State::Area)));
    }

    #[test]
    fn manual_focus_says_so_with_nothing_to_draw() {
        let mut v = vec![96, 0, 9, 0, 5616, 3744, 5616, 3744];
        v.extend([0; 36 + 3]);
        let f = canon_af_info2(&v, true, true, false).unwrap().0;
        assert_eq!(f, Focus { mode: Some("Manual focus"), points: vec![] });
        // A 5D Mark II on manual focus keeps its nine points valid and the
        // centre one chosen: not where it focused.
        let mut v = five_d_mark_ii();
        v[1] = 0;
        v[8 + 4 * 9] = 0;
        v[8 + 4 * 9 + 1] = 1 << 8;
        let f = canon_af_info2(&v, true, true, false).unwrap().0;
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

    /// The one point drawn: its centre, size and state.
    fn only(f: &Focus) -> (f32, f32, f32, f32, State) {
        assert_eq!(f.points.len(), 1, "{f:?}");
        let p = f.points[0];
        (p.x + p.w / 2.0, p.y + p.h / 2.0, p.w, p.h, p.state)
    }

    fn about(a: (f32, f32, f32, f32, State), b: (f32, f32, f32, f32, State)) -> bool {
        close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2) && close(a.3, b.3) && a.4 == b.4
    }

    /// A directory kept in memory: the tags as the file stores them.
    #[derive(Clone, Default)]
    struct Tags(Vec<(u16, Vec<u16>)>);

    impl Directory for Tags {
        fn value(&self, tag: u16) -> Option<Value<'_>> {
            self.0.iter().find(|t| t.0 == tag).map(|t| Value::Short(&t.1))
        }
        fn sub(&self, _: u16) -> Option<Self> {
            None
        }
        fn big_endian(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_directory_of_another_reader_is_read() {
        let note = Tags(vec![(0x2027, vec![6000, 4000, 4500, 1000]), (0x2037, vec![300, 200, 257])]);
        let src = Source { make: "SONY", model: "", ifds: &[], note: Some(note) };
        let f = read(&src, None).unwrap();
        assert!(about(only(&f), (0.75, 0.25, 0.05, 0.05, State::InFocus)));
        // Not a Canon: no bracket.
        assert!(bracket(&src).is_none());
    }
}
