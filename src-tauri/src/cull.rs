//! The culling assistant: what is wrong with a photo (blurry, a face out of
//! focus, too dark, blown out, nothing in it), which frames are the same shot
//! (a burst), and which one of them to keep.
//!
//! It only suggests, keep or reject. A reject is a photo marked for deletion,
//! as Cull does by hand: nothing is deleted until the user confirms. Marks
//! are the user's decisions; suggestions are worked out again whenever they
//! are asked for.
//!
//! A photo is measured from Shoebox's own preview when Cull is asked about it
//! (a shoot opened in Cull, thumbnails on screen with Cull's badges on), so
//! photos on unplugged disks are measured too, and the rest of an archive
//! costs nothing until it is culled. A preview is small (~1080 px):
//! it shows a blurry photo, not one that is only soft at 100%. So when a
//! shoot's disk is plugged in, Cull can look closer: its originals are read at
//! half size (read-only, as always) and measured again. Measures live in
//! `quality.db`, a file of their own like word search's: they can always be
//! measured again, so the catalog's backup doesn't need them.
//!
//! A camera shaken during a long exposure needs a measure of its own: it
//! draws the picture several times, each copy with crisp edges, which the
//! edge blur reads as sharp ([`shake_in`]).

use crate::catalog::{Catalog, Filters, PhotoDetail, Scope, Sort};
use crate::db::PREVIEW_OK;
use crate::focus::{Focus, Point, State};
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
pub const QUALITY_VERSION: i64 = 2;

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
/// is out of focus: the focus landed elsewhere. The spot the camera focused
/// on too: the focus missed it. Small (thousandths), and at 100% (px).
const FACE_SOFT: f32 = 2.4;
const FACE_SOFT_PX: f32 = 6.0;
const FACE_VS_SHARPEST: f32 = 2.0;
/// Originals up to this long are read whole for a closer look; longer ones at half size.
const CLOSE_WHOLE: u32 = 3200;
/// Shaken ([`shake_in`]): doubled or smeared this much, with a shutter slow
/// enough for the hand to shake. Seen small, on a preview, only gross shake
/// shows, and patterns and shallow focus come close: a third above every
/// photo that wasn't shaken among 1,240 previews of a real catalog. At 100%
/// (half-size originals): a sixth above the sharp frames of two handheld
/// test shoots (0.25-2 s) and of 190 other photos taken as slowly; 52 of
/// the shoots' 63 shaken frames reach it.
const DOUBLED: f32 = 0.45;
const SMEARED: f32 = 0.40;
const DOUBLED_CLOSE: f32 = 0.20;
const SMEARED_CLOSE: f32 = 0.09;
/// A face smaller than this share of the long edge isn't the subject.
const SUBJECT_FACE: f32 = 0.06;
/// A camera's focus point is measured over at least this share of the long
/// edge around its middle: a point is smaller than that on a preview, too
/// small to hold the edges a measure needs.
const POINT_AREA: f32 = 0.06;

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
    /// Edge blur where the camera focused, when its file says: the sharpest
    /// of its points in focus (else of those it was given).
    pub focus_point: Option<f32>,
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
    /// How doubled the picture is, 0..1: how strongly the slope of its edges
    /// comes back a few px further on along one direction, the same over the
    /// frame, as copies left by a shaken camera do ([`shake_in`]). None: too
    /// few edges to tell.
    pub doubled: Option<f32>,
    /// How smeared it is one way, 0..1: how much longer the slope of its
    /// edges lasts along one direction than across it.
    pub smeared: Option<f32>,
    /// The shutter was open long enough for the hand to shake the camera
    /// (slower than 1/focal): only then is a doubled or smeared picture put
    /// down to shake.
    pub slow_shutter: bool,
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

    /// How shaken this look found the photo, as a share of shaken (1 or
    /// more is): of the preview's thresholds, or of a closer look's. 0 when
    /// the shutter was too fast for the hand to shake.
    fn shake(&self, close: bool) -> f32 {
        if !self.slow_shutter {
            return 0.0;
        }
        let (doubled, smeared) = if close { (DOUBLED_CLOSE, SMEARED_CLOSE) } else { (DOUBLED, SMEARED) };
        (self.doubled.unwrap_or(0.0) / doubled).max(self.smeared.unwrap_or(0.0) / smeared)
    }

    /// How the picture looks shaken, when it is: "doubled" or "smeared".
    fn shaken(&self, close: bool) -> Option<&'static str> {
        if self.shake(close) < 1.0 {
            return None;
        }
        let doubled = if close { DOUBLED_CLOSE } else { DOUBLED };
        Some(if self.doubled.is_some_and(|d| d >= doubled) { "doubled" } else { "smeared" })
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
    /// Where the camera focused is soft while something else is sharp: the
    /// focus missed, or the subject moved.
    FocusMissed,
}

impl Problem {
    /// Enough to suggest rejecting the photo. The others are shown, and left
    /// to the user: a focus missed too, since a photographer who focused,
    /// then turned the camera to frame the shot leaves the point where the
    /// subject was, not where it is.
    pub fn major(self) -> bool {
        !matches!(self, Problem::Blown | Problem::Bright | Problem::FocusMissed)
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
            Problem::FocusMissed => 128,
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
    // Copies of the picture, each with crisp edges: the edge blur misses them.
    let with_shutter = |text: String| match slow_shutter(shot) {
        Some(slow) => format!("{text} ({slow})"),
        None => text,
    };
    if let Some(how) = p.shaken(false) {
        add(Problem::Blurry, with_shutter(format!("Shaken: the picture is {how}")));
        return out;
    }
    if let Some(how) = q.close.as_ref().and_then(|c| c.shaken(true)) {
        add(Problem::Soft, with_shutter(format!("Shaken at 100%: the picture is {how}")));
    } else if close && best.full_px(best.sharpest).is_some_and(|s| s >= SOFT_PX) {
        add(Problem::Soft, "Soft at 100%: nothing is quite sharp".into());
    }
    if let (Some(face), Some(face_px)) = (best.rel(best.face), best.full_px(best.face)) {
        let soft = if close { face_px >= FACE_SOFT_PX } else { face >= FACE_SOFT };
        if soft && face >= FACE_VS_SHARPEST * sharpest.unwrap_or(0.0) {
            add(Problem::FaceSoft, "Face out of focus: the focus landed elsewhere".into());
        }
    }
    // Without a face to judge by, where the camera focused, as faces are
    // judged; on the closest look that measured it.
    let (m, m_close) = if best.focus_point.is_some() { (best, close) } else { (p, false) };
    if best.face.is_none()
        && let (Some(point), Some(point_px)) = (m.rel(m.focus_point), m.full_px(m.focus_point))
    {
        let soft = if m_close { point_px >= FACE_SOFT_PX } else { point >= FACE_SOFT };
        if soft && point >= FACE_VS_SHARPEST * m.rel(m.sharpest).unwrap_or(0.0) {
            add(
                Problem::FocusMissed,
                "Focus missed: the camera focused here, but the sharpest part is elsewhere".into(),
            );
        }
    }
    out
}

impl Shot {
    pub fn of(info: &ShotInfo) -> Self {
        Shot { exposure: info.exposure.clone(), focal: info.focal }
    }

    /// How long the shutter was open, s, and the focal length, mm, when the file says.
    fn timing(&self) -> Option<(f64, f64)> {
        let e = self.exposure.as_deref()?;
        let secs = match e.strip_prefix("1/") {
            Some(d) => 1.0 / d.parse::<f64>().ok()?,
            None => e.trim_end_matches('s').parse::<f64>().ok()?,
        };
        Some((secs, self.focal.filter(|f| *f > 0.0)?))
    }

    /// Open long enough for the hand holding the camera to shake it: 1/focal or slower.
    pub fn shaky(&self) -> bool {
        self.timing().is_some_and(|(secs, focal)| secs >= 1.0 / focal)
    }
}

/// "1/15 s at 200 mm is slow for a steady hand", when it is: slower than 1/focal.
fn slow_shutter(shot: &Shot) -> Option<String> {
    let (secs, focal) = shot.timing()?;
    let e = shot.exposure.as_deref()?;
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

// Camera shake. A camera shaken during a long exposure draws the picture
// several times a few px apart, where the hand paused, and smears it in
// between. Each copy has crisp edges, so the edge blur above reads it sharp.
// What gives it away is the slope of the picture along the shake: past an
// edge, it comes back a few px further on (a copy: the picture is doubled)
// or lasts (a smear), while across the shake it doesn't. Edges are taken one
// direction at a time, only those facing it, so slanted lettering or a
// shirt's stripes don't pass for a smear; and over the frame's parts with the
// most edges, since a shake moves the whole picture while a pattern or a
// design is in a few parts. A pattern repeats again at twice its period, a
// copy doesn't. Ranges and thresholds were set on two handheld test shoots
// (0.25-2 s) labelled at 100%, and checked on 1,240 previews and 470
// originals of a real catalog.

/// Directions looked along, over a half turn.
const SHAKE_DIRS: usize = 16;
/// An edge faces a direction when its slope lies within 21.8° of it (tan):
/// short of 22.5°, two directions apart, so that an edge square to the
/// frame faces the same ones, and as many, whichever way it runs.
const SHAKE_FACING: f32 = 0.4;
/// Slope (levels per px) a pixel needs to be part of an edge.
const SHAKE_SLOPE: f32 = 2.0;
/// The parts looked at: those with the most edges, up to this many, each
/// with this share of the edges of the part with most at least; and enough
/// of them to tell.
const SHAKE_PARTS: usize = 12;
const SHAKE_PART_SHARE: f64 = 0.1;
const SHAKE_MIN_PARTS: usize = 3;
/// A direction counts in a part whose edges facing it carry this share of
/// those facing its most edged direction, and over this many parts.
const SHAKE_DIR_SHARE: f64 = 0.1;
const SHAKE_DIR_PARTS: usize = 2;
/// Copies are looked for up to the long edge / [`SHAKE_REACH`] px away, and
/// from [`SHAKE_NEAR`] px (nearer is the edge itself, or a fine pattern on an
/// original); a smear is measured over [`SMEAR`] px. Edges and patterns are
/// as wide in px whatever the picture's size, so these are in px, for a
/// picture seen small (a preview) and for one seen at 100% (a closer look,
/// [`CLOSE_SCALE`] px or longer).
const SHAKE_REACH: usize = 60;
const SHAKE_NEAR: [usize; 2] = [4, 7];
const SMEAR: [(usize, usize); 2] = [(3, 5), (6, 10)];
const CLOSE_SCALE: usize = 1600;

/// How doubled and how smeared `fine` is, 0..1 each (see above). None: too
/// few parts with edges to tell.
fn shake_in(fine: &Plane) -> Option<(f32, f32)> {
    let (w, h, v) = (fine.w, fine.h, &fine.v);
    let long = w.max(h);
    let scale = usize::from(long >= CLOSE_SCALE);
    let (near, (from, to)) = (SHAKE_NEAR[scale], SMEAR[scale]);
    let reach = (long / SHAKE_REACH).max(2 * near);
    // Slopes are compared up to twice as far (and a px): a pattern repeats there.
    let far = 2 * reach;
    let margin = far + 2;
    if w < 2 * margin + 16 || h < 2 * margin + 16 {
        return None;
    }
    let dirs: Vec<(f32, f32)> = (0..SHAKE_DIRS)
        .map(|k| {
            let a = std::f32::consts::PI * k as f32 / SHAKE_DIRS as f32;
            (a.cos(), a.sin())
        })
        .collect();
    let gradient = |i: usize| ((v[i + 1] - v[i - 1]) * 0.5, (v[i + w] - v[i - w]) * 0.5);
    // One pixel in `step` (across and down) of `r` on an edge: its index and gradient.
    let edges = |r: Rect, step: usize| {
        (r.y0..r.y1).step_by(step).flat_map(move |y| {
            (r.x0..r.x1).step_by(step).filter_map(move |x| {
                let i = y * w + x;
                let (a, b) = gradient(i);
                (a.abs().max(b.abs()) >= SHAKE_SLOPE).then_some((i, a, b))
            })
        })
    };
    // The slope at an edge along a direction it faces.
    let facing = |a: f32, b: f32, (c, s): (f32, f32)| {
        let along = a * c + b * s;
        ((b * c - a * s).abs() <= SHAKE_FACING * along.abs()).then_some(along)
    };

    // The parts with the most edges (their slope's energy per pixel, from
    // one pixel in 4: enough to rank them).
    let part = (long / 10).max(24);
    let mut parts: Vec<(f64, Rect)> = Vec::new();
    for y in (0..h).step_by(part) {
        for x in (0..w).step_by(part) {
            let r = Rect {
                x0: x.max(margin),
                y0: y.max(margin),
                x1: (x + part).min(w - margin),
                y1: (y + part).min(h - margin),
            };
            if r.x1 < r.x0 + 8 || r.y1 < r.y0 + 8 {
                continue;
            }
            let pixels = (r.x1 - r.x0).div_ceil(4) * (r.y1 - r.y0).div_ceil(4);
            let energy: f64 = edges(r, 4)
                .map(|(_, a, b)| dirs.iter().filter_map(|&d| facing(a, b, d)).map(|s| (s * s) as f64).sum::<f64>())
                .sum();
            parts.push((energy / pixels as f64, r));
        }
    }
    parts.sort_by(|a, b| b.0.total_cmp(&a.0));
    let most = parts.first().map_or(0.0, |p| p.0);
    parts.retain(|p| p.0 > 0.0 && p.0 >= SHAKE_PART_SHARE * most);
    parts.truncate(SHAKE_PARTS);
    if parts.len() < SHAKE_MIN_PARTS {
        return None;
    }

    // Along each direction, how the slope at an edge compares with the slope
    // r px further on (1..=reach), as a share of its own (1 at r = 0): the
    // mean over the parts. Each part's edges, the directions they face and
    // their slope along them are kept for a second look further on.
    let offset = |k: usize, r: usize| {
        let (c, s) = dirs[k];
        (r as f32 * s).round() as isize * w as isize + (r as f32 * c).round() as isize
    };
    let offsets: Vec<Vec<isize>> = (0..SHAKE_DIRS).map(|k| (1..=reach).map(|r| offset(k, r)).collect()).collect();
    let slope_at = |j: isize, (c, s): (f32, f32)| {
        let (a, b) = gradient(j as usize);
        a * c + b * s
    };
    let mut sums = vec![vec![0f64; reach]; SHAKE_DIRS];
    let mut counted = [0usize; SHAKE_DIRS];
    /// A part's edges facing each direction (index, slope along it), and
    /// their slopes' energy (0 where the direction doesn't count in the part).
    struct Seen {
        edges: Vec<Vec<(usize, f32)>>,
        own: [f64; SHAKE_DIRS],
    }
    let mut seen: Vec<Seen> = Vec::new();
    for (_, r) in &parts {
        let mut facing_edges = vec![Vec::new(); SHAKE_DIRS];
        for (i, a, b) in edges(*r, 2) {
            for (k, &d) in dirs.iter().enumerate() {
                if let Some(slope) = facing(a, b, d) {
                    facing_edges[k].push((i, slope));
                }
            }
        }
        let own: Vec<f64> = facing_edges.iter().map(|e| e.iter().map(|&(_, s)| (s * s) as f64).sum()).collect();
        let strongest = own.iter().copied().fold(0.0, f64::max);
        let mut own_counted = [0f64; SHAKE_DIRS];
        for k in 0..SHAKE_DIRS {
            if own[k] <= 0.0 || own[k] < SHAKE_DIR_SHARE * strongest {
                continue;
            }
            // One distance at a time, the edges in order: the picture is read in order.
            for (sum, &o) in sums[k].iter_mut().zip(&offsets[k]) {
                let along: f64 =
                    facing_edges[k].iter().map(|&(i, s)| (s * slope_at(i as isize + o, dirs[k])) as f64).sum();
                *sum += along / own[k];
            }
            own_counted[k] = own[k];
            counted[k] += 1;
        }
        seen.push(Seen { edges: facing_edges, own: own_counted });
    }
    let curves: Vec<Option<Vec<f32>>> = (0..SHAKE_DIRS)
        .map(|k| {
            (counted[k] >= SHAKE_DIR_PARTS).then(|| sums[k].iter().map(|s| (s / counted[k] as f64) as f32).collect())
        })
        .collect();

    // Doubled: how far the slope comes back up past its lowest since the
    // edge, less what comes back at twice that distance (a pattern).
    let rise = |c: &[f32]| {
        let (mut low, mut best, mut at) = (f32::MAX, f32::MIN, near);
        for r in 1..=reach {
            low = low.min(c[r - 1]);
            if r >= near && c[r - 1] - low.max(0.0) > best {
                (best, at) = (c[r - 1] - low.max(0.0), r);
            }
        }
        (best, at)
    };
    let mut doubled = 0f32;
    for (k, c) in curves.iter().enumerate() {
        let Some(c) = c else { continue };
        let (best, at) = rise(c);
        // Twice as far, give or take a px (an edge is a px or two wide).
        let again = (2 * at - 1..=2 * at + 1)
            .map(|r| {
                let twice = offset(k, r);
                seen.iter()
                    .filter(|part| part.own[k] > 0.0)
                    .map(|part| {
                        let sum: f64 = part.edges[k]
                            .iter()
                            .map(|&(i, s)| (s * slope_at(i as isize + twice, dirs[k])) as f64)
                            .sum();
                        sum / part.own[k]
                    })
                    .sum::<f64>()
                    / counted[k] as f64
            })
            .fold(0.0, f64::max);
        doubled = doubled.max(best - again as f32);
    }
    // Smeared: how much more the slope lasts along a direction than across it.
    let mut smeared = 0f32;
    for k in 0..SHAKE_DIRS {
        if let (Some(along), Some(across)) = (&curves[k], &curves[(k + SHAKE_DIRS / 2) % SHAKE_DIRS]) {
            let more: f32 = (from..=to).map(|r| along[r - 1] - across[r - 1].max(0.0)).sum();
            smeared = smeared.max(more / (to - from + 1) as f32);
        }
    }
    Some((doubled, smeared))
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

/// Measures a picture (upright, as shown), with the faces found in it and
/// the points its camera focused on ([`aimed`]); how shaken it looks too.
pub fn measure(img: &RgbImage, faces: &[FaceBox], points: &[Point]) -> Measures {
    measured(img, faces, points, true)
}

/// [`measure`] for a photo taken with a shutter slow enough for the hand to
/// shake the camera, or not ([`Shot::shaky`]): only then is how shaken it
/// looks measured (a quarter to a third more time; and only then does it count).
pub fn measure_taken(img: &RgbImage, faces: &[FaceBox], points: &[Point], slow_shutter: bool) -> Measures {
    Measures { slow_shutter, ..measured(img, faces, points, slow_shutter) }
}

fn measured(img: &RgbImage, faces: &[FaceBox], points: &[Point], shake: bool) -> Measures {
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
    if let Some((doubled, smeared)) = shake.then(|| shake_in(&fine)).flatten() {
        (m.doubled, m.smeared) = (Some(doubled), Some(smeared));
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

    // Where the camera focused, measured as a face is: the sharpest of its
    // points (focus found on one of them is focus found).
    let least = POINT_AREA * long as f32;
    m.focus_point = points
        .iter()
        .filter_map(|p| {
            let (cx, cy) = ((p.x + p.w * 0.5) * fw, (p.y + p.h * 0.5) * fh);
            let (rx, ry) = ((p.w * fw).max(least) * 0.5, (p.h * fh).max(least) * 0.5);
            let r = Rect {
                x0: (cx - rx).max(0.0) as usize,
                x1: (cx + rx).max(0.0) as usize,
                y0: (cy - ry).max(0.0) as usize,
                y1: (cy + ry).max(0.0) as usize,
            };
            blur_in(&fine, &coarse, r, FACE_SLOPE, FACE_EDGES)
        })
        .min_by(|a, b| a.total_cmp(b));
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
    /// Photos measured while the catalog didn't have their focus points
    /// (read again from an older photo's file since): measured again once it has.
    without_focus: RwLock<HashSet<i64>>,
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
                 close       TEXT,               -- Measures from the original, when Cull looked closer
                 focus       INTEGER NOT NULL DEFAULT 0  -- the catalog had its focus points when measured
             );",
        )?;
        // Measures from before Cull knew focus points: the catalog had none for them.
        if !conn.prepare("SELECT 1 FROM pragma_table_info('quality') WHERE name = 'focus'")?.exists([])? {
            conn.execute_batch("ALTER TABLE quality ADD COLUMN focus INTEGER NOT NULL DEFAULT 0")?;
        }
        let threads = (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) / 2).clamp(1, 6);
        Ok(Self {
            db: Mutex::new(conn),
            measured: RwLock::new(HashMap::new()),
            without_faces: RwLock::new(HashSet::new()),
            without_focus: RwLock::new(HashSet::new()),
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
        let mut without_focus = HashSet::new();
        let mut stmt = conn.prepare("SELECT photo_id, faces, preview, close, focus FROM quality WHERE version = ?1")?;
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
            if !r.get::<_, bool>(4)? {
                without_focus.insert(id);
            }
        }
        log::info!("cull: {} photos' measures read in {} ms", measured.len(), started.elapsed().as_millis());
        *self.measured.write().unwrap() = measured;
        *self.without_faces.write().unwrap() = without_faces;
        *self.without_focus.write().unwrap() = without_focus;
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

    /// Measures every photo of the catalog that needs it (`shoebox-cli
    /// quality`). The window only measures what Cull is asked about: see
    /// [`Self::measure`].
    pub fn index_pending(
        &self,
        cat: &Catalog,
        cancel: &AtomicBool,
        progress: &dyn Fn(&crate::search::AiProgress),
    ) -> Result<()> {
        self.ready.wait()?;
        let ready = cat.preview_ready()?;
        self.forget_stale(&ready)?;
        let ids: Vec<i64> = ready.into_iter().map(|(id, _)| id).collect();
        let started = std::time::Instant::now();
        let measured = self.measure(cat, &ids, cancel, &|done, total| {
            progress(&crate::search::AiProgress { task: "quality".into(), done, total, ..Default::default() })
        })?;
        if measured > 0 {
            log::info!("cull: {measured} photos measured in {:.1} s", started.elapsed().as_secs_f64());
        }
        Ok(())
    }

    /// Measures those of `ids` that need it, from their previews: not
    /// measured yet, or measured from another picture (turned since, or an id
    /// now another photo's), or before their faces were looked for or their
    /// focus points read. Returns how many were measured.
    pub fn measure(
        &self,
        cat: &Catalog,
        ids: &[i64],
        cancel: &AtomicBool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<usize> {
        self.ready.wait()?;
        let todo = self.pending(cat, ids)?;
        if todo.is_empty() {
            return Ok(0);
        }
        let total = todo.len() as u64;
        progress(0, total);
        let (mut measured, mut done) = (0, 0);
        for chunk in todo.chunks(64) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let ids: Vec<i64> = chunk.iter().map(|(id, _)| *id).collect();
            let (faces, points, looked) = {
                let db = cat.read()?;
                (faces_of(&db, &ids)?, points_of(&db, &ids)?, looked_of(&db, &ids)?)
            };
            let shaky = shaky_of(cat, &ids)?;
            let rows: Vec<Measured> = self.pool.install(|| {
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
                        let m = measure_taken(&img, of(&faces, id), of(&points, id), shaky.contains(id));
                        Some(Measured {
                            id: *id,
                            fingerprint: fp.clone(),
                            faces: looked.contains(id),
                            focus: points.contains_key(id),
                            m,
                        })
                    })
                    .collect()
            });
            self.store_previews(&rows)?;
            measured += rows.len();
            done += chunk.len() as u64;
            progress(done, total);
        }
        Ok(measured)
    }

    /// Those of `ids` [`Self::measure`] would measure (with a preview), in
    /// their order, with what their preview shows. Per photo, through
    /// indexes: a view costs what it shows.
    fn pending(&self, cat: &Catalog, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        let mut todo = Vec::new();
        for chunk in ids.chunks(500) {
            let (shown, looked, pointed) = {
                let db = cat.read()?;
                (fingerprints_of(&db, chunk)?, looked_of(&db, chunk)?, pointed_of(&db, chunk)?)
            };
            let stored: HashMap<i64, String> = self
                .db
                .lock()
                .unwrap()
                .prepare(
                    "SELECT photo_id, fingerprint FROM quality
                     WHERE version = ?1 AND photo_id IN (SELECT value FROM json_each(?2))",
                )?
                .query_map(params![QUALITY_VERSION, serde_json::to_string(chunk)?], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let measured = self.measured.read().unwrap();
            let without = self.without_faces.read().unwrap();
            let unpointed = self.without_focus.read().unwrap();
            todo.extend(chunk.iter().filter_map(|id| {
                let fp = shown.get(id)?;
                let fresh = measured.contains_key(id)
                    && stored.get(id) == Some(fp)
                    && !(without.contains(id) && looked.contains(id))
                    && !(unpointed.contains(id) && pointed.contains(id));
                (!fresh).then(|| (*id, fp.clone()))
            }));
        }
        Ok(todo)
    }

    /// Forgets measures that no longer fit their photo, catalog-wide: it left
    /// the catalog, was turned, or its id now belongs to another photo. Cheap
    /// (Cull's measures are of the shoots it was asked about), for after a
    /// scan.
    pub fn forget_gone(&self, cat: &Catalog) -> Result<usize> {
        self.ready.wait()?;
        self.forget_stale(&cat.preview_ready()?)
    }

    fn store_previews(&self, rows: &[Measured]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut closes = Vec::with_capacity(rows.len());
        {
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction()?;
            for r in rows {
                // A photo's closer look stays while it is of the same picture:
                // it doesn't depend on the preview.
                let close: Option<String> = tx.query_row(
                    "INSERT INTO quality (photo_id, fingerprint, version, faces, focus, preview)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT (photo_id) DO UPDATE SET fingerprint = ?2, version = ?3, faces = ?4, focus = ?5,
                         preview = ?6, close = CASE WHEN fingerprint = ?2 AND version = ?3 THEN close END
                     RETURNING close",
                    params![r.id, r.fingerprint, QUALITY_VERSION, r.faces, r.focus, serde_json::to_string(&r.m)?],
                    |row| row.get(0),
                )?;
                closes.push(close.and_then(|c| serde_json::from_str(&c).ok()));
            }
            tx.commit()?;
        }
        let mut measured = self.measured.write().unwrap();
        let mut without = self.without_faces.write().unwrap();
        let mut unpointed = self.without_focus.write().unwrap();
        for (r, close) in rows.iter().zip(closes) {
            measured.insert(r.id, Quality { preview: r.m, close });
            for (set, has) in [(&mut *without, r.faces), (&mut *unpointed, r.focus)] {
                if has {
                    set.remove(&r.id);
                } else {
                    set.insert(r.id);
                }
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
        let mut unpointed = self.without_focus.write().unwrap();
        for id in &gone {
            measured.remove(id);
            without.remove(id);
            unpointed.remove(id);
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
            let points = points_of(&cat.db(), chunk)?;
            let fingerprints = fingerprints_of(&cat.db(), chunk)?;
            let shaky = shaky_of(cat, chunk)?;
            let results: Vec<(i64, std::result::Result<Measures, String>)> = self.pool.install(|| {
                chunk
                    .par_iter()
                    .with_max_len(1)
                    .map(|id| (*id, closer_look(cat, *id, of(&faces, id), of(&points, id), shaky.contains(id))))
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
            self.store_close(cat, &found, &fingerprints, &faces, &points)?;
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
        points: &HashMap<i64, Vec<Point>>,
    ) -> Result<()> {
        // A photo not measured from its preview yet is now, so it has a row.
        let missing: Vec<Measured> = {
            let measured = self.measured.read().unwrap();
            found
                .iter()
                .filter(|(id, _)| !measured.contains_key(id))
                .filter_map(|(id, close)| {
                    let img = image::open(cat.data.preview_path(*id)).ok()?.to_rgb8();
                    let m = measure_taken(&img, of(faces, id), of(points, id), close.slow_shutter);
                    Some(Measured {
                        id: *id,
                        fingerprint: fingerprints.get(id)?.clone(),
                        faces: faces.contains_key(id),
                        focus: points.contains_key(id),
                        m,
                    })
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
        let (faces, mut points) = {
            let db = cat.read()?;
            (faces_of(&db, &list.ids)?, points_of(&db, &list.ids)?)
        };
        let vectors = search.vectors(&list.ids);
        let to_measure = self.pending(cat, &list.ids)?.into_iter().map(|(id, _)| id).collect();
        let measured = self.measured.read().unwrap();
        let frames: Vec<FrameIn> = list
            .ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let info = info.get(id).cloned().unwrap_or_default();
                let points = points.remove(id).unwrap_or_default();
                FrameIn {
                    id: *id,
                    taken: list.taken[i],
                    camera: info.camera.clone(),
                    vector: vectors.get(id).cloned(),
                    quality: measured.get(id).copied(),
                    shot: Shot::of(&info),
                    row: FrameRow {
                        id: *id,
                        name: info.name.clone(),
                        taken: list.taken[i],
                        state: list.state[i],
                        w: list.w[i],
                        h: list.h[i],
                        rev: list.rev[i],
                        marked: list.marked[i] == 1,
                        focus: first_look(&points, of(&faces, id)),
                        focus_points: points,
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
            to_measure,
        })
    }
}

/// A photo's original, read at half its size (whole when small, read-only),
/// and measured; or why it couldn't be.
fn closer_look(
    cat: &Catalog,
    id: i64,
    faces: &[FaceBox],
    points: &[Point],
    slow_shutter: bool,
) -> std::result::Result<Measures, String> {
    let (path, kind, turn) = cat.original(id).map_err(|e| format!("{e:#}"))?;
    let full = cat.full_size(id);
    let long = full.map_or(3000, |l| if l <= CLOSE_WHOLE { l } else { l / 2 });
    let loaded = crate::imaging::load(&path, kind, long);
    let full = full.or(loaded.meta.width.max(loaded.meta.height));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Turned as its preview is: the faces' boxes and the focus points are on the photo as it stands.
    let img = crate::imaging::turned(loaded.image.map_err(|e| format!("{name}: {e}"))?, turn).to_rgb8();
    let mut m = measure_taken(&img, faces, points, slow_shutter);
    m.full = full.filter(|&f| f >= m.long);
    Ok(m)
}

/// Where to look at 100% first (0..1 of the picture): where the camera
/// focused, the middle of the point nearest the middle of them all (on a
/// point, not between two); else the middle of the largest face.
fn first_look(points: &[Point], faces: &[FaceBox]) -> Option<[f32; 2]> {
    let mid = |p: &Point| [p.x + p.w * 0.5, p.y + p.h * 0.5];
    let n = points.len() as f32;
    let [cx, cy] = points.iter().map(mid).fold([0.0, 0.0], |[x, y], [a, b]| [x + a / n, y + b / n]);
    let far = |[x, y]: &[f32; 2]| (x - cx) * (x - cx) + (y - cy) * (y - cy);
    let point = points.iter().map(mid).min_by(|a, b| far(a).total_cmp(&far(b)));
    point.map(|[x, y]| [x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)]).or_else(|| {
        let f = faces.iter().max_by(|a, b| (a.x2 - a.x1).total_cmp(&(b.x2 - b.x1)))?;
        Some([(f.x1 + f.x2) * 0.5, (f.y1 + f.y2) * 0.5])
    })
}

/// The points a camera focused on: those it found focus at, else those it
/// was given (chosen). None when it says neither, or was focused by hand.
pub fn aimed(f: &Focus) -> Vec<Point> {
    if f.mode == Some("Manual focus") {
        return Vec::new();
    }
    let of = |state| f.points.iter().copied().filter(|p| p.state == state).collect::<Vec<_>>();
    let found = of(State::InFocus);
    if found.is_empty() { of(State::Chosen) } else { found }
}

/// Where the camera focused in these photos, as each is shown ([`aimed`]):
/// every photo whose file said something about focus, even if not where.
fn points_of(db: &Connection, ids: &[i64]) -> Result<HashMap<i64, Vec<Point>>> {
    let mut out = HashMap::new();
    let mut stmt = db.prepare(
        "SELECT id, focus, turn FROM photos WHERE focus IS NOT NULL AND id IN (SELECT value FROM json_each(?1))",
    )?;
    let mut rows = stmt.query([serde_json::to_string(ids)?])?;
    while let Some(r) = rows.next()? {
        let (blob, turn): (Vec<u8>, i64) = (r.get(1)?, r.get(2)?);
        let focus = crate::focus::decode(db, &blob)?.map(|f| f.turned(turn));
        out.insert(r.get(0)?, focus.as_ref().map(aimed).unwrap_or_default());
    }
    Ok(out)
}

/// Those of these photos whose shutter was slow enough for the hand to shake ([`Shot::shaky`]).
fn shaky_of(cat: &Catalog, ids: &[i64]) -> Result<HashSet<i64>> {
    Ok(cat.shot_info(ids)?.into_iter().filter(|(_, info)| Shot::of(info).shaky()).map(|(id, _)| id).collect())
}

/// A photo's own in a map of photos' things (none if it has none).
fn of<'a, T>(map: &'a HashMap<i64, Vec<T>>, id: &i64) -> &'a [T] {
    map.get(id).map(Vec::as_slice).unwrap_or(&[])
}

/// A photo measured from its preview, as `quality.db` keeps it.
struct Measured {
    id: i64,
    /// What its preview showed ([`Catalog::preview_ready`]).
    fingerprint: String,
    /// Its faces had been looked for.
    faces: bool,
    /// The catalog had its focus points.
    focus: bool,
    m: Measures,
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

/// What each photo's preview shows, as `Catalog::preview_ready` says it.
fn fingerprints_of(db: &Connection, ids: &[i64]) -> Result<HashMap<i64, String>> {
    let mut stmt = db.prepare(&format!(
        "SELECT p.id, {} FROM photos p WHERE p.preview_state = {PREVIEW_OK} AND p.id IN (SELECT value FROM json_each(?1))",
        crate::db::SHOWN_SQL
    ))?;
    let out = stmt
        .query_map([serde_json::to_string(ids)?], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

/// Those of these photos already looked at for faces.
fn looked_of(db: &Connection, ids: &[i64]) -> Result<HashSet<i64>> {
    let out = db
        .prepare("SELECT photo_id FROM face_scan WHERE photo_id IN (SELECT value FROM json_each(?1))")?
        .query_map([serde_json::to_string(ids)?], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

/// Those of these photos whose focus points the catalog has.
fn pointed_of(db: &Connection, ids: &[i64]) -> Result<HashSet<i64>> {
    let out = db
        .prepare("SELECT id FROM photos WHERE focus IS NOT NULL AND id IN (SELECT value FROM json_each(?1))")?
        .query_map([serde_json::to_string(ids)?], |r| r.get(0))?
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
    /// Photos to measure from their previews ([`QualityIndex::measure`]):
    /// not measured yet, or to measure again. The shoot is asked for again
    /// once they are.
    #[serde(skip)]
    pub to_measure: Vec<i64>,
}

/// Frames of the same shot, in the order they were taken.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    pub frames: Vec<FrameRow>,
    /// The frame Cull would keep (an index into `frames`).
    pub keeper: usize,
    /// Where its frames' sharpness was compared (each frame's `blur`).
    pub compared: Compared,
    /// Compared on what the originals showed: every frame was looked at closer.
    pub closer: bool,
}

/// Where the frames of a stack are compared, to find the one to keep: where
/// the subject is, as far as Cull can tell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Compared {
    /// Around the eyes of the largest face: every frame has one measured.
    Face,
    /// Where the camera focused: every frame's file says, and the point is
    /// sharp in one of them at least ([`Compared::of`]).
    FocusPoint,
    /// The frame's sharpest part.
    #[default]
    Sharpest,
}

impl Compared {
    /// Where frames measured as `looks` (one look each, the same kind for
    /// all) are compared: on their faces when every one has one measured,
    /// else where the camera focused when every one's file says, else on
    /// their sharpest part.
    ///
    /// The camera's point counts only when it is sharp in one of the frames
    /// at least: within [`FACE_VS_SHARPEST`] of the sharpest part of any of
    /// them. Softer than that in all of them, it isn't where the subject is:
    /// a photographer who focused, then turned the camera to frame the shot,
    /// leaves the point where the subject was, now on the background. Frames
    /// compared there would differ by chance, and one softer where the
    /// subject is could be kept.
    fn of(looks: &[Option<Measures>]) -> Self {
        let every = |at: Compared| looks.iter().all(|m| m.as_ref().is_some_and(|m| at.blur(m).is_some()));
        if every(Compared::Face) {
            return Compared::Face;
        }
        let least = |at: Compared| looks.iter().flatten().filter_map(|m| at.blur(m)).min_by(f32::total_cmp);
        let sharp_somewhere = match (least(Compared::FocusPoint), least(Compared::Sharpest)) {
            (Some(point), Some(sharpest)) => point <= FACE_VS_SHARPEST * sharpest,
            _ => false,
        };
        if every(Compared::FocusPoint) && sharp_somewhere { Compared::FocusPoint } else { Compared::Sharpest }
    }

    /// Edge blur there, in thousandths of the long edge.
    fn blur(self, m: &Measures) -> Option<f32> {
        m.rel(match self {
            Compared::Face => m.face,
            Compared::FocusPoint => m.focus_point,
            Compared::Sharpest => m.sharpest,
        })
    }
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
    /// (lower is sharper): at the face, where the camera focused or the
    /// sharpest part, as the stack's `compared` says. None: not measured yet.
    pub blur: Option<f32>,
    /// Measured from the original, not only the preview.
    pub closer: bool,
    /// How alike CLIP finds it and the frame before (0..1).
    pub alike: Option<f32>,
    /// How alike its layout and the frame before's are (-1..1).
    pub framing: Option<f32>,
    /// Where to look at 100% first (0..1 of the picture): where the camera
    /// focused, when its file says, else the middle of its largest face.
    pub focus: Option<[f32; 2]>,
    /// The points the camera focused on (as the photo is shown; see
    /// [`aimed`]): `focus` is on one of them. Empty when its file doesn't say.
    pub focus_points: Vec<Point>,
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
    // Frames compare on their faces, else where the camera focused, else on
    // their sharpest part ([`Compared::of`]); on the originals when each was
    // looked at closer.
    let all_close = frames.iter().all(|f| f.quality.is_some_and(|q| q.close.is_some()));
    let looks: Vec<Option<Measures>> =
        frames.iter().map(|f| f.quality.map(|q| if all_close { *q.best() } else { q.preview })).collect();
    let compared = Compared::of(&looks);
    let blur: Vec<Option<f32>> = looks.iter().map(|m| m.as_ref().and_then(|m| compared.blur(m))).collect();
    // Shake short of a problem counts too: of frames all shaken a little,
    // the steadiest is kept.
    let shake: Vec<f32> = looks.iter().map(|m| m.as_ref().map_or(0.0, |m| m.shake(all_close).min(1.0))).collect();
    let score = |i: usize| -> f32 {
        let major = found[i].iter().filter(|p| p.problem.major()).count() as f32;
        let minor = found[i].len() as f32 - major;
        let blown = frames[i].quality.map_or(0.0, |q| q.preview.blown);
        blur[i].unwrap_or(5.0) + 5.0 * major + 0.5 * minor + 2.0 * blown + shake[i]
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
    Stack { frames: rows, keeper, compared, closer: all_close }
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
        let shot = Shot::of(&info);
        let problems = quality.map(|q| problems(&q, &shot)).unwrap_or_default();
        Ok(Report { id, quality, problems })
    }

    /// A photo's details with what Cull found wrong with it: a camera that
    /// confirmed focus says nothing of shake or motion after. A photo not
    /// measured yet is measured first (one preview, a few ms). Never waits for
    /// the measures to be read (nothing until then, as the grid's badges).
    pub fn photo_detail(&self, cat: &Catalog, id: i64) -> Result<PhotoDetail> {
        if self.ready.is_open()
            && let Err(e) = self.measure(cat, &[id], &AtomicBool::new(false), &|_, _| {})
        {
            log::warn!("cull: photo {id}: not measured: {e:#}");
        }
        let mut d = cat.photo_detail(id)?;
        let shot = Shot { exposure: d.exposure.clone(), focal: d.focal };
        d.problems = self.get(id).map(|q| problems(&q, &shot)).unwrap_or_default();
        Ok(d)
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
        let sharp = measure(&squares(480, 320, 0.0), &[], &[]);
        assert!(sharp.sharpest.unwrap() < 0.3, "sharp edges: {sharp:?}");
        for sigma in [1.0f32, 2.0, 3.0] {
            let m = measure(&squares(480, 320, sigma), &[], &[]);
            let got = m.sharpest.unwrap();
            assert!((got - sigma).abs() < 0.35 * sigma, "blurred by {sigma}: measured {got}");
        }
        // Half the contrast, the same blur.
        let faint = RgbImage::from_fn(480, 320, |x, y| {
            if (x / 24 + y / 24).is_multiple_of(2) { Rgb([110; 3]) } else { Rgb([175; 3]) }
        });
        let (a, b) = (
            measure(&image::imageops::blur(&faint, 2.0), &[], &[]).sharpest,
            measure(&squares(480, 320, 2.0), &[], &[]).sharpest,
        );
        assert!((a.unwrap() - b.unwrap()).abs() < 0.3, "{a:?} vs {b:?}");
    }

    #[test]
    fn shake_in_one_direction_counts() {
        let m = measure(&moved(&squares(480, 320, 0.0), 6), &[], &[]);
        assert!(m.sharpest.unwrap() > 2.0, "{m:?}");
    }

    /// Tiles of `side` px, each its own grey: edges everywhere, but no
    /// pattern (squares of two greys repeat themselves diagonally).
    fn tiles(w: u32, h: u32, side: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| {
            let n = (x / side).wrapping_mul(73_856_093) ^ (y / side).wrapping_mul(19_349_663);
            Rgb([(40 + n.wrapping_mul(2_654_435_761) % 170) as u8; 3])
        })
    }

    /// `img` as a camera shaken during the exposure leaves it: copies of it
    /// added together, each `(weight, dx, dy)`, `dx, dy` px away.
    fn ghosted(img: &RgbImage, copies: &[(f32, i32, i32)]) -> RgbImage {
        let (w, h) = (img.width() as i32, img.height() as i32);
        RgbImage::from_fn(img.width(), img.height(), |x, y| {
            let mut sum = [0f32; 3];
            for &(weight, dx, dy) in copies {
                let p = img.get_pixel((x as i32 - dx).clamp(0, w - 1) as u32, (y as i32 - dy).clamp(0, h - 1) as u32);
                for (s, v) in sum.iter_mut().zip(p.0) {
                    *s += weight * v as f32;
                }
            }
            Rgb(sum.map(|s| s.round() as u8))
        })
    }

    /// Measured as a closer look at a photo taken with a slow shutter (or not).
    fn looked_closer(img: &RgbImage, slow_shutter: bool) -> Quality {
        let preview = Measures { slow_shutter, ..measure(&squares(1080, 720, 0.0), &[], &[]) };
        Quality { preview, close: Some(Measures { slow_shutter, ..measure(img, &[], &[]) }) }
    }

    #[test]
    fn copies_left_by_a_shaken_camera_are_found_where_edge_blur_sees_none() {
        // At 100%: the picture twice, the second copy fainter, 13 px away.
        let sharp = tiles(2400, 1600, 30);
        let shaken = ghosted(&sharp, &[(0.7, 0, 0), (0.3, 12, 5)]);
        let (s, g) = (measure(&sharp, &[], &[]), measure(&shaken, &[], &[]));
        // Each copy has crisp edges: as sharp as the picture without shake.
        assert!(g.sharpest.unwrap() < 0.6, "{g:?}");
        assert!(s.doubled.unwrap() < 0.05 && s.smeared.unwrap() < 0.05, "{s:?}");
        assert!(g.doubled.unwrap() >= DOUBLED_CLOSE, "{g:?}");
        let q = looked_closer(&shaken, true);
        let shot = Shot { exposure: Some("1s".into()), focal: Some(35.0) };
        let found = problems(&q, &shot);
        assert_eq!(found.iter().map(|f| f.problem).collect::<Vec<_>>(), [Problem::Soft], "{found:?}");
        assert_eq!(found[0].text, "Shaken at 100%: the picture is doubled (1 s at 35 mm is slow for a steady hand)");
        // Copies anywhere (three, two directions) are as doubled.
        let thrice = ghosted(&sharp, &[(0.5, 0, 0), (0.3, 10, 8), (0.2, 26, 12)]);
        assert!(measure(&thrice, &[], &[]).doubled.unwrap() >= DOUBLED_CLOSE);
        // Taken at 1/500, the hand can't have shaken it: a pattern, a design.
        assert!(found_in(&looked_closer(&shaken, false)).is_empty());
        assert!(found_in(&looked_closer(&sharp, true)).is_empty());
    }

    #[test]
    fn a_smear_one_way_is_found_and_patterns_are_not_copies() {
        let smeared = measure(&moved(&tiles(1080, 720, 24), 6), &[], &[]);
        assert!(smeared.smeared.unwrap() >= SMEARED, "{smeared:?}");
        // Blurred all round (out of focus): not smeared one way.
        let soft = measure(&image::imageops::blur(&tiles(1080, 720, 24), 3.0), &[], &[]);
        assert!(soft.smeared.unwrap() < 0.1, "{soft:?}");
        // Stripes repeat every 12 px: they come back at twice that too, a copy doesn't.
        let stripes = RgbImage::from_fn(2400, 1600, |x, _| if x % 12 < 6 { Rgb([60; 3]) } else { Rgb([190; 3]) });
        let m = measure(&stripes, &[], &[]);
        assert!(m.doubled.unwrap() < 0.1, "{m:?}");
        // Nothing with edges: nothing to tell.
        assert_eq!(measure(&RgbImage::from_pixel(1080, 720, Rgb([128; 3])), &[], &[]).doubled, None);
    }

    fn found_in(q: &Quality) -> Vec<Problem> {
        problems(q, &Shot::default()).iter().map(|f| f.problem).collect()
    }

    #[test]
    fn of_frames_all_shaken_a_little_the_steadiest_is_kept() {
        // Equally sharp edges; the second frame's copies faint, below a problem.
        let shaken = |id: i64, doubled: f32| {
            let mut f = frame(id, 100 + id, None, 0.4);
            let q = f.quality.as_mut().unwrap();
            (q.preview.slow_shutter, q.preview.doubled) = (true, Some(doubled));
            f
        };
        let s = &suggest(vec![shaken(1, 0.4), shaken(2, 0.1), shaken(3, 0.3)])[0];
        assert_eq!(s.keeper, 1, "{:?}", s.frames);
        assert!(s.frames.iter().all(|f| f.problems.is_empty()));
        // Doubled enough on screen: blurry, rejected.
        let s = &suggest(vec![shaken(1, 0.6), shaken(2, 0.1)])[0];
        assert_eq!(s.frames[0].problems, [Problem::Blurry]);
        assert_eq!((s.keeper, s.frames[0].suggest), (1, Suggest::Reject));
    }

    #[test]
    fn a_shallow_focus_photo_is_judged_by_its_sharp_part() {
        // Sharp on the left third, very blurred elsewhere: bokeh, not a blurry photo.
        let sharp = squares(480, 320, 0.0);
        let soft = squares(480, 320, 4.0);
        let img =
            RgbImage::from_fn(480, 320, |x, y| if x < 160 { *sharp.get_pixel(x, y) } else { *soft.get_pixel(x, y) });
        let m = measure(&img, &[], &[]);
        assert!(m.sharpest.unwrap() < 0.5 && m.typical.unwrap() > 2.5, "{m:?}");
        let q = Quality { preview: m, close: None };
        assert!(problems(&q, &Shot::default()).is_empty());
    }

    #[test]
    fn problems_of_typical_frames() {
        // 1080 px previews.
        let shot = Shot { exposure: Some("1/15".into()), focal: Some(200.0) };
        let blurry = Quality { preview: measure(&squares(1080, 720, 2.5), &[], &[]), close: None };
        let found = problems(&blurry, &shot);
        assert_eq!(found.iter().map(|f| f.problem).collect::<Vec<_>>(), [Problem::Blurry]);
        assert!(found[0].text.contains("1/15 s at 200 mm"), "{found:?}");

        let black = Quality { preview: measure(&RgbImage::from_pixel(1080, 720, Rgb([2; 3])), &[], &[]), close: None };
        assert_eq!(problems(&black, &Shot::default())[0].problem, Problem::Empty);

        let sharp = Quality { preview: measure(&squares(1080, 720, 0.0), &[], &[]), close: None };
        assert!(problems(&sharp, &Shot::default()).is_empty());
        // Fine on screen, soft at 100%: only the original says so.
        let soft_close = Quality { preview: sharp.preview, close: Some(measure(&squares(3000, 2000, 2.0), &[], &[])) };
        assert_eq!(problems(&soft_close, &Shot::default())[0].problem, Problem::Soft);
    }

    /// A 1080 x 720 preview, sharp but for a patch `x0..x1, y0..y1` (0..1 of
    /// the picture) whose own edges are blurred.
    fn soft_patch((x0, y0, x1, y1): (f32, f32, f32, f32)) -> RgbImage {
        let sharp = squares(1080, 720, 0.0);
        let soft = squares(1080, 720, 5.0);
        RgbImage::from_fn(1080, 720, |x, y| {
            let (fx, fy) = (x as f32 / 1080.0, y as f32 / 720.0);
            if fx > x0 && fx < x1 && fy > y0 && fy < y1 { *soft.get_pixel(x, y) } else { *sharp.get_pixel(x, y) }
        })
    }

    fn found(q: &Quality) -> Vec<Problem> {
        problems(q, &Shot::default()).iter().map(|f| f.problem).collect()
    }

    #[test]
    fn a_face_out_of_focus_is_found() {
        // Sharp background; a smooth face-sized patch whose own edges are blurred.
        let face = FaceBox { x1: 0.4, y1: 0.3, x2: 0.6, y2: 0.7, kps: None };
        let img = soft_patch((0.38, 0.28, 0.62, 0.72));
        let q = Quality { preview: measure(&img, std::slice::from_ref(&face), &[]), close: None };
        assert_eq!(found(&q), [Problem::FaceSoft], "{:?}", q.preview);
        // The same face in focus: nothing to say.
        let ok = Quality { preview: measure(&squares(1080, 720, 0.0), &[face], &[]), close: None };
        assert!(problems(&ok, &Shot::default()).is_empty());
    }

    /// One of the camera's points, 0.04 x 0.06 of the picture, its middle at (`x`, `y`).
    fn point(x: f32, y: f32, state: State) -> Point {
        Point { x: x - 0.02, y: y - 0.03, w: 0.04, h: 0.06, state }
    }

    #[test]
    fn a_focus_point_out_of_focus_is_found() {
        // The camera found focus in the middle of a soft patch; the rest is sharp.
        let img = soft_patch((0.6, 0.3, 0.9, 0.7));
        let missed = Quality { preview: measure(&img, &[], &[point(0.75, 0.5, State::InFocus)]), close: None };
        assert_eq!(found(&missed), [Problem::FocusMissed], "{:?}", missed.preview);
        assert_eq!(
            problems(&missed, &Shot::default())[0].text,
            "Focus missed: the camera focused here, but the sharpest part is elsewhere"
        );
        // Focus found on the sharp part, or on one of two points: nothing to say.
        let sharp_point = point(0.25, 0.5, State::InFocus);
        for points in [vec![sharp_point], vec![point(0.75, 0.5, State::InFocus), sharp_point]] {
            let q = Quality { preview: measure(&img, &[], &points), close: None };
            assert!(found(&q).is_empty(), "{points:?}: {:?}", q.preview);
        }
        // A face decides over the camera's point: here it is sharp, elsewhere.
        let face = FaceBox { x1: 0.1, y1: 0.3, x2: 0.3, y2: 0.7, kps: None };
        let q = Quality { preview: measure(&img, &[face], &[point(0.75, 0.5, State::InFocus)]), close: None };
        assert!(q.preview.face.is_some() && q.preview.focus_point.is_some(), "{:?}", q.preview);
        assert!(found(&q).is_empty(), "{:?}", q.preview);
        // A closer look from before Cull knew the points: the preview's measure of them counts.
        let sharp_close = measure(&squares(3000, 2000, 0.0), &[], &[]);
        let q = Quality { preview: missed.preview, close: Some(sharp_close) };
        assert_eq!(found(&q), [Problem::FocusMissed]);
        // Shown, not enough alone to suggest rejecting: a single frame is left to the user.
        assert!(!Problem::FocusMissed.major());
        let mut f = frame(1, 100, None, 0.3);
        f.quality = Some(missed);
        let stacks = suggest(vec![f]);
        assert_eq!(stacks[0].frames[0].suggest, Suggest::None);
        assert_eq!(stacks[0].frames[0].problems, [Problem::FocusMissed]);
    }

    #[test]
    fn the_first_look_at_100_percent_is_where_the_camera_focused() {
        let near = |got: Option<[f32; 2]>, want: [f32; 2]| {
            got.is_some_and(|[x, y]| (x - want[0]).abs() < 1e-4 && (y - want[1]).abs() < 1e-4)
        };
        let face = FaceBox { x1: 0.6, y1: 0.6, x2: 0.8, y2: 0.8, kps: None };
        let faces = std::slice::from_ref(&face);
        // Without points, the largest face; with one, the point.
        assert!(near(first_look(&[], faces), [0.7, 0.7]));
        assert!(near(first_look(&[point(0.2, 0.3, State::InFocus)], faces), [0.2, 0.3]));
        // Points apart: on the one nearest their middle, not between them.
        let row = [point(0.1, 0.5, State::InFocus), point(0.45, 0.5, State::InFocus), point(0.95, 0.5, State::InFocus)];
        assert!(near(first_look(&row, &[]), [0.45, 0.5]));
        assert_eq!(first_look(&[], &[]), None);

        // The points it found focus at, else the ones it was given; nothing when focused by hand.
        let focus = |mode, points: &[Point]| Focus { mode, points: points.to_vec() };
        let (other, chosen, in_focus) =
            (point(0.1, 0.1, State::Other), point(0.3, 0.3, State::Chosen), point(0.5, 0.5, State::InFocus));
        let zone = Point { x: 0.2, y: 0.2, w: 0.6, h: 0.6, state: State::Area };
        assert_eq!(aimed(&focus(Some("Single point"), &[other, chosen, in_focus, zone])), [in_focus]);
        assert_eq!(aimed(&focus(None, &[other, chosen, zone])), [chosen]);
        assert!(aimed(&focus(None, &[other, zone])).is_empty());
        assert!(aimed(&focus(Some("Manual focus"), &[chosen])).is_empty());
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

    /// Frame `id` of a burst: its sharpest part, and where the camera focused
    /// when its file says (edge blur, thousandths of the long edge).
    fn focused(id: i64, sharpest: f32, point: Option<f32>) -> FrameIn {
        let mut f = frame(id, 100 + id, None, sharpest);
        f.quality.as_mut().unwrap().preview.focus_point = point;
        f
    }

    #[test]
    fn a_burst_is_compared_where_the_camera_focused() {
        // The subject sharp where the camera focused; then a frame whose focus
        // jumped to the background, sharper there than anything in the first.
        let burst = || vec![focused(1, 0.5, Some(0.5)), focused(2, 0.3, Some(1.6))];
        let s = &suggest(burst())[0];
        assert_eq!((s.compared, s.keeper), (Compared::FocusPoint, 0));
        assert!(s.frames[1].blur.is_some_and(|b| (b - 1.6).abs() < 1e-4), "{:?}", s.frames[1]);
        assert_eq!(s.frames[1].reasons[0], "Same shot: frame 1 is better");
        // A frame whose file doesn't say: their sharpest parts decide.
        let s = &suggest(vec![focused(1, 0.5, Some(0.5)), focused(2, 0.3, None)])[0];
        assert_eq!((s.compared, s.keeper), (Compared::Sharpest, 1));
        // Faces first: every frame has one measured, the sharper face is kept.
        let mut frames = burst();
        for (f, face) in frames.iter_mut().zip([1.4, 0.9]) {
            f.quality.as_mut().unwrap().preview.face = Some(face);
        }
        let s = &suggest(frames)[0];
        assert_eq!((s.compared, s.keeper), (Compared::Face, 1));
        // Every frame looked at closer: compared where the originals show the point.
        let closer = |points: [Option<f32>; 2]| {
            let mut frames = burst();
            for (f, point) in frames.iter_mut().zip(points) {
                let q = f.quality.as_mut().unwrap();
                q.close = Some(Measures { long: 3000, sharpest: Some(1.2), focus_point: point, ..q.preview });
            }
            suggest(frames).remove(0)
        };
        let s = closer([Some(2.0), Some(0.8)]);
        assert_eq!((s.compared, s.closer, s.keeper), (Compared::FocusPoint, true, 1));
        // Looked at closer before Cull knew the points: the originals' sharpest parts.
        let s = closer([None, None]);
        assert_eq!((s.compared, s.closer), (Compared::Sharpest, true));
        assert!(!suggest(burst())[0].closer);
    }

    #[test]
    fn a_point_left_on_the_background_doesnt_choose_the_keeper() {
        // Focus and recompose: the camera's point stays where the subject was,
        // on the soft background now, in every frame; a little less soft, by
        // chance, in the frame where the subject moved. The sharpest part
        // decides, and the sharper subject is kept.
        let s = &suggest(vec![focused(1, 0.3, Some(1.4)), focused(2, 0.7, Some(1.3))])[0];
        assert_eq!((s.compared, s.keeper), (Compared::Sharpest, 0));
        // The same when the point is soft enough for Focus missed in each.
        let s = &suggest(vec![focused(1, 0.3, Some(3.0)), focused(2, 0.7, Some(2.8))])[0];
        assert_eq!((s.compared, s.keeper), (Compared::Sharpest, 0));
        assert!(s.frames.iter().all(|f| f.problems == [Problem::FocusMissed]), "{:?}", s.frames);
        // A frame shaken all over has its point barely softer than its own
        // sharpest part: that doesn't make the point the subject either.
        let s = &suggest(vec![focused(1, 0.3, Some(1.5)), focused(2, 0.9, Some(1.45))])[0];
        assert_eq!((s.compared, s.keeper), (Compared::Sharpest, 0));
        // Sharp in one frame, the point is where the subject is.
        let s = &suggest(vec![focused(1, 0.3, Some(2.5)), focused(2, 0.45, Some(0.55))])[0];
        assert_eq!((s.compared, s.keeper), (Compared::FocusPoint, 1));

        // Measured: the subject on the left, sharp in the first frame and
        // softer in the next (it moved); the point on the right, on the
        // background, blurred in both and a little less in the second. Compared
        // there, the second frame would be kept.
        let shot = |subject: f32, background: f32| {
            let (s, b) = (squares(1080, 720, subject), squares(1080, 720, background));
            RgbImage::from_fn(1080, 720, |x, y| if x < 360 { *s.get_pixel(x, y) } else { *b.get_pixel(x, y) })
        };
        // The camera focused at `x` (0..1 of the width).
        let measured = |id: i64, img: RgbImage, x: f32| {
            let mut f = frame(id, 100 + id, None, 0.0);
            let preview = measure(&img, &[], &[point(x, 0.5, State::InFocus)]);
            f.quality = Some(Quality { preview, close: None });
            f
        };
        let s = &suggest(vec![measured(1, shot(0.6, 4.0), 0.75), measured(2, shot(1.0, 3.5), 0.75)])[0];
        assert_eq!((s.frames.len(), s.compared, s.keeper), (2, Compared::Sharpest, 0), "{:?}", s.frames);
        // The point on the subject, in focus in the first frame; in the next
        // the focus jumped to the background, now sharper than the subject
        // was. Compared on their sharpest parts, the second would be kept.
        let s = &suggest(vec![measured(1, shot(0.8, 4.0), 0.17), measured(2, shot(2.0, 0.6), 0.17)])[0];
        assert_eq!((s.frames.len(), s.compared, s.keeper), (2, Compared::FocusPoint, 0), "{:?}", s.frames);
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
        // Shake is possible from 1/focal on; unknown without both.
        let shaky = |e: Option<&str>, f: Option<f64>| Shot { exposure: e.map(String::from), focal: f }.shaky();
        assert!(shaky(Some("1/50"), Some(50.0)) && shaky(Some("2s"), Some(17.0)));
        assert!(!shaky(Some("1/125"), Some(35.0)) && !shaky(Some("1s"), None) && !shaky(None, Some(35.0)));
    }
}
