//! A face's eyes and smile: whether the eyes are closed, whether the face smiles.
//!
//! Both come from Google's MediaPipe Face Landmarker, run as ONNX (tf2onnx conversions of the
//! `face_landmarker.task` models, Apache-2.0). A face mesh model places 478 landmarks on a face
//! cropped around it, and a blendshapes model turns 146 of them into 52 scores, among them how
//! closed each eye is and how much each side of the mouth smiles.
//!
//! A face found by the face detector is cropped square and levelled around its eyes, the mesh
//! model places its landmarks, the face is cropped again around them and its landmarks placed once
//! more, brought back to the picture's pixels (as MediaPipe gives them), and the blendshapes model
//! scores them. Faces too small to tell, or that the mesh model
//! doesn't see as a face, are not scored.

use crate::faces::bilinear;
use anyhow::{Result, anyhow, bail};
use image::RgbImage;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::time::{Duration, Instant};

/// A face found in a picture, in pixels of that picture. `kps` are InsightFace's five points in
/// its order: the eye on the image's left (x, y), the eye on its right, the nose, the mouth's left
/// corner, the mouth's right corner. Same as faces.rs's detections.
pub struct Face {
    pub bbox: [f32; 4],
    pub kps: [f32; 10],
}

/// Faces narrower than this (the box's width, in pixels of the picture read) are not read: their
/// eyes are a few pixels tall, too few to tell open from closed. A closer look reads them on the
/// original, where they are several times wider.
///
/// Measured on two shoots by reading the same faces shrunk to each width and comparing with a
/// reading of them at full size (the original's, or the preview's at 128 px and more; about 8,800
/// reads). Every verdict (no face, open, closed) agrees 92-99% at every width, open faces being most
/// of them. On faces whose reference is lidded (blink 0.4 and more), the ones a closed call is about,
/// the verdict agrees 48% under 48 px, 66% at 48-64, 69% at 64-80, 80% at 80-96 and 93% from 96 px;
/// and open faces read closed 2-5% of the time under 80 px against 0.4-0.5% above.
pub const MIN_FACE_PX: f32 = 96.0;

/// The mesh model's input is a square of this many pixels.
const CROP: usize = 256;

/// MediaPipe's scale from a face box to the square it crops: a box holds the face, the crop adds
/// the forehead, chin and ears the mesh covers.
const ROI_SCALE: f32 = 1.5;

/// MediaPipe's scale from the box around a face's landmarks to the square it crops again (see
/// [`mesh_roi`]): the landmarks reach the face's edges, the crop leaves a margin around them.
const MESH_ROI_SCALE: f32 = 1.5;

/// The square a face is read in: its centre and side in picture pixels, and the angle of the line
/// between the eyes in the picture (radians, y down). The crop is turned by that angle so the eyes
/// come out level.
struct Roi {
    cx: f32,
    cy: f32,
    side: f32,
    angle: f32,
}

fn roi(face: &Face) -> Roi {
    let [x1, y1, x2, y2] = face.bbox;
    let k = &face.kps;
    Roi {
        cx: (x1 + x2) / 2.0,
        cy: (y1 + y2) / 2.0,
        side: ROI_SCALE * (x2 - x1).max(y2 - y1),
        angle: (k[3] - k[1]).atan2(k[2] - k[0]),
    }
}

/// The square a face is read in again, from its 478 landmarks in picture pixels: centred on the
/// box around them, [`MESH_ROI_SCALE`] times its long side, levelled along the line from the outer corner
/// of the eye on the image's left (landmark 33) to the other's (263). It is what MediaPipe does
/// when it tracks a face: the detector's box here (SCRFD's) isn't the one MediaPipe's crop was made
/// for (BlazeFace's), so the crop is taken again from the face's own landmarks.
fn mesh_roi(points: &[(f32, f32)]) -> Roi {
    let (mut x1, mut y1, mut x2, mut y2) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in points {
        (x1, y1, x2, y2) = (x1.min(x), y1.min(y), x2.max(x), y2.max(y));
    }
    let ((ax, ay), (bx, by)) = (points[33], points[263]);
    Roi {
        cx: (x1 + x2) / 2.0,
        cy: (y1 + y2) / 2.0,
        side: MESH_ROI_SCALE * (x2 - x1).max(y2 - y1),
        angle: (by - ay).atan2(bx - ax),
    }
}

/// The 146 landmarks the blendshapes model takes, as its input: x, y, x, y...
fn subset(points: &[(f32, f32)]) -> Vec<f32> {
    SUBSET.iter().flat_map(|&i| [points[i].0, points[i].1]).collect()
}

/// Where a point of the crop, in the model's 0..256 pixels, is in the picture.
fn to_picture(roi: &Roi, x: f32, y: f32) -> (f32, f32) {
    let (dx, dy) = ((x / CROP as f32 - 0.5) * roi.side, (y / CROP as f32 - 0.5) * roi.side);
    let (sin, cos) = roi.angle.sin_cos();
    (roi.cx + cos * dx - sin * dy, roi.cy + sin * dx + cos * dy)
}

/// The face's square, levelled, as the mesh model takes it: 256 x 256 RGB, 0..1, row by row
/// (NHWC). Each crop pixel is read at its centre from the picture, blended from its four
/// neighbours; where the square reaches outside the picture it is black.
fn crop(img: &RgbImage, roi: &Roi) -> Vec<f32> {
    let mut out = Vec::with_capacity(CROP * CROP * 3);
    for v in 0..CROP {
        for u in 0..CROP {
            let (x, y) = to_picture(roi, u as f32 + 0.5, v as f32 + 0.5);
            out.extend(bilinear(img, x, y).map(|c| c / 255.0));
        }
    }
    out
}

/// The 146 of the mesh's 478 landmarks the blendshapes model takes, in its order (MediaPipe's
/// face_blendshapes_graph).
const SUBSET: [usize; 146] = [
    0, 1, 4, 5, 6, 7, 8, 10, 13, 14, 17, 21, 33, 37, 39, 40, 46, 52, 53, 54, 55, 58, 61, 63, 65, 66, 67, 70, 78, 80,
    81, 82, 84, 87, 88, 91, 93, 95, 103, 105, 107, 109, 127, 132, 133, 136, 144, 145, 146, 148, 149, 150, 152, 153,
    154, 155, 157, 158, 159, 160, 161, 162, 163, 168, 172, 173, 176, 178, 181, 185, 191, 195, 197, 234, 246, 249, 251,
    263, 267, 269, 270, 276, 282, 283, 284, 285, 288, 291, 293, 295, 296, 297, 300, 308, 310, 311, 312, 314, 317, 318,
    321, 323, 324, 332, 334, 336, 338, 356, 361, 362, 365, 373, 374, 375, 377, 378, 379, 380, 381, 382, 384, 385, 386,
    387, 388, 389, 390, 397, 398, 400, 402, 405, 409, 415, 454, 466, 468, 469, 470, 471, 472, 473, 474, 475, 476, 477,
];

/// Eyes this closed (the blendshapes' blink score, 0 open to 1 shut), both of them, are closed eyes.
///
/// Set from about 800 faces of two shoots checked by eye, among them every face whose less shut eye
/// read over 0.3. Fully closed eyes read 0.6-0.7 on frontal faces; a laughing squint or eyes caught
/// half shut 0.3-0.55. Eyes looking down, and sunglasses, read like closed eyes (0.4-0.9) and are
/// most of what any threshold flags; the share of flags that are closed eyes stays about the same
/// (17-19%) from 0.45 to 0.6, while the number of flags falls. From 0.5 to 0.55 the flags fall from
/// 100 to 69 (false ones 83 to 56), losing 4 of 17 closed faces; 0.6 loses a clearly closed frontal
/// face that reads 0.62/0.596.
pub const CLOSED: f32 = 0.55;

/// How sure the mesh model must be that it saw a face (0..1) for its landmarks to be read.
const PRESENCE: f32 = 0.5;

/// What a face's expression reads: how closed each eye is, how much it smiles (both mouth corners'
/// average), all 0..1, and how sure the mesh model was of a face.
#[derive(Debug, Clone, Copy)]
pub struct Expression {
    /// The person's left eye (on the image's right), then their right eye: MediaPipe's eyeBlinkLeft, eyeBlinkRight.
    pub blink: [f32; 2],
    pub smile: f32,
    pub presence: f32,
}

impl Expression {
    /// How shut the face's eyes are, 0..1: its less shut eye's score, so a wink, or one eye
    /// hidden, reads as open. On the faces checked by eye, the mean of both eyes, or a rule on the
    /// more shut eye, found a few more closed eyes (the far eye of a face turned aside reads low)
    /// but flagged more faces that weren't closed for each one found: eyes looking down,
    /// sunglasses, and turned faces whose near eye reads 0.5-0.9 open.
    pub fn shut(&self) -> f32 {
        self.blink[0].min(self.blink[1])
    }

    /// The eyes are closed: [`Self::shut`] reaches [`CLOSED`].
    pub fn closed(&self) -> bool {
        self.shut() >= CLOSED
    }
}

/// What reading a face gave.
#[derive(Debug, Clone, Copy)]
pub enum Read {
    Scored(Expression),
    /// Narrower than [`MIN_FACE_PX`]: not read.
    Small,
    /// The mesh model saw no face there (a profile, a hidden face), or its points make no face.
    NoFace,
}

/// What can be told before the models run: a face too small to read, or one without two eyes to
/// level its crop by.
fn unreadable(face: &Face) -> Option<Read> {
    if face.bbox[2] - face.bbox[0] < MIN_FACE_PX {
        return Some(Read::Small);
    }
    let k = &face.kps;
    let eyes = (k[2] - k[0]).hypot(k[3] - k[1]);
    if face.bbox.iter().chain(k).any(|v| !v.is_finite()) || eyes < 1.0 {
        return Some(Read::NoFace);
    }
    None
}

/// The two models, loaded: the face mesh and the blendshapes. Can be used from several threads at
/// once; each model runs one face at a time.
pub struct Reader {
    landmarks: Mutex<Session>,
    blendshapes: Mutex<Session>,
    mesh_in: String,
    blend_in: String,
    mesh_gpu: bool,
    blend_gpu: bool,
}

impl Reader {
    pub fn load() -> Result<Self> {
        crate::models::require(crate::models::EXPRESSIONS)?;
        let (landmarks, blendshapes) = crate::models::expression_models();
        Self::with(crate::ai::session(&landmarks)?, crate::ai::session(&blendshapes)?)
    }

    /// From the sessions, each with whether it runs on the GPU. Inputs and outputs are taken by
    /// position: the mesh's landmarks first, its face presence second.
    fn with((landmarks, mesh_gpu): (Session, bool), (blendshapes, blend_gpu): (Session, bool)) -> Result<Self> {
        let input =
            |s: &Session, what| s.inputs.first().map(|i| i.name.clone()).ok_or_else(|| anyhow!("{what} has no input"));
        if landmarks.outputs.len() < 2 {
            bail!("the face mesh model has {} outputs, not landmarks and face presence", landmarks.outputs.len());
        }
        Ok(Self {
            mesh_in: input(&landmarks, "the face mesh model")?,
            blend_in: input(&blendshapes, "the blendshapes model")?,
            landmarks: Mutex::new(landmarks),
            blendshapes: Mutex::new(blendshapes),
            mesh_gpu,
            blend_gpu,
        })
    }

    /// Where the models run, for the log.
    fn on(&self) -> String {
        let at = |gpu| if gpu { "GPU" } else { "CPU" };
        if self.mesh_gpu == self.blend_gpu {
            at(self.mesh_gpu).into()
        } else {
            format!("face mesh on the {}, blendshapes on the {}", at(self.mesh_gpu), at(self.blend_gpu))
        }
    }

    /// False when a model on the GPU has gone wrong since it was loaded.
    fn still_right(&self) -> bool {
        let (landmarks, blendshapes) = crate::models::expression_models();
        (!self.mesh_gpu || crate::ai::still_right(&landmarks, &mut self.landmarks.lock().unwrap()))
            && (!self.blend_gpu || crate::ai::still_right(&blendshapes, &mut self.blendshapes.lock().unwrap()))
    }

    /// Reads a face of `img`, as the face detector found it there. The landmarks are read twice: once
    /// from the detector's box, then from the square the face's own landmarks give (see
    /// [`mesh_roi`]); the blendshapes score the second reading.
    pub fn read(&self, img: &RgbImage, face: &Face) -> Result<Read> {
        if let Some(r) = unreadable(face) {
            return Ok(r);
        }
        // A logit that isn't a number gives no presence: no face.
        let no_face = |presence: f32| presence.is_nan() || presence < PRESENCE;
        let (first, presence) = self.landmarks(img, &roi(face))?;
        if no_face(presence) {
            return Ok(Read::NoFace);
        }
        let (points, presence) = self.landmarks(img, &mesh_roi(&first))?;
        if no_face(presence) {
            return Ok(Read::NoFace);
        }
        let s = self.blendshapes(subset(&points))?;
        Ok(Read::Scored(Expression { blink: [s[9], s[10]], smile: (s[44] + s[45]) / 2.0, presence }))
    }

    /// The face's 478 landmarks (x, y in picture pixels), and how sure the mesh model is that the
    /// crop holds a face (0..1).
    fn landmarks(&self, img: &RgbImage, roi: &Roi) -> Result<(Vec<(f32, f32)>, f32)> {
        let t = Tensor::from_array(([1usize, CROP, CROP, 3], crop(img, roi)))?;
        let (mesh, logit) = {
            let mut s = self.landmarks.lock().unwrap();
            let out = s.run(ort::inputs![self.mesh_in.as_str() => t])?;
            let (_, mesh) = out[0].try_extract_tensor::<f32>()?;
            let (_, logit) = out[1].try_extract_tensor::<f32>()?;
            let logit = *logit.first().ok_or_else(|| anyhow!("the face mesh model gave no face presence"))?;
            (mesh.to_vec(), logit)
        };
        if mesh.len() != 478 * 3 {
            bail!("the face mesh model gave {} numbers, not 478 landmarks", mesh.len());
        }
        let points = mesh.chunks(3).map(|p| to_picture(roi, p[0], p[1])).collect();
        Ok((points, 1.0 / (1.0 + (-logit).exp())))
    }

    /// The 52 blendshape scores (0..1, in MediaPipe's order) of the 146 landmarks.
    fn blendshapes(&self, points: Vec<f32>) -> Result<Vec<f32>> {
        let t = Tensor::from_array(([1usize, SUBSET.len(), 2], points))?;
        let mut s = self.blendshapes.lock().unwrap();
        let out = s.run(ort::inputs![self.blend_in.as_str() => t])?;
        let (_, scores) = out[0].try_extract_tensor::<f32>()?;
        if scores.len() != 52 {
            bail!("the blendshapes model gave {} scores, not 52", scores.len());
        }
        Ok(scores.to_vec())
    }
}

/// The reader every look at photos shares, and when it was last checked on the GPU, or last failed
/// to load.
#[derive(Default)]
struct Shared {
    reader: Option<Arc<Reader>>,
    checked: Option<Instant>,
    failed: Option<Instant>,
}

/// A load that panics poisons it: it is used all the same (its state is always written whole), so the
/// eye check, and the rest of Cull's measuring with it, goes on.
static SHARED: Mutex<Shared> = Mutex::new(Shared { reader: None, checked: None, failed: None });

/// How often a reader on the GPU is checked again, and a failed load tried again.
const RETRY: Duration = Duration::from_secs(60);

/// The models, loaded on first use and shared. None when they or the AI runtime aren't here
/// (downloaded later, they load on the next call), or when loading them failed in the last minute
/// (logged once a minute at most).
pub fn shared() -> Option<Arc<Reader>> {
    let mut s = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
    if !crate::models::ready(crate::models::EXPRESSIONS) {
        // Deleted, or being deleted: a reader loaded again after unload() lets go of the files too,
        // and a load that failed is forgotten (files downloaded again get a fresh try).
        *s = Shared::default();
        return None;
    }
    if let Some(reader) = s.reader.clone() {
        let on_gpu = reader.mesh_gpu || reader.blend_gpu;
        if !on_gpu || s.checked.is_some_and(|t| t.elapsed() < RETRY) {
            return Some(reader);
        }
        s.checked = Some(Instant::now());
        if reader.still_right() {
            return Some(reader);
        }
        // Gone wrong on the GPU since it was loaded: loaded again, checked again.
        s.reader = None;
    }
    if !runtime_here() {
        return None;
    }
    if s.failed.is_some_and(|t| t.elapsed() < RETRY) {
        return None;
    }
    match Reader::load() {
        Ok(reader) => {
            log::info!("expressions: models loaded ({})", reader.on());
            let reader = Arc::new(reader);
            *s = Shared { reader: Some(reader.clone()), checked: Some(Instant::now()), failed: None };
            Some(reader)
        }
        Err(e) => {
            log::warn!("expressions: couldn't load the models: {e:#}");
            s.failed = Some(Instant::now());
            None
        }
    }
}

fn runtime_here() -> bool {
    crate::models::runtime_ready() || crate::ai::runtime_in_use().is_some()
}

/// Whether [`shared`] would give a reader, told without loading or checking anything: for a view,
/// which must cost what it shows. A load or check in progress (the reader is busy) counts as
/// usable; the next look at photos decides.
pub fn usable() -> bool {
    let s = match SHARED.try_lock() {
        Ok(s) => s,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        Err(TryLockError::WouldBlock) => return true,
    };
    let files = || crate::models::ready(crate::models::EXPRESSIONS) && runtime_here();
    usable_with(s.reader.is_some(), files, s.failed.map(|t| t.elapsed()))
}

/// A reader is loaded, or the files and runtime are here (`files`, asked only when needed) and
/// loading them didn't fail in the last [`RETRY`] (`failed`: how long ago it did). After that,
/// [`shared`] tries again: a runtime or model mended since loads.
fn usable_with(loaded: bool, files: impl FnOnce() -> bool, failed: Option<Duration>) -> bool {
    loaded || (failed.is_none_or(|ago| ago >= RETRY) && files())
}

/// Lets go of the models (they are being deleted). A look in progress keeps its own hold on them
/// until it ends.
pub fn unload() {
    *SHARED.lock().unwrap_or_else(PoisonError::into_inner) = Shared::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;
    use std::f32::consts::{FRAC_PI_6, PI};

    /// A face box of the given size at (100, 100) with the eyes `d` apart on a line tilted by `tilt`.
    fn face(w: f32, h: f32, tilt: f32) -> Face {
        let (left, d) = ((120.0, 130.0), 40.0);
        let right = (left.0 + d * tilt.cos(), left.1 + d * tilt.sin());
        Face {
            bbox: [100.0, 100.0, 100.0 + w, 100.0 + h],
            kps: [left.0, left.1, right.0, right.1, 150.0, 160.0, 130.0, 190.0, 170.0, 190.0],
        }
    }

    /// Where a picture point falls in the crop (the inverse of `to_picture`).
    fn to_crop(roi: &Roi, x: f32, y: f32) -> (f32, f32) {
        let (dx, dy) = (x - roi.cx, y - roi.cy);
        let (sin, cos) = roi.angle.sin_cos();
        let (u, v) = (cos * dx + sin * dy, -sin * dx + cos * dy);
        ((u / roi.side + 0.5) * CROP as f32, (v / roi.side + 0.5) * CROP as f32)
    }

    #[test]
    fn roi_is_level_square_and_one_and_a_half_faces() {
        let f = face(80.0, 100.0, FRAC_PI_6);
        let r = roi(&f);
        assert_eq!((r.cx, r.cy), (140.0, 150.0));
        assert_eq!(r.side, 150.0);
        assert!((r.angle - FRAC_PI_6).abs() < 1e-5);
        // Both eyes land at the same height in the crop: it is level.
        let (_, ly) = to_crop(&r, f.kps[0], f.kps[1]);
        let (_, ry) = to_crop(&r, f.kps[2], f.kps[3]);
        assert!((ly - ry).abs() < 1e-3, "{ly} vs {ry}");
        // A wider-than-tall box sizes the square by its width.
        assert_eq!(roi(&face(120.0, 90.0, 0.0)).side, 180.0);
    }

    #[test]
    fn back_projection_inverts_the_crop() {
        let r = Roi { cx: 140.0, cy: 150.0, side: 150.0, angle: PI / 6.0 };
        let (x, y) = to_picture(&r, 128.0, 128.0);
        assert!((x - 140.0).abs() < 1e-4 && (y - 150.0).abs() < 1e-4);
        // The crop's top-left corner is half a side to the left and above the centre, turned by the angle.
        let (x, y) = to_picture(&r, 0.0, 0.0);
        let (sin, cos) = (PI / 6.0).sin_cos();
        assert!((x - (140.0 - 75.0 * cos + 75.0 * sin)).abs() < 1e-3);
        assert!((y - (150.0 - 75.0 * sin - 75.0 * cos)).abs() < 1e-3);
        // Going there and back lands where it started.
        let (u, v) = to_crop(&r, x, y);
        assert!(u.abs() < 1e-3 && v.abs() < 1e-3);
    }

    #[test]
    fn crop_is_the_picture_where_it_has_one_and_black_outside() {
        let img = RgbImage::from_pixel(200, 200, Rgb([255, 102, 51]));
        // A face at the picture's top-left corner: most of its square is outside.
        let f =
            Face { bbox: [0.0, 0.0, 60.0, 60.0], kps: [15.0, 20.0, 45.0, 20.0, 30.0, 30.0, 20.0, 45.0, 40.0, 45.0] };
        let r = roi(&f);
        let px = crop(&img, &r);
        assert_eq!(px.len(), CROP * CROP * 3);
        let at = |u: usize, v: usize| &px[(v * CROP + u) * 3..][..3];
        for (got, want) in at(128, 128).iter().zip([1.0, 0.4, 0.2]) {
            assert!((got - want).abs() < 1e-4, "{got} vs {want}");
        }
        // The square's top-left corner is at (-45, -45) in the picture.
        assert_eq!(at(0, 0), [0.0; 3]);
    }

    #[test]
    fn models_are_usable_when_loaded_or_here_and_not_failing() {
        let never = || -> bool { panic!("files looked at while a reader is loaded") };
        let (just, long_ago) = (Some(Duration::from_secs(5)), Some(RETRY + Duration::from_secs(1)));
        assert!(usable_with(true, never, None));
        assert!(usable_with(true, never, just), "loaded since a failure");
        assert!(usable_with(false, || true, None), "here, not tried yet");
        assert!(!usable_with(false, || true, just), "here, but they just failed to load");
        assert!(usable_with(false, || true, long_ago), "failed a while ago: tried again");
        assert!(!usable_with(false, || false, None), "not here");
        assert!(!usable_with(false, || false, long_ago), "not here");
    }

    /// A load that panicked (holding the shared reader's lock) leaves the lock poisoned: the next
    /// looks at photos go on, and whether the models are usable is still told by what is here.
    #[test]
    fn a_panicking_load_doesnt_stop_the_next_looks() {
        let _ = std::thread::spawn(|| {
            let _held = SHARED.lock();
            panic!("a load that panics");
        })
        .join();
        assert!(SHARED.is_poisoned());
        let here = crate::models::ready(crate::models::EXPRESSIONS) && runtime_here();
        assert_eq!(shared().is_some(), here);
        unload();
        assert_eq!(usable(), here);
    }

    #[test]
    fn subset_has_146_points_all_below_478() {
        assert_eq!(SUBSET.len(), 146);
        assert!(SUBSET.iter().all(|&i| i < 478));
        assert!(SUBSET.windows(2).all(|w| w[0] < w[1]), "ascending, so no point twice");
    }

    #[test]
    fn small_faces_are_not_read() {
        // No model is needed to turn these away: none is loaded here.
        assert!(matches!(unreadable(&face(MIN_FACE_PX - 1.0, 80.0, 0.0)), Some(Read::Small)));
        assert!(unreadable(&face(MIN_FACE_PX, 80.0, 0.0)).is_none());
        // Eyes on top of each other, or not numbers, give no line to level the crop by.
        let mut f = face(100.0, 100.0, 0.0);
        (f.kps[2], f.kps[3]) = (f.kps[0] + 0.5, f.kps[1]);
        assert!(matches!(unreadable(&f), Some(Read::NoFace)));
        let mut f = face(100.0, 100.0, 0.0);
        f.kps[1] = f32::NAN;
        assert!(matches!(unreadable(&f), Some(Read::NoFace)));
        let mut f = face(100.0, 100.0, 0.0);
        f.bbox[2] = f32::INFINITY;
        assert!(matches!(unreadable(&f), Some(Read::NoFace)));
    }

    #[test]
    fn closed_needs_both_eyes() {
        let e = |l: f32, r: f32| Expression { blink: [l, r], smile: 0.0, presence: 1.0 };
        assert!(e(CLOSED, CLOSED).closed());
        assert!(e(0.9, 0.95).closed());
        assert!(!e(0.9, CLOSED - 0.01).closed(), "a wink is not closed eyes");
        assert!(!e(0.1, 0.9).closed());
        assert!(!e(0.05, 0.1).closed());
        // How shut a face's eyes are is its less shut eye's score.
        assert_eq!(e(0.9, 0.3).shut(), 0.3);
        assert_eq!(e(0.2, 0.7).shut(), 0.2);
    }

    #[test]
    fn landmarks_give_the_square_around_them_levelled_by_the_eyes() {
        // A 100-pixel square centred on (300, 200), turned by 30°: its corners, with the eyes'
        // outer corners (33, 263) at the middles of its left and right sides.
        let (c, half, angle) = ((300.0f32, 200.0f32), 50.0f32, FRAC_PI_6);
        let (sin, cos) = angle.sin_cos();
        let at = |u: f32, v: f32| (c.0 + cos * u - sin * v, c.1 + sin * u + cos * v);
        let corners = [at(-half, -half), at(half, -half), at(half, half), at(-half, half)];
        let mut points: Vec<(f32, f32)> = (0..478).map(|i| corners[i % 4]).collect();
        (points[33], points[263]) = (at(-half, 0.0), at(half, 0.0));
        let r = mesh_roi(&points);
        assert!((r.cx - c.0).abs() < 1e-3 && (r.cy - c.1).abs() < 1e-3, "{} {}", r.cx, r.cy);
        // The box around a turned square is wider than the square: 100 (cos + sin) on each side.
        let boxed = 2.0 * half * (cos + sin);
        assert!((r.side - MESH_ROI_SCALE * boxed).abs() < 1e-3, "{}", r.side);
        assert!((r.angle - angle).abs() < 1e-5);
    }

    /// The test faces: CC0 portraits, boxes and points measured by hand (tests/fixtures/faces).
    const OPEN: Face = Face {
        bbox: [78.0, 95.0, 205.0, 255.0],
        kps: [102.0, 152.0, 158.0, 150.0, 128.0, 185.0, 115.0, 213.0, 160.0, 212.0],
    };
    const SHUT: Face = Face {
        bbox: [50.0, 80.0, 185.0, 240.0],
        kps: [80.0, 145.0, 147.0, 146.0, 113.0, 175.0, 90.0, 198.0, 145.0, 198.0],
    };

    fn fixture(name: &str) -> RgbImage {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("faces");
        image::open(path.join(name)).unwrap().to_rgb8()
    }

    /// The models, or none (with a note) when they or the runtime aren't here: CI has neither.
    fn models() -> Option<Reader> {
        if !crate::models::ready(crate::models::EXPRESSIONS) || !crate::models::runtime_ready() {
            eprintln!("skipped: no expression models or AI runtime in {}", crate::paths::models_dir().display());
            return None;
        }
        Some(Reader::load().unwrap())
    }

    fn scored(what: &str, r: Read) -> Expression {
        let Read::Scored(e) = r else { panic!("{what}: {r:?}") };
        eprintln!(
            "{what}: blink L {:.3} R {:.3}, smile {:.3}, presence {:.3}",
            e.blink[0], e.blink[1], e.smile, e.presence
        );
        e
    }

    fn min_blink(e: &Expression) -> f32 {
        e.blink[0].min(e.blink[1])
    }

    #[test]
    fn a_real_face_reads_open_eyes_and_a_smile() {
        let Some(reader) = models() else { return };
        let open = scored("open", reader.read(&fixture("eyes_open.jpg"), &OPEN).unwrap());
        assert!(min_blink(&open) < 0.3 && !open.closed());
        assert!(open.smile > 0.3, "she smiles");
        assert!(open.presence > 0.5);
    }

    #[test]
    fn a_real_face_reads_closed_eyes() {
        let Some(reader) = models() else { return };
        let shut = scored("closed", reader.read(&fixture("eyes_closed_smiling.jpg"), &SHUT).unwrap());
        assert!(min_blink(&shut) > 0.5 && shut.closed());
        assert!(shut.smile > 0.3, "she laughs");
        assert!(shut.presence > 0.5);
    }

    /// The face and its points as image's rotate90 turns a picture `h` pixels tall: a quarter turn clockwise.
    fn turned(f: &Face, h: f32) -> Face {
        let turn = |x: f32, y: f32| (h - 1.0 - y, x);
        let [x1, y1, x2, y2] = f.bbox;
        let mut kps = f.kps;
        for p in kps.chunks_mut(2) {
            (p[0], p[1]) = turn(p[0], p[1]);
        }
        Face { bbox: [h - 1.0 - y2, x1, h - 1.0 - y1, x2], kps }
    }

    #[test]
    fn a_turned_face_reads_as_upright() {
        let Some(reader) = models() else { return };
        for (name, face) in [("eyes_open.jpg", &OPEN), ("eyes_closed_smiling.jpg", &SHUT)] {
            let img = fixture(name);
            let upright = scored(name, reader.read(&img, face).unwrap());
            let turned_img = image::imageops::rotate90(&img);
            let turned = scored(
                &format!("{name}, turned"),
                reader.read(&turned_img, &turned(face, img.height() as f32)).unwrap(),
            );
            assert!((min_blink(&upright) - min_blink(&turned)).abs() < 0.15, "{name}");
            assert!((upright.smile - turned.smile).abs() < 0.15, "{name}");
        }
    }

    #[test]
    fn blendshapes_take_points_at_any_scale_and_place() {
        let Some(reader) = models() else { return };
        for (name, face) in [("eyes_open.jpg", &OPEN), ("eyes_closed_smiling.jpg", &SHUT)] {
            let (points, _) = reader.landmarks(&fixture(name), &roi(face)).unwrap();
            let moved: Vec<(f32, f32)> = points.iter().map(|&(x, y)| (x * 2.0 + 100.0, y * 2.0 + 50.0)).collect();
            let (a, b) = (reader.blendshapes(subset(&points)).unwrap(), reader.blendshapes(subset(&moved)).unwrap());
            let off = a.iter().zip(&b).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            eprintln!("{name}: scores at twice the size, moved, differ by at most {off:.4}");
            assert!(off < 0.05);
        }
    }

    /// MediaPipe names the eyes as the person sees them: eyeBlinkLeft is the person's left eye, on
    /// the image's right. Shown by shutting one eye of the open face (its landmarks pressed flat
    /// onto the line between its corners) and seeing which score rises.
    #[test]
    fn eye_blink_left_is_the_eye_on_the_image_right() {
        let Some(reader) = models() else { return };
        let img = fixture("eyes_open.jpg");
        let (first, _) = reader.landmarks(&img, &roi(&OPEN)).unwrap();
        let (points, _) = reader.landmarks(&img, &mesh_roi(&first)).unwrap();
        assert!(points[33].0 < points[263].0, "landmark 33 is the outer corner of the eye on the image's left");
        assert!(points[468].0 < points[473].0, "iris 468.. is the image-left eye's, 473.. the other's");
        // The landmarks around each eye, its iris included, and its two corners.
        let image_left: Vec<usize> = [7, 33, 133, 144, 145, 153, 154, 155, 157, 158, 159, 160, 161, 163, 173, 246]
            .into_iter()
            .chain(468..473)
            .collect();
        let image_right: Vec<usize> = [249, 263, 362, 373, 374, 380, 381, 382, 384, 385, 386, 387, 388, 390, 398, 466]
            .into_iter()
            .chain(473..478)
            .collect();
        let shut = |eye: &[usize], (a, b): (usize, usize)| {
            let (pa, pb) = (points[a], points[b]);
            let (dx, dy) = (pb.0 - pa.0, pb.1 - pa.1);
            let mut p = points.clone();
            for &i in eye {
                let t = ((p[i].0 - pa.0) * dx + (p[i].1 - pa.1) * dy) / (dx * dx + dy * dy);
                p[i] = (pa.0 + t * dx, pa.1 + t * dy);
            }
            let s = reader.blendshapes(subset(&p)).unwrap();
            (s[9], s[10])
        };
        let open = reader.blendshapes(subset(&points)).unwrap();
        let (left_shut, right_shut) = (shut(&image_left, (33, 133)), shut(&image_right, (263, 362)));
        eprintln!("open: eyeBlinkLeft {:.3} eyeBlinkRight {:.3}", open[9], open[10]);
        eprintln!("image-left eye shut: eyeBlinkLeft {:.3} eyeBlinkRight {:.3}", left_shut.0, left_shut.1);
        eprintln!("image-right eye shut: eyeBlinkLeft {:.3} eyeBlinkRight {:.3}", right_shut.0, right_shut.1);
        // Each eye shut raises its own score, not the other's: much for the eye facing the camera, less
        // for the far one (the face is turned a little).
        assert!(right_shut.0 > right_shut.1 + 0.3, "shutting the image-right eye raises eyeBlinkLeft");
        assert!(left_shut.1 > left_shut.0 + 0.1, "shutting the image-left eye raises eyeBlinkRight");
    }

    #[test]
    fn the_models_are_the_ones_expected() {
        let Some(reader) = models() else { return };
        assert_eq!(reader.mesh_in, "input_12");
        assert_eq!(reader.blend_in, "serving_default_input_points:0");
        let outputs = |s: &Mutex<Session>| s.lock().unwrap().outputs.iter().map(|o| o.name.clone()).collect::<Vec<_>>();
        assert_eq!(outputs(&reader.landmarks)[..2], ["Identity", "Identity_1"]);
        assert_eq!(outputs(&reader.blendshapes), ["StatefulPartitionedCall:0"]);
    }

    /// How long reading a face takes, on the CPU and (when it passes its check) the GPU.
    #[test]
    #[ignore = "timing, run by hand"]
    fn time_a_read() {
        let Some(gpu) = models() else { return };
        let (landmarks, blendshapes) = crate::models::expression_models();
        let cpu_session = |p: &std::path::Path| Session::builder().unwrap().commit_from_file(p).unwrap();
        let cpu = Reader::with((cpu_session(&landmarks), false), (cpu_session(&blendshapes), false)).unwrap();
        let img = fixture("eyes_open.jpg");
        let mut readers = vec![("CPU", &cpu)];
        if gpu.mesh_gpu || gpu.blend_gpu {
            readers.push(("GPU", &gpu));
        }
        for (what, r) in readers {
            for _ in 0..5 {
                r.read(&img, &OPEN).unwrap();
            }
            let n = 50;
            let t = std::time::Instant::now();
            for _ in 0..n {
                r.read(&img, &OPEN).unwrap();
            }
            eprintln!("{what}: {:.2} ms per face", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
        }
    }
}
