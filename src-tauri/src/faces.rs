//! Faces. Found with SCRFD and described with ArcFace (InsightFace's buffalo_l,
//! Immich's export), grouped into people. Runs on Shoebox's previews, so photos on
//! unplugged disks get faces too.
//!
//! Names and groupings live in the catalog (the user's work, backed up). The face
//! fingerprints live in `faces.db` (rebuildable, kept out of backups).

use crate::catalog::Catalog;
use anyhow::{Result, anyhow, bail};
use image::{DynamicImage, GenericImageView, RgbImage, imageops::FilterType};
use ort::session::Session;
use ort::value::Tensor;
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
/// Less alike than the same person, but enough to suggest who a face may be.
const LOOKS_LIKE: f32 = 0.3;
/// Faces smaller than this (short side, in the preview) give fingerprints too
/// weak to group automatically. They are kept, ungrouped, for a later pass on
/// the full-size photo.
const GROUP_MIN_PX: f32 = 40.0;
/// A photo taken this close to one of a person's own is "near" them:
/// children change fast, and to the model babies look alike.
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
const DIM: usize = 512;
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

/// "Check" in People: likely mix-ups among named people, found from their
/// faces and the dates of their photos. Nothing changes until the user acts
/// on a finding (`reassign`, `merge`). A face is in one finding at most: twice
/// in a photo, else much more like someone near in time, else an age jump,
/// else much more like someone far off in time.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeopleCheck {
    /// Named people checked (ignored ones aren't), and their faces.
    pub people: usize,
    pub faces: usize,
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

pub struct FaceIndex {
    /// faces.db: one fingerprint per face.
    db: Mutex<Connection>,
    models: Mutex<Option<Models>>,
    /// Per person: the sum of its faces' fingerprints and how many. Sums add,
    /// so merging people is just adding.
    centroids: RwLock<HashMap<i64, (Vec<f32>, u32)>>,
    /// Open once the averages are read (and any regrouping done): every
    /// method waits for it, so none works from half the people.
    ready: crate::Ready,
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
            centroids: RwLock::new(HashMap::new()),
            ready: crate::Ready::default(),
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
        let loose: Vec<(i64, i64, [f64; 4])> = cat
            .db()
            .prepare(
                "SELECT f.id, f.photo_id, f.x1, f.y1, f.x2, f.y2 FROM faces f LEFT JOIN people p ON p.id = f.person_id
                  WHERE f.by_hand = 0 AND (f.person_id IS NULL OR (p.name IS NULL AND p.ignored = 0))
                  ORDER BY f.photo_id, f.score DESC",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?])))?
            .collect::<rusqlite::Result<_>>()?;
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            for (fid, _, _) in &loose {
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
            let (pw, ph) = image::image_dimensions(cat.data.preview_path(photo)).unwrap_or((1080, 720));
            let mut taken: Vec<i64> = cat
                .db()
                .prepare("SELECT person_id FROM faces WHERE photo_id = ?1 AND person_id IS NOT NULL")?
                .query_map([photo], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut db = cat.db();
            let tx = db.transaction()?;
            for (fid, _, b) in &loose[i..j] {
                let short = ((b[2] - b[0]) * pw as f64).min((b[3] - b[1]) * ph as f64) as f32;
                let Some(v) = self.vector(*fid)? else { continue };
                if short < GROUP_MIN_PX {
                    continue;
                }
                let pid = match self.closest(&v, &taken) {
                    Some(pid) => pid,
                    None => {
                        tx.execute("INSERT INTO people (name, created_at) VALUES (NULL, ?1)", [crate::db::now()])?;
                        people_made += 1;
                        tx.last_insert_rowid()
                    }
                };
                tx.execute("UPDATE faces SET person_id = ?1 WHERE id = ?2", params![pid, fid])?;
                self.add_to(pid, &v, 1.0);
                taken.push(pid);
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
        let faces: Vec<(i64, i64)> = cat
            .read()?
            // In face order: the sums come out the same to the last bit, whatever index the query uses.
            .prepare("SELECT id, person_id FROM faces WHERE person_id IS NOT NULL ORDER BY id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut faces = faces.into_iter().peekable();
        let mut c: HashMap<i64, (Vec<f32>, u32)> = HashMap::new();
        let db = self.db.lock().unwrap();
        let mut stmt = db.prepare("SELECT face_id, vec FROM emb ORDER BY face_id")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let fid: i64 = r.get(0)?;
            while faces.next_if(|(id, _)| *id < fid).is_some() {}
            let Some((_, pid)) = faces.next_if(|(id, _)| *id == fid) else { continue };
            let blob = r.get_ref(1)?.as_blob()?;
            if blob.len() != DIM * 4 {
                continue;
            }
            let e = c.entry(pid).or_insert_with(|| (vec![0.0; DIM], 0));
            e.0.iter_mut().zip(blob.as_chunks::<4>().0).for_each(|(s, x)| *s += f32::from_le_bytes(*x));
            e.1 += 1;
        }
        *self.centroids.write().unwrap() = c;
        Ok(())
    }

    /// The person this fingerprint most resembles, if close enough, other than
    /// `taken` (people already in this photo: nobody appears twice in one photo).
    fn closest(&self, v: &[f32], taken: &[i64]) -> Option<i64> {
        let c = self.centroids.read().unwrap();
        c.iter()
            .filter(|(pid, (_, n))| *n > 0 && !taken.contains(pid))
            .map(|(pid, (sum, _))| {
                let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::EPSILON);
                (*pid, dot(sum, v) / norm)
            })
            .filter(|(_, s)| *s >= SAME_PERSON)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(pid, _)| pid)
    }

    fn add_to(&self, pid: i64, v: &[f32], sign: f32) {
        let mut c = self.centroids.write().unwrap();
        let e = c.entry(pid).or_insert_with(|| (vec![0.0; DIM], 0));
        e.0.iter_mut().zip(v).for_each(|(s, x)| *s += sign * x);
        e.1 = if sign > 0.0 { e.1 + 1 } else { e.1.saturating_sub(1) };
    }

    /// Finds and groups the faces of every photo not looked at yet.
    pub fn index_pending(
        &self,
        cat: &Catalog,
        cancel: &AtomicBool,
        progress: &dyn Fn(&crate::search::AiProgress),
    ) -> Result<()> {
        self.ready.wait()?;
        let todo: Vec<i64> = cat
            .db()
            .prepare(&format!(
                "SELECT id FROM photos WHERE preview_state = {} AND id NOT IN (SELECT photo_id FROM face_scan)
                 ORDER BY taken_ts DESC",
                crate::db::PREVIEW_OK
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
            let found = match image::open(cat.data.preview_path(*photo_id)) {
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
                    log::warn!("faces: photo {photo_id}: preview unreadable: {e}");
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
        let mut taken: Vec<i64> = Vec::new();
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            for (d, v) in &faces {
                let short = (d.bbox[2] - d.bbox[0]).min(d.bbox[3] - d.bbox[1]);
                let pid: Option<i64> = if short < GROUP_MIN_PX {
                    None
                } else {
                    Some(match self.closest(v, &taken) {
                        Some(pid) => pid,
                        None => {
                            tx.execute("INSERT INTO people (name, created_at) VALUES (NULL, ?1)", [crate::db::now()])?;
                            tx.last_insert_rowid()
                        }
                    })
                };
                taken.extend(pid);
                let kps: Vec<String> = d
                    .kps
                    .iter()
                    .enumerate()
                    .map(|(i, k)| format!("{:.4}", *k as f64 / if i % 2 == 0 { fw } else { fh }))
                    .collect();
                tx.execute(
                    "INSERT INTO faces (photo_id, person_id, x1, y1, x2, y2, kps, score) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        photo_id,
                        pid,
                        (d.bbox[0] as f64 / fw).clamp(0.0, 1.0),
                        (d.bbox[1] as f64 / fh).clamp(0.0, 1.0),
                        (d.bbox[2] as f64 / fw).clamp(0.0, 1.0),
                        (d.bbox[3] as f64 / fh).clamp(0.0, 1.0),
                        kps.join(","),
                        d.score
                    ],
                )?;
                written.push(tx.last_insert_rowid());
                joined.extend(pid.map(|pid| (pid, v)));
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
                    Some(bytes) => cat.data.write(&path, bytes)?,
                    // Not a picture left by an attempt that never finished.
                    None => cat.data.remove(&path)?,
                }
            }
            tx.execute(
                "INSERT OR REPLACE INTO face_scan (photo_id, model, faces) VALUES (?1, ?2, ?3)",
                params![photo_id, FACE_MODEL, faces.len() as i64],
            )?;
            tx.commit()?;
        }
        // Only now that they're saved do these faces count for their person.
        for (pid, v) in joined {
            self.add_to(pid, v, 1.0);
        }
        Ok(())
    }

    /// Everyone, named people first, then by how many photos they're in.
    pub fn people(&self, cat: &Catalog) -> Result<Vec<Person>> {
        self.ready.wait()?;
        let db = cat.read()?;
        let people = db
            .prepare(
                "SELECT p.id, p.name, COUNT(f.id), COUNT(DISTINCT f.photo_id),
                        (SELECT f2.id FROM faces f2 WHERE f2.person_id = p.id ORDER BY f2.score DESC LIMIT 1),
                        p.ignored, MAX(f.seen) = 0, COUNT(DISTINCT CASE WHEN f.seen = 0 THEN f.photo_id END)
                   FROM people p JOIN faces f ON f.person_id = p.id
                  GROUP BY p.id
                  ORDER BY p.ignored, p.name IS NULL, COUNT(DISTINCT f.photo_id) DESC, p.id",
            )?
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
    fn join_now(&self, cat: &Catalog) -> Result<usize> {
        // A join moves an average, which can bring someone else close enough:
        // again until nobody joins (two or three passes on a real catalog).
        let mut joined = 0;
        for _ in 0..10 {
            let n = self.join_pass(cat)?;
            joined += n;
            if n == 0 {
                break;
            }
        }
        if joined > 0 {
            log::info!("faces: {joined} people found twice joined the person they are");
        }
        Ok(joined)
    }

    fn join_pass(&self, cat: &Catalog) -> Result<usize> {
        let (everyone, movable, mut photos) = {
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
            let mut photos: HashMap<i64, HashSet<i64>> = HashMap::new();
            let mut stmt = db.prepare("SELECT person_id, photo_id FROM faces WHERE person_id IS NOT NULL")?;
            for r in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
                let (person, photo) = r?;
                photos.entry(person).or_default().insert(photo);
            }
            (everyone, movable, photos)
        };
        // Everyone's average face as a unit vector, and how many faces it holds.
        let average: Vec<(i64, Vec<f32>, u32)> = {
            let c = self.centroids.read().unwrap();
            c.iter()
                .filter(|(id, (_, n))| *n > 0 && everyone.contains(id))
                .map(|(id, (sum, n))| (*id, unit(sum.clone()), *n))
                .collect()
        };
        // Every join alike enough, best first: (similarity, who moves, to whom).
        let bigger = |a: (u32, i64), b: (u32, i64)| a.0 > b.0 || (a.0 == b.0 && a.1 < b.1);
        let mut joins: Vec<(f32, i64, i64)> = Vec::new();
        for (l, lv, ln) in average.iter().filter(|(id, _, _)| movable.contains(id)) {
            for (t, tv, tn) in &average {
                if t == l || (movable.contains(t) && !bigger((*tn, *t), (*ln, *l))) {
                    continue;
                }
                let s = dot(lv, tv);
                if s >= SAME_PERSON {
                    joins.push((s, *l, *t));
                }
            }
        }
        joins.sort_by(|a, b| b.0.total_cmp(&a.0));
        // One step per pass: nobody joins someone who moves, nobody moves who
        // was joined.
        let mut joining: HashMap<i64, Vec<i64>> = HashMap::new();
        let mut moved: HashSet<i64> = HashSet::new();
        for (_, l, t) in joins {
            if moved.contains(&l) || moved.contains(&t) || joining.contains_key(&l) {
                continue;
            }
            let theirs = photos.get(&l).cloned().unwrap_or_default();
            let target = photos.entry(t).or_default();
            // Nobody twice in one photo.
            if !target.is_disjoint(&theirs) {
                continue;
            }
            target.extend(theirs);
            joining.entry(t).or_default().push(l);
            moved.insert(l);
        }
        for (t, ls) in &joining {
            let mut ids = ls.clone();
            ids.push(*t);
            self.merge_now(cat, &ids, *t)?;
        }
        Ok(moved.len())
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
        let mut c = self.centroids.write().unwrap();
        let mut sum = c.remove(&into).unwrap_or_else(|| (vec![0.0; DIM], 0));
        for o in &others {
            if let Some((s, n)) = c.remove(o) {
                sum.0.iter_mut().zip(&s).for_each(|(a, b)| *a += b);
                sum.1 += n;
            }
        }
        c.insert(into, sum);
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
            if let Some(v) = self.vector(fid)? {
                self.add_to(merged.into, &v, -1.0);
                self.add_to(to, &v, 1.0);
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
            if let Some(v) = self.vector(fid)? {
                if let Some(from) = from {
                    self.add_to(from, &v, -1.0);
                }
                self.add_to(to, &v, 1.0);
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
            if let Some(v) = self.vector(fid)? {
                if let Some(from) = from {
                    self.add_to(from, &v, -1.0);
                }
                if let Some(to) = to {
                    self.add_to(to, &v, 1.0);
                }
            }
        }
        Ok(changes.len())
    }

    /// Who a face goes to when the user doesn't say: whoever it resembles most,
    /// other than `taken`, or someone new.
    fn place(&self, tx: &rusqlite::Transaction, face_id: i64, taken: &[i64]) -> Result<i64> {
        if let Some(p) = self.vector(face_id)?.and_then(|v| self.closest(&v, taken)) {
            return Ok(p);
        }
        tx.execute("INSERT INTO people (name, created_at) VALUES (NULL, ?1)", [crate::db::now()])?;
        Ok(tx.last_insert_rowid())
    }

    /// The people these faces look most like, best first, to suggest who
    /// they are: by each person's average face, not counting `except`.
    pub fn look_alikes(&self, face_ids: &[i64], except: &[i64], n: usize) -> Result<Vec<LookAlike>> {
        self.ready.wait()?;
        let mut vs = Vec::new();
        for &fid in face_ids {
            if let Some(v) = self.vector(fid)? {
                vs.push(v);
            }
        }
        if vs.is_empty() {
            return Ok(Vec::new());
        }
        let face = mean(&vs.iter().map(Vec::as_slice).collect::<Vec<_>>());
        let c = self.centroids.read().unwrap();
        let mut out: Vec<LookAlike> = c
            .iter()
            .filter(|(pid, (_, n))| *n > 0 && !except.contains(pid))
            .map(|(pid, (sum, _))| {
                let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::EPSILON);
                LookAlike { person_id: *pid, similarity: dot(sum, &face) / norm }
            })
            .filter(|l| l.similarity >= LOOKS_LIKE)
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
            .prepare("SELECT id, photo_id, by_hand FROM faces WHERE person_id = ?1 ORDER BY id")?
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

    /// "Check" in People: likely mix-ups among named people ([`PeopleCheck`]).
    /// Every named face's fingerprint is read once, in face order, on a
    /// connection of its own: faces found or moved meanwhile don't wait.
    pub fn check_people(&self, cat: &Catalog) -> Result<PeopleCheck> {
        self.ready.wait()?;
        let (names, faces) = {
            let db = cat.read()?;
            let names: HashMap<i64, String> = db
                .prepare("SELECT id, name FROM people WHERE name IS NOT NULL AND ignored = 0")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            // (face, person, photo, by hand, when taken), in face order.
            let faces: Vec<(i64, i64, i64, bool, Option<i64>)> = db
                .prepare(
                    "SELECT f.id, f.person_id, f.photo_id, f.by_hand, ph.taken_ts FROM faces f
                       JOIN people p ON p.id = f.person_id JOIN photos ph ON ph.id = f.photo_id
                      WHERE p.name IS NOT NULL AND p.ignored = 0 ORDER BY f.id",
                )?
                .query_map([], |r| {
                    let taken: Option<i64> = r.get(4)?;
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0, taken.filter(|t| *t > 0)))
                })?
                .collect::<rusqlite::Result<_>>()?;
            (names, faces)
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
                age_jumps.push(AgeJump { person_id: people[pi], face, year: y, own, like_year, like });
            }
        }

        // Near: within NEAR_DAYS of a photo of the one they look like.
        for l in &mut like_others {
            let theirs = &taken[index[&l.other_id]];
            l.near = when.get(&l.face.face_id).is_some_and(|&ts| {
                let i = theirs.partition_point(|&t| t < ts - NEAR_DAYS * 86_400);
                theirs.get(i).is_some_and(|&t| t <= ts + NEAR_DAYS * 86_400)
            });
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
                });
            }
        }
        same_people.sort_by(|a, b| {
            b.similarity.total_cmp(&a.similarity).then((a.person_id, a.other_id).cmp(&(b.person_id, b.other_id)))
        });

        Ok(PeopleCheck { people: k, faces: checked, twice, age_jumps, like_others, same_people })
    }

    /// The faces found in one photo, for the viewer.
    pub fn photo_faces(&self, cat: &Catalog, photo_id: i64) -> Result<Vec<PhotoFace>> {
        self.ready.wait()?;
        self.faces_in(cat, &[photo_id])
    }

    /// The faces found in some photos, photo by photo, left to right.
    pub fn faces_in(&self, cat: &Catalog, photo_ids: &[i64]) -> Result<Vec<PhotoFace>> {
        self.ready.wait()?;
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
        let img = match image::open(cat.data.preview_path(photo)) {
            Ok(img) => img,
            Err(e) => {
                log::warn!("faces: photo {photo}: preview unreadable, face pictures not cut: {e}");
                continue;
            }
        };
        let (w, h) = (img.width() as f64, img.height() as f64);
        for (fid, b) in faces {
            let bbox = [(b[0] * w) as f32, (b[1] * h) as f32, (b[2] * w) as f32, (b[3] * h) as f32];
            match face_thumb(&img, &bbox) {
                Ok(bytes) => {
                    cat.data.write(&cat.data.face_path(fid), &bytes)?;
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
}
