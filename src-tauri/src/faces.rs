//! Faces. Found with SCRFD and described with ArcFace (InsightFace's buffalo_l,
//! Immich's export), grouped into people. Runs on Shoebox's previews, so photos on
//! unplugged disks get faces too.
//!
//! Names and groupings live in the catalog (the user's work, backed up). The face
//! fingerprints live in `faces.db` (rebuildable, kept out of backups).

use crate::catalog::Catalog;
use anyhow::{Result, anyhow, bail};
use chrono::Datelike;
use image::{DynamicImage, GenericImageView, RgbImage, imageops::FilterType};
use ort::session::Session;
use ort::value::Tensor;
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub const FACE_MODEL: &str = "buffalo_l";
const DET_SIZE: u32 = 640;
/// Immich's default minimum detection score.
const DET_THRESHOLD: f32 = 0.7;
const NMS_IOU: f32 = 0.4;
/// Faces smaller than this in the preview are too blurry to recognise.
const MIN_FACE_PX: f32 = 24.0;
/// Two faces are the same person when their fingerprints are at least this
/// similar (Immich: maximum distance 0.5).
const SAME_PERSON: f32 = 0.5;
/// A new face is compared with each person as they looked around the date of
/// its photo: their faces from the nearest months holding at least this
/// many...
const AROUND_FACES: u32 = 5;
/// ...no further than this many months either side; otherwise with their
/// whole-life average.
const AROUND_MONTHS: i64 = 12;
/// Less alike than the same person, but enough to suggest who a face may be.
const LOOKS_LIKE: f32 = 0.3;
/// Faces smaller than this (short side, in the preview) give fingerprints too
/// weak to group automatically. They are kept, ungrouped, for a later pass on
/// the full-size photo.
const GROUP_MIN_PX: f32 = 40.0;
/// A photo taken this close to one of a person's own is near them: "Find
/// more" ticks its faces, the people check suggests its look-alikes. Children
/// change fast, and to the model babies look alike.
pub const NEAR_DAYS: i64 = 31;
/// People check, age jumps: a year counts once it holds this many of the
/// person's faces...
const CHECK_YEAR_FACES: u32 = 6;
/// ...and a face is flagged when another year at least this far away...
const CHECK_YEARS_APART: i32 = 3;
/// ...is at least this alike to it...
const CHECK_YEAR_LIKE: f32 = 0.45;
/// ...and this much more alike than its own year.
const CHECK_YEAR_GAP: f32 = 0.15;
/// People check: a face is much more like another named person when it is
/// `SAME_PERSON` alike to them and this much more than to its own.
const CHECK_OTHER_GAP: f32 = 0.1;
/// People check: two named people this alike, never in the same photo, may
/// be one person under two names (one split by age).
const CHECK_SAME_PERSON: f32 = 0.55;
/// Bump when the grouping rules change: catalogs sorted under older rules
/// get their untouched faces regrouped once at start.
const GROUPING_RULES: &str = "2";
/// Bump to look once more, at start, for people found twice
/// (`join_look_alikes`).
const JOIN_RULES: &str = "1";
/// Bump to look once more, at start, for turned photos holding the same
/// face twice ([`FaceIndex::mend_turned`]).
const MEND_TURNED: &str = "1";
const DIM: usize = 512;

/// A person as the look for people found twice saw them: their faces
/// ([`Averages::digest`]) and whether they may join someone.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Standing {
    faces: u64,
    movable: bool,
}

/// Where ArcFace expects the eyes, nose and mouth corners in its 112x112 input.
const ARCFACE_DST: [[f32; 2]; 5] =
    [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]];

fn model_dir() -> PathBuf {
    crate::paths::models_dir().join("faces").join("buffalo_l")
}

pub struct Detected {
    /// Pixels of the image it was found in.
    pub bbox: [f32; 4],
    pub kps: [f32; 10],
    pub score: f32,
}

struct Models {
    det: Session,
    det_in: String,
    rec: Session,
    rec_in: String,
    on_gpu: bool,
}

impl Models {
    fn load() -> Result<Self> {
        crate::models::require(crate::models::FACES)?;
        let (det, g1) = crate::ai::session(&model_dir().join("detection").join("model.onnx"))?;
        let (rec, g2) = crate::ai::session(&model_dir().join("recognition").join("model.onnx"))?;
        let det_in = det.inputs.first().map(|i| i.name.clone()).ok_or_else(|| anyhow!("detector has no input"))?;
        let rec_in = rec.inputs.first().map(|i| i.name.clone()).ok_or_else(|| anyhow!("recogniser has no input"))?;
        Ok(Self { det, det_in, rec, rec_in, on_gpu: g1 && g2 })
    }

    /// Faces in `img`, best first. As InsightFace: the picture is fitted into
    /// 640x640 (top-left, rest padded), pixels (v - 127.5) / 128.
    fn detect(&mut self, img: &RgbImage) -> Result<Vec<Detected>> {
        let (w, h) = img.dimensions();
        let scale = (DET_SIZE as f32 / w as f32).min(DET_SIZE as f32 / h as f32);
        let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
        let resized = image::imageops::resize(img, nw, nh, FilterType::Triangle);
        let side = DET_SIZE as usize;
        let plane = side * side;
        let mut input = vec![-127.5f32 / 128.0; plane * 3];
        for (x, y, p) in resized.enumerate_pixels() {
            let i = y as usize * side + x as usize;
            for c in 0..3 {
                input[c * plane + i] = (p[c] as f32 - 127.5) / 128.0;
            }
        }
        let t = Tensor::from_array(([1usize, 3, side, side], input))?;
        let out = self.det.run(ort::inputs![self.det_in.as_str() => t])?;

        // Outputs come per stride (8, 16, 32): scores [n,1], boxes [n,4],
        // landmarks [n,10]. Group them by n rather than trusting their order.
        let mut by_n: HashMap<usize, [Option<Vec<f32>>; 3]> = HashMap::new();
        for i in 0..out.len() {
            let (shape, data) = out[i].try_extract_tensor::<f32>()?;
            let c = *shape.last().unwrap_or(&1) as usize;
            if c == 0 {
                continue;
            }
            let n = data.len() / c;
            let slot = match c {
                1 => 0,
                4 => 1,
                10 => 2,
                _ => continue,
            };
            by_n.entry(n).or_default()[slot] = Some(data.to_vec());
        }

        let mut found = Vec::new();
        for (n, [s, b, k]) in by_n {
            let (Some(s), Some(b), Some(k)) = (s, b, k) else { continue };
            // n = (640 / stride)^2 * 2 anchors per position
            let per_side = ((n / 2) as f32).sqrt().round() as usize;
            if per_side == 0 {
                continue;
            }
            let stride = (side / per_side) as f32;
            for i in 0..n {
                if s[i] < DET_THRESHOLD {
                    continue;
                }
                let p = i / 2;
                let (cx, cy) = ((p % per_side) as f32 * stride, (p / per_side) as f32 * stride);
                let bb = &b[i * 4..i * 4 + 4];
                let bbox = [
                    (cx - bb[0] * stride) / scale,
                    (cy - bb[1] * stride) / scale,
                    (cx + bb[2] * stride) / scale,
                    (cy + bb[3] * stride) / scale,
                ];
                let mut kps = [0f32; 10];
                for j in 0..5 {
                    kps[2 * j] = (cx + k[i * 10 + 2 * j] * stride) / scale;
                    kps[2 * j + 1] = (cy + k[i * 10 + 2 * j + 1] * stride) / scale;
                }
                found.push(Detected { bbox, kps, score: s[i] });
            }
        }

        found.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut kept: Vec<Detected> = Vec::new();
        for d in found {
            let small = (d.bbox[2] - d.bbox[0]).min(d.bbox[3] - d.bbox[1]) < MIN_FACE_PX;
            if !small && kept.iter().all(|k| iou(&k.bbox, &d.bbox) < NMS_IOU) {
                kept.push(d);
            }
        }
        Ok(kept)
    }

    /// The face's fingerprint: aligned to ArcFace's template by its five
    /// landmarks, 112x112, pixels (v - 127.5) / 127.5, unit length.
    fn embed(&mut self, img: &RgbImage, kps: &[f32; 10]) -> Result<Vec<f32>> {
        let (a, b, tx, ty) = similarity(kps);
        let det = a * a + b * b;
        if det <= f32::EPSILON {
            return Err(anyhow!("degenerate landmarks"));
        }
        let plane = 112 * 112;
        let mut input = vec![0f32; plane * 3];
        for v in 0..112 {
            for u in 0..112 {
                let (du, dv) = (u as f32 - tx, v as f32 - ty);
                let x = (a * du + b * dv) / det;
                let y = (-b * du + a * dv) / det;
                let px = bilinear(img, x, y);
                for c in 0..3 {
                    input[c * plane + v * 112 + u] = (px[c] - 127.5) / 127.5;
                }
            }
        }
        let t = Tensor::from_array(([1usize, 3, 112, 112], input))?;
        let out = self.rec.run(ort::inputs![self.rec_in.as_str() => t])?;
        let (_, v) = out[0].try_extract_tensor::<f32>()?;
        Ok(unit(v.to_vec()))
    }
}

/// Least-squares similarity transform (rotation + uniform scale + shift)
/// taking the five landmarks onto ARCFACE_DST: u = a x - b y + tx, v = b x + a y + ty.
fn similarity(kps: &[f32; 10]) -> (f32, f32, f32, f32) {
    let (mut sx, mut sy, mut du, mut dv) = (0f32, 0f32, 0f32, 0f32);
    for j in 0..5 {
        sx += kps[2 * j];
        sy += kps[2 * j + 1];
        du += ARCFACE_DST[j][0];
        dv += ARCFACE_DST[j][1];
    }
    let (mx, my, mu, mv) = (sx / 5.0, sy / 5.0, du / 5.0, dv / 5.0);
    let (mut num_a, mut num_b, mut den) = (0f32, 0f32, 0f32);
    for j in 0..5 {
        let (x, y) = (kps[2 * j] - mx, kps[2 * j + 1] - my);
        let (u, v) = (ARCFACE_DST[j][0] - mu, ARCFACE_DST[j][1] - mv);
        num_a += x * u + y * v;
        num_b += x * v - y * u;
        den += x * x + y * y;
    }
    if den <= f32::EPSILON {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let (a, b) = (num_a / den, num_b / den);
    (a, b, mu - (a * mx - b * my), mv - (b * mx + a * my))
}

fn bilinear(img: &RgbImage, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = img.dimensions();
    if x < 0.0 || y < 0.0 || x > (w - 1) as f32 || y > (h - 1) as f32 {
        return [0.0; 3];
    }
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let (p00, p10, p01, p11) =
        (img.get_pixel(x0, y0), img.get_pixel(x1, y0), img.get_pixel(x0, y1), img.get_pixel(x1, y1));
    let mut out = [0f32; 3];
    for c in 0..3 {
        let top = p00[c] as f32 * (1.0 - fx) + p10[c] as f32 * fx;
        let bot = p01[c] as f32 * (1.0 - fx) + p11[c] as f32 * fx;
        out[c] = top * (1.0 - fy) + bot * fy;
    }
    out
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let (x1, y1, x2, y2) = (a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3]));
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area = |r: &[f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let union = area(a) + area(b) - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// [`dot`] in sixteen lanes, which the compiler turns into vector
/// instructions (a plain sum must add in order): the people check makes
/// millions of comparisons.
fn dot_lanes(a: &[f32], b: &[f32]) -> f32 {
    let ((ca, ta), (cb, tb)) = (a.as_chunks::<16>(), b.as_chunks::<16>());
    let mut lanes = [0f32; 16];
    for (x, y) in ca.iter().zip(cb) {
        for ((l, p), q) in lanes.iter_mut().zip(x).zip(y) {
            *l += p * q;
        }
    }
    lanes.iter().sum::<f32>() + ta.iter().zip(tb).map(|(x, y)| x * y).sum::<f32>()
}

/// How much a fingerprint `v` looks like the rest of a person whose `n`
/// fingerprints, `v` among them, add up to `sum`: from sum . sum, sum . v and
/// v . v, with no new vector. None when `v` is all they have.
fn rest_like(n: u32, sum_sq: f64, sv: f32, vv: f32) -> Option<f32> {
    if n < 2 {
        return None;
    }
    let (sv, vv) = (sv as f64, vv as f64);
    let rest_sq = sum_sq - 2.0 * sv + vv;
    (rest_sq > 1e-9).then(|| ((sv - vv) / rest_sq.sqrt()) as f32)
}

/// A fingerprint in a quarter of the memory - a byte a number, scaled to the
/// largest - kept by the people check until every year's average is known.
/// Likeness comes out within about 0.001 of the full one.
struct Small {
    scale: f32,
    q: Vec<i8>,
}

impl Small {
    fn of(v: &[f32]) -> Self {
        let scale = v.iter().fold(0f32, |m, x| m.max(x.abs())).max(f32::EPSILON) / 127.0;
        Self { scale, q: v.iter().map(|x| (x / scale).round() as i8).collect() }
    }

    fn dot(&self, w: &[f32]) -> f32 {
        let ((cq, tq), (cw, tw)) = (self.q.as_chunks::<16>(), w.as_chunks::<16>());
        let mut lanes = [0f32; 16];
        for (x, y) in cq.iter().zip(cw) {
            for ((l, p), q) in lanes.iter_mut().zip(x).zip(y) {
                *l += *p as f32 * q;
            }
        }
        (lanes.iter().sum::<f32>() + tq.iter().zip(tw).map(|(x, y)| *x as f32 * y).sum::<f32>()) * self.scale
    }

    fn dot_self(&self) -> f32 {
        self.q.iter().map(|x| (*x as f32 * self.scale).powi(2)).sum()
    }
}

/// A photo taken at `ts` is within [`NEAR_DAYS`] of one of these (sorted) times.
fn near_in_time(sorted: &[i64], ts: i64) -> bool {
    let i = sorted.partition_point(|&t| t < ts - NEAR_DAYS * 86_400);
    sorted.get(i).is_some_and(|&t| t <= ts + NEAR_DAYS * 86_400)
}

/// People check: sets `left` on the findings left as is (as `seen`, nothing
/// new in them since) and keeps those only when `all`. Returns how many.
fn left_out<T>(
    found: &mut Vec<T>,
    seen: &HashMap<(&'static str, i64, i64), LeftAsIs>,
    all: bool,
    left: fn(&mut T) -> &mut bool,
) -> usize
where
    for<'a> &'a T: Into<LeftAsIs>,
{
    let mut n = 0;
    found.retain_mut(|f| {
        let now: LeftAsIs = (&*f).into();
        let is_left = seen.get(&now.key()).is_some_and(|s| s.covers(&now));
        *left(f) = is_left;
        n += is_left as usize;
        all || !is_left
    });
    n
}

/// The calendar year of a photo's `taken_ts` (camera time, read as UTC).
fn year_of(ts: i64) -> Option<i32> {
    chrono::DateTime::from_timestamp(ts, 0).map(|d| chrono::Datelike::year(&d))
}

fn mean(vs: &[&[f32]]) -> Vec<f32> {
    let mut m = vec![0.0; vs.first().map_or(0, |v| v.len())];
    for v in vs {
        m.iter_mut().zip(v.iter()).for_each(|(s, x)| *s += x);
    }
    unit(m)
}

/// The direction most of `vs` agree on: the mean, then the mean of the
/// vectors close to it, a few times over.
fn dominant(vs: &[&[f32]]) -> Vec<f32> {
    let mut c = mean(vs);
    for _ in 0..5 {
        let close: Vec<&[f32]> = vs.iter().copied().filter(|v| dot(v, &c) >= SAME_PERSON).collect();
        if close.is_empty() {
            break;
        }
        c = mean(&close);
    }
    c
}

/// The month a photo was taken (camera local time read as UTC), counted from
/// January 1970; none when its date is unknown.
/// People seen only in private photos, while those are hidden: never listed
/// or suggested, their names would tell.
fn hidden_people(cat: &Catalog, db: &Connection) -> Result<HashSet<i64>> {
    if !cat.hides_any() {
        return Ok(HashSet::new());
    }
    let private = cat.hidden_ids_sql();
    Ok(db
        .prepare(&format!(
            "SELECT DISTINCT f.person_id FROM faces f WHERE f.photo_id IN {private} AND f.person_id IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM faces o WHERE o.person_id = f.person_id AND o.photo_id NOT IN {private})"
        ))?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Those of `face_ids` in photos shown right now.
fn shown_faces(cat: &Catalog, face_ids: &[i64]) -> Result<Vec<i64>> {
    if face_ids.is_empty() || !cat.hides_any() {
        return Ok(face_ids.to_vec());
    }
    let db = cat.read()?;
    let hidden: HashSet<i64> = db
        .prepare(&format!(
            "SELECT id FROM faces WHERE id IN (SELECT value FROM json_each(?1)) AND photo_id IN {}",
            cat.hidden_ids_sql()
        ))?
        .query_map([serde_json::to_string(face_ids)?], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(face_ids.iter().copied().filter(|f| !hidden.contains(f)).collect())
}

fn month_of(taken_ts: Option<i64>) -> Option<i64> {
    let d = chrono::DateTime::from_timestamp(taken_ts.filter(|t| *t > 0)?, 0)?;
    Some((d.year() as i64 - 1970) * 12 + d.month0() as i64)
}

/// Everyone's faces added up: over their whole life, and month by month (the
/// month the photo was taken), so a new face is compared with how each person
/// looked around then. To the model a child's baby and school-age faces are
/// far apart and all newborns look alike: an average over a whole childhood
/// draws in a newborn brother or cousin, the faces from the same months don't.
/// Sums add, so merging people is just adding.
///
/// Read as a map, it holds each person's whole-life sum and how many faces.
#[derive(Clone, Default)]
struct Averages {
    life: HashMap<i64, (Vec<f32>, u32)>,
    /// Per person, per month: only months holding faces.
    months: HashMap<i64, BTreeMap<i64, (Vec<f32>, u32)>>,
}

impl std::ops::Deref for Averages {
    type Target = HashMap<i64, (Vec<f32>, u32)>;

    fn deref(&self) -> &Self::Target {
        &self.life
    }
}

impl Averages {
    fn add(&mut self, person: i64, v: &[f32], month: Option<i64>, sign: f32) {
        let add = |e: &mut (Vec<f32>, u32)| {
            e.0.iter_mut().zip(v).for_each(|(s, x)| *s += sign * x);
            e.1 = if sign > 0.0 { e.1 + 1 } else { e.1.saturating_sub(1) };
        };
        add(self.life.entry(person).or_insert_with(|| (vec![0.0; DIM], 0)));
        if let Some(m) = month {
            let months = self.months.entry(person).or_default();
            let e = months.entry(m).or_insert_with(|| (vec![0.0; DIM], 0));
            add(e);
            if e.1 == 0 {
                months.remove(&m);
            }
        }
    }

    /// `others` become part of `into`.
    fn merge(&mut self, into: i64, others: &[i64]) {
        for o in others {
            if let Some((s, n)) = self.life.remove(o) {
                let e = self.life.entry(into).or_insert_with(|| (vec![0.0; DIM], 0));
                e.0.iter_mut().zip(&s).for_each(|(a, b)| *a += b);
                e.1 += n;
            }
            for (m, (s, n)) in self.months.remove(o).unwrap_or_default() {
                let e = self.months.entry(into).or_default().entry(m).or_insert_with(|| (vec![0.0; DIM], 0));
                e.0.iter_mut().zip(&s).for_each(|(a, b)| *a += b);
                e.1 += n;
            }
        }
    }

    /// How a person looked over `months` (first, last): their faces from
    /// then and the nearest months either side holding AROUND_FACES, as a
    /// unit vector; none when there are fewer within AROUND_MONTHS.
    fn around(&self, person: i64, months: (i64, i64)) -> Option<Vec<f32>> {
        self.around_sum(person, months).map(|(sum, _)| unit(sum))
    }

    /// [`Averages::around`], as the sum of those faces and how many.
    fn around_sum(&self, person: i64, (first, last): (i64, i64)) -> Option<(Vec<f32>, u32)> {
        let months = self.months.get(&person)?;
        let faces = |k: i64| months.range(first - k..=last + k).map(|(_, (_, n))| n).sum::<u32>();
        if faces(AROUND_MONTHS) < AROUND_FACES {
            return None;
        }
        // The nearest months holding enough of them.
        let k = (0..AROUND_MONTHS).find(|&k| faces(k) >= AROUND_FACES).unwrap_or(AROUND_MONTHS);
        let mut sum = vec![0.0; DIM];
        for (_, (s, _)) in months.range(first - k..=last + k) {
            sum.iter_mut().zip(s).for_each(|(a, b)| *a += b);
        }
        Some((sum, faces(k)))
    }

    /// A person's faces in short: their sum, how many, and in which months.
    /// Any face added, removed or moved changes it.
    fn digest(&self, person: i64) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        if let Some((sum, n)) = self.life.get(&person) {
            n.hash(&mut h);
            sum.iter().for_each(|x| x.to_bits().hash(&mut h));
        }
        for (m, (_, n)) in self.months.get(&person).into_iter().flatten() {
            (m, n).hash(&mut h);
        }
        h.finish()
    }

    /// How much `v` looks like a person's whole life of faces, but `without`
    /// (one of theirs, about to make room for `v`). None when that was all
    /// they had.
    fn like_rest(&self, person: i64, v: &[f32], without: Option<&[f32]>) -> Option<f32> {
        let (sum, n) = self.life.get(&person)?;
        if *n <= without.is_some() as u32 {
            return None;
        }
        let mut rest = sum.clone();
        if let Some(w) = without {
            rest.iter_mut().zip(w).for_each(|(s, x)| *s -= x);
        }
        let norm = rest.iter().map(|x| x * x).sum::<f32>().sqrt();
        (norm > f32::EPSILON).then(|| dot(&rest, v) / norm)
    }

    /// The first and last month of a person's photos with a date.
    fn months_of(&self, person: i64) -> Option<(i64, i64)> {
        let m = self.months.get(&person)?;
        Some((*m.first_key_value()?.0, *m.last_key_value()?.0))
    }

    /// The pairs of face (unit vector) and person, other than `except`, at
    /// least `min` alike, for faces from `months` (first, last; none: date
    /// unknown): (likeness, face, person). Each person as they looked around
    /// then; one with too few photos from then (or a face with no date), by
    /// their whole-life average.
    fn alike(&self, faces: &[&[f32]], months: Option<(i64, i64)>, except: &[i64], min: f32) -> Vec<(f32, usize, i64)> {
        self.alike_among(self.life.keys(), faces, months, except, min)
    }

    /// [`Averages::alike`], with only these people.
    fn alike_among<'a>(
        &self,
        people: impl IntoIterator<Item = &'a i64>,
        faces: &[&[f32]],
        months: Option<(i64, i64)>,
        except: &[i64],
        min: f32,
    ) -> Vec<(f32, usize, i64)> {
        let mut pairs = Vec::new();
        for pid in people {
            let Some((sum, n)) = self.life.get(pid) else { continue };
            if *n == 0 || except.contains(pid) {
                continue;
            }
            let then = months.and_then(|m| self.around(*pid, m));
            let (c, norm) = match &then {
                Some(then) => (then.as_slice(), 1.0),
                None => (sum.as_slice(), sum.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::EPSILON)),
            };
            for (i, v) in faces.iter().enumerate() {
                let s = dot(c, v) / norm;
                if s >= min {
                    pairs.push((s, i, *pid));
                }
            }
        }
        pairs
    }

    /// Who each face of a photo taken in `month` is: the most alike pairs of
    /// face and person first, each person once, none of `taken` (already in
    /// the photo). None: nobody alike enough - someone new.
    fn choose(&self, faces: &[&[f32]], month: Option<i64>, taken: &[i64]) -> Vec<Option<i64>> {
        let mut pairs = self.alike(faces, month.map(|m| (m, m)), taken, SAME_PERSON);
        // Ties: the earlier face, then the older person, so a regroup comes out the same.
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut out = vec![None; faces.len()];
        let mut used: HashSet<i64> = HashSet::new();
        for (_, i, pid) in pairs {
            if out[i].is_none() && used.insert(pid) {
                out[i] = Some(pid);
            }
        }
        out
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stray {
    pub face_id: i64,
    pub photo_id: i64,
    /// Likeness to the person's dominant face, 0..1.
    pub similarity: f32,
    /// The user put it in this person.
    pub by_hand: bool,
}

/// A face in someone else that may be this person ("Find more").
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub face_id: i64,
    pub photo_id: i64,
    /// The person it's in now.
    pub person_id: i64,
    /// Likeness to the person asked about around the photo's date, 0..1.
    pub similarity: f32,
    /// Likeness to the rest of the person it's in, around then; none when
    /// it's their only face.
    pub now: Option<f32>,
    /// The user put it where it is.
    pub by_hand: bool,
}

/// One day's faces in "Find more".
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateDay {
    /// The day the photos were taken (YYYY-MM-DD); none when unknown.
    pub day: Option<String>,
    /// Within [`NEAR_DAYS`] of a photo the person is in: ticked to start with.
    pub near: bool,
    /// The most alike first.
    pub faces: Vec<Candidate>,
}

/// "Check" in People: likely mix-ups among named people, found from their
/// faces and the dates of their photos. Nothing changes until the user acts
/// on a finding (`reassign`, `merge`), or leaves it as is ([`leave_as_is`]).
/// A face is in one finding at most: twice in a photo, else much more like
/// someone near in time, else an age jump, else much more like someone far
/// off in time - findings left as is included.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeopleCheck {
    /// Named people checked (ignored ones aren't), and their faces.
    pub people: usize,
    pub faces: usize,
    /// Findings the user left as is: left out, unless all were asked for
    /// (then listed, `left` set).
    pub left: usize,
    /// Someone twice in one photo: all but one of the faces are someone else.
    pub twice: Vec<Twice>,
    /// Faces unlike the person that year but like them years apart: a
    /// sibling, a child or a parent filed under them.
    pub age_jumps: Vec<AgeJump>,
    /// Faces much more like another named person than like their own.
    pub like_others: Vec<LikeOther>,
    /// Two named people who may be one.
    pub same_people: Vec<SamePerson>,
}

/// A face in a [`PeopleCheck`] finding.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckFace {
    pub face_id: i64,
    pub photo_id: i64,
    /// Likeness to the rest of its person, 0..1.
    pub similarity: f32,
    /// The user put it in this person.
    pub by_hand: bool,
}

/// Someone twice (or more) in one photo.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Twice {
    pub person_id: i64,
    pub photo_id: i64,
    /// The most like them first.
    pub faces: Vec<CheckFace>,
    /// Suggested: these leave them for whoever they resemble
    /// (`FaceTarget::Elsewhere`) - all but the most alike.
    pub elsewhere: Vec<i64>,
    /// Left as is by the user.
    pub left: bool,
}

/// A face unlike the person in its own year, like them years apart.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgeJump {
    pub person_id: i64,
    pub face: CheckFace,
    /// The year the photo was taken, and the face's likeness to the person's
    /// other faces that year.
    pub year: i32,
    pub own: f32,
    /// The year of theirs it looks most like, and how much.
    pub like_year: i32,
    pub like: f32,
    /// Left as is by the user.
    pub left: bool,
}

/// A face much more like another named person than like its own.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LikeOther {
    pub person_id: i64,
    /// `similarity`: likeness to the rest of `person_id`.
    pub face: CheckFace,
    /// Who it looks like, and how much.
    pub other_id: i64,
    pub other: f32,
    /// Its photo is within [`NEAR_DAYS`] of one of theirs: suggested. When
    /// not, unsure - to the model newborns all look alike.
    pub near: bool,
    /// Left as is by the user.
    pub left: bool,
}

/// Two named people who may be one: alike and never in the same photo (one
/// person split, by age for instance), or called the same.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SamePerson {
    /// The one with more faces: "Same person" merges `other` into them,
    /// keeping their name.
    pub person_id: i64,
    pub other_id: i64,
    /// How alike their average faces are.
    pub similarity: f32,
    /// Their names read the same (case and accents aside).
    pub same_name: bool,
    /// Photos they are both in. Only with none is merging suggested: a
    /// person is once per photo.
    pub together: usize,
    /// Left as is by the user.
    pub left: bool,
}

/// A [`PeopleCheck`] finding the user looked at and kept ("Leave as is"):
/// someone really twice in a photo (a reflection, a collage), look-alikes who
/// are two people. Stored as it was seen ([`leave_as_is`]); the check leaves
/// it out until what it is about changes - another face of theirs in that
/// photo, the face now in someone else or more like someone else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LeftAsIs {
    /// Someone twice in a photo, with these faces.
    Twice { person_id: i64, photo_id: i64, faces: Vec<i64> },
    /// A face of a person from another age.
    AgeJump { person_id: i64, face_id: i64 },
    /// A face of a person, much more like another.
    LikeOther { person_id: i64, face_id: i64, other_id: i64 },
    /// Two people who may be one, in either order.
    SamePerson { person_id: i64, other_id: i64 },
}

impl LeftAsIs {
    /// What it is about: its row in `check_left`.
    fn key(&self) -> (&'static str, i64, i64) {
        match *self {
            Self::Twice { person_id, photo_id, .. } => ("twice", photo_id, person_id),
            Self::AgeJump { face_id, .. } => ("ageJump", face_id, 0),
            Self::LikeOther { face_id, .. } => ("likeOther", face_id, 0),
            Self::SamePerson { person_id, other_id } => {
                ("samePerson", person_id.min(other_id), person_id.max(other_id))
            }
        }
    }

    /// Left as is when seen as `self`, and so still as `now` (about the same
    /// thing): nothing new in it. Faces gone from the photo since don't count.
    fn covers(&self, now: &Self) -> bool {
        match (self, now) {
            (Self::Twice { faces, .. }, Self::Twice { faces: now, .. }) => now.iter().all(|f| faces.contains(f)),
            (Self::SamePerson { .. }, Self::SamePerson { .. }) => true,
            (seen, now) => seen == now,
        }
    }
}

impl From<&Twice> for LeftAsIs {
    fn from(t: &Twice) -> Self {
        let faces = t.faces.iter().map(|f| f.face_id).collect();
        Self::Twice { person_id: t.person_id, photo_id: t.photo_id, faces }
    }
}

impl From<&AgeJump> for LeftAsIs {
    fn from(a: &AgeJump) -> Self {
        Self::AgeJump { person_id: a.person_id, face_id: a.face.face_id }
    }
}

impl From<&LikeOther> for LeftAsIs {
    fn from(l: &LikeOther) -> Self {
        Self::LikeOther { person_id: l.person_id, face_id: l.face.face_id, other_id: l.other_id }
    }
}

impl From<&SamePerson> for LeftAsIs {
    fn from(s: &SamePerson) -> Self {
        Self::SamePerson { person_id: s.person_id, other_id: s.other_id }
    }
}

/// "Leave as is": the check leaves these findings out from now on, until
/// they change; not `left`, they are back in it. Returns how many changed.
pub fn leave_as_is(cat: &Catalog, findings: &[LeftAsIs], left: bool) -> Result<usize> {
    let mut db = cat.db();
    let tx = db.transaction()?;
    let mut n = 0;
    for f in findings {
        let (kind, id, id2) = f.key();
        n += if left {
            tx.execute(
                "INSERT OR REPLACE INTO check_left (kind, id, id2, seen, left_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![kind, id, id2, serde_json::to_string(f)?, crate::db::now()],
            )?
        } else {
            tx.execute("DELETE FROM check_left WHERE kind = ?1 AND id = ?2 AND id2 = ?3", params![kind, id, id2])?
        };
    }
    tx.commit()?;
    Ok(n)
}

/// What "Same person" changed: `unmerge` puts it back (Undo).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Merged {
    pub into: i64,
    /// `into`'s name before (it takes the first name among them when it has none).
    pub name: Option<String>,
    /// The people who joined, as they were, with their faces.
    pub joined: Vec<Joined>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Joined {
    pub id: i64,
    pub name: Option<String>,
    pub ignored: bool,
    pub created_at: i64,
    pub faces: Vec<i64>,
}

/// Where faces go when the user says who they are.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum FaceTarget {
    /// Someone already in People.
    Person { id: i64 },
    /// Someone new - one person for all the faces moved - named or not.
    New { name: Option<String> },
    /// "Not them, don't know who": each face joins whoever it resembles
    /// most - never the person it leaves - or becomes someone new.
    Elsewhere,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Moved {
    /// Faces that changed person.
    pub faces: usize,
    /// Who they went to.
    pub people: Vec<i64>,
    /// Faces that made room (their new person was already in the photo):
    /// who each went to.
    pub bumped: Vec<i64>,
    /// Every face changed, as it was: `restore` puts them back (Undo).
    pub undo: Vec<Placement>,
}

/// Where a face is: its person, and whether the user put it there and has seen it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    pub face_id: i64,
    pub person_id: Option<i64>,
    pub by_hand: bool,
    pub seen: bool,
}

/// Someone a face looks like (`look_alikes`).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookAlike {
    pub person_id: i64,
    pub similarity: f32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: i64,
    pub name: Option<String>,
    pub faces: i64,
    pub photos: i64,
    /// The clearest face, for the person's picture.
    pub cover: i64,
    /// A stranger the user chose to hide.
    pub ignored: bool,
    /// None of their faces has been seen yet: someone new.
    pub is_new: bool,
    /// Photos they were found in that the user hasn't seen on their page yet.
    pub new_photos: i64,
}

impl Person {
    /// New, or found in new photos: what "Only new" in People shows.
    pub fn has_news(&self) -> bool {
        self.is_new || self.new_photos > 0
    }

    /// The group People shows them in.
    pub fn group(&self) -> PeopleGroup {
        if self.ignored {
            PeopleGroup::Ignored
        } else if self.name.is_some() {
            PeopleGroup::Named
        } else if self.photos < 2 {
            PeopleGroup::Once
        } else {
            PeopleGroup::ToName
        }
    }
}

/// People's groups: named, still to name, seen in one photo only (hidden
/// unless asked, then shown with the people to name), ignored strangers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeopleGroup {
    Named,
    ToName,
    Once,
    Ignored,
}

impl PeopleGroup {
    /// As the CLI spells it: named, to-name, once, ignored.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "named" => Self::Named,
            "to-name" => Self::ToName,
            "once" => Self::Once,
            "ignored" => Self::Ignored,
            _ => return None,
        })
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoFace {
    pub id: i64,
    pub photo_id: i64,
    pub person_id: Option<i64>,
    pub name: Option<String>,
    /// 0..1 of the photo: x1, y1, x2, y2
    pub bbox: [f64; 4],
}

/// Turned photos holding the same face twice ([`FaceIndex::mend_turned`]).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mended {
    /// Turned photos with faces, looked at.
    pub photos: usize,
    /// Each face found twice, and what becomes of it.
    pub twice: Vec<FoundTwice>,
    /// Mended; false: only listed.
    pub done: bool,
}

/// One face of a turned photo found twice.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundTwice {
    pub photo_id: i64,
    /// The newer face, found with the photo upright: it stays.
    pub kept: i64,
    /// The older face, found while the photo stood the wrong way: it goes.
    pub gone: i64,
    /// Who the face kept is then, and whether by the user's word.
    pub person_id: Option<i64>,
    pub by_hand: bool,
    /// It takes the place of the face that goes (rather than its own).
    pub took_place: bool,
}

pub struct FaceIndex {
    /// faces.db: one fingerprint per face.
    db: Mutex<Connection>,
    models: Mutex<Option<Models>>,
    /// Per person: the sum of its faces' fingerprints and how many, whole
    /// life and month by month.
    centroids: RwLock<Averages>,
    /// How everyone stood when the last look for people found twice found
    /// nobody more: the next look compares only who changed since (none:
    /// everyone, as after a start).
    joined: Mutex<Option<HashMap<i64, Standing>>>,
    /// Open once the averages are read (and any regrouping done): every
    /// method waits for it, so none works from half the people.
    ready: crate::Ready,
    /// People as last listed, until the catalog changes.
    kept_people: crate::catalog::Kept<Vec<Person>>,
}

impl FaceIndex {
    /// Lets go of the face models (they are being deleted). Waits for a
    /// face search in progress: cancel it first.
    pub fn unload(&self) {
        *self.models.lock().unwrap() = None;
    }

    /// Opens the face index and reads it before returning.
    pub fn open(cat: &Catalog) -> Result<Self> {
        let index = Self::unread(cat)?;
        let result = index.read(cat);
        index.ready.open(result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")));
        result.map(|_| index)
    }

    /// Opens the face index at once and reads it in the background (the
    /// window opens meanwhile); what needs it waits until it's read.
    pub fn open_in_background(cat: Arc<Catalog>) -> Result<Arc<Self>> {
        let index = Arc::new(Self::unread(&cat)?);
        let reading = index.clone();
        std::thread::Builder::new().name("faces-open".into()).spawn(move || {
            let result = reading.read(&cat).map_err(|e| {
                log::warn!("faces: couldn't read the face index: {e:#}");
                format!("The face index couldn't be read: {e:#}")
            });
            reading.ready.open(result);
        })?;
        Ok(index)
    }

    fn unread(cat: &Catalog) -> Result<Self> {
        let conn = Connection::open(cat.data.root.join("faces.db"))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS emb (face_id INTEGER PRIMARY KEY, vec BLOB NOT NULL);")?;
        Ok(Self {
            db: Mutex::new(conn),
            models: Mutex::new(None),
            centroids: RwLock::new(Averages::default()),
            joined: Mutex::new(None),
            ready: crate::Ready::default(),
            kept_people: crate::catalog::Kept::new(1),
        })
    }

    /// Everyone's average face, then the regrouping or joining a catalog
    /// from an older Shoebox needs once.
    fn read(&self, cat: &Catalog) -> Result<()> {
        let started = std::time::Instant::now();
        self.rebuild_centroids(cat)?;
        log::info!(
            "faces: {} people's average faces read in {} ms",
            self.centroids.read().unwrap().len(),
            started.elapsed().as_millis()
        );
        // Turned photos given the same face twice, before a face found again
        // took the old one's place, get mended once.
        let mended: Option<String> = cat
            .db()
            .query_row("SELECT value FROM meta WHERE key = 'turned_faces_mended'", [], |r| r.get(0))
            .optional()?;
        if mended.as_deref() != Some(MEND_TURNED) {
            match self.mend_turned_now(cat, true) {
                Ok(m) => {
                    if !m.twice.is_empty() {
                        log::info!("faces: {} faces found twice in turned photos, the older ones gone", m.twice.len());
                    }
                    cat.db().execute(
                        "INSERT OR REPLACE INTO meta (key, value) VALUES ('turned_faces_mended', ?1)",
                        [MEND_TURNED],
                    )?;
                }
                // Faces still work; the next start tries again.
                Err(e) => log::warn!("faces: mending turned photos failed: {e:#}"),
            }
        }
        // Catalogs grouped under older rules get regrouped once, and so does
        // one whose regrouping was cut short.
        let done: Option<String> =
            cat.db().query_row("SELECT value FROM meta WHERE key = 'face_grouping'", [], |r| r.get(0)).optional()?;
        if done.as_deref() != Some(GROUPING_RULES) {
            let (faces, people) = self.regroup_now(cat)?;
            log::info!("faces: regrouped {faces} faces into {people} people (rules {GROUPING_RULES})");
        }
        // Catalogs grouped before people found twice were joined get theirs
        // joined once.
        let joined: Option<String> = cat
            .db()
            .query_row("SELECT value FROM meta WHERE key = 'look_alikes_joined'", [], |r| r.get(0))
            .optional()?;
        if joined.as_deref() != Some(JOIN_RULES) {
            match self.join_now(cat) {
                Ok(_) => {
                    cat.db().execute(
                        "INSERT OR REPLACE INTO meta (key, value) VALUES ('look_alikes_joined', ?1)",
                        [JOIN_RULES],
                    )?;
                }
                // Faces still work; the next start tries again.
                Err(e) => log::warn!("faces: joining people found twice failed: {e:#}"),
            }
        }
        Ok(())
    }

    /// Sorts again every face the user hasn't touched - not in a named or ignored
    /// person, not placed by hand - with the current rules. Names, ignored
    /// strangers and hand placements are never changed. Returns (faces, people).
    ///
    /// The faces are first taken out of their people, then sorted photo by
    /// photo. The catalog counts as sorted only once the last photo is done:
    /// a Shoebox stopped halfway sorts them all again at its next start,
    /// rather than leaving faces with nobody.
    pub fn regroup(&self, cat: &Catalog) -> Result<(usize, usize)> {
        self.ready.wait()?;
        self.regroup_now(cat)
    }

    /// [`FaceIndex::regroup`], also while the index is being read.
    fn regroup_now(&self, cat: &Catalog) -> Result<(usize, usize)> {
        let loose: Vec<(i64, i64, [f64; 4], Option<i64>)> = cat
            .db()
            .prepare(
                "SELECT f.id, f.photo_id, f.x1, f.y1, f.x2, f.y2, ph.taken_ts FROM faces f
                   JOIN photos ph ON ph.id = f.photo_id LEFT JOIN people p ON p.id = f.person_id
                  WHERE f.by_hand = 0 AND (f.person_id IS NULL OR (p.name IS NULL AND p.ignored = 0))
                  ORDER BY f.photo_id, f.score DESC",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?], r.get(6)?)))?
            .collect::<rusqlite::Result<_>>()?;
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            for (fid, ..) in &loose {
                tx.execute("UPDATE faces SET person_id = NULL WHERE id = ?1", [fid])?;
            }
            tx.execute(
                "DELETE FROM people WHERE name IS NULL AND ignored = 0
                   AND NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = people.id)",
                [],
            )?;
            tx.execute("DELETE FROM meta WHERE key = 'face_grouping'", [])?;
            tx.commit()?;
        }
        self.rebuild_centroids(cat)?;

        let mut people_made: usize = 0;
        let mut i = 0;
        while i < loose.len() {
            let photo = loose[i].1;
            let mut j = i;
            while j < loose.len() && loose[j].1 == photo {
                j += 1;
            }
            // Short side of each face in preview pixels, as when it was found.
            let (pw, ph) = cat.copy_dimensions(&cat.data.preview_path(photo)).unwrap_or((1080, 720));
            let taken: Vec<i64> = cat
                .db()
                .prepare("SELECT person_id FROM faces WHERE photo_id = ?1 AND person_id IS NOT NULL")?
                .query_map([photo], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let month = month_of(loose[i].3);
            let mut sort: Vec<(i64, Vec<f32>)> = Vec::new();
            for (fid, _, b, _) in &loose[i..j] {
                let short = ((b[2] - b[0]) * pw as f64).min((b[3] - b[1]) * ph as f64) as f32;
                let Some(v) = self.vector(*fid)? else { continue };
                if short >= GROUP_MIN_PX {
                    sort.push((*fid, v));
                }
            }
            let chosen = self.centroids.read().unwrap().choose(
                &sort.iter().map(|(_, v)| v.as_slice()).collect::<Vec<_>>(),
                month,
                &taken,
            );
            let mut db = cat.db();
            let tx = db.transaction()?;
            for ((fid, v), pid) in sort.iter().zip(chosen) {
                let pid = match pid {
                    Some(pid) => pid,
                    None => {
                        tx.execute("INSERT INTO people (name, created_at) VALUES (NULL, ?1)", [crate::db::now()])?;
                        people_made += 1;
                        tx.last_insert_rowid()
                    }
                };
                tx.execute("UPDATE faces SET person_id = ?1 WHERE id = ?2", params![pid, fid])?;
                self.centroids.write().unwrap().add(pid, v, month, 1.0);
            }
            tx.commit()?;
            i = j;
        }
        cat.db().execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('face_grouping', ?1)", [GROUPING_RULES])?;
        // Only people just made can join someone (the rest have a name, are
        // ignored or hold a face placed by hand).
        let joined = self.join_now(cat).unwrap_or_else(|e| {
            log::warn!("faces: joining people found twice failed: {e:#}");
            0
        });
        Ok((loose.len(), people_made.saturating_sub(joined)))
    }

    fn vector(&self, face_id: i64) -> Result<Option<Vec<f32>>> {
        let blob: Option<Vec<u8>> = self
            .db
            .lock()
            .unwrap()
            .query_row("SELECT vec FROM emb WHERE face_id = ?1", [face_id], |r| r.get(0))
            .optional()?;
        Ok(blob
            .filter(|b| b.len() == DIM * 4)
            .map(|b| b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()))
    }

    /// Everyone's average face, from the catalog's faces and faces.db's
    /// fingerprints read side by side in face order: one pass over each, not
    /// one look-up per face (seconds on a big catalog).
    fn rebuild_centroids(&self, cat: &Catalog) -> Result<()> {
        let faces: Vec<(i64, i64, Option<i64>)> = cat
            .read()?
            // In face order: the sums come out the same to the last bit, whatever index the query uses.
            .prepare(
                "SELECT f.id, f.person_id, p.taken_ts FROM faces f JOIN photos p ON p.id = f.photo_id
                  WHERE f.person_id IS NOT NULL ORDER BY f.id",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut faces = faces.into_iter().peekable();
        let mut c = Averages::default();
        let mut v = vec![0f32; DIM];
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare("SELECT face_id, vec FROM emb ORDER BY face_id")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let fid: i64 = r.get(0)?;
            while faces.next_if(|(id, ..)| *id < fid).is_some() {}
            let Some((_, pid, taken)) = faces.next_if(|(id, ..)| *id == fid) else { continue };
            let blob = r.get_ref(1)?.as_blob()?;
            if blob.len() != DIM * 4 {
                continue;
            }
            v.iter_mut().zip(blob.as_chunks::<4>().0).for_each(|(x, b)| *x = f32::from_le_bytes(*b));
            c.add(pid, &v, month_of(taken), 1.0);
        }
        *self.centroids.write().unwrap() = c;
        Ok(())
    }

    /// A face's fingerprint and the month its photo was taken.
    fn vector_and_month(&self, cat: &Catalog, face_id: i64) -> Result<Option<(Vec<f32>, Option<i64>)>> {
        let Some(v) = self.vector(face_id)? else { return Ok(None) };
        let taken: Option<i64> = cat
            .db()
            .query_row(
                "SELECT p.taken_ts FROM faces f JOIN photos p ON p.id = f.photo_id WHERE f.id = ?1",
                [face_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(Some((v, month_of(taken))))
    }

    /// Finds and groups the faces of every photo not looked at yet.
    pub fn index_pending(
        &self,
        cat: &Catalog,
        cancel: &AtomicBool,
        progress: &dyn Fn(&crate::search::AiProgress),
    ) -> Result<()> {
        self.ready.wait()?;
        // Private photos' previews can only be read while they are unlocked.
        let todo: Vec<i64> = cat
            .db()
            .prepare(&format!(
                "SELECT id FROM photos p WHERE preview_state = {} AND id NOT IN (SELECT photo_id FROM face_scan)
                   AND {} ORDER BY taken_ts DESC",
                crate::db::PREVIEW_OK,
                cat.shown_sql("p")
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if todo.is_empty() {
            return Ok(());
        }
        let mut guard = self.models.lock().unwrap();
        if guard.is_none() {
            *guard = Some(Models::load()?);
        }
        let models = guard.as_mut().unwrap();
        let mut p = crate::search::AiProgress {
            task: "faces".into(),
            total: todo.len() as u64,
            gpu: models.on_gpu,
            ..Default::default()
        };
        progress(&p);
        log::info!("faces: looking at {} photos ({})", todo.len(), if p.gpu { "GPU" } else { "CPU" });
        let started = std::time::Instant::now();
        let mut total_faces = 0;

        for (n, photo_id) in todo.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let found = match cat.open_copy(&cat.data.preview_path(*photo_id)) {
                Ok(img) => {
                    let rgb = img.to_rgb8();
                    match models.detect(&rgb) {
                        Ok(faces) => {
                            let mut out = Vec::new();
                            for d in faces {
                                match models.embed(&rgb, &d.kps) {
                                    Ok(v) if v.len() == DIM => out.push((d, v)),
                                    Ok(_) => {}
                                    Err(e) => log::warn!("faces: photo {photo_id}: {e:#}"),
                                }
                            }
                            Some((img, out))
                        }
                        Err(e) => {
                            log::warn!("faces: photo {photo_id}: detection failed: {e:#}");
                            None
                        }
                    }
                }
                Err(e) => {
                    log::warn!("faces: photo {photo_id}: preview unreadable: {e:#}");
                    None
                }
            };
            if let Some((img, faces)) = found {
                total_faces += faces.len();
                self.store(cat, *photo_id, &img, faces)?;
            }
            if n % 16 == 0 || n + 1 == todo.len() {
                p.done = n as u64 + 1;
                progress(&p);
            }
        }
        log::info!("faces: {} photos, {total_faces} faces in {:.1} s", p.done, started.elapsed().as_secs_f64());
        // The faces are saved either way; a failed join is tried again after
        // the next photos.
        if total_faces > 0
            && let Err(e) = self.join_now(cat)
        {
            log::warn!("faces: joining people found twice failed: {e:#}");
        }
        Ok(())
    }

    /// Records a photo's faces: each joins the person it resembles, or starts a new one.
    /// The photo counts as looked at only once its faces, their fingerprints and
    /// their pictures are all saved: a Shoebox stopped halfway looks at it
    /// again, rather than leaving faces with no picture or fingerprint.
    ///
    /// A photo looked at again after a turn still holds the faces found while
    /// it stood the wrong way ([`turn_faces`]). A face found again takes the
    /// old one's place, with its box and fingerprint from the photo upright,
    /// so no face is in it twice; the old face's person stays when it was the
    /// user's word, or when the face upright still looks like them (see
    /// [`Before::keeps_place`]). Faces found only then, or only before, stay.
    pub fn store(
        &self,
        cat: &Catalog,
        photo_id: i64,
        img: &DynamicImage,
        faces: Vec<(Detected, Vec<f32>)>,
    ) -> Result<()> {
        self.ready.wait()?;
        let (w, h) = img.dimensions();
        let (fw, fh) = (w as f64, h as f64);
        // Cut out before the catalog is held.
        let thumbs: Vec<Option<Vec<u8>>> = faces.iter().map(|(d, _)| face_thumb(img, &d.bbox).ok()).collect();
        let mut written = Vec::new();
        let mut joined = Vec::new();
        let taken_ts: Option<i64> = cat
            .db()
            .query_row("SELECT taken_ts FROM photos WHERE id = ?1", [photo_id], |r| r.get(0))
            .optional()?
            .flatten();
        let month = month_of(taken_ts);
        let before = self.faces_before(cat, photo_id)?;
        let boxes: Vec<[f32; 4]> = faces
            .iter()
            .map(|(d, _)| [d.bbox[0] / w as f32, d.bbox[1] / h as f32, d.bbox[2] / w as f32, d.bbox[3] / h as f32])
            .collect();
        let again = found_again(&before.iter().map(|b| b.bbox).collect::<Vec<_>>(), &boxes);
        // Faces too small to group stay with nobody.
        let groupable: Vec<bool> =
            faces.iter().map(|(d, _)| (d.bbox[2] - d.bbox[0]).min(d.bbox[3] - d.bbox[1]) >= GROUP_MIN_PX).collect();
        // Faces found again keeping the old one's place; the others are sorted
        // as new, each person once in the photo.
        let mut places: Vec<Option<Place>> = vec![None; faces.len()];
        let chosen: HashMap<usize, Option<i64>> = {
            let c = self.centroids.read().unwrap();
            let mut taken: Vec<i64> = before
                .iter()
                .enumerate()
                .filter(|(j, _)| !again.contains(&Some(*j)))
                .filter_map(|(_, b)| b.person)
                .collect();
            for (i, j) in again.iter().enumerate() {
                if let Some(b) = j.map(|j| &before[j]) {
                    places[i] = b.keeps_place(&c, Some(&faces[i].1), &taken);
                    taken.extend(places[i].and_then(|p| p.person));
                }
            }
            let open: Vec<usize> = (0..faces.len()).filter(|&i| places[i].is_none() && groupable[i]).collect();
            let sort: Vec<&[f32]> = open.iter().map(|&i| faces[i].1.as_slice()).collect();
            open.into_iter().zip(c.choose(&sort, month, &taken)).collect()
        };
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            for (i, (d, v)) in faces.iter().enumerate() {
                let place = match places[i] {
                    Some(place) => place,
                    None if !groupable[i] => Place::default(),
                    None => Place {
                        person: Some(match chosen.get(&i).copied().flatten() {
                            Some(pid) => pid,
                            None => {
                                tx.execute(
                                    "INSERT INTO people (name, created_at) VALUES (NULL, ?1)",
                                    [crate::db::now()],
                                )?;
                                tx.last_insert_rowid()
                            }
                        }),
                        ..Place::default()
                    },
                };
                let kps: Vec<String> = d
                    .kps
                    .iter()
                    .enumerate()
                    .map(|(i, k)| format!("{:.4}", *k as f64 / if i % 2 == 0 { fw } else { fh }))
                    .collect();
                tx.execute(
                    "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, by_hand, seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        photo_id,
                        place.person,
                        (d.bbox[0] as f64 / fw).clamp(0.0, 1.0),
                        (d.bbox[1] as f64 / fh).clamp(0.0, 1.0),
                        (d.bbox[2] as f64 / fw).clamp(0.0, 1.0),
                        (d.bbox[3] as f64 / fh).clamp(0.0, 1.0),
                        kps.join(","),
                        d.score,
                        place.by_hand,
                        place.seen
                    ],
                )?;
                let new = tx.last_insert_rowid();
                written.push(new);
                joined.extend(place.person.map(|pid| (pid, v)));
                // The face it was found as before goes; what the user left as
                // is about it follows.
                if let Some(b) = again[i].map(|j| &before[j]) {
                    follow_left(&tx, b.id, new)?;
                    tx.execute("DELETE FROM faces WHERE id = ?1", [b.id])?;
                }
            }
            {
                let mut emb = self.db.lock().unwrap();
                let etx = emb.transaction()?;
                for (fid, (_, v)) in written.iter().zip(&faces) {
                    let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
                    etx.execute("INSERT OR REPLACE INTO emb (face_id, vec) VALUES (?1, ?2)", params![fid, blob])?;
                }
                etx.commit()?;
            }
            for (fid, thumb) in written.iter().zip(&thumbs) {
                let path = cat.data.face_path(*fid);
                match thumb {
                    // Sealed when the photo is private.
                    Some(bytes) => cat.write_copy(photo_id, &path, bytes)?,
                    // Not a picture left by an attempt that never finished.
                    None => cat.remove_copy(&path)?,
                }
            }
            // Every face of the photo, those found before a turn included.
            tx.execute(
                "INSERT OR REPLACE INTO face_scan (photo_id, model, faces)
                 VALUES (?1, ?2, (SELECT COUNT(*) FROM faces WHERE photo_id = ?1))",
                params![photo_id, FACE_MODEL],
            )?;
            tx.commit()?;
        }
        // The faces found again are gone only once that's saved: until then
        // they keep their fingerprints and pictures.
        let gone: Vec<&Before> = again.iter().flatten().map(|&j| &before[j]).collect();
        self.forget_faces(cat, &gone.iter().map(|b| b.id).collect::<Vec<_>>())?;
        // Only now that they're saved do these faces count for their person.
        let mut c = self.centroids.write().unwrap();
        for b in gone {
            if let (Some(pid), Some(v)) = (b.person, &b.v) {
                c.add(pid, v, month, -1.0);
            }
        }
        for (pid, v) in joined {
            c.add(pid, v, month, 1.0);
        }
        Ok(())
    }

    /// Turned photos holding the same face twice: found while the photo stood
    /// the wrong way, and again once it was upright, before a face found
    /// again took the old one's place ([`FaceIndex::store`]). Of the two, the
    /// newer stays (its box and fingerprint are from the photo upright), with
    /// the older one's place when that should stay ([`Before::keeps_place`]);
    /// the older goes. Two people overlapping aren't one face found twice
    /// ([`same_face`]): both stay. Only listed unless `apply`. Done once at
    /// start. Private photos are mended too, but never listed.
    pub fn mend_turned(&self, cat: &Catalog, apply: bool) -> Result<Mended> {
        self.ready.wait()?;
        self.mend_turned_now(cat, apply)
    }

    fn mend_turned_now(&self, cat: &Catalog, apply: bool) -> Result<Mended> {
        // Every face of the turned photos, photo by photo, oldest first.
        let rows: Vec<(i64, Option<i64>, Before)> = cat
            .db()
            .prepare(
                "SELECT f.photo_id, p.taken_ts, f.id, f.person_id, f.by_hand, f.seen, f.x1, f.y1, f.x2, f.y2
                   FROM photos p JOIN faces f ON f.photo_id = p.id
                  WHERE p.turn <> 0 ORDER BY f.photo_id, f.id",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, Before::from_row(r, 2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut photos: Vec<(i64, Option<i64>, Vec<Before>)> = Vec::new();
        for (photo, taken, face) in rows {
            match photos.last_mut() {
                Some((p, _, faces)) if *p == photo => faces.push(face),
                _ => photos.push((photo, taken, vec![face])),
            }
        }
        let hidden = cat.hidden_ids()?;
        let mut mended = Mended {
            photos: photos.iter().filter(|(p, ..)| !hidden.contains(p)).count(),
            done: apply,
            ..Default::default()
        };
        let mut work: Vec<Mending> = Vec::new();
        {
            let c = self.centroids.read().unwrap();
            for (photo, taken, faces) in photos.into_iter().filter(|(_, _, f)| f.len() > 1) {
                let was = faces.iter().map(Before::place).collect();
                let mut m = Mending { photo, month: month_of(taken), was, faces, pairs: Vec::new() };
                // The most overlapping two left first; a newer face that pairs
                // with a newer one still goes in turn.
                loop {
                    let mut best: Option<(f32, usize, usize)> = None;
                    for a in (0..m.faces.len()).filter(|&a| !m.goes(a)) {
                        for b in (a + 1..m.faces.len()).filter(|&b| !m.goes(b)) {
                            let (fa, fb) = (&m.faces[a].bbox, &m.faces[b].bbox);
                            let s = iou(fa, fb);
                            if same_face(fa, fb) && best.is_none_or(|(t, ..)| s > t) {
                                best = Some((s, a, b));
                            }
                        }
                    }
                    let Some((_, a, b)) = best else { break };
                    for k in [a, b] {
                        if m.faces[k].v.is_none() {
                            m.faces[k].v = self.vector(m.faces[k].id)?;
                        }
                    }
                    let others: Vec<i64> = (0..m.faces.len())
                        .filter(|&k| k != a && k != b && !m.goes(k))
                        .filter_map(|k| m.faces[k].person)
                        .collect();
                    let (old, new) = (&m.faces[a], &m.faces[b]);
                    let took = if new.by_hand { None } else { old.keeps_place(&c, new.v.as_deref(), &others) };
                    if let Some(p) = took {
                        let seen = p.seen || (p.person == new.person && new.seen);
                        let new = &mut m.faces[b];
                        (new.person, new.by_hand, new.seen) = (p.person, p.by_hand, seen);
                    }
                    m.pairs.push((a, b));
                    if !hidden.contains(&photo) {
                        let (old, new) = (&m.faces[a], &m.faces[b]);
                        mended.twice.push(FoundTwice {
                            photo_id: photo,
                            kept: new.id,
                            gone: old.id,
                            person_id: new.person,
                            by_hand: new.by_hand,
                            took_place: took.is_some(),
                        });
                    }
                }
                if !m.pairs.is_empty() {
                    work.push(m);
                }
            }
        }
        if !apply || work.is_empty() {
            return Ok(mended);
        }
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            for m in &work {
                for &(a, b) in &m.pairs {
                    follow_left(&tx, m.faces[a].id, m.faces[b].id)?;
                    tx.execute("DELETE FROM faces WHERE id = ?1", [m.faces[a].id])?;
                }
                for (k, (f, was)) in m.faces.iter().zip(&m.was).enumerate() {
                    if f.place() != *was && !m.goes(k) {
                        tx.execute(
                            "UPDATE faces SET person_id = ?1, by_hand = ?2, seen = ?3 WHERE id = ?4",
                            params![f.person, f.by_hand, f.seen, f.id],
                        )?;
                    }
                }
                tx.execute(
                    "UPDATE face_scan SET faces = (SELECT COUNT(*) FROM faces WHERE photo_id = ?1) WHERE photo_id = ?1",
                    [m.photo],
                )?;
            }
            tx.commit()?;
        }
        let gone: Vec<i64> = work.iter().flat_map(|m| m.pairs.iter().map(|(a, _)| m.faces[*a].id)).collect();
        self.forget_faces(cat, &gone)?;
        // Each person's average: without the faces gone, with the faces kept where they are now.
        let mut c = self.centroids.write().unwrap();
        for m in &work {
            for (k, (f, was)) in m.faces.iter().zip(&m.was).enumerate() {
                let Some(v) = &f.v else { continue };
                if (m.goes(k) || f.person != was.person)
                    && let Some(p) = was.person
                {
                    c.add(p, v, m.month, -1.0);
                }
                if !m.goes(k)
                    && f.person != was.person
                    && let Some(p) = f.person
                {
                    c.add(p, v, m.month, 1.0);
                }
            }
        }
        Ok(mended)
    }

    /// The faces of a photo now, as found before it is looked at again.
    fn faces_before(&self, cat: &Catalog, photo_id: i64) -> Result<Vec<Before>> {
        let rows: Vec<Before> = cat
            .db()
            .prepare("SELECT id, person_id, by_hand, seen, x1, y1, x2, y2 FROM faces WHERE photo_id = ?1 ORDER BY id")?
            .query_map([photo_id], |r| Before::from_row(r, 0))?
            .collect::<rusqlite::Result<_>>()?;
        rows.into_iter()
            .map(|mut b| {
                b.v = self.vector(b.id)?;
                Ok(b)
            })
            .collect()
    }

    /// Fingerprints and pictures of faces taken out of the catalog.
    fn forget_faces(&self, cat: &Catalog, face_ids: &[i64]) -> Result<()> {
        if face_ids.is_empty() {
            return Ok(());
        }
        {
            let mut emb = self.db.lock().unwrap();
            let etx = emb.transaction()?;
            for id in face_ids {
                etx.execute("DELETE FROM emb WHERE face_id = ?1", [id])?;
            }
            etx.commit()?;
        }
        for &id in face_ids {
            cat.remove_copy(&cat.data.face_path(id))?;
        }
        Ok(())
    }

    /// Everyone, named people first, then by how many photos they're in.
    pub fn people(&self, cat: &Catalog) -> Result<Vec<Person>> {
        self.ready.wait()?;
        self.kept_people.get(cat, String::new(), || self.people_now(cat))
    }

    /// Forgets the people list kept, so the next one is worked out again (to time it).
    pub fn forget_kept(&self) {
        self.kept_people.clear();
    }

    fn people_now(&self, cat: &Catalog) -> Result<Vec<Person>> {
        let db = cat.read()?;
        // Counted from the photos shown: someone seen only in private photos,
        // while they are hidden, isn't listed.
        let people = db
            .prepare(&format!(
                "SELECT p.id, p.name, COUNT(f.id), COUNT(DISTINCT f.photo_id),
                        (SELECT f2.id FROM faces f2 WHERE f2.person_id = p.id AND {} ORDER BY f2.score DESC LIMIT 1),
                        p.ignored, MAX(f.seen) = 0, COUNT(DISTINCT CASE WHEN f.seen = 0 THEN f.photo_id END)
                   FROM people p JOIN faces f ON f.person_id = p.id
                  WHERE {}
                  GROUP BY p.id
                  ORDER BY p.ignored, p.name IS NULL, COUNT(DISTINCT f.photo_id) DESC, p.id",
                cat.shown_face_sql("f2"),
                cat.shown_face_sql("f")
            ))?
            .query_map([], |r| {
                Ok(Person {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    faces: r.get(2)?,
                    photos: r.get(3)?,
                    cover: r.get(4)?,
                    ignored: r.get::<_, i64>(5)? != 0,
                    is_new: r.get(6)?,
                    new_photos: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(people)
    }

    /// People's search box: whoever's name matches what's typed, best first.
    pub fn find_people(&self, cat: &Catalog, query: &str) -> Result<Vec<Person>> {
        self.ready.wait()?;
        Ok(find(self.people(cat)?, query))
    }

    /// The user has seen these people's photos (they left their page): no longer new.
    /// Returns how many faces that was.
    pub fn mark_read(&self, cat: &Catalog, person_ids: &[i64]) -> Result<usize> {
        self.ready.wait()?;
        let mut db = cat.db();
        let tx = db.transaction()?;
        let mut n = 0;
        for id in person_ids {
            n += tx.execute("UPDATE faces SET seen = 1 WHERE person_id = ?1 AND seen = 0", [id])?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// "Mark as read" on one of People's groups: its people are no longer new.
    /// Returns how many faces that was.
    pub fn mark_group_read(&self, cat: &Catalog, group: PeopleGroup) -> Result<usize> {
        self.ready.wait()?;
        let ids: Vec<i64> =
            self.people(cat)?.into_iter().filter(|p| p.group() == group && p.has_news()).map(|p| p.id).collect();
        self.mark_read(cat, &ids)
    }

    /// "Mark all as read" in People: every face found so far has been seen.
    pub fn mark_all_read(&self, cat: &Catalog) -> Result<usize> {
        self.ready.wait()?;
        Ok(cat.db().execute("UPDATE faces SET seen = 1 WHERE seen = 0", [])?)
    }

    /// Names someone (an empty name: unnamed again). A name already taken
    /// stays two people called the same: only `merge` makes them one.
    pub fn rename(&self, cat: &Catalog, person_id: i64, name: &str) -> Result<()> {
        self.ready.wait()?;
        let name = name.trim();
        let n = cat.db().execute(
            "UPDATE people SET name = ?1 WHERE id = ?2",
            params![(!name.is_empty()).then_some(name), person_id],
        )?;
        if n == 0 {
            bail!("no person {person_id}");
        }
        Ok(())
    }

    /// Hides (or shows again) strangers. Their future faces still join them,
    /// so they never come back as someone new. All or nothing: an unknown
    /// id changes nobody.
    pub fn set_ignored(&self, cat: &Catalog, ids: &[i64], ignored: bool) -> Result<()> {
        self.ready.wait()?;
        let mut db = cat.db();
        let tx = db.transaction()?;
        for id in ids {
            if tx.execute("UPDATE people SET ignored = ?1 WHERE id = ?2", params![ignored as i64, id])? == 0 {
                bail!("no person {id}");
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// People found twice. A face joins someone only when it is
    /// `SAME_PERSON` alike to them at that moment, so the same person seen
    /// from another angle or in other light can start a second person: a
    /// second card in People, still there after the first is named or
    /// ignored. An unnamed person whose average face is that alike to
    /// someone else's joins them - whoever they match best; of two unnamed
    /// people, the smaller joins the bigger. Never moved: named and ignored
    /// people (the user's word) and people holding a face placed by hand.
    /// Two people seen in the same photo never join. Returns how many people
    /// joined someone.
    pub fn join_look_alikes(&self, cat: &Catalog) -> Result<usize> {
        self.ready.wait()?;
        self.join_now(cat)
    }

    /// [`FaceIndex::join_look_alikes`], also while the index is being read.
    ///
    /// Only who changed since the last look can have come close enough to
    /// someone: they are compared with everyone, the others only with them.
    /// After a few photos' faces that is a handful of people, not every
    /// person against every other (seconds on a big catalog).
    fn join_now(&self, cat: &Catalog) -> Result<usize> {
        let before = self.joined.lock().unwrap().take();
        // A join moves an average, which can bring someone else close enough:
        // again until nobody joins (two or three passes on a real catalog).
        let mut joined = 0;
        for _ in 0..10 {
            let (n, seen) = self.join_pass(cat, before.as_ref())?;
            joined += n;
            if n == 0 {
                *self.joined.lock().unwrap() = Some(seen);
                break;
            }
        }
        if joined > 0 {
            log::info!("faces: {joined} people found twice joined the person they are");
        }
        Ok(joined)
    }

    /// One round of joins. `before`: how everyone stood after the last look
    /// that found nobody (none: compare everyone). Returns how many joined
    /// someone, and how everyone stood for this round.
    fn join_pass(
        &self,
        cat: &Catalog,
        before: Option<&HashMap<i64, Standing>>,
    ) -> Result<(usize, HashMap<i64, Standing>)> {
        let (everyone, movable) = {
            let db = cat.db();
            let everyone: HashSet<i64> =
                db.prepare("SELECT id FROM people")?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            let movable: HashSet<i64> = db
                .prepare(
                    "SELECT id FROM people p WHERE p.name IS NULL AND p.ignored = 0
                       AND NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = p.id AND f.by_hand = 1)",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            (everyone, movable)
        };
        // Every join alike enough, best first: (similarity, who moves, to whom).
        // Who may move is compared, as a whole, with each person as they
        // looked over the months of their photos.
        let mut joins: Vec<(f32, i64, i64)> = Vec::new();
        // A copy: a face moved meanwhile ("who is this") doesn't wait for the comparing.
        let c = self.centroids.read().unwrap().clone();
        let seen: HashMap<i64, Standing> =
            c.life.keys().map(|p| (*p, Standing { faces: c.digest(*p), movable: movable.contains(p) })).collect();
        let changed: Option<HashSet<i64>> =
            before.map(|b| seen.iter().filter(|(p, s)| b.get(p) != Some(s)).map(|(p, _)| *p).collect());
        let is_changed = |p: &i64| changed.as_ref().is_none_or(|ch| ch.contains(p));
        {
            let size = |id: &i64| c.life.get(id).map_or(0, |(_, n)| *n);
            let bigger = |a: i64, b: i64| size(&a) > size(&b) || (size(&a) == size(&b) && a < b);
            // Grouped by their months, so each person's look then is worked out once.
            let mut by_months: BTreeMap<Option<(i64, i64)>, Vec<i64>> = BTreeMap::new();
            for (l, (_, n)) in &c.life {
                if *n > 0 && movable.contains(l) {
                    by_months.entry(c.months_of(*l)).or_default().push(*l);
                }
            }
            for (months, ls) in by_months {
                // Who changed, against everyone; the others, against who changed.
                let (fresh, settled): (Vec<i64>, Vec<i64>) = ls.into_iter().partition(|l| is_changed(l));
                for (ls, among) in [(fresh, None), (settled, changed.as_ref())] {
                    if ls.is_empty() || among.is_some_and(HashSet::is_empty) {
                        continue;
                    }
                    let averages: Vec<Vec<f32>> = ls.iter().map(|l| unit(c.life[l].0.clone())).collect();
                    let faces: Vec<&[f32]> = averages.iter().map(Vec::as_slice).collect();
                    let pairs = match among {
                        None => c.alike(&faces, months, &[], SAME_PERSON),
                        Some(among) => c.alike_among(among, &faces, months, &[], SAME_PERSON),
                    };
                    for (s, i, t) in pairs {
                        let l = ls[i];
                        if t != l && everyone.contains(&t) && (!movable.contains(&t) || bigger(t, l)) {
                            joins.push((s, l, t));
                        }
                    }
                }
            }
        }
        joins.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        // One step per pass: nobody joins someone who moves, nobody moves who
        // was joined.
        let mut joining: HashMap<i64, Vec<i64>> = HashMap::new();
        let mut moved: HashSet<i64> = HashSet::new();
        {
            // Each one's photos, read once they're needed.
            let db = cat.db();
            let mut stmt = db.prepare("SELECT photo_id FROM faces WHERE person_id = ?1")?;
            let mut photos: HashMap<i64, HashSet<i64>> = HashMap::new();
            let mut photos_of = |p: i64, photos: &mut HashMap<i64, HashSet<i64>>| -> Result<()> {
                if let std::collections::hash_map::Entry::Vacant(e) = photos.entry(p) {
                    e.insert(stmt.query_map([p], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?);
                }
                Ok(())
            };
            for (_, l, t) in joins {
                if moved.contains(&l) || moved.contains(&t) || joining.contains_key(&l) {
                    continue;
                }
                photos_of(l, &mut photos)?;
                photos_of(t, &mut photos)?;
                let theirs = photos[&l].clone();
                let target = photos.get_mut(&t).unwrap();
                // Nobody twice in one photo.
                if !target.is_disjoint(&theirs) {
                    continue;
                }
                target.extend(theirs);
                joining.entry(t).or_default().push(l);
                moved.insert(l);
            }
        }
        for (t, ls) in &joining {
            let mut ids = ls.clone();
            ids.push(*t);
            self.merge_now(cat, &ids, *t)?;
        }
        Ok((moved.len(), seen))
    }

    /// Makes several people one ("Same person"): the only way two people
    /// join - sharing a name never does it. Keeps `into`'s name, or the
    /// first name found. Returns what changed, for `unmerge` (Undo).
    pub fn merge(&self, cat: &Catalog, ids: &[i64], into: i64) -> Result<Merged> {
        self.ready.wait()?;
        self.merge_now(cat, ids, into)
    }

    /// [`FaceIndex::merge`], also while the index is being read.
    fn merge_now(&self, cat: &Catalog, ids: &[i64], into: i64) -> Result<Merged> {
        let others: Vec<i64> = ids.iter().copied().filter(|&i| i != into).collect();
        let mut merged = Merged { into, ..Default::default() };
        if others.is_empty() {
            return Ok(merged);
        }
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            let into_name: Option<String> = tx
                .query_row("SELECT name FROM people WHERE id = ?1", [into], |r| r.get(0))
                .optional()?
                .ok_or_else(|| anyhow!("no person {into}"))?;
            merged.name = into_name.clone();
            for o in &others {
                let row = tx
                    .query_row("SELECT name, ignored, created_at FROM people WHERE id = ?1", [o], |r| {
                        Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?))
                    })
                    .optional()?;
                let Some((name, ignored, created_at)) = row else { continue };
                let faces = tx
                    .prepare("SELECT id FROM faces WHERE person_id = ?1 ORDER BY id")?
                    .query_map([o], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                merged.joined.push(Joined { id: *o, name, ignored, created_at, faces });
            }
            if into_name.is_none()
                && let Some(n) = merged.joined.iter().find_map(|j| j.name.as_ref())
            {
                tx.execute("UPDATE people SET name = ?1 WHERE id = ?2", params![n, into])?;
            }
            for o in &others {
                tx.execute("UPDATE faces SET person_id = ?1 WHERE person_id = ?2", params![into, o])?;
                tx.execute("DELETE FROM people WHERE id = ?1", [o])?;
            }
            tx.commit()?;
        }
        self.centroids.write().unwrap().merge(into, &others);
        Ok(merged)
    }

    /// Undo of a merge: the people who joined are back as they were (their
    /// own id, unless someone new took it since), with those of their faces
    /// still in the merged person, and that person gets back the name it had.
    /// Returns how many faces went back.
    pub fn unmerge(&self, cat: &Catalog, merged: &Merged) -> Result<usize> {
        self.ready.wait()?;
        let mut back: Vec<(i64, i64)> = Vec::new();
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            for j in &merged.joined {
                let taken =
                    tx.query_row("SELECT 1 FROM people WHERE id = ?1", [j.id], |_| Ok(())).optional()?.is_some();
                let id = if taken {
                    tx.execute(
                        "INSERT INTO people (name, ignored, created_at) VALUES (?1, ?2, ?3)",
                        params![j.name, j.ignored as i64, j.created_at],
                    )?;
                    tx.last_insert_rowid()
                } else {
                    tx.execute(
                        "INSERT INTO people (id, name, ignored, created_at) VALUES (?1, ?2, ?3, ?4)",
                        params![j.id, j.name, j.ignored as i64, j.created_at],
                    )?;
                    j.id
                };
                for &fid in &j.faces {
                    let n = tx.execute(
                        "UPDATE faces SET person_id = ?1 WHERE id = ?2 AND person_id = ?3",
                        params![id, fid, merged.into],
                    )?;
                    if n > 0 {
                        back.push((fid, id));
                    }
                }
            }
            // The name it took, unless renamed since.
            if merged.name.is_none()
                && let Some(took) = merged.joined.iter().find_map(|j| j.name.as_ref())
            {
                tx.execute("UPDATE people SET name = NULL WHERE id = ?1 AND name = ?2", params![merged.into, took])?;
            }
            tx.commit()?;
        }
        for &(fid, to) in &back {
            if let Some((v, month)) = self.vector_and_month(cat, fid)? {
                let mut c = self.centroids.write().unwrap();
                c.add(merged.into, &v, month, -1.0);
                c.add(to, &v, month, 1.0);
            }
        }
        Ok(back.len())
    }

    /// The user says who faces are - one photo's face, or several picked on a
    /// person's page or in Clean up. Each face is placed by hand: regrouping
    /// never moves it again, so it can't drift back to the person it left.
    ///
    /// A person stays once per photo: if the face's new person was already in
    /// its photo, that other face makes room - it goes to whoever it resembles
    /// most among the people not in the photo, or becomes someone new. Faces
    /// Shoebox places itself (`Elsewhere`, and faces making room) count as new
    /// for the person they join, so they show up there to be checked.
    pub fn reassign(&self, cat: &Catalog, face_ids: &[i64], to: &FaceTarget) -> Result<Moved> {
        self.ready.wait()?;
        let face_ids = &shown_faces(cat, face_ids)?[..];
        let mut moved = Moved::default();
        // Fingerprint changes, applied to the people's averages once saved.
        let mut changes: Vec<(i64, Option<i64>, i64)> = Vec::new();
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            let new_person = match to {
                FaceTarget::New { name } => {
                    let name = name.as_deref().map(str::trim).filter(|n| !n.is_empty());
                    tx.execute(
                        "INSERT INTO people (name, created_at) VALUES (?1, ?2)",
                        params![name, crate::db::now()],
                    )?;
                    Some(tx.last_insert_rowid())
                }
                FaceTarget::Person { id } => {
                    let known: Option<i64> =
                        tx.query_row("SELECT id FROM people WHERE id = ?1", [id], |r| r.get(0)).optional()?;
                    known.ok_or_else(|| anyhow!("no person {id}"))?;
                    Some(*id)
                }
                FaceTarget::Elsewhere => None,
            };
            for &fid in face_ids {
                let row: Option<(i64, Option<i64>)> = tx
                    .query_row("SELECT photo_id, person_id FROM faces WHERE id = ?1", [fid], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
                let Some((photo, from)) = row else { continue };
                remember(&tx, &mut moved.undo, fid)?;
                let dest = match new_person {
                    Some(p) => p,
                    None => {
                        let mut taken = people_in(&tx, photo, fid)?;
                        taken.extend(from);
                        self.place(&tx, fid, &taken)?
                    }
                };
                if from == Some(dest) {
                    // Already them: now it's the user's word, not Shoebox's guess.
                    tx.execute("UPDATE faces SET by_hand = 1 WHERE id = ?1", [fid])?;
                    continue;
                }
                let seen = new_person.is_some() as i64;
                tx.execute(
                    "UPDATE faces SET person_id = ?1, by_hand = 1, seen = ?2 WHERE id = ?3",
                    params![dest, seen, fid],
                )?;
                changes.push((fid, from, dest));
                moved.faces += 1;
                if !moved.people.contains(&dest) {
                    moved.people.push(dest);
                }
                // Make room: `dest` was already in this photo.
                let others: Vec<i64> = tx
                    .prepare("SELECT id FROM faces WHERE photo_id = ?1 AND person_id = ?2 AND id <> ?3")?
                    .query_map(params![photo, dest, fid], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                for other in others {
                    remember(&tx, &mut moved.undo, other)?;
                    let taken = people_in(&tx, photo, other)?;
                    let to = self.place(&tx, other, &taken)?;
                    tx.execute(
                        "UPDATE faces SET person_id = ?1, by_hand = 0, seen = 0 WHERE id = ?2",
                        params![to, other],
                    )?;
                    changes.push((other, Some(dest), to));
                    moved.bumped.push(to);
                }
            }
            tx.commit()?;
        }
        for (fid, from, to) in changes {
            if let Some((v, month)) = self.vector_and_month(cat, fid)? {
                let mut c = self.centroids.write().unwrap();
                if let Some(from) = from {
                    c.add(from, &v, month, -1.0);
                }
                c.add(to, &v, month, 1.0);
            }
        }
        Ok(moved)
    }

    /// Undo: puts faces back as they were (`Moved::undo`). Returns how many
    /// changed.
    pub fn restore(&self, cat: &Catalog, placements: &[Placement]) -> Result<usize> {
        self.ready.wait()?;
        let mut changes: Vec<(i64, Option<i64>, Option<i64>)> = Vec::new();
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            for p in placements {
                let now: Option<Option<i64>> =
                    tx.query_row("SELECT person_id FROM faces WHERE id = ?1", [p.face_id], |r| r.get(0)).optional()?;
                let Some(now) = now else { continue };
                tx.execute(
                    "UPDATE faces SET person_id = ?1, by_hand = ?2, seen = ?3 WHERE id = ?4",
                    params![p.person_id, p.by_hand as i64, p.seen as i64, p.face_id],
                )?;
                changes.push((p.face_id, now, p.person_id));
            }
            tx.commit()?;
        }
        for &(fid, from, to) in &changes {
            if from == to {
                continue;
            }
            if let Some((v, month)) = self.vector_and_month(cat, fid)? {
                let mut c = self.centroids.write().unwrap();
                if let Some(from) = from {
                    c.add(from, &v, month, -1.0);
                }
                if let Some(to) = to {
                    c.add(to, &v, month, 1.0);
                }
            }
        }
        Ok(changes.len())
    }

    /// Who a face goes to when the user doesn't say: whoever it resembles most
    /// around when its photo was taken, other than `taken`, or someone new.
    fn place(&self, tx: &rusqlite::Transaction, face_id: i64, taken: &[i64]) -> Result<i64> {
        let when: Option<i64> = tx
            .query_row(
                "SELECT p.taken_ts FROM faces f JOIN photos p ON p.id = f.photo_id WHERE f.id = ?1",
                [face_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let found = self
            .vector(face_id)?
            .and_then(|v| self.centroids.read().unwrap().choose(&[v.as_slice()], month_of(when), taken)[0]);
        if let Some(p) = found {
            return Ok(p);
        }
        tx.execute("INSERT INTO people (name, created_at) VALUES (NULL, ?1)", [crate::db::now()])?;
        Ok(tx.last_insert_rowid())
    }

    /// The people these faces look most like, best first, to suggest who
    /// they are, not counting `except`: each person as they looked around
    /// the months of the faces' photos, as grouping compares them.
    pub fn look_alikes(&self, cat: &Catalog, face_ids: &[i64], except: &[i64], n: usize) -> Result<Vec<LookAlike>> {
        self.ready.wait()?;
        let db = cat.read()?;
        let mut taken =
            db.prepare("SELECT p.taken_ts FROM faces f JOIN photos p ON p.id = f.photo_id WHERE f.id = ?1")?;
        let (mut vs, mut months) = (Vec::new(), None::<(i64, i64)>);
        for &fid in face_ids {
            let Some(v) = self.vector(fid)? else { continue };
            vs.push(v);
            let when: Option<i64> = taken.query_row([fid], |r| r.get(0)).optional()?.flatten();
            if let Some(m) = month_of(when) {
                months = Some(months.map_or((m, m), |(a, b)| (a.min(m), b.max(m))));
            }
        }
        if vs.is_empty() {
            return Ok(Vec::new());
        }
        let face = mean(&vs.iter().map(Vec::as_slice).collect::<Vec<_>>());
        let hidden = hidden_people(cat, &db)?;
        let mut out: Vec<LookAlike> = self
            .centroids
            .read()
            .unwrap()
            .alike(&[&face], months, except, LOOKS_LIKE)
            .into_iter()
            .filter(|(_, _, person_id)| !hidden.contains(person_id))
            .map(|(similarity, _, person_id)| LookAlike { person_id, similarity })
            .collect();
        out.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
        out.truncate(n);
        Ok(out)
    }

    /// Every face of a person, the least like them first: how much each looks
    /// like the person's dominant face - the average of the faces close to
    /// the average, refined a few times, so a minority of strangers can't
    /// pull it their way. Faces without a fingerprint are left out.
    pub fn person_faces(&self, cat: &Catalog, person_id: i64) -> Result<Vec<Stray>> {
        self.ready.wait()?;
        let faces: Vec<(i64, i64, bool)> = cat
            .read()?
            .prepare(&format!(
                "SELECT id, photo_id, by_hand FROM faces WHERE person_id = ?1 AND {} ORDER BY id",
                cat.shown_photo_sql("photo_id")
            ))?
            .query_map([person_id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut faces_v = Vec::new();
        for (fid, photo, by_hand) in faces {
            if let Some(v) = self.vector(fid)? {
                faces_v.push((fid, photo, by_hand, v));
            }
        }
        if faces_v.is_empty() {
            return Ok(Vec::new());
        }
        let centre = dominant(&faces_v.iter().map(|f| f.3.as_slice()).collect::<Vec<_>>());
        let mut out: Vec<Stray> = faces_v
            .iter()
            .map(|f| Stray { face_id: f.0, photo_id: f.1, similarity: dot(&f.3, &centre), by_hand: f.2 })
            .collect();
        out.sort_by(|a, b| a.similarity.total_cmp(&b.similarity));
        Ok(out)
    }

    /// Faces in a person that don't look like the rest of them (a group named
    /// before the grouping rules were tightened can hold other people), worst
    /// first. Faces placed by hand are never offered, and a person with fewer
    /// than 3 faces has no "rest of them" to compare with.
    pub fn strays(&self, cat: &Catalog, person_id: i64) -> Result<Vec<Stray>> {
        self.ready.wait()?;
        let all = self.person_faces(cat, person_id)?;
        if all.len() < 3 {
            return Ok(Vec::new());
        }
        Ok(all.into_iter().filter(|s| !s.by_hand && s.similarity < SAME_PERSON).collect())
    }

    /// "Find more": faces in other people that look more like this person
    /// than like the rest of the person they're in - Clean up the other way
    /// round. Both as they looked around the date of the face's photo, as
    /// grouping compares them: a baby's faces are found by the baby's own
    /// months, not by an average over their childhood. Faces placed by hand
    /// are offered too: the user decides, face by face, and nothing moves
    /// until they do. Grouped by the day the photo was taken: to the model
    /// babies all look alike, while a person looks like themselves around the
    /// same time, so the days near the person's own photos come first and
    /// start ticked. A photo they're already in is left out (a person stays
    /// once per photo).
    pub fn more_of(&self, cat: &Catalog, person_id: i64) -> Result<Vec<CandidateDay>> {
        self.ready.wait()?;
        let db = cat.read()?;
        // (face, person, photo, by hand, taken)
        let faces: Vec<(i64, i64, i64, bool, Option<i64>)> = db
            .prepare(&format!(
                "SELECT f.id, f.person_id, f.photo_id, f.by_hand, p.taken_ts FROM faces f
                   JOIN photos p ON p.id = f.photo_id WHERE f.person_id IS NOT NULL AND {} ORDER BY f.id",
                cat.shown_sql("p")
            ))?
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0, r.get::<_, Option<i64>>(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let theirs: HashSet<i64> = faces.iter().filter(|f| f.1 == person_id).map(|f| f.2).collect();

        // How they looked around each month the other faces were taken: the
        // nearest months holding enough of their faces, else their whole life
        // (also for a photo with no date).
        let looks: HashMap<Option<i64>, Vec<f32>> = {
            let c = self.centroids.read().unwrap();
            let life = match c.get(&person_id) {
                Some((sum, n)) if *n > 0 => unit(sum.clone()),
                _ => return Ok(Vec::new()),
            };
            let months: HashSet<Option<i64>> = faces.iter().map(|f| month_of(f.4)).collect();
            months
                .into_iter()
                .map(|m| (m, m.and_then(|m| c.around(person_id, (m, m))).unwrap_or_else(|| life.clone())))
                .collect()
        };

        // Alike enough to be them, as (which of `faces`, likeness, fingerprint):
        // one pass over the fingerprints, in face order.
        let mut alike: Vec<(usize, f32, Vec<f32>)> = Vec::new();
        {
            let mut faces =
                faces.iter().enumerate().filter(|(_, f)| f.1 != person_id && !theirs.contains(&f.2)).peekable();
            let emb = self.db.lock().unwrap();
            let mut stmt = emb.prepare("SELECT face_id, vec FROM emb ORDER BY face_id")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let fid: i64 = r.get(0)?;
                while faces.next_if(|(_, f)| f.0 < fid).is_some() {}
                let Some((i, f)) = faces.next_if(|(_, f)| f.0 == fid) else { continue };
                let blob = r.get_ref(1)?.as_blob()?;
                if blob.len() != DIM * 4 {
                    continue;
                }
                let v: Vec<f32> = blob.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
                let s = dot(&v, &looks[&month_of(f.4)]);
                if s >= SAME_PERSON {
                    alike.push((i, s, v));
                }
            }
        }

        // More like them than like the rest of their own person around then,
        // or over its whole life (a face alone in its person has no rest to
        // compare with).
        let mut found: Vec<(Candidate, Option<i64>)> = Vec::new();
        {
            let c = self.centroids.read().unwrap();
            for (i, s, v) in alike {
                let (fid, pid, photo, by_hand, taken) = faces[i];
                let rest_of = |sum: &[f32], n: u32| {
                    (n > 1).then(|| {
                        let rest: Vec<f32> = sum.iter().zip(&v).map(|(a, b)| a - b).collect();
                        let norm = rest.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::EPSILON);
                        dot(&rest, &v) / norm
                    })
                };
                let now = match month_of(taken).and_then(|m| c.around_sum(pid, (m, m))) {
                    Some((sum, n)) => rest_of(&sum, n),
                    None => c.get(&pid).and_then(|(sum, n)| rest_of(sum, *n)),
                };
                if now.is_none_or(|n| s > n) {
                    let f = Candidate { face_id: fid, photo_id: photo, person_id: pid, similarity: s, now, by_hand };
                    found.push((f, taken.filter(|t| *t > 0)));
                }
            }
        }

        // By day, and near the person's own photos or not.
        let mut own: Vec<i64> =
            faces.iter().filter(|f| f.1 == person_id).filter_map(|f| f.4.filter(|t| *t > 0)).collect();
        own.sort_unstable();
        let near = |ts: i64| near_in_time(&own, ts);
        let mut days: HashMap<Option<String>, CandidateDay> = HashMap::new();
        for (f, ts) in found {
            let day = ts.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.format("%Y-%m-%d").to_string());
            let d = days.entry(day.clone()).or_insert_with(|| CandidateDay { day, near: false, faces: Vec::new() });
            d.near |= ts.is_some_and(near);
            d.faces.push(f);
        }
        let mut days: Vec<CandidateDay> = days.into_values().collect();
        for d in &mut days {
            d.faces.sort_by(|a, b| b.similarity.total_cmp(&a.similarity).then(a.face_id.cmp(&b.face_id)));
        }
        let best = |d: &CandidateDay| d.faces[0].similarity;
        days.sort_by(|a, b| b.near.cmp(&a.near).then(best(b).total_cmp(&best(a))).then(a.day.cmp(&b.day)));
        Ok(days)
    }

    /// "Check" in People: likely mix-ups among named people ([`PeopleCheck`]),
    /// but those left as is ([`leave_as_is`]) unless `all`. Every named face's
    /// fingerprint is read once, in face order, on a connection of its own:
    /// faces found or moved meanwhile don't wait.
    pub fn check_people(&self, cat: &Catalog, all: bool) -> Result<PeopleCheck> {
        self.ready.wait()?;
        let (names, faces, seen) = {
            let db = cat.read()?;
            // Findings left as is, as they were seen.
            let seen: HashMap<(&'static str, i64, i64), LeftAsIs> = db
                .prepare("SELECT seen FROM check_left")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .iter()
                .filter_map(|s| serde_json::from_str::<LeftAsIs>(s).ok())
                .map(|l| (l.key(), l))
                .collect();
            let hidden = hidden_people(cat, &db)?;
            let names: HashMap<i64, String> = db
                .prepare("SELECT id, name FROM people WHERE name IS NOT NULL AND ignored = 0")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<HashMap<i64, String>>>()?
                .into_iter()
                .filter(|(id, _)| !hidden.contains(id))
                .collect();
            // (face, person, photo, by hand, when taken), in face order.
            let faces: Vec<(i64, i64, i64, bool, Option<i64>)> = db
                .prepare(&format!(
                    "SELECT f.id, f.person_id, f.photo_id, f.by_hand, ph.taken_ts FROM faces f
                       JOIN people p ON p.id = f.person_id JOIN photos ph ON ph.id = f.photo_id
                      WHERE p.name IS NOT NULL AND p.ignored = 0 AND {} ORDER BY f.id",
                    cat.shown_sql("ph")
                ))?
                .query_map([], |r| {
                    let taken: Option<i64> = r.get(4)?;
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0, taken.filter(|t| *t > 0)))
                })?
                .collect::<rusqlite::Result<_>>()?;
            (names, faces, seen)
        };

        // The named people with faces, by index, and their average faces side
        // by side: every face is compared with all of them.
        let (people, sums) = {
            let c = self.centroids.read().unwrap();
            let mut people: Vec<i64> =
                names.keys().copied().filter(|id| c.get(id).is_some_and(|(_, n)| *n > 0)).collect();
            people.sort_unstable();
            let sums: Vec<(Vec<f32>, u32)> = people.iter().map(|p| c[p].clone()).collect();
            (people, sums)
        };
        let index: HashMap<i64, usize> = people.iter().enumerate().map(|(i, p)| (*p, i)).collect();
        let averages: Vec<f32> = sums.iter().flat_map(|(s, _)| unit(s.clone())).collect();
        let sum_sq: Vec<f64> = sums.iter().map(|(s, _)| dot_lanes(s, s) as f64).collect();

        // Per person: their photos, when they were taken, faces per year; and
        // the photos they're in more than once.
        let k = people.len();
        let mut counts: Vec<usize> = vec![0; k];
        let mut photos: Vec<HashSet<i64>> = vec![HashSet::new(); k];
        let mut taken: Vec<Vec<i64>> = vec![Vec::new(); k];
        let mut per_year: HashMap<(usize, i32), u32> = HashMap::new();
        let mut in_photo: HashMap<(usize, i64), Vec<i64>> = HashMap::new();
        for &(fid, pid, photo, _, ts) in &faces {
            let Some(&pi) = index.get(&pid) else { continue };
            counts[pi] += 1;
            photos[pi].insert(photo);
            taken[pi].extend(ts);
            if let Some(y) = ts.and_then(year_of) {
                *per_year.entry((pi, y)).or_default() += 1;
            }
            in_photo.entry((pi, photo)).or_default().push(fid);
        }
        taken.iter_mut().for_each(|t| t.sort_unstable());
        let twice_of: HashMap<i64, usize> = {
            let mut groups: Vec<&Vec<i64>> = in_photo.values().filter(|f| f.len() > 1).collect();
            groups.sort();
            groups.iter().enumerate().flat_map(|(g, fs)| fs.iter().map(move |f| (*f, g))).collect()
        };
        // Years worth an average (enough faces), compared with such a year far enough off.
        let mut full_years: Vec<Vec<i32>> = vec![Vec::new(); k];
        for (&(pi, y), &n) in &per_year {
            if n >= CHECK_YEAR_FACES {
                full_years[pi].push(y);
            }
        }
        let compared = |pi: usize, y: i32| {
            full_years[pi].contains(&y) && full_years[pi].iter().any(|o| (o - y).abs() >= CHECK_YEARS_APART)
        };

        // One pass over the fingerprints, each batch compared with everyone on
        // every core - a pool of the check's own, not the one reading photos.
        struct Read {
            face: CheckFace,
            /// Likeness to the rest of their person; none when it is all they have.
            own: Option<f32>,
            pid: i64,
            pi: usize,
            taken: Option<i64>,
            v: Vec<f32>,
        }
        let pool = rayon::ThreadPoolBuilder::new().thread_name(|i| format!("people-check-{i}")).build()?;
        let mut checked = 0;
        let mut twice: Vec<Vec<CheckFace>> = vec![Vec::new(); twice_of.values().max().map_or(0, |g| g + 1)];
        let mut owners: Vec<i64> = vec![0; twice.len()];
        let mut like_others: Vec<LikeOther> = Vec::new();
        let mut when: HashMap<i64, i64> = HashMap::new();
        let mut year_sums: HashMap<(usize, i32), (Vec<f32>, u32)> = HashMap::new();
        // Looked at again once every year's average is known.
        let mut aged: Vec<(usize, i32, CheckFace, Small)> = Vec::new();
        let mut take = |batch: &mut Vec<Read>| {
            // Whom else each looks like most.
            let best: Vec<(usize, f32)> = pool.install(|| {
                batch
                    .par_iter()
                    .map(|f| {
                        let mut best = (f.pi, f32::MIN);
                        for (o, avg) in averages.as_chunks::<DIM>().0.iter().enumerate() {
                            let s = dot_lanes(avg, &f.v);
                            if o != f.pi && s > best.1 {
                                best = (o, s);
                            }
                        }
                        best
                    })
                    .collect()
            });
            for (f, (o, other)) in batch.drain(..).zip(best) {
                let fid = f.face.face_id;
                if let Some(&g) = twice_of.get(&fid) {
                    twice[g].push(f.face.clone());
                    owners[g] = f.pid;
                }
                if let Some(own) = f.own
                    && other >= SAME_PERSON
                    && other >= own + CHECK_OTHER_GAP
                {
                    like_others.push(LikeOther {
                        person_id: f.pid,
                        face: f.face.clone(),
                        other_id: people[o],
                        other,
                        near: false,
                        left: false,
                    });
                    when.extend(f.taken.map(|t| (fid, t)));
                }
                if let Some(y) = f.taken.and_then(year_of)
                    && compared(f.pi, y)
                {
                    let e = year_sums.entry((f.pi, y)).or_insert_with(|| (vec![0.0; DIM], 0));
                    e.0.iter_mut().zip(&f.v).for_each(|(s, x)| *s += x);
                    e.1 += 1;
                    aged.push((f.pi, y, f.face, Small::of(&f.v)));
                }
            }
        };
        {
            let emb = Connection::open(cat.data.root.join("faces.db"))?;
            emb.busy_timeout(std::time::Duration::from_secs(30))?;
            emb.pragma_update(None, "query_only", true)?;
            let mut stmt = emb.prepare("SELECT face_id, vec FROM emb ORDER BY face_id")?;
            let mut rows = stmt.query([])?;
            let mut next = faces.iter().peekable();
            let mut batch: Vec<Read> = Vec::with_capacity(4096);
            while let Some(r) = rows.next()? {
                let fid: i64 = r.get(0)?;
                while next.next_if(|f| f.0 < fid).is_some() {}
                let Some(&(_, pid, photo_id, by_hand, taken)) = next.next_if(|f| f.0 == fid) else { continue };
                let Some(&pi) = index.get(&pid) else { continue };
                let blob = r.get_ref(1)?.as_blob()?;
                if blob.len() != DIM * 4 {
                    continue;
                }
                let v: Vec<f32> = blob.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
                checked += 1;
                let (sum, n) = &sums[pi];
                let own = rest_like(*n, sum_sq[pi], dot_lanes(sum, &v), dot_lanes(&v, &v));
                let face = CheckFace { face_id: fid, photo_id, similarity: own.unwrap_or(0.0), by_hand };
                batch.push(Read { face, own, pid, pi, taken, v });
                if batch.len() == batch.capacity() {
                    take(&mut batch);
                }
            }
            take(&mut batch);
        }

        // Age jumps: unlike their own year, like a year far off.
        let mut years: HashMap<usize, Vec<(i32, Vec<f32>)>> = HashMap::new();
        for ((pi, y), (s, n)) in &year_sums {
            if *n >= CHECK_YEAR_FACES {
                years.entry(*pi).or_default().push((*y, unit(s.clone())));
            }
        }
        let mut age_jumps: Vec<AgeJump> = Vec::new();
        for (pi, y, face, v) in aged {
            let (s, n) = &year_sums[&(pi, y)];
            if *n < CHECK_YEAR_FACES {
                continue;
            }
            let Some(own) = rest_like(*n, dot_lanes(s, s) as f64, v.dot(s), v.dot_self()) else { continue };
            let far = years.get(&pi).into_iter().flatten().filter(|(o, _)| (o - y).abs() >= CHECK_YEARS_APART);
            let Some((like_year, like)) =
                far.map(|(o, avg)| (*o, v.dot(avg))).max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            else {
                continue;
            };
            if like >= CHECK_YEAR_LIKE && own <= like - CHECK_YEAR_GAP {
                age_jumps.push(AgeJump { person_id: people[pi], face, year: y, own, like_year, like, left: false });
            }
        }

        // Near: within NEAR_DAYS of a photo of the one they look like.
        for l in &mut like_others {
            let theirs = &taken[index[&l.other_id]];
            l.near = when.get(&l.face.face_id).is_some_and(|&ts| near_in_time(theirs, ts));
        }

        // Each face in one finding at most, the surest first.
        let twice: Vec<Twice> = {
            let mut out: Vec<Twice> = twice
                .into_iter()
                .zip(owners)
                .filter(|(fs, _)| fs.len() > 1)
                .map(|(mut fs, person_id)| {
                    fs.sort_by(|a, b| b.similarity.total_cmp(&a.similarity).then(a.face_id.cmp(&b.face_id)));
                    Twice {
                        person_id,
                        photo_id: fs[0].photo_id,
                        elsewhere: fs[1..].iter().map(|f| f.face_id).collect(),
                        faces: fs,
                        left: false,
                    }
                })
                .collect();
            out.sort_by_key(|t| (t.person_id, t.photo_id));
            out
        };
        let mut used: HashSet<i64> = twice.iter().flat_map(|t| t.faces.iter().map(|f| f.face_id)).collect();
        let (near, far): (Vec<LikeOther>, Vec<LikeOther>) =
            like_others.into_iter().filter(|l| !used.contains(&l.face.face_id)).partition(|l| l.near);
        used.extend(near.iter().map(|l| l.face.face_id));
        age_jumps.retain(|a| !used.contains(&a.face.face_id));
        used.extend(age_jumps.iter().map(|a| a.face.face_id));
        let mut like_others: Vec<LikeOther> = near;
        like_others.extend(far.into_iter().filter(|l| !used.contains(&l.face.face_id)));
        let margin = |l: &LikeOther| l.other - l.face.similarity;
        like_others.sort_by(|a, b| {
            b.near.cmp(&a.near).then(margin(b).total_cmp(&margin(a))).then(a.face.face_id.cmp(&b.face.face_id))
        });
        let gap = |a: &AgeJump| a.like - a.own;
        age_jumps.sort_by(|a, b| {
            a.person_id.cmp(&b.person_id).then(gap(b).total_cmp(&gap(a))).then(a.face.face_id.cmp(&b.face.face_id))
        });

        // Two people who may be one.
        let spelled: Vec<String> = people.iter().map(|p| words(&names[p]).join(" ")).collect();
        let mut same_people: Vec<SamePerson> = Vec::new();
        for a in 0..k {
            for b in a + 1..k {
                let similarity = dot_lanes(&averages[a * DIM..(a + 1) * DIM], &averages[b * DIM..(b + 1) * DIM]);
                let same_name = !spelled[a].is_empty() && spelled[a] == spelled[b];
                if similarity < CHECK_SAME_PERSON && !same_name {
                    continue;
                }
                let together = photos[a].intersection(&photos[b]).count();
                if together > 0 && !same_name {
                    continue;
                }
                // The one with more faces keeps their name; of two as big, the first.
                let (keep, join) = if counts[b] > counts[a] { (b, a) } else { (a, b) };
                same_people.push(SamePerson {
                    person_id: people[keep],
                    other_id: people[join],
                    similarity,
                    same_name,
                    together,
                    left: false,
                });
            }
        }
        same_people.sort_by(|a, b| {
            b.similarity.total_cmp(&a.similarity).then((a.person_id, a.other_id).cmp(&(b.person_id, b.other_id)))
        });

        // Left as is: still holding their faces above, left out here.
        let mut twice = twice;
        let left = left_out(&mut twice, &seen, all, |t| &mut t.left)
            + left_out(&mut age_jumps, &seen, all, |a| &mut a.left)
            + left_out(&mut like_others, &seen, all, |l| &mut l.left)
            + left_out(&mut same_people, &seen, all, |s| &mut s.left);
        Ok(PeopleCheck { people: k, faces: checked, left, twice, age_jumps, like_others, same_people })
    }

    /// The faces found in one photo, for the viewer.
    pub fn photo_faces(&self, cat: &Catalog, photo_id: i64) -> Result<Vec<PhotoFace>> {
        self.ready.wait()?;
        self.faces_in(cat, &[photo_id])
    }

    /// The faces found in some photos, photo by photo, left to right.
    pub fn faces_in(&self, cat: &Catalog, photo_ids: &[i64]) -> Result<Vec<PhotoFace>> {
        self.ready.wait()?;
        let photo_ids = cat.shown_ids(photo_ids)?;
        let db = cat.read()?;
        let mut stmt = db.prepare(
            "SELECT f.id, f.photo_id, f.person_id, p.name, f.x1, f.y1, f.x2, f.y2 FROM faces f
               LEFT JOIN people p ON p.id = f.person_id WHERE f.photo_id = ?1 ORDER BY f.x1",
        )?;
        let mut out = Vec::new();
        for photo in photo_ids {
            let faces = stmt.query_map([photo], |r| {
                Ok(PhotoFace {
                    id: r.get(0)?,
                    photo_id: r.get(1)?,
                    person_id: r.get(2)?,
                    name: r.get(3)?,
                    bbox: [r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?],
                })
            })?;
            for f in faces {
                out.push(f?);
            }
        }
        Ok(out)
    }
}

/// Notes where a face is before it changes (once: the first state is the one
/// Undo goes back to).
fn remember(tx: &rusqlite::Transaction, undo: &mut Vec<Placement>, face_id: i64) -> Result<()> {
    if undo.iter().any(|p| p.face_id == face_id) {
        return Ok(());
    }
    let p = tx.query_row("SELECT person_id, by_hand, seen FROM faces WHERE id = ?1", [face_id], |r| {
        Ok(Placement {
            face_id,
            person_id: r.get(0)?,
            by_hand: r.get::<_, i64>(1)? != 0,
            seen: r.get::<_, i64>(2)? != 0,
        })
    })?;
    undo.push(p);
    Ok(())
}

/// The people in a photo, but for one of its faces.
fn people_in(tx: &rusqlite::Transaction, photo: i64, except_face: i64) -> Result<Vec<i64>> {
    Ok(tx
        .prepare("SELECT person_id FROM faces WHERE photo_id = ?1 AND person_id IS NOT NULL AND id <> ?2")?
        .query_map(params![photo, except_face], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Who a face is: its person, whether the user put it there, whether they
/// have seen it there.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Place {
    person: Option<i64>,
    by_hand: bool,
    seen: bool,
}

/// A face already in a photo when it is found again: one a turn carried over
/// from when the photo stood the wrong way ([`turn_faces`]).
struct Before {
    id: i64,
    person: Option<i64>,
    by_hand: bool,
    seen: bool,
    /// 0..1 of the preview.
    bbox: [f32; 4],
    /// Its fingerprint, as found then.
    v: Option<Vec<f32>>,
}

impl Before {
    /// From `id, person_id, by_hand, seen, x1, y1, x2, y2`, from column `at` on.
    fn from_row(r: &rusqlite::Row, at: usize) -> rusqlite::Result<Self> {
        let bbox = |k: usize| r.get::<_, f64>(at + k).map(|x| x as f32);
        Ok(Self {
            id: r.get(at)?,
            person: r.get(at + 1)?,
            by_hand: r.get::<_, i64>(at + 2)? != 0,
            seen: r.get::<_, i64>(at + 3)? != 0,
            bbox: [bbox(4)?, bbox(5)?, bbox(6)?, bbox(7)?],
            v: None,
        })
    }

    fn place(&self) -> Place {
        Place { person: self.person, by_hand: self.by_hand, seen: self.seen }
    }

    /// This face's place, for it found again with the fingerprint `v`, when
    /// it should keep it: always when the user put it there; when Shoebox
    /// did (from a fingerprint taken the wrong way up, often poor), only while
    /// the face still looks like the rest of that person, and that person
    /// isn't already someone else in the photo (`taken`). None: it is sorted
    /// again from `v`, as a new face.
    fn keeps_place(&self, c: &Averages, v: Option<&[f32]>, taken: &[i64]) -> Option<Place> {
        if self.by_hand {
            return Some(self.place());
        }
        let (person, v) = (self.person?, v?);
        let alike = c.like_rest(person, v, self.v.as_deref()).is_some_and(|s| s >= LOOKS_LIKE);
        (alike && !taken.contains(&person)).then(|| self.place())
    }
}

/// A turned photo's faces, as [`FaceIndex::mend_turned`] mends them.
struct Mending {
    photo: i64,
    month: Option<i64>,
    /// Each face's place as it was; `faces` hold them as they will be.
    was: Vec<Place>,
    faces: Vec<Before>,
    /// Each face found twice: (older, newer), the older to go.
    pairs: Vec<(usize, usize)>,
}

impl Mending {
    fn goes(&self, k: usize) -> bool {
        self.pairs.iter().any(|(a, _)| *a == k)
    }
}

/// Two boxes of one photo are the same face found twice: as much overlap as
/// two finds of one face ([`NMS_IOU`]), or each one's centre inside the
/// other, as a tight box inside the loose one a face gets when found while
/// its photo stood sideways. Two people side by side, or one behind another,
/// are neither, however much a loose box covers.
fn same_face(a: &[f32; 4], b: &[f32; 4]) -> bool {
    let centre_in = |r: &[f32; 4], s: &[f32; 4]| {
        let (x, y) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        x >= s[0] && x <= s[2] && y >= s[1] && y <= s[3]
    };
    iou(a, b) >= NMS_IOU || (centre_in(a, b) && centre_in(b, a))
}

/// Which of the faces found `before` each face found `now` is, if any
/// ([`same_face`]): the most overlapping pairs first, each face once.
fn found_again(before: &[[f32; 4]], now: &[[f32; 4]]) -> Vec<Option<usize>> {
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (i, n) in now.iter().enumerate() {
        for (j, b) in before.iter().enumerate() {
            if same_face(b, n) {
                pairs.push((iou(b, n), i, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut out = vec![None; now.len()];
    let mut used = HashSet::new();
    for (_, i, j) in pairs {
        if out[i].is_none() && used.insert(j) {
            out[i] = Some(j);
        }
    }
    out
}

/// What the user left as is about face `old` (its age jump, its look-alike)
/// is now about face `new`, the same face under a new id. One `new` already
/// has stays.
fn follow_left(tx: &Connection, old: i64, new: i64) -> Result<()> {
    tx.execute(
        "UPDATE OR IGNORE check_left SET id = ?1, seen = json_set(seen, '$.faceId', ?1)
          WHERE kind IN ('ageJump', 'likeOther') AND id = ?2",
        params![new, old],
    )?;
    tx.execute("DELETE FROM check_left WHERE kind IN ('ageJump', 'likeOther') AND id = ?1", [old])?;
    Ok(())
}

/// A point (0..1 of the picture) once the picture is turned clockwise by
/// `turn` quarter turns.
fn turn_point(x: f64, y: f64, turn: i64) -> (f64, f64) {
    match turn.rem_euclid(4) {
        1 => (1.0 - y, x),
        2 => (1.0 - x, 1.0 - y),
        3 => (y, 1.0 - x),
        _ => (x, y),
    }
}

/// The faces of a photo the user turned clockwise by `by` quarter turns, in
/// the caller's transaction. Each becomes a new face - a new id, so no window
/// keeps showing its old picture - with its box and landmarks turned, keeping
/// its person, how it was placed and whether it was seen; findings of the
/// people check left as is follow it. Returns (old id, new id) of each.
pub fn turn_faces(tx: &Connection, photo_id: i64, by: i64) -> Result<Vec<(i64, i64)>> {
    type Row = (i64, Option<i64>, [f64; 4], String, f64, i64, i64);
    let faces: Vec<Row> = tx
        .prepare(
            "SELECT id, person_id, x1, y1, x2, y2, kps, score, by_hand, seen FROM faces WHERE photo_id = ?1 ORDER BY id",
        )?
        .query_map([photo_id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?],
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut moved = Vec::new();
    for (old, person, b, kps, score, by_hand, seen) in faces {
        let (ax, ay) = turn_point(b[0], b[1], by);
        let (bx, by_) = turn_point(b[2], b[3], by);
        let points: Vec<f64> = kps.split(',').filter_map(|v| v.trim().parse().ok()).collect();
        let kps = points
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[x, y]| {
                let (x, y) = turn_point(x, y, by);
                [format!("{x:.4}"), format!("{y:.4}")]
            })
            .collect::<Vec<_>>()
            .join(",");
        tx.execute(
            "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score, by_hand, seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![photo_id, person, ax.min(bx), ay.min(by_), ax.max(bx), ay.max(by_), kps, score, by_hand, seen],
        )?;
        let new = tx.last_insert_rowid();
        follow_left(tx, old, new)?;
        tx.execute("DELETE FROM faces WHERE id = ?1", [old])?;
        moved.push((old, new));
    }
    Ok(moved)
}

/// faces.db's fingerprints of turned faces, copied to their new ids (see
/// [`turn_faces`]): done before the catalog's change is committed, so no face
/// is ever without its fingerprint. A fingerprint copied for a change that
/// never landed belongs to no face, and is never read.
pub fn copy_fingerprints(cat: &Catalog, moved: &[(i64, i64)]) -> Result<()> {
    let path = cat.data.root.join("faces.db");
    if moved.is_empty() || !path.is_file() {
        return Ok(());
    }
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    let tx = conn.transaction()?;
    for (old, new) in moved {
        tx.execute(
            "INSERT OR REPLACE INTO emb (face_id, vec) SELECT ?1, vec FROM emb WHERE face_id = ?2",
            params![new, old],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// After a turn has landed and the photo's preview is turned: the new faces'
/// pictures cut from it, the old faces' pictures and fingerprints gone.
pub fn after_turn(cat: &Catalog, moved: &[(i64, i64)]) -> Result<()> {
    if moved.is_empty() {
        return Ok(());
    }
    cut_pictures_again(cat, &moved.iter().map(|(_, new)| *new).collect::<Vec<_>>())?;
    for (old, _) in moved {
        cat.remove_copy(&cat.data.face_path(*old))?;
    }
    let path = cat.data.root.join("faces.db");
    if path.is_file() {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        let tx = conn.transaction()?;
        for (old, _) in moved {
            tx.execute("DELETE FROM emb WHERE face_id = ?1", [old])?;
        }
        tx.commit()?;
    }
    Ok(())
}

/// Cuts these faces' pictures again from their photo's preview, framed as when
/// they were found (a picture lost, or left damaged by a PC that stopped while
/// saving it). Returns the faces cut; one whose preview can't be read is left.
pub fn cut_pictures_again(cat: &Catalog, face_ids: &[i64]) -> Result<Vec<i64>> {
    let mut by_photo: HashMap<i64, Vec<(i64, [f64; 4])>> = HashMap::new();
    for &fid in face_ids {
        let face = cat
            .db()
            .query_row("SELECT photo_id, x1, y1, x2, y2 FROM faces WHERE id = ?1", [fid], |r| {
                Ok((r.get::<_, i64>(0)?, [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?]))
            })
            .optional()?;
        if let Some((photo, b)) = face {
            by_photo.entry(photo).or_default().push((fid, b));
        }
    }
    let mut cut = Vec::new();
    for (photo, faces) in by_photo {
        let img = match cat.open_copy(&cat.data.preview_path(photo)) {
            Ok(img) => img,
            Err(e) => {
                log::warn!("faces: photo {photo}: preview unreadable, face pictures not cut: {e:#}");
                continue;
            }
        };
        let (w, h) = (img.width() as f64, img.height() as f64);
        for (fid, b) in faces {
            let bbox = [(b[0] * w) as f32, (b[1] * h) as f32, (b[2] * w) as f32, (b[3] * h) as f32];
            match face_thumb(&img, &bbox) {
                Ok(bytes) => {
                    cat.write_copy(photo, &cat.data.face_path(fid), &bytes)?;
                    cut.push(fid);
                }
                Err(e) => log::warn!("faces: face {fid}: picture can't be cut: {e:#}"),
            }
        }
    }
    Ok(cut)
}

/// A square 160 px picture of the face, with some margin.
fn face_thumb(img: &DynamicImage, bbox: &[f32; 4]) -> Result<Vec<u8>> {
    let (w, h) = img.dimensions();
    let (cx, cy) = ((bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0);
    let side = ((bbox[2] - bbox[0]).max(bbox[3] - bbox[1]) * 1.6).min(w.min(h) as f32).max(8.0);
    let x0 = (cx - side / 2.0).clamp(0.0, (w as f32 - side).max(0.0));
    let y0 = (cy - side / 2.0).clamp(0.0, (h as f32 - side).max(0.0));
    let crop = img.crop_imm(x0 as u32, y0 as u32, side as u32, side as u32);
    let small = DynamicImage::ImageRgb8(crop.resize_exact(160, 160, FilterType::CatmullRom).to_rgb8());
    crate::imaging::encode_jpeg(&small, 85)
}

/// Finding someone by name, as you type. Every word typed starts a word of
/// their name, in any order ("smi jan" finds "Jane Smith"), or the text is
/// somewhere in it ("maryann" finds "Mary-Ann"). Case and accents
/// don't count ("zoe" finds "Zoé"). Names that start with the text
/// come first, then those whose words do, then the rest; among equals,
/// whoever is in more photos. Ignored people come last. Unnamed people have
/// no name to match, and typing nothing finds nobody.
pub fn find(people: Vec<Person>, query: &str) -> Vec<Person> {
    let q = words(query);
    if q.is_empty() {
        return Vec::new();
    }
    let (q_spaced, q_joined) = (q.join(" "), q.concat());
    let mut hits: Vec<(u8, Person)> = people
        .into_iter()
        .filter_map(|p| {
            let name = words(p.name.as_deref()?);
            let rank = if name.join(" ").starts_with(&q_spaced) {
                0
            } else if q.iter().all(|w| name.iter().any(|n| n.starts_with(w.as_str()))) {
                1
            } else if name.concat().contains(&q_joined) {
                2
            } else {
                return None;
            };
            Some((rank, p))
        })
        .collect();
    hits.sort_by_key(|(rank, p)| (p.ignored, *rank, std::cmp::Reverse(p.photos), p.id));
    hits.into_iter().map(|(_, p)| p).collect()
}

/// A name's words, lower case and without accents: "Mary-Ann Doe" ->
/// ["mary", "ann", "doe"].
fn words(s: &str) -> Vec<String> {
    use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
    let mut folded = String::new();
    for c in s.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase) {
        match c {
            // Letters with no accent to take off.
            'ø' => folded.push('o'),
            'ł' => folded.push('l'),
            'đ' => folded.push('d'),
            'ı' => folded.push('i'),
            'æ' => folded.push_str("ae"),
            'œ' => folded.push_str("oe"),
            'ß' => folded.push_str("ss"),
            _ => folded.push(c),
        }
    }
    folded.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: i64, name: Option<&str>, photos: i64, ignored: bool) -> Person {
        Person {
            id,
            name: name.map(str::to_string),
            faces: photos,
            photos,
            cover: id,
            ignored,
            is_new: false,
            new_photos: 0,
        }
    }

    fn found(people: &[Person], query: &str) -> Vec<i64> {
        find(people.to_vec(), query).iter().map(|p| p.id).collect()
    }

    #[test]
    fn a_person_is_found_by_any_part_of_their_name() {
        let people = [
            person(1, Some("Jane Smith"), 283, false),
            person(2, Some("Mary-Ann Doe"), 21, false),
            person(3, Some("Zoé Bœuf"), 5, false),
            person(4, None, 400, false),
        ];
        assert_eq!(found(&people, "jane"), [1]);
        assert_eq!(found(&people, "SMI"), [1]);
        assert_eq!(found(&people, "smi jan"), [1]);
        assert_eq!(found(&people, "ann"), [2]);
        assert_eq!(found(&people, "mary ann"), [2]);
        assert_eq!(found(&people, "maryann"), [2]);
        assert_eq!(found(&people, "doe"), [2]);
        assert_eq!(found(&people, "zoe boeuf"), [3]);
        assert_eq!(found(&people, "  Zoé  "), [3]);
        assert_eq!(found(&people, "ith"), [1]);
        assert!(found(&people, "zz").is_empty());
        assert!(found(&people, "").is_empty());
        assert!(found(&people, " - ").is_empty());
    }

    #[test]
    fn the_best_match_comes_first_and_ignored_people_last() {
        let people = [
            person(1, Some("Omar Doe"), 30, false),
            person(2, Some("Jane Marlow"), 105, false),
            person(3, Some("Mara"), 2, false),
            person(4, Some("Marie Roe"), 50, false),
            person(5, Some("Marco"), 900, true),
        ];
        // Names starting "mar" (most photos first), then a word starting
        // "mar", then "mar" inside a word, then the ignored one.
        assert_eq!(found(&people, "mar"), [4, 3, 2, 1, 5]);
    }

    /// A unit vector mostly along axis `a`, with a little of axis `b`.
    fn near(a: usize, b: usize, t: f32) -> Vec<f32> {
        let mut v = vec![0.0; 8];
        v[a] = 1.0;
        v[b] = t;
        unit(v)
    }

    #[test]
    fn a_minority_of_strangers_doesnt_move_the_person() {
        // Seven faces of one person, three of someone else.
        let mut vs: Vec<Vec<f32>> = (0..7).map(|i| near(0, 2 + i % 3, 0.2)).collect();
        vs.extend((0..3).map(|i| near(1, 5 + i, 0.2)));
        let refs: Vec<&[f32]> = vs.iter().map(Vec::as_slice).collect();
        let c = dominant(&refs);
        assert!(vs[..7].iter().all(|v| dot(v, &c) >= SAME_PERSON));
        assert!(vs[7..].iter().all(|v| dot(v, &c) < SAME_PERSON));
    }

    /// Boxes of real turned photos (0..1 of the preview): one found while the
    /// photo stood sideways, one once it was upright, and whether they are one
    /// face, as seen on their pictures. In all of them the upright box is
    /// mostly inside the sideways one (60% of it or more), and their overlap
    /// is under NMS_IOU.
    const TURNED_PAIRS: [([f32; 4], [f32; 4], bool); 12] = [
        // A loose box around a tight one.
        ([0.476, 0.267, 0.609, 0.392], [0.519, 0.290, 0.595, 0.349], true),
        ([0.684, 0.259, 0.809, 0.374], [0.699, 0.273, 0.774, 0.340], true),
        ([0.375, 0.085, 0.548, 0.224], [0.436, 0.122, 0.533, 0.206], true),
        ([0.074, 0.091, 0.206, 0.202], [0.093, 0.109, 0.158, 0.164], true),
        // A box around two people, and each of them.
        ([0.147, 0.124, 0.382, 0.309], [0.129, 0.141, 0.272, 0.269], true),
        ([0.147, 0.124, 0.382, 0.309], [0.243, 0.187, 0.369, 0.296], true),
        ([0.516, 0.093, 0.635, 0.211], [0.564, 0.130, 0.659, 0.210], true),
        // Someone else, behind or beside the face found sideways.
        ([0.516, 0.093, 0.635, 0.211], [0.496, 0.102, 0.560, 0.175], false),
        ([0.309, 0.261, 0.431, 0.372], [0.391, 0.333, 0.441, 0.379], false),
        ([0.228, 0.249, 0.335, 0.346], [0.205, 0.275, 0.263, 0.328], false),
        ([0.684, 0.259, 0.809, 0.374], [0.755, 0.288, 0.808, 0.344], false),
        ([0.232, 0.149, 0.378, 0.290], [0.332, 0.151, 0.408, 0.219], false),
    ];

    #[test]
    fn a_face_found_again_is_told_from_someone_beside_it() {
        let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
        for (i, (sideways, upright, same)) in TURNED_PAIRS.iter().enumerate() {
            let inter = [
                sideways[0].max(upright[0]),
                sideways[1].max(upright[1]),
                sideways[2].min(upright[2]),
                sideways[3].min(upright[3]),
            ];
            assert!(area(&inter) / area(upright) >= 0.59, "pair {i}: mostly inside");
            assert!(iou(sideways, upright) < NMS_IOU, "pair {i}: overlap too small to tell");
            assert_eq!(same_face(sideways, upright), *same, "pair {i}");
            assert_eq!(same_face(upright, sideways), *same, "pair {i}, either way round");
        }
        // Two finds of one face overlapping as much as the detector's own: the same.
        assert!(same_face(&[0.1, 0.1, 0.3, 0.3], &[0.12, 0.1, 0.32, 0.3]));
        // Side by side: not.
        assert!(!same_face(&[0.1, 0.1, 0.3, 0.3], &[0.3, 0.1, 0.5, 0.3]));
    }

    #[test]
    fn each_face_is_found_again_once() {
        let (around_two, left, right) = (TURNED_PAIRS[4].0, TURNED_PAIRS[4].1, TURNED_PAIRS[5].1);
        let (face, beside) = (TURNED_PAIRS[6].1, TURNED_PAIRS[7].1);
        let before = [around_two, TURNED_PAIRS[6].0];
        // The box around two people is the one it overlaps most; the other
        // is new. Someone beside a face found again is new too.
        assert_eq!(found_again(&before, &[right, left, beside, face]), [None, Some(0), None, Some(1)]);
        assert!(found_again(&before, &[]).is_empty());
        assert_eq!(found_again(&[], &[face]), [None]);
    }
}
