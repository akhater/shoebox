//! The culling assistant: what is wrong with a photo (blurry, a face out of
//! focus, too dark, blown out, nothing in it), which frames are the same shot
//! (a burst), and which one of them to keep.
//!
//! It only suggests, keep or reject. A reject is a photo marked for deletion,
//! as Cull does by hand: nothing is deleted until the user confirms. Marks
//! are the user's decisions; suggestions are worked out again whenever they
//! are asked for.
//!
//! Every photo is measured from Shoebox's own preview, in the background, so
//! photos on unplugged disks are measured too. A preview is small (~1080 px):
//! it shows a blurry photo, not one that is only soft at 100%. So when a
//! shoot's disk is plugged in, Cull can look closer: its originals are read at
//! half size (read-only, as always) and measured again. Measures live in
//! `quality.db`, a file of their own like word search's: they can always be
//! measured again, so the catalog's backup doesn't need them.

use crate::catalog::{Catalog, Filters, Scope, Sort};
use crate::db::PREVIEW_OK;
use anyhow::{Result, bail};
use image::RgbImage;
use rayon::prelude::*;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// Version of how photos are measured: photos measured by an older one are
/// measured again.
pub const QUALITY_VERSION: i64 = 1;

/// Cull works on a shoot, not a whole archive: views bigger than this ask
/// for a folder or a day instead.
pub const MAX_SHOOT: usize = 5_000;

// What counts as a problem. Seen small (on screen), edge blur is in
// thousandths of the photo's long edge, so a preview and an original
// compare: 1 = 6 px on a 6000 px photo. Seen at 100%, it is in pixels of the
// full-size photo. First values, from a portrait shoot whose frames were
// all usable: sharpest parts up to 0.7 on previews and 2.3 px on originals;
// faces (whose features are soft-edged by nature) 0.9-1.8 and 1.9-4.7 px.
/// Blurry even small, on screen: shake, motion, focus missed by far.
const BLURRY: f32 = 1.2;
/// Soft at 100%, px (only an original shows it).
const SOFT_PX: f32 = 2.5;
/// A face this blurred, while something else in the photo is much sharper,
/// is out of focus: the focus landed elsewhere. Small (thousandths), and at 100% (px).
const FACE_SOFT: f32 = 2.4;
const FACE_SOFT_PX: f32 = 6.0;
const FACE_VS_SHARPEST: f32 = 2.0;
/// Originals up to this long are read whole for a closer look; longer ones at half size.
const CLOSE_WHOLE: u32 = 3200;
/// A face smaller than this share of the long edge isn't the subject.
const SUBJECT_FACE: f32 = 0.06;

/// Two frames are the same shot when CLIP finds them this alike, their
/// layouts this alike (and alike to the first frame of the stack), taken this
/// close together by the same camera.
const SAME_SHOT: f32 = 0.92;
const SAME_FRAMING: f32 = 0.85;
const SAME_FRAMING_AS_FIRST: f32 = 0.8;
const SAME_SHOT_GAP: i64 = 30;
/// Without CLIP descriptions: frames of one burst, within a second.
const BURST_GAP: i64 = 1;

/// What one look at a photo found. Blur is a Gaussian sigma, in pixels of
/// the picture measured (`long` px on its long edge).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Measures {
    /// Long edge of the picture measured, px.
    pub long: u32,
    /// Long edge of the photo at full size, px, when known.
    pub full: Option<u32>,
    /// Edge blur where the photo is sharpest. None: no edges to measure.
    pub sharpest: Option<f32>,
    /// Edge blur over most of the frame (the median of its parts with edges).
    pub typical: Option<f32>,
    /// Edge blur around the eyes of the largest face, when it's big enough to
    /// be the subject.
    pub face: Option<f32>,
    /// That face's width, px.
    pub face_w: Option<f32>,
    /// Mean brightness, 0..1.
    pub mean: f32,
    /// Spread of brightness (standard deviation), 0..1.
    pub spread: f32,
    /// Share of the frame blown out to white.
    pub blown: f32,
    /// Share of the frame crushed to black.
    pub black: f32,
    /// Share of the frame's parts with edges in them.
    pub detail: f32,
    /// How the frame is laid out, to tell frames of one shot from the next pose.
    pub layout: Option<Layout>,
}

/// A picture's brightness on an 8 x 8 grid: frames of the same shot (the same
/// framing, a burst) have nearly the same; another pose, a step aside or the
/// camera turned don't. Kept as 128 hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout(pub [u8; 64]);

impl Layout {
    fn of(p: &Plane) -> Self {
        let mut out = [0u8; 64];
        for (i, cell) in out.iter_mut().enumerate() {
            let (gx, gy) = (i % 8, i / 8);
            let (x0, x1) = (gx * p.w / 8, ((gx + 1) * p.w / 8).max(gx * p.w / 8 + 1));
            let (y0, y1) = (gy * p.h / 8, ((gy + 1) * p.h / 8).max(gy * p.h / 8 + 1));
            let mut sum = 0.0;
            for y in y0..y1.min(p.h) {
                sum += p.v[y * p.w + x0..y * p.w + x1.min(p.w)].iter().sum::<f32>();
            }
            *cell = (sum / ((y1 - y0) * (x1 - x0)) as f32).round().clamp(0.0, 255.0) as u8;
        }
        Self(out)
    }

    /// How alike two layouts are, -1..1: the correlation of their grids, so a
    /// frame a little brighter or darker is still the same shot. Flat ones are
    /// alike only to flat ones.
    pub fn alike(&self, other: &Layout) -> f32 {
        let centred = |l: &Layout| {
            let mean = l.0.iter().map(|&v| v as f32).sum::<f32>() / 64.0;
            l.0.map(|v| v as f32 - mean)
        };
        let (a, b) = (centred(self), centred(other));
        let (na, nb) = (dot(&a, &a).sqrt(), dot(&b, &b).sqrt());
        if na < 1.0 || nb < 1.0 {
            return if na < 1.0 && nb < 1.0 { 1.0 } else { 0.0 };
        }
        dot(&a, &b) / (na * nb)
    }
}

impl Serialize for Layout {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.iter().map(|b| format!("{b:02x}")).collect::<String>())
    }
}

impl<'de> Deserialize<'de> for Layout {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let s = String::deserialize(d)?;
        let mut out = [0u8; 64];
        if s.len() != 128 {
            return Err(D::Error::custom("a layout is 128 hex digits"));
        }
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(D::Error::custom)?;
        }
        Ok(Self(out))
    }
}

impl Measures {
    /// Blur as thousandths of the long edge.
    fn rel(&self, blur: Option<f32>) -> Option<f32> {
        blur.map(|b| b / self.long.max(1) as f32 * 1000.0)
    }

    /// Blur in pixels of the full-size photo (twice the picture measured when
    /// its full size isn't known: Cull reads originals at half size).
    fn full_px(&self, blur: Option<f32>) -> Option<f32> {
        let long = self.long.max(1) as f32;
        blur.map(|b| b * self.full.map_or(2.0 * long, |f| f as f32) / long)
    }
}

/// A photo's measures: from its preview, and from its original when Cull looked closer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quality {
    pub preview: Measures,
    pub close: Option<Measures>,
}

impl Quality {
    /// The closest look there is.
    pub fn best(&self) -> &Measures {
        self.close.as_ref().unwrap_or(&self.preview)
    }
}

/// What can be wrong with a photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Problem {
    /// Lens cap, pocket, the ground: nothing in it.
    Empty,
    Dark,
    /// Blurry everywhere: shake, motion, or focus missed by far.
    Blurry,
    /// Nothing quite sharp at 100% (seen on the original only).
    Soft,
    /// The face is out of focus while something else is sharp.
    FaceSoft,
    Blown,
    Bright,
}

impl Problem {
    /// Enough to suggest rejecting the photo. The others are shown, and left to the user.
    pub fn major(self) -> bool {
        !matches!(self, Problem::Blown | Problem::Bright)
    }

    /// Its bit in [`flags`]: the grid's badge (the window knows the same bits).
    pub fn bit(self) -> u32 {
        match self {
            Problem::Empty => 1,
            Problem::Dark => 2,
            Problem::Blurry => 4,
            Problem::Soft => 8,
            Problem::FaceSoft => 16,
            Problem::Blown => 32,
            Problem::Bright => 64,
        }
    }
}

/// How a photo was taken, for the words that go with a problem.
#[derive(Clone, Debug, Default)]
pub struct Shot {
    /// "1/15" or "0.5s", as the catalog keeps it.
    pub exposure: Option<String>,
    pub focal: Option<f64>,
}

/// A problem and how to say it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub problem: Problem,
    pub text: String,
}

/// What's wrong with a photo measured as `q`.
pub fn problems(q: &Quality, shot: &Shot) -> Vec<Found> {
    let mut out = Vec::new();
    let mut add = |problem, text: String| out.push(Found { problem, text });
    let p = &q.preview;
    let best = q.best();
    let close = q.close.is_some();
    let empty = p.spread < 0.035 || (p.detail < 0.05 && p.mean < 0.12);
    if empty {
        add(Problem::Empty, "Nothing in it: lens cap, pocket or the ground?".into());
        return out;
    }
    if p.mean < 0.08 && p.black > 0.35 {
        add(Problem::Dark, "Very dark".into());
    }
    if p.blown > 0.25 {
        add(Problem::Blown, format!("Highlights blown: {:.0}% of the frame is white", p.blown * 100.0));
    } else if p.mean > 0.88 {
        add(Problem::Bright, "Very bright".into());
    }
    let sharpest = best.rel(best.sharpest);
    // No edge anywhere in a frame with something in it: blurred beyond measure.
    let blurry = sharpest.map_or(p.detail < 0.15, |s| s >= BLURRY);
    if blurry {
        let mut text = "Blurry everywhere: shake, motion or focus missed".to_string();
        if let Some(slow) = slow_shutter(shot) {
            text = format!("Blurry everywhere: {slow}");
        }
        add(Problem::Blurry, text);
        return out;
    }
    if close && best.full_px(best.sharpest).is_some_and(|s| s >= SOFT_PX) {
        add(Problem::Soft, "Soft at 100%: nothing is quite sharp".into());
    }
    if let (Some(face), Some(face_px)) = (best.rel(best.face), best.full_px(best.face)) {
        let soft = if close { face_px >= FACE_SOFT_PX } else { face >= FACE_SOFT };
        if soft && face >= FACE_VS_SHARPEST * sharpest.unwrap_or(0.0) {
            add(Problem::FaceSoft, "Face out of focus: the focus landed elsewhere".into());
        }
    }
    out
}

/// "1/15 s at 200 mm is slow for a steady hand", when it is: slower than 1/focal.
fn slow_shutter(shot: &Shot) -> Option<String> {
    let e = shot.exposure.as_deref()?;
    let secs = match e.strip_prefix("1/") {
        Some(d) => 1.0 / d.parse::<f64>().ok()?,
        None => e.trim_end_matches('s').parse::<f64>().ok()?,
    };
    let focal = shot.focal.filter(|f| *f > 0.0)?;
    let shown = if e.ends_with('s') { e.trim_end_matches('s').to_string() } else { e.to_string() };
    (secs > 1.0 / focal && secs > 1.0 / 500.0).then(|| format!("{shown} s at {focal:.0} mm is slow for a steady hand"))
}

/// The grid's badge for a photo measured as `q`: the bits of its problems.
pub fn flags_of(q: &Quality) -> u32 {
    problems(q, &Shot::default()).iter().fold(0, |f, p| f | p.problem.bit())
}

// ---------------------------------------------------------------------------
// Measuring a picture.

/// Brightness of a picture, 0..255, one value per pixel.
struct Plane {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Plane {
    fn luma(img: &RgbImage) -> Self {
        let v = img.pixels().map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).collect();
        Self { w: img.width() as usize, h: img.height() as usize, v }
    }

    #[inline]
    fn at(&self, x: usize, y: usize) -> f32 {
        self.v[y * self.w + x]
    }

    /// Blurred with a Gaussian of `sigma` px (edges repeat the last pixel).
    fn blurred(&self, sigma: f32) -> Self {
        let r = (sigma * 3.0).ceil() as isize;
        let k: Vec<f32> = (-r..=r).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
        let sum: f32 = k.iter().sum();
        let k: Vec<f32> = k.iter().map(|x| x / sum).collect();
        let (w, h) = (self.w as isize, self.h as isize);
        let mut across = vec![0f32; self.v.len()];
        for y in 0..h {
            let row = &self.v[(y * w) as usize..((y + 1) * w) as usize];
            for x in 0..w {
                let mut a = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    a += kv * row[(x + i as isize - r).clamp(0, w - 1) as usize];
                }
                across[(y * w + x) as usize] = a;
            }
        }
        let mut out = vec![0f32; self.v.len()];
        for y in 0..h {
            for x in 0..w {
                let mut a = 0.0;
                for (i, kv) in k.iter().enumerate() {
                    a += kv * across[((y + i as isize - r).clamp(0, h - 1) * w + x) as usize];
                }
                out[(y * w + x) as usize] = a;
            }
        }
        Self { w: self.w, h: self.h, v: out }
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

/// How much the second look blurs: the coarse picture is the fine one blurred by this.
const COARSE: f32 = 1.5;
/// Slope (levels per px, on the coarse picture) an edge needs to be measured:
/// below it, noise and texture would count as edges.
const EDGE_SLOPE: f32 = 3.0;
const FACE_SLOPE: f32 = 2.0;
/// Edges a part needs before its blur is believed.
const PART_EDGES: usize = 10;
const FACE_EDGES: usize = 6;

/// How blurred the edges in `r` are: the Gaussian sigma (px) that would make
/// perfectly sharp edges look like these. Measured by blurring once more (as
/// Crété-Roffet et al., "The blur effect", 2007): a sharp edge loses much of
/// its slope to a further blur, an already blurred one little. Only the
/// shape of an edge counts, not its contrast. None: too few edges in `r`.
fn blur_in(fine: &Plane, coarse: &Plane, r: Rect, min_slope: f32, min_edges: usize) -> Option<f32> {
    slope_ratio(fine, coarse, r, min_slope, min_edges).map(sigma_of)
}

/// How many times steeper the middles of the edges in `r` are in `fine` than
/// in `coarse` (the median). Along x and along y apart: shake and motion
/// blur one direction more, and the blurrier one counts.
fn slope_ratio(fine: &Plane, coarse: &Plane, r: Rect, min_slope: f32, min_edges: usize) -> Option<f32> {
    let (x0, y0) = (r.x0.max(2), r.y0.max(2));
    let (x1, y1) = (r.x1.min(fine.w.saturating_sub(2)), r.y1.min(fine.h.saturating_sub(2)));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let along = |dx: usize, dy: usize| -> Option<f32> {
        // Slope at (x, y) along (dx, dy), and across it.
        let slope = |p: &Plane, x: usize, y: usize| (p.at(x + dx, y + dy) - p.at(x - dx, y - dy)) * 0.5;
        let across = |p: &Plane, x: usize, y: usize| (p.at(x + dy, y + dx) - p.at(x - dy, y - dx)) * 0.5;
        let mut ratios = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let c = slope(coarse, x, y);
                if c.abs() < min_slope || across(coarse, x, y).abs() * 2.0 > c.abs() {
                    continue; // too faint, or an edge mostly across this direction
                }
                let (before, after) = (slope(coarse, x - dx, y - dy), slope(coarse, x + dx, y + dy));
                if c.abs() < before.abs() || c.abs() <= after.abs() {
                    continue; // not the middle of the edge
                }
                let f = slope(fine, x, y) * c.signum();
                if f > 0.0 {
                    ratios.push(f / c.abs());
                }
            }
        }
        if ratios.len() < min_edges {
            return None;
        }
        Some(median(&mut ratios))
    };
    match (along(1, 0), along(0, 1)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The blur that makes a further blur of [`COARSE`] divide edges' slope by `ratio`.
fn seen_sigma(ratio: f32) -> f32 {
    if ratio > 1.0005 { COARSE / (ratio * ratio - 1.0).sqrt() } else { 40.0 }
}

/// [`seen_sigma`], less what any picture shows: its sharpest possible edge
/// (from one pixel to the next) measures as a little blur.
fn sigma_of(ratio: f32) -> f32 {
    let (seen, floor) = (seen_sigma(ratio), floor());
    (seen * seen - floor * floor).max(0.0).sqrt()
}

/// What a perfectly sharp edge measures as.
fn floor() -> f32 {
    static FLOOR: OnceLock<f32> = OnceLock::new();
    *FLOOR.get_or_init(|| {
        let stripes = RgbImage::from_fn(64, 64, |x, _| if x % 16 < 8 { [40u8; 3].into() } else { [200u8; 3].into() });
        let fine = Plane::luma(&stripes);
        let coarse = fine.blurred(COARSE);
        let all = Rect { x0: 0, y0: 0, x1: 64, y1: 64 };
        slope_ratio(&fine, &coarse, all, EDGE_SLOPE, 1).map_or(0.0, seen_sigma)
    })
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// A face in a photo, as the faces table keeps it: 0..1 of the picture.
#[derive(Clone, Debug, Default)]
pub struct FaceBox {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    /// Left eye, right eye, nose, mouth corners (x, y), when known.
    pub kps: Option<[f32; 10]>,
}

impl FaceBox {
    pub fn parse_kps(s: &str) -> Option<[f32; 10]> {
        let v: Vec<f32> = s.split(',').map(|t| t.trim().parse().ok()).collect::<Option<_>>()?;
        v.try_into().ok()
    }
}

/// Measures a picture (upright, as shown), with the faces found in it.
pub fn measure(img: &RgbImage, faces: &[FaceBox]) -> Measures {
    let fine = Plane::luma(img);
    let (w, h) = (fine.w, fine.h);
    let long = w.max(h);
    let mut m = Measures { long: long as u32, ..Default::default() };
    if w < 16 || h < 16 {
        return m;
    }

    let n = fine.v.len() as f32;
    let mean = fine.v.iter().sum::<f32>() / n;
    m.mean = mean / 255.0;
    m.spread = (fine.v.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n).sqrt() / 255.0;
    m.blown = img.pixels().filter(|p| p[0].max(p[1]).max(p[2]) >= 250).count() as f32 / n;
    m.black = fine.v.iter().filter(|v| **v <= 4.0).count() as f32 / n;

    let coarse = fine.blurred(COARSE);
    // About ten parts along the long edge.
    let part = (long / 10).max(24);
    let mut parts = Vec::new();
    let mut total = 0;
    for y in (0..h).step_by(part) {
        for x in (0..w).step_by(part) {
            total += 1;
            let r = Rect { x0: x, y0: y, x1: (x + part).min(w), y1: (y + part).min(h) };
            if let Some(b) = blur_in(&fine, &coarse, r, EDGE_SLOPE, PART_EDGES) {
                parts.push(b);
            }
        }
    }
    m.detail = parts.len() as f32 / total.max(1) as f32;
    m.layout = Some(Layout::of(&fine));
    if !parts.is_empty() {
        parts.sort_by(|a, b| a.total_cmp(b));
        // The second sharpest: one odd part (a reflection, noise) doesn't decide.
        m.sharpest = Some(parts[if parts.len() >= 3 { 1 } else { 0 }]);
        m.typical = Some(parts[parts.len() / 2]);
    }

    // The largest face, if it's big enough to be the subject: around its eyes.
    let (fw, fh) = (w as f32, h as f32);
    if let Some(face) = faces
        .iter()
        .filter(|f| (f.x2 - f.x1) * fw >= SUBJECT_FACE * long as f32)
        .max_by(|a, b| (a.x2 - a.x1).total_cmp(&(b.x2 - b.x1)))
    {
        let (x1, x2, y1, y2) = (face.x1 * fw, face.x2 * fw, face.y1 * fh, face.y2 * fh);
        let fhgt = y2 - y1;
        let eyes_y = face.kps.map(|k| (k[1] + k[3]) * 0.5 * fh).unwrap_or(y1 + 0.4 * fhgt);
        let inset = (x2 - x1) * 0.1;
        let r = Rect {
            x0: (x1 + inset).max(0.0) as usize,
            x1: (x2 - inset).max(0.0) as usize,
            y0: (eyes_y - 0.3 * fhgt).max(0.0) as usize,
            y1: (eyes_y + 0.2 * fhgt).max(0.0) as usize,
        };
        m.face = blur_in(&fine, &coarse, r, FACE_SLOPE, FACE_EDGES);
        m.face_w = Some(x2 - x1);
    }
    m
}

// ---------------------------------------------------------------------------
// The measures of every photo.

/// Every photo's measures: in `quality.db`, and in memory.
pub struct QualityIndex {
    db: Mutex<Connection>,
    measured: RwLock<HashMap<i64, Quality>>,
    /// Photos measured while their faces weren't looked for yet: measured
    /// again once they are.
    without_faces: RwLock<HashSet<i64>>,
    /// Originals a closer look couldn't read, and why (until Shoebox restarts):
    /// Cull stops offering them and says so.
    unreadable: Mutex<HashMap<i64, String>>,
    /// Open once the measures are read.
    ready: crate::Ready,
    /// Measuring takes a few threads, never all of them: the window's own
    /// work (pictures read from originals) shares the computer.
    pool: rayon::ThreadPool,
}

#[derive(Clone, Default, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CloserProgress {
    pub done: u64,
    pub total: u64,
    pub finished: bool,
}

impl QualityIndex {
    /// Opens the measures and reads them before returning.
    pub fn open(cat: &Catalog) -> Result<Self> {
        let index = Self::unread(cat)?;
        let result = index.read();
        index.ready.open(result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")));
        result.map(|_| index)
    }

    /// Opens the measures at once and reads them in the background.
    pub fn open_in_background(cat: &Catalog) -> Result<Arc<Self>> {
        let index = Arc::new(Self::unread(cat)?);
        let reading = index.clone();
        std::thread::Builder::new().name("quality-open".into()).spawn(move || {
            let result = reading.read().map_err(|e| {
                log::warn!("cull: couldn't read the measures: {e:#}");
                format!("Cull's measures couldn't be read: {e:#}")
            });
            reading.ready.open(result);
        })?;
        Ok(index)
    }

    fn unread(cat: &Catalog) -> Result<Self> {
        let conn = Connection::open(cat.data.root.join("quality.db"))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // A photo's id is used again once it leaves the catalog: each row
        // keeps the fingerprint of the photo it measured.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS quality (
                 photo_id    INTEGER PRIMARY KEY,
                 fingerprint TEXT    NOT NULL,
                 version     INTEGER NOT NULL,
                 faces       INTEGER NOT NULL,   -- its faces were looked for when measured
                 preview     TEXT    NOT NULL,   -- Measures (JSON)
                 close       TEXT                -- Measures from the original, when Cull looked closer
             );",
        )?;
        let threads = (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) / 2).clamp(1, 6);
        Ok(Self {
            db: Mutex::new(conn),
            measured: RwLock::new(HashMap::new()),
            without_faces: RwLock::new(HashSet::new()),
            unreadable: Mutex::new(HashMap::new()),
            ready: crate::Ready::default(),
            pool: rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("cull-{i}")).build()?,
        })
    }

    fn read(&self) -> Result<()> {
        let started = std::time::Instant::now();
        let conn = self.db.lock().unwrap();
        let mut measured = HashMap::new();
        let mut without_faces = HashSet::new();
        let mut stmt = conn.prepare("SELECT photo_id, faces, preview, close FROM quality WHERE version = ?1")?;
        let mut rows = stmt.query([QUALITY_VERSION])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            let preview: String = r.get(2)?;
            let close: Option<String> = r.get(3)?;
            let Ok(preview) = serde_json::from_str(&preview) else { continue };
            let close = close.and_then(|c| serde_json::from_str(&c).ok());
            measured.insert(id, Quality { preview, close });
            if !r.get::<_, bool>(1)? {
                without_faces.insert(id);
            }
        }
        log::info!("cull: {} photos' measures read in {} ms", measured.len(), started.elapsed().as_millis());
        *self.measured.write().unwrap() = measured;
        *self.without_faces.write().unwrap() = without_faces;
        Ok(())
    }

    pub fn measured(&self) -> usize {
        self.measured.read().unwrap().len()
    }

    pub fn get(&self, id: i64) -> Option<Quality> {
        self.measured.read().unwrap().get(&id).copied()
    }

    /// The grid's badges for these photos (0: nothing wrong, or not measured yet).
    pub fn flags(&self, ids: &[i64]) -> Vec<u32> {
        let measured = self.measured.read().unwrap();
        ids.iter().map(|id| measured.get(id).map_or(0, flags_of)).collect()
    }

    /// Measures every photo that has a preview and no measures yet (or older
    /// ones, or measured before its faces were looked for), from its preview.
    pub fn index_pending(
        &self,
        cat: &Catalog,
        cancel: &AtomicBool,
        progress: &dyn Fn(&crate::search::AiProgress),
    ) -> Result<()> {
        self.ready.wait()?;
        let ready = cat.preview_ready()?;
        self.forget_stale(&ready)?;
        let looked: HashSet<i64> = cat
            .db()
            .prepare("SELECT photo_id FROM face_scan")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let todo: Vec<(i64, String)> = {
            let measured = self.measured.read().unwrap();
            let without = self.without_faces.read().unwrap();
            ready
                .into_iter()
                .filter(|(id, _)| !measured.contains_key(id) || (without.contains(id) && looked.contains(id)))
                .collect()
        };
        if todo.is_empty() {
            return Ok(());
        }
        let mut p =
            crate::search::AiProgress { task: "quality".into(), total: todo.len() as u64, ..Default::default() };
        progress(&p);
        log::info!("cull: measuring {} photos", todo.len());
        let started = std::time::Instant::now();
        let mut measured = 0;
        for chunk in todo.chunks(64) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let ids: Vec<i64> = chunk.iter().map(|(id, _)| *id).collect();
            let faces = faces_of(&cat.db(), &ids)?;
            let rows: Vec<(i64, String, bool, Measures)> = self.pool.install(|| {
                chunk
                    .par_iter()
                    .filter_map(|(id, fp)| {
                        let img = match image::open(cat.data.preview_path(*id)) {
                            Ok(img) => img.to_rgb8(),
                            Err(e) => {
                                log::warn!("cull: photo {id}: preview unreadable: {e}");
                                return None;
                            }
                        };
                        let m = measure(&img, faces.get(id).map(Vec::as_slice).unwrap_or(&[]));
                        Some((*id, fp.clone(), looked.contains(id), m))
                    })
                    .collect()
            });
            self.store_previews(&rows)?;
            measured += rows.len();
            p.done += chunk.len() as u64;
            progress(&p);
        }
        log::info!("cull: {measured} of {} photos measured in {:.1} s", p.done, started.elapsed().as_secs_f64());
        Ok(())
    }

    fn store_previews(&self, rows: &[(i64, String, bool, Measures)]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        {
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction()?;
            for (id, fp, faces, m) in rows {
                // A photo's closer look stays: it doesn't depend on its preview.
                tx.execute(
                    "INSERT INTO quality (photo_id, fingerprint, version, faces, preview) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (photo_id) DO UPDATE SET fingerprint = ?2, version = ?3, faces = ?4, preview = ?5,
                         close = CASE WHEN fingerprint = ?2 AND version = ?3 THEN close END",
                    params![id, fp, QUALITY_VERSION, faces, serde_json::to_string(m)?],
                )?;
            }
            tx.commit()?;
        }
        let mut measured = self.measured.write().unwrap();
        let mut without = self.without_faces.write().unwrap();
        for (id, _, faces, m) in rows {
            let close = measured.get(id).and_then(|q| q.close);
            measured.insert(*id, Quality { preview: *m, close });
            if *faces {
                without.remove(id);
            } else {
                without.insert(*id);
            }
        }
        Ok(())
    }

    /// Forgets measures that no longer fit their photo: it left the catalog
    /// (or has no preview right now), or its id now belongs to another photo.
    fn forget_stale(&self, ready: &[(i64, String)]) -> Result<usize> {
        let ready: HashMap<i64, &str> = ready.iter().map(|(id, fp)| (*id, fp.as_str())).collect();
        let mut db = self.db.lock().unwrap();
        let rows: Vec<(i64, String, i64)> = db
            .prepare("SELECT photo_id, fingerprint, version FROM quality")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let gone: Vec<i64> = rows
            .into_iter()
            .filter(|(id, fp, v)| *v != QUALITY_VERSION || ready.get(id).is_none_or(|now| fp != now))
            .map(|(id, ..)| id)
            .collect();
        if gone.is_empty() {
            return Ok(0);
        }
        {
            let tx = db.transaction()?;
            for id in &gone {
                tx.execute("DELETE FROM quality WHERE photo_id = ?1", [id])?;
            }
            tx.commit()?;
        }
        drop(db);
        let mut measured = self.measured.write().unwrap();
        let mut without = self.without_faces.write().unwrap();
        for id in &gone {
            measured.remove(id);
            without.remove(id);
        }
        Ok(gone.len())
    }

    /// Looks closer at photos whose originals are reachable now: each read at
    /// half its size, or whole when small (read-only), and measured again.
    /// Those already looked at, and those whose disk is unplugged, are
    /// skipped. Returns how many were.
    pub fn look_closer(
        &self,
        cat: &Catalog,
        ids: &[i64],
        cancel: &AtomicBool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<usize> {
        self.ready.wait()?;
        let todo: Vec<i64> = {
            let measured = self.measured.read().unwrap();
            ids.iter().copied().filter(|id| measured.get(id).is_none_or(|q| q.close.is_none())).collect()
        };
        let total = todo.len() as u64;
        progress(0, total);
        let mut done = 0u64;
        let mut looked = 0;
        let started = std::time::Instant::now();
        // A few at a time: a USB disk reads one file at a time anyway.
        for chunk in todo.chunks(12) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let faces = faces_of(&cat.db(), chunk)?;
            let fingerprints = fingerprints_of(&cat.db(), chunk)?;
            let results: Vec<(i64, std::result::Result<Measures, String>)> = self.pool.install(|| {
                chunk
                    .par_iter()
                    .with_max_len(1)
                    .map(|id| (*id, closer_look(cat, *id, faces.get(id).map(Vec::as_slice).unwrap_or(&[]))))
                    .collect()
            });
            let mut found = Vec::new();
            {
                let mut unreadable = self.unreadable.lock().unwrap();
                for (id, r) in results {
                    match r {
                        Ok(m) => {
                            unreadable.remove(&id);
                            found.push((id, m));
                        }
                        Err(why) => {
                            log::warn!("cull: photo {id}: original not read for a closer look: {why}");
                            unreadable.insert(id, why);
                        }
                    }
                }
            }
            looked += found.len();
            self.store_close(cat, &found, &fingerprints, &faces)?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        log::info!("cull: looked closer at {looked} of {total} photos in {:.1} s", started.elapsed().as_secs_f64());
        Ok(looked)
    }

    fn store_close(
        &self,
        cat: &Catalog,
        found: &[(i64, Measures)],
        fingerprints: &HashMap<i64, String>,
        faces: &HashMap<i64, Vec<FaceBox>>,
    ) -> Result<()> {
        // A photo not measured from its preview yet is now, so it has a row.
        let missing: Vec<(i64, String, bool, Measures)> = {
            let measured = self.measured.read().unwrap();
            found
                .iter()
                .filter(|(id, _)| !measured.contains_key(id))
                .filter_map(|(id, _)| {
                    let img = image::open(cat.data.preview_path(*id)).ok()?.to_rgb8();
                    let m = measure(&img, faces.get(id).map(Vec::as_slice).unwrap_or(&[]));
                    Some((*id, fingerprints.get(id)?.clone(), faces.contains_key(id), m))
                })
                .collect()
        };
        self.store_previews(&missing)?;
        {
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction()?;
            for (id, m) in found {
                tx.execute(
                    "UPDATE quality SET close = ?1 WHERE photo_id = ?2",
                    params![serde_json::to_string(m)?, id],
                )?;
            }
            tx.commit()?;
        }
        let mut measured = self.measured.write().unwrap();
        for (id, m) in found {
            if let Some(q) = measured.get_mut(id) {
                q.close = Some(*m);
            }
        }
        Ok(())
    }

    /// A shoot as Cull shows it: the photos of `scope` narrowed by
    /// `filters`, in the order they were taken, bursts gathered into stacks,
    /// each frame with what's wrong with it and what Cull suggests.
    pub fn shoot(
        &self,
        cat: &Catalog,
        search: &crate::search::SearchIndex,
        scope: &Scope,
        filters: &Filters,
    ) -> Result<Shoot> {
        self.ready.wait()?;
        let started = std::time::Instant::now();
        let list = cat.list_photos(scope, Sort::Oldest, filters)?;
        if list.ids.len() > MAX_SHOOT {
            bail!(
                "Cull works on one shoot at a time, up to {} photos; this view has {}. Pick a folder or a day.",
                MAX_SHOOT,
                list.ids.len()
            );
        }
        let info = cat.shot_info(&list.ids)?;
        let faces = faces_of(&*cat.read()?, &list.ids)?;
        let vectors = search.vectors(&list.ids);
        let measured = self.measured.read().unwrap();
        let frames: Vec<FrameIn> = list
            .ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let info = info.get(id).cloned().unwrap_or_default();
                FrameIn {
                    id: *id,
                    taken: list.taken[i],
                    camera: info.camera.clone(),
                    vector: vectors.get(id).cloned(),
                    quality: measured.get(id).copied(),
                    shot: Shot { exposure: info.exposure.clone(), focal: info.focal },
                    row: FrameRow {
                        id: *id,
                        name: info.name.clone(),
                        taken: list.taken[i],
                        state: list.state[i],
                        w: list.w[i],
                        h: list.h[i],
                        rev: list.rev[i],
                        marked: list.marked[i] == 1,
                        focus: faces.get(id).and_then(|f| focus_of(f)),
                        ..Default::default()
                    },
                }
            })
            .collect();
        drop(measured);
        let measured_n = frames.iter().filter(|f| f.quality.is_some()).count();
        let closer_n = frames.iter().filter(|f| f.quality.is_some_and(|q| q.close.is_some())).count();
        let failed = self.unreadable.lock().unwrap().clone();
        let can_look = frames
            .iter()
            .filter(|f| f.row.state == 2 && f.quality.is_none_or(|q| q.close.is_none()) && !failed.contains_key(&f.id))
            .map(|f| f.id)
            .collect();
        let unreadable = frames
            .iter()
            .filter_map(|f| {
                let reason = failed.get(&f.id)?.clone();
                Some(Unreadable { id: f.id, name: f.row.name.clone(), reason })
            })
            .collect();
        let described = frames.iter().filter(|f| f.vector.is_some()).count();
        let stacks = suggest(frames);
        log::info!("cull: {} photos in {} shots in {} ms", list.ids.len(), stacks.len(), started.elapsed().as_millis());
        Ok(Shoot {
            photos: list.ids.len(),
            measured: measured_n,
            closer: closer_n,
            described,
            can_look_closer: can_look,
            unreadable,
            stacks,
        })
    }
}

/// A photo's original, read at half its size (whole when small, read-only),
/// and measured; or why it couldn't be.
fn closer_look(cat: &Catalog, id: i64, faces: &[FaceBox]) -> std::result::Result<Measures, String> {
    let (path, kind) = cat.original(id).map_err(|e| format!("{e:#}"))?;
    let full = cat.full_size(id);
    let long = full.map_or(3000, |l| if l <= CLOSE_WHOLE { l } else { l / 2 });
    let loaded = crate::imaging::load(&path, kind, long);
    let full = full.or(loaded.meta.width.max(loaded.meta.height));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let img = loaded.image.map_err(|e| format!("{name}: {e}"))?.to_rgb8();
    let mut m = measure(&img, faces);
    m.full = full.filter(|&f| f >= m.long);
    Ok(m)
}

/// Where to look at 100% first: the middle of the largest face (0..1 of the picture).
fn focus_of(faces: &[FaceBox]) -> Option<[f32; 2]> {
    let f = faces.iter().max_by(|a, b| (a.x2 - a.x1).total_cmp(&(b.x2 - b.x1)))?;
    Some([(f.x1 + f.x2) * 0.5, (f.y1 + f.y2) * 0.5])
}

/// The faces found in these photos.
fn faces_of(db: &Connection, ids: &[i64]) -> Result<HashMap<i64, Vec<FaceBox>>> {
    let mut out: HashMap<i64, Vec<FaceBox>> = HashMap::new();
    let mut stmt = db.prepare(
        "SELECT photo_id, x1, y1, x2, y2, kps FROM faces WHERE photo_id IN (SELECT value FROM json_each(?1))",
    )?;
    let mut rows = stmt.query([serde_json::to_string(ids)?])?;
    while let Some(r) = rows.next()? {
        let kps: String = r.get(5)?;
        out.entry(r.get(0)?).or_default().push(FaceBox {
            x1: r.get::<_, f64>(1)? as f32,
            y1: r.get::<_, f64>(2)? as f32,
            x2: r.get::<_, f64>(3)? as f32,
            y2: r.get::<_, f64>(4)? as f32,
            kps: FaceBox::parse_kps(&kps),
        });
    }
    Ok(out)
}

fn fingerprints_of(db: &Connection, ids: &[i64]) -> Result<HashMap<i64, String>> {
    let mut stmt = db.prepare(&format!(
        "SELECT id, fingerprint FROM photos WHERE preview_state = {PREVIEW_OK} AND id IN (SELECT value FROM json_each(?1))"
    ))?;
    let out = stmt
        .query_map([serde_json::to_string(ids)?], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Bursts, and what to keep.

/// A shoot, as Cull shows it.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shoot {
    pub photos: usize,
    /// Photos measured (from their preview at least).
    pub measured: usize,
    /// Photos looked at closer, from their original.
    pub closer: usize,
    /// Photos with a CLIP description (what tells bursts apart).
    pub described: usize,
    /// Photos whose original is reachable now and not looked at closer yet.
    pub can_look_closer: Vec<i64>,
    /// Originals a closer look couldn't read, and why.
    pub unreadable: Vec<Unreadable>,
    pub stacks: Vec<Stack>,
}

/// Frames of the same shot, in the order they were taken.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    pub frames: Vec<FrameRow>,
    /// The frame Cull would keep (an index into `frames`).
    pub keeper: usize,
}

/// What Cull suggests for a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Suggest {
    Keep,
    Reject,
    /// Left to the user.
    #[default]
    None,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameRow {
    pub id: i64,
    pub name: Option<String>,
    pub taken: i64,
    /// 2 = on a plugged-in disk, 1 = unplugged, 0 = missing.
    pub state: i64,
    /// Thumbnail size, and its revision (for its address).
    pub w: i64,
    pub h: i64,
    pub rev: i64,
    /// Marked for deletion.
    pub marked: bool,
    pub suggest: Suggest,
    /// Why, in words, the first one the main reason.
    pub reasons: Vec<String>,
    pub problems: Vec<Problem>,
    /// Edge blur used to compare frames, in thousandths of the long edge
    /// (lower is sharper). None: not measured yet.
    pub blur: Option<f32>,
    /// Measured from the original, not only the preview.
    pub closer: bool,
    /// How alike CLIP finds it and the frame before (0..1).
    pub alike: Option<f32>,
    /// How alike its layout and the frame before's are (-1..1).
    pub framing: Option<f32>,
    /// Where to look at 100% first: the middle of its largest face (0..1 of the picture).
    pub focus: Option<[f32; 2]>,
}

/// An original a closer look couldn't read.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Unreadable {
    pub id: i64,
    pub name: Option<String>,
    pub reason: String,
}

struct FrameIn {
    id: i64,
    taken: i64,
    camera: Option<String>,
    vector: Option<Vec<f32>>,
    quality: Option<Quality>,
    shot: Shot,
    row: FrameRow,
}

/// A photo's camera, exposure and file name, for Cull.
#[derive(Clone, Debug, Default)]
pub struct ShotInfo {
    pub camera: Option<String>,
    pub exposure: Option<String>,
    pub focal: Option<f64>,
    pub name: Option<String>,
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Gathers frames (in the order taken) into stacks of the same shot, and
/// works out what to suggest for each frame. A frame joins the stack before
/// it when taken by the same camera, the same way up, soon after, and laid
/// out like both the frame before it and the stack's first (so a stack never
/// drifts from pose to pose), and CLIP finds it alike. Without measures or
/// descriptions, only frames a second apart are one burst.
fn suggest(frames: Vec<FrameIn>) -> Vec<Stack> {
    let layout = |f: &FrameIn| f.quality.and_then(|q| q.preview.layout);
    let upright = |f: &FrameIn| (f.row.w > 0 && f.row.h > 0).then_some(f.row.h > f.row.w);
    let mut groups: Vec<Vec<FrameIn>> = Vec::new();
    for mut f in frames {
        let joins = groups.last().is_some_and(|g| {
            let (first, prev) = (&g[0], &g[g.len() - 1]);
            f.row.alike = match (&prev.vector, &f.vector) {
                (Some(a), Some(b)) => Some(dot(a, b)),
                _ => None,
            };
            let framing = match (layout(prev), layout(first), layout(&f)) {
                (Some(p), Some(s), Some(l)) => Some((p.alike(&l), s.alike(&l))),
                _ => None,
            };
            f.row.framing = framing.map(|(p, _)| p);
            let gap = (f.taken - prev.taken).abs();
            let together = prev.camera == f.camera && f.taken > 0 && upright(prev) == upright(&f);
            let looks = match (framing, f.row.alike) {
                (None, None) => gap <= BURST_GAP,
                (framing, alike) => {
                    gap <= SAME_SHOT_GAP
                        && framing.is_none_or(|(p, s)| p >= SAME_FRAMING && s >= SAME_FRAMING_AS_FIRST)
                        && alike.is_none_or(|a| a >= SAME_SHOT)
                }
            };
            together && looks
        });
        if joins {
            groups.last_mut().unwrap().push(f);
        } else {
            groups.push(vec![f]);
        }
    }
    groups.into_iter().map(stack_of).collect()
}

/// Two files of one capture: the same name but for the extension, taken the
/// same second by the same camera (a RAW and its JPEG, or an export).
fn same_capture(a: &FrameIn, b: &FrameIn) -> bool {
    let stem = |f: &FrameIn| f.row.name.as_deref().map(|n| n.rsplit_once('.').map_or(n, |(s, _)| s).to_lowercase());
    a.taken > 0 && a.taken == b.taken && a.camera == b.camera && stem(a).is_some() && stem(a) == stem(b)
}

fn stack_of(frames: Vec<FrameIn>) -> Stack {
    let found: Vec<Vec<Found>> =
        frames.iter().map(|f| f.quality.map(|q| problems(&q, &f.shot)).unwrap_or_default()).collect();
    // Frames compare on their faces when each has one measured, else on their
    // sharpest part; on the originals when each was looked at closer.
    let all_close = frames.iter().all(|f| f.quality.is_some_and(|q| q.close.is_some()));
    let pick_measures = |f: &FrameIn| f.quality.map(|q| if all_close { *q.best() } else { q.preview });
    let all_faces = frames.iter().all(|f| pick_measures(f).is_some_and(|m| m.face.is_some()));
    let blur: Vec<Option<f32>> = frames
        .iter()
        .map(|f| pick_measures(f).and_then(|m| m.rel(if all_faces { m.face } else { m.sharpest })))
        .collect();
    let score = |i: usize| -> f32 {
        let major = found[i].iter().filter(|p| p.problem.major()).count() as f32;
        let minor = found[i].len() as f32 - major;
        let blown = frames[i].quality.map_or(0.0, |q| q.preview.blown);
        blur[i].unwrap_or(5.0) + 5.0 * major + 0.5 * minor + 2.0 * blown
    };
    let keeper = (0..frames.len()).min_by(|a, b| score(*a).total_cmp(&score(*b))).unwrap_or(0);
    // Versions of one capture (a RAW and its camera JPEG or an export) are
    // one frame to Cull: they share the verdict of the best of them.
    let capture: Vec<usize> =
        (0..frames.len()).map(|i| (0..i).find(|&j| same_capture(&frames[j], &frames[i])).unwrap_or(i)).collect();
    let n = capture.iter().enumerate().filter(|(i, c)| i == *c).count();
    let kept = capture[keeper];
    let keeper_major = found[keeper].iter().any(|p| p.problem.major());
    let keeper_minor = !found[keeper].is_empty() && !keeper_major;
    let keeper_ok = !keeper_major;
    let measured = frames[keeper].quality.is_some();
    let rows = frames
        .into_iter()
        .zip(found)
        .enumerate()
        .map(|(i, (f, found))| {
            let mut row = f.row;
            row.blur = blur[i];
            row.closer = f.quality.is_some_and(|q| q.close.is_some());
            row.problems = found.iter().map(|p| p.problem).collect();
            let own: Vec<String> = found.into_iter().map(|p| p.text).collect();
            let version = (i != keeper && capture[i] == kept).then(|| "Another version of the same frame".to_string());
            if f.quality.is_none() {
                row.suggest = Suggest::None;
                row.reasons = vec!["Not measured yet".into()];
            } else if n == 1 {
                row.suggest = if keeper_major {
                    Suggest::Reject
                } else if keeper_minor {
                    Suggest::None
                } else {
                    Suggest::Keep
                };
                row.reasons = version.into_iter().chain(own).collect();
            } else if capture[i] == kept && keeper_ok {
                row.suggest = Suggest::Keep;
                let best = (i == keeper).then(|| format!("The best of these {n} frames"));
                row.reasons = best.into_iter().chain(version).chain(own).collect();
            } else if capture[i] == kept {
                row.suggest = Suggest::Reject;
                row.reasons = version.into_iter().chain(own).collect();
            } else {
                row.suggest = if measured { Suggest::Reject } else { Suggest::None };
                let why = if keeper_ok {
                    format!("Same shot: frame {} is better", keeper + 1)
                } else {
                    "Same shot, and none of its frames is good".to_string()
                };
                row.reasons = std::iter::once(why).chain(own).collect();
            }
            row
        })
        .collect();
    Stack { frames: rows, keeper }
}

impl Shoot {
    /// The frames Cull suggests rejecting that aren't marked for deletion yet:
    /// what accepting its suggestions marks.
    pub fn suggested(&self) -> Vec<i64> {
        self.stacks
            .iter()
            .flat_map(|s| &s.frames)
            .filter(|f| !f.marked && f.suggest == Suggest::Reject)
            .map(|f| f.id)
            .collect()
    }
}

/// One photo's measures and problems, for `shoebox-cli measure`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub id: i64,
    pub quality: Option<Quality>,
    pub problems: Vec<Found>,
}

impl QualityIndex {
    pub fn report(&self, cat: &Catalog, id: i64) -> Result<Report> {
        self.ready.wait()?;
        let quality = self.get(id);
        let info = cat.shot_info(&[id])?.remove(&id).unwrap_or_default();
        let shot = Shot { exposure: info.exposure, focal: info.focal };
        let problems = quality.map(|q| problems(&q, &shot)).unwrap_or_default();
        Ok(Report { id, quality, problems })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    /// Squares of two greys, edges in both directions, blurred by `sigma` px.
    fn squares(w: u32, h: u32, sigma: f32) -> RgbImage {
        let img =
            RgbImage::from_fn(
                w,
                h,
                |x, y| if (x / 24 + y / 24).is_multiple_of(2) { Rgb([60; 3]) } else { Rgb([190; 3]) },
            );
        if sigma > 0.0 { image::imageops::blur(&img, sigma) } else { img }
    }

    /// Blurred along x only, as a camera moving sideways does.
    fn moved(img: &RgbImage, px: u32) -> RgbImage {
        RgbImage::from_fn(img.width(), img.height(), |x, y| {
            let n = px * 2 + 1;
            let mut s = [0u32; 3];
            for d in 0..n {
                let xx = (x + d).saturating_sub(px).min(img.width() - 1);
                for (sum, v) in s.iter_mut().zip(img.get_pixel(xx, y).0) {
                    *sum += v as u32;
                }
            }
            Rgb([(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8])
        })
    }

    #[test]
    fn edge_blur_is_measured_whatever_the_contrast() {
        let sharp = measure(&squares(480, 320, 0.0), &[]);
        assert!(sharp.sharpest.unwrap() < 0.3, "sharp edges: {sharp:?}");
        for sigma in [1.0f32, 2.0, 3.0] {
            let m = measure(&squares(480, 320, sigma), &[]);
            let got = m.sharpest.unwrap();
            assert!((got - sigma).abs() < 0.35 * sigma, "blurred by {sigma}: measured {got}");
        }
        // Half the contrast, the same blur.
        let faint = RgbImage::from_fn(480, 320, |x, y| {
            if (x / 24 + y / 24).is_multiple_of(2) { Rgb([110; 3]) } else { Rgb([175; 3]) }
        });
        let (a, b) = (
            measure(&image::imageops::blur(&faint, 2.0), &[]).sharpest,
            measure(&squares(480, 320, 2.0), &[]).sharpest,
        );
        assert!((a.unwrap() - b.unwrap()).abs() < 0.3, "{a:?} vs {b:?}");
    }

    #[test]
    fn shake_in_one_direction_counts() {
        let m = measure(&moved(&squares(480, 320, 0.0), 6), &[]);
        assert!(m.sharpest.unwrap() > 2.0, "{m:?}");
    }

    #[test]
    fn a_shallow_focus_photo_is_judged_by_its_sharp_part() {
        // Sharp on the left third, very blurred elsewhere: bokeh, not a blurry photo.
        let sharp = squares(480, 320, 0.0);
        let soft = squares(480, 320, 4.0);
        let img =
            RgbImage::from_fn(480, 320, |x, y| if x < 160 { *sharp.get_pixel(x, y) } else { *soft.get_pixel(x, y) });
        let m = measure(&img, &[]);
        assert!(m.sharpest.unwrap() < 0.5 && m.typical.unwrap() > 2.5, "{m:?}");
        let q = Quality { preview: m, close: None };
        assert!(problems(&q, &Shot::default()).is_empty());
    }

    #[test]
    fn problems_of_typical_frames() {
        // 1080 px previews.
        let shot = Shot { exposure: Some("1/15".into()), focal: Some(200.0) };
        let blurry = Quality { preview: measure(&squares(1080, 720, 2.5), &[]), close: None };
        let found = problems(&blurry, &shot);
        assert_eq!(found.iter().map(|f| f.problem).collect::<Vec<_>>(), [Problem::Blurry]);
        assert!(found[0].text.contains("1/15 s at 200 mm"), "{found:?}");

        let black = Quality { preview: measure(&RgbImage::from_pixel(1080, 720, Rgb([2; 3])), &[]), close: None };
        assert_eq!(problems(&black, &Shot::default())[0].problem, Problem::Empty);

        let sharp = Quality { preview: measure(&squares(1080, 720, 0.0), &[]), close: None };
        assert!(problems(&sharp, &Shot::default()).is_empty());
        // Fine on screen, soft at 100%: only the original says so.
        let soft_close = Quality { preview: sharp.preview, close: Some(measure(&squares(3000, 2000, 2.0), &[])) };
        assert_eq!(problems(&soft_close, &Shot::default())[0].problem, Problem::Soft);
    }

    #[test]
    fn a_face_out_of_focus_is_found() {
        // Sharp background; a smooth face-sized patch whose own edges are blurred.
        let sharp = squares(1080, 720, 0.0);
        let soft = squares(1080, 720, 5.0);
        let face = FaceBox { x1: 0.4, y1: 0.3, x2: 0.6, y2: 0.7, kps: None };
        let img = RgbImage::from_fn(1080, 720, |x, y| {
            let (fx, fy) = (x as f32 / 1080.0, y as f32 / 720.0);
            if fx > 0.38 && fx < 0.62 && fy > 0.28 && fy < 0.72 {
                *soft.get_pixel(x, y)
            } else {
                *sharp.get_pixel(x, y)
            }
        });
        let q = Quality { preview: measure(&img, std::slice::from_ref(&face)), close: None };
        let found: Vec<Problem> = problems(&q, &Shot::default()).iter().map(|f| f.problem).collect();
        assert_eq!(found, [Problem::FaceSoft], "{:?}", q.preview);
        // The same face in focus: nothing to say.
        let ok = Quality { preview: measure(&sharp, &[face]), close: None };
        assert!(problems(&ok, &Shot::default()).is_empty());
    }

    fn frame(id: i64, taken: i64, vector: Option<Vec<f32>>, blur: f32) -> FrameIn {
        let preview = Measures {
            long: 1000,
            sharpest: Some(blur),
            typical: Some(blur),
            mean: 0.5,
            spread: 0.2,
            detail: 0.8,
            ..Default::default()
        };
        FrameIn {
            id,
            taken,
            camera: Some("EOS R6".into()),
            vector,
            quality: Some(Quality { preview, close: None }),
            shot: Shot::default(),
            row: FrameRow { id, taken, ..Default::default() },
        }
    }

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = dot(v, v).sqrt();
        v.iter().map(|x| x / n).collect()
    }

    #[test]
    fn bursts_become_stacks_and_the_sharpest_is_kept() {
        let a = unit(&[1.0, 0.0, 0.1]);
        let a2 = unit(&[1.0, 0.05, 0.12]);
        let b = unit(&[0.0, 1.0, 0.1]);
        let stacks = suggest(vec![
            frame(1, 100, Some(a.clone()), 0.5),
            frame(2, 101, Some(a2.clone()), 0.2), // sharper
            frame(3, 102, Some(a.clone()), 0.4),
            frame(4, 103, Some(b.clone()), 0.3), // another subject
            frame(5, 500, Some(b), 0.3),         // the same subject, much later
        ]);
        assert_eq!(stacks.iter().map(|s| s.frames.len()).collect::<Vec<_>>(), [3, 1, 1]);
        let burst = &stacks[0];
        assert_eq!(burst.keeper, 1);
        let s: Vec<Suggest> = burst.frames.iter().map(|f| f.suggest).collect();
        assert_eq!(s, [Suggest::Reject, Suggest::Keep, Suggest::Reject]);
        assert_eq!(burst.frames[0].reasons[0], "Same shot: frame 2 is better");
        assert_eq!(stacks[1].frames[0].suggest, Suggest::Keep);

        // Without descriptions, only frames a second apart are one burst.
        let stacks = suggest(vec![frame(1, 100, None, 0.5), frame(2, 101, None, 0.2), frame(3, 110, None, 0.2)]);
        assert_eq!(stacks.iter().map(|s| s.frames.len()).collect::<Vec<_>>(), [2, 1]);
    }

    #[test]
    fn versions_of_one_capture_share_its_verdict() {
        let named = |id: i64, taken: i64, blur: f32, name: &str| {
            let mut f = frame(id, taken, None, blur);
            f.row.name = Some(name.into());
            f
        };
        // A RAW, its export (sharpened a little), and the next frame of the burst.
        let stacks = suggest(vec![
            named(1, 100, 0.4, "IMG_0001.CR2"),
            named(2, 100, 0.3, "IMG_0001.jpg"),
            named(3, 101, 0.6, "IMG_0002.CR2"),
        ]);
        assert_eq!(stacks.len(), 1);
        let s: Vec<Suggest> = stacks[0].frames.iter().map(|f| f.suggest).collect();
        assert_eq!(s, [Suggest::Keep, Suggest::Keep, Suggest::Reject]);
        assert_eq!(stacks[0].frames[1].reasons[0], "The best of these 2 frames");
        assert_eq!(stacks[0].frames[0].reasons[0], "Another version of the same frame");
        // A lone RAW and its JPEG: both kept, neither "the same shot".
        let stacks = suggest(vec![named(1, 100, 0.4, "IMG_0001.CR2"), named(2, 100, 0.3, "IMG_0001.JPG")]);
        assert!(stacks[0].frames.iter().all(|f| f.suggest == Suggest::Keep), "{:?}", stacks[0].frames);
    }

    #[test]
    fn a_blurry_single_is_suggested_for_rejection_and_a_blurry_burst_keeps_nothing() {
        let stacks = suggest(vec![frame(1, 100, None, 3.0)]);
        assert_eq!(stacks[0].frames[0].suggest, Suggest::Reject);
        let stacks = suggest(vec![frame(1, 100, None, 3.0), frame(2, 100, None, 2.5)]);
        assert!(stacks[0].frames.iter().all(|f| f.suggest == Suggest::Reject));
    }

    #[test]
    fn shutter_speeds_read_as_the_catalog_keeps_them() {
        let slow = |e: &str, f: f64| slow_shutter(&Shot { exposure: Some(e.into()), focal: Some(f) });
        assert!(slow("1/15", 200.0).is_some());
        assert!(slow("1/400", 200.0).is_none());
        assert_eq!(slow("0.5s", 35.0).unwrap(), "0.5 s at 35 mm is slow for a steady hand");
        assert!(slow("1/1000", 1200.0).is_none(), "faster than 1/500 is never slow");
    }
}
