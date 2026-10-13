//! Faces. Found with SCRFD and described with ArcFace (InsightFace's buffalo_l,
//! Immich's export), grouped into people. Runs on Shoebox's previews, so photos on
//! unplugged disks get faces too.
//!
//! Names and groupings live in the catalog (the user's work, backed up). The face
//! fingerprints live in `faces.db` (rebuildable, kept out of backups).

use crate::catalog::Catalog;
use anyhow::{Result, anyhow};
use image::{DynamicImage, GenericImageView, RgbImage, imageops::FilterType};
use ort::session::Session;
use ort::value::Tensor;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

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
/// Faces smaller than this (short side, in the preview) give fingerprints too
/// weak to group automatically. They are kept, ungrouped, for a later pass on
/// the full-size photo.
const GROUP_MIN_PX: f32 = 40.0;
/// Bump when the grouping rules change: catalogs sorted under older rules
/// get their untouched faces regrouped once at start.
const GROUPING_RULES: &str = "2";
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
    /// Every face of theirs was found since the visit to People before this one.
    pub is_new: bool,
    /// Photos they were found in since then.
    pub new_photos: i64,
}

/// Faces after this id were found since People was last opened.
const SEEN_UPTO: &str = "people_seen_upto";
/// What "new" means during the current visit: faces after this id. Kept for the
/// whole visit, so naming or merging doesn't clear the badges under the user's eyes.
const NEW_AFTER: &str = "people_new_after";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoFace {
    pub id: i64,
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
}

impl FaceIndex {
    /// Lets go of the face models (they are being deleted). Waits for a
    /// face search in progress: cancel it first.
    pub fn unload(&self) {
        *self.models.lock().unwrap() = None;
    }

    pub fn open(cat: &Catalog) -> Result<Self> {
        let conn = Connection::open(cat.data.root.join("faces.db"))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS emb (face_id INTEGER PRIMARY KEY, vec BLOB NOT NULL);")?;
        let index = Self { db: Mutex::new(conn), models: Mutex::new(None), centroids: RwLock::new(HashMap::new()) };
        index.rebuild_centroids(cat)?;
        // Catalogs grouped under older rules get regrouped once.
        let done: Option<String> =
            cat.db().query_row("SELECT value FROM meta WHERE key = 'face_grouping'", [], |r| r.get(0)).optional()?;
        if done.as_deref() != Some(GROUPING_RULES) {
            let (faces, people) = index.regroup(cat)?;
            log::info!("faces: regrouped {faces} faces into {people} people (rules {GROUPING_RULES})");
            cat.db()
                .execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('face_grouping', ?1)", [GROUPING_RULES])?;
        }
        // Faces found from now on are new at the next visit to People, even the first one.
        cat.db().execute(
            "INSERT OR IGNORE INTO meta (key, value) SELECT ?1, COALESCE(MAX(id), 0) FROM faces",
            [SEEN_UPTO],
        )?;
        Ok(index)
    }

    /// Sorts again every face the user hasn't touched - not in a named or ignored
    /// person, not placed by hand - with the current rules. Names, ignored
    /// strangers and hand placements are never changed. Returns (faces, people).
    pub fn regroup(&self, cat: &Catalog) -> Result<(usize, usize)> {
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
            tx.commit()?;
        }
        self.rebuild_centroids(cat)?;

        let mut people_made = 0;
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
        Ok((loose.len(), people_made))
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

    fn rebuild_centroids(&self, cat: &Catalog) -> Result<()> {
        let faces: Vec<(i64, i64)> = cat
            .db()
            .prepare("SELECT id, person_id FROM faces WHERE person_id IS NOT NULL")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut c: HashMap<i64, (Vec<f32>, u32)> = HashMap::new();
        for (fid, pid) in faces {
            if let Some(v) = self.vector(fid)? {
                let e = c.entry(pid).or_insert_with(|| (vec![0.0; DIM], 0));
                e.0.iter_mut().zip(&v).for_each(|(s, x)| *s += x);
                e.1 += 1;
            }
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
        Ok(())
    }

    /// Records a photo's faces: each joins the person it resembles, or starts a new one.
    fn store(&self, cat: &Catalog, photo_id: i64, img: &DynamicImage, faces: Vec<(Detected, Vec<f32>)>) -> Result<()> {
        let (w, h) = img.dimensions();
        let (fw, fh) = (w as f64, h as f64);
        let mut written = Vec::new();
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
                let fid = tx.last_insert_rowid();
                if let Some(pid) = pid {
                    self.add_to(pid, v, 1.0);
                }
                written.push((fid, d.bbox, v.clone()));
            }
            tx.execute(
                "INSERT OR REPLACE INTO face_scan (photo_id, model, faces) VALUES (?1, ?2, ?3)",
                params![photo_id, FACE_MODEL, faces.len() as i64],
            )?;
            tx.commit()?;
        }
        {
            let mut db = self.db.lock().unwrap();
            let tx = db.transaction()?;
            for (fid, _, v) in &written {
                let blob: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
                tx.execute("INSERT OR REPLACE INTO emb (face_id, vec) VALUES (?1, ?2)", params![fid, blob])?;
            }
            tx.commit()?;
        }
        for (fid, bbox, _) in &written {
            if let Ok(bytes) = face_thumb(img, bbox) {
                let _ = cat.data.write(&cat.data.face_path(*fid), &bytes);
            }
        }
        Ok(())
    }

    /// Everyone, named people first, then by how many photos they're in.
    pub fn people(&self, cat: &Catalog) -> Result<Vec<Person>> {
        let db = cat.db();
        // Never opened People yet: nothing is new.
        let new_after = meta_id(&db, NEW_AFTER)?.unwrap_or(i64::MAX);
        let people = db
            .prepare(
                "SELECT p.id, p.name, COUNT(f.id), COUNT(DISTINCT f.photo_id),
                        (SELECT f2.id FROM faces f2 WHERE f2.person_id = p.id ORDER BY f2.score DESC LIMIT 1),
                        p.ignored, MIN(f.id) > ?1, COUNT(DISTINCT CASE WHEN f.id > ?1 THEN f.photo_id END)
                   FROM people p JOIN faces f ON f.person_id = p.id
                  GROUP BY p.id
                  ORDER BY p.ignored, p.name IS NULL, COUNT(DISTINCT f.photo_id) DESC, p.id",
            )?
            .query_map([new_after], |r| {
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

    /// The user opens People: what was found since their last visit is new for this
    /// visit, and the next visit starts counting from now.
    pub fn visit_people(&self, cat: &Catalog) -> Result<()> {
        let db = cat.db();
        let now: i64 = db.query_row("SELECT COALESCE(MAX(id), 0) FROM faces", [], |r| r.get(0))?;
        let seen = meta_id(&db, SEEN_UPTO)?.unwrap_or(now);
        db.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![NEW_AFTER, seen.to_string()])?;
        db.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![SEEN_UPTO, now.to_string()])?;
        Ok(())
    }

    pub fn rename(&self, cat: &Catalog, person_id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        cat.db().execute(
            "UPDATE people SET name = ?1 WHERE id = ?2",
            params![(!name.is_empty()).then_some(name), person_id],
        )?;
        Ok(())
    }

    /// Hides (or shows again) strangers. Their future faces still join them,
    /// so they never come back as someone new.
    pub fn set_ignored(&self, cat: &Catalog, ids: &[i64], ignored: bool) -> Result<()> {
        let mut db = cat.db();
        let tx = db.transaction()?;
        for id in ids {
            tx.execute("UPDATE people SET ignored = ?1 WHERE id = ?2", params![ignored as i64, id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Makes several people one. Keeps `into`'s name, or the first name found.
    pub fn merge(&self, cat: &Catalog, ids: &[i64], into: i64) -> Result<()> {
        let others: Vec<i64> = ids.iter().copied().filter(|&i| i != into).collect();
        if others.is_empty() {
            return Ok(());
        }
        {
            let mut db = cat.db();
            let tx = db.transaction()?;
            let into_name: Option<String> =
                tx.query_row("SELECT name FROM people WHERE id = ?1", [into], |r| r.get(0)).optional()?.flatten();
            if into_name.is_none() {
                for o in &others {
                    let n: Option<String> =
                        tx.query_row("SELECT name FROM people WHERE id = ?1", [o], |r| r.get(0)).optional()?.flatten();
                    if let Some(n) = n {
                        tx.execute("UPDATE people SET name = ?1 WHERE id = ?2", params![n, into])?;
                        break;
                    }
                }
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
        Ok(())
    }

    /// "That's not them": the face leaves its person and becomes someone new,
    /// and is never regrouped automatically.
    pub fn split_face(&self, cat: &Catalog, face_id: i64) -> Result<i64> {
        let old: Option<i64> = cat
            .db()
            .query_row("SELECT person_id FROM faces WHERE id = ?1", [face_id], |r| r.get(0))
            .optional()?
            .flatten();
        let new_pid = {
            let mut db = cat.db();
            let tx = db.transaction()?;
            tx.execute("INSERT INTO people (name, created_at) VALUES (NULL, ?1)", [crate::db::now()])?;
            let pid = tx.last_insert_rowid();
            tx.execute("UPDATE faces SET person_id = ?1, by_hand = 1 WHERE id = ?2", params![pid, face_id])?;
            tx.commit()?;
            pid
        };
        if let Some(v) = self.vector(face_id)? {
            if let Some(old) = old {
                self.add_to(old, &v, -1.0);
            }
            self.add_to(new_pid, &v, 1.0);
        }
        Ok(new_pid)
    }

    /// Faces in a person that don't look like the rest of them (a group named
    /// before the grouping rules were tightened can hold other people), worst
    /// first. "The rest of them" is the person's dominant face: the average of
    /// the faces close to the average, refined a few times, so a minority of
    /// strangers can't pull it their way. Faces placed by hand are never offered.
    pub fn strays(&self, cat: &Catalog, person_id: i64) -> Result<Vec<Stray>> {
        let faces: Vec<(i64, i64, bool)> = cat
            .db()
            .prepare("SELECT id, photo_id, by_hand FROM faces WHERE person_id = ?1")?
            .query_map([person_id], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut faces_v = Vec::new();
        for (fid, photo, by_hand) in faces {
            if let Some(v) = self.vector(fid)? {
                faces_v.push((fid, photo, by_hand, v));
            }
        }
        // Too few faces to tell who the person "really" is.
        if faces_v.len() < 3 {
            return Ok(Vec::new());
        }
        let centre = dominant(&faces_v.iter().map(|f| f.3.as_slice()).collect::<Vec<_>>());
        let mut out: Vec<Stray> = faces_v
            .iter()
            .filter(|f| !f.2)
            .map(|f| Stray { face_id: f.0, photo_id: f.1, similarity: dot(&f.3, &centre) })
            .filter(|s| s.similarity < SAME_PERSON)
            .collect();
        out.sort_by(|a, b| a.similarity.total_cmp(&b.similarity));
        Ok(out)
    }

    /// Moves faces out of a person: each joins whoever it resembles most -
    /// never the person it left, never someone already in its photo - or
    /// starts someone new. Returns how many moved.
    pub fn move_out(&self, cat: &Catalog, person_id: i64, face_ids: &[i64]) -> Result<usize> {
        let mut moved = 0;
        for &fid in face_ids {
            let row: Option<(i64, Option<i64>)> = cat
                .db()
                .query_row("SELECT photo_id, person_id FROM faces WHERE id = ?1", [fid], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((photo, Some(cur))) = row else { continue };
            if cur != person_id {
                continue;
            }
            let Some(v) = self.vector(fid)? else { continue };
            self.add_to(person_id, &v, -1.0);
            let mut taken: Vec<i64> = cat
                .db()
                .prepare("SELECT person_id FROM faces WHERE photo_id = ?1 AND person_id IS NOT NULL AND id <> ?2")?
                .query_map(params![photo, fid], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            taken.push(person_id);
            let to = {
                let mut db = cat.db();
                let tx = db.transaction()?;
                let to = match self.closest(&v, &taken) {
                    Some(p) => p,
                    None => {
                        tx.execute("INSERT INTO people (name, created_at) VALUES (NULL, ?1)", [crate::db::now()])?;
                        tx.last_insert_rowid()
                    }
                };
                tx.execute("UPDATE faces SET person_id = ?1 WHERE id = ?2", params![to, fid])?;
                tx.commit()?;
                to
            };
            self.add_to(to, &v, 1.0);
            moved += 1;
        }
        Ok(moved)
    }

    /// The faces found in one photo, for the viewer.
    pub fn photo_faces(&self, cat: &Catalog, photo_id: i64) -> Result<Vec<PhotoFace>> {
        let db = cat.db();
        let faces = db
            .prepare(
                "SELECT f.id, f.person_id, p.name, f.x1, f.y1, f.x2, f.y2 FROM faces f
                   LEFT JOIN people p ON p.id = f.person_id WHERE f.photo_id = ?1 ORDER BY f.x1",
            )?
            .query_map([photo_id], |r| {
                Ok(PhotoFace {
                    id: r.get(0)?,
                    person_id: r.get(1)?,
                    name: r.get(2)?,
                    bbox: [r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?],
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(faces)
    }
}

/// A square 160 px picture of the face, with some margin.
fn meta_id(db: &Connection, key: &str) -> rusqlite::Result<Option<i64>> {
    let v: Option<String> = db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?;
    Ok(v.and_then(|v| v.parse().ok()))
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
