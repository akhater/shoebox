//! Sideways photos. A model looks at each photo's preview and says whether it
//! stands upright, or which quarter turn would make it (Duarte Barbosa's image
//! orientation detector: EfficientNetV2 fine-tuned on 189,000 photos turned
//! four ways, MIT). It only suggests: the photos it finds wait in "Sideways"
//! until the user turns them (see turn.rs) or says they're fine as they are.
//! Its verdict is kept with each photo in the catalog (`photos.sideways`, two
//! small numbers) and worked out again when the photo's content changes; a
//! photo the user has decided about is never looked at again.
//!
//! It runs before faces and word search, from Shoebox's previews: photos on
//! unplugged disks are looked at too. Opt-in like every model: it runs only
//! once its model is here (Settings > AI models).

use crate::catalog::Catalog;
use crate::db::{PREVIEW_OK, SIDEWAYS_DECIDED};
use crate::search::AiProgress;
use anyhow::{Result, anyhow};
use image::{DynamicImage, imageops::FilterType};
use ort::session::Session;
use ort::value::Tensor;
use rayon::prelude::*;
use rusqlite::params;
use serde::Serialize;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// How sure the model must be before a turn is suggested: below, a photo
/// counts as upright. The model was trained never to be quite sure (its
/// surest answers stop near 0.93). Measured on previews of a real catalog
/// turned four ways (`shoebox-cli upright-eval`): at 0.9 it finds 97% of the
/// turned ones and flags 0.6% of the upright ones; with its second look
/// ([`confirms`]), 96% and 0.2%.
pub const SURE: f32 = 0.9;

/// The model's input: the picture squeezed to 416x416, its middle 384x384,
/// ImageNet's mean and spread (as the model was trained).
const SIZE: u32 = 384;
const SQUEEZED: u32 = SIZE + 32;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// The orientation detector, on the GPU when it can.
pub struct Model {
    session: Session,
    input: String,
    pub on_gpu: bool,
}

impl Model {
    pub fn load() -> Result<Self> {
        crate::models::require(crate::models::UPRIGHT)?;
        let (session, on_gpu) = crate::ai::session(&crate::models::upright_model())?;
        let input =
            session.inputs.first().map(|i| i.name.clone()).ok_or_else(|| anyhow!("orientation model has no input"))?;
        Ok(Self { session, input, on_gpu })
    }

    /// How likely each turn (0-3 quarter turns clockwise) is to make the
    /// picture stand upright.
    pub fn odds(&mut self, img: &DynamicImage) -> Result<[f32; 4]> {
        self.odds_of(preprocess(img))
    }

    /// [`Model::odds`] of a picture already prepared ([`preprocess`]).
    fn odds_of(&mut self, input: Vec<f32>) -> Result<[f32; 4]> {
        let t = Tensor::from_array(([1usize, 3, SIZE as usize, SIZE as usize], input))?;
        let out = self.session.run(ort::inputs![self.input.as_str() => t])?;
        let (_, logits) = out[0].try_extract_tensor::<f32>()?;
        let [a, b, c, d] = logits
            .get(..4)
            .and_then(|l| <[f32; 4]>::try_from(l).ok())
            .ok_or_else(|| anyhow!("orientation model: expected 4 numbers, got {}", logits.len()))?;
        Ok(softmax([a, b, c, d]))
    }
}

fn softmax(v: [f32; 4]) -> [f32; 4] {
    let top = v.iter().copied().fold(f32::MIN, f32::max);
    let e = v.map(|x| (x - top).exp());
    let sum: f32 = e.iter().sum();
    e.map(|x| x / sum)
}

/// Squeezed to 416x416 (bilinear), the middle 384x384, ImageNet's mean and
/// spread, channels first.
fn preprocess(img: &DynamicImage) -> Vec<f32> {
    let squeezed = img.resize_exact(SQUEEZED, SQUEEZED, FilterType::Triangle).to_rgb8();
    let at = (SQUEEZED - SIZE) / 2;
    let plane = (SIZE * SIZE) as usize;
    let mut out = vec![0f32; plane * 3];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = squeezed.get_pixel(at + x, at + y);
            let i = (y * SIZE + x) as usize;
            for c in 0..3 {
                out[c * plane + i] = (p[c] as f32 / 255.0 - MEAN[c]) / STD[c];
            }
        }
    }
    out
}

/// What the odds say: the quarter turns clockwise to suggest (0: upright, or
/// not sure enough), and how sure the model is of its best answer.
pub fn verdict(odds: [f32; 4]) -> (i64, f32) {
    let (best, sure) =
        odds.iter().copied().enumerate().fold((0, f32::MIN), |b, (i, p)| if p > b.1 { (i, p) } else { b });
    (if best != 0 && sure >= SURE { best as i64 } else { 0 }, sure)
}

/// The model, loaded on first use, for the AI worker.
pub struct Upright {
    model: Mutex<Option<Model>>,
    /// Reads and prepares previews: threads of its own, so the viewer's
    /// pictures read from originals (on the shared pool) never wait behind it.
    pool: rayon::ThreadPool,
}

impl Default for Upright {
    fn default() -> Self {
        let threads = (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) / 2).clamp(1, 6);
        Self {
            model: Mutex::new(None),
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("upright-{i}"))
                .build()
                .expect("threads for the upright model"),
        }
    }
}

impl Upright {
    /// Lets go of the model (it is being deleted). Waits for a look in
    /// progress: cancel it first.
    pub fn unload(&self) {
        *self.model.lock().unwrap() = None;
    }

    /// Looks at every photo with a preview the model hasn't seen yet.
    pub fn check_pending(&self, cat: &Catalog, cancel: &AtomicBool, progress: &dyn Fn(&AiProgress)) -> Result<()> {
        // Private photos' previews can only be read while they are unlocked.
        let todo: Vec<i64> = cat
            .db()
            .prepare(&format!(
                "SELECT id FROM photos p WHERE sideways IS NULL AND preview_state = {PREVIEW_OK} AND {}
                  ORDER BY taken_ts DESC",
                cat.shown_sql("p")
            ))?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if todo.is_empty() {
            return Ok(());
        }
        let mut guard = self.model.lock().unwrap();
        if guard.is_none() {
            *guard = Some(Model::load()?);
        }
        let model = guard.as_mut().unwrap();
        let mut p =
            AiProgress { task: "upright".into(), total: todo.len() as u64, gpu: model.on_gpu, ..Default::default() };
        progress(&p);
        log::info!("upright: looking at {} photos ({})", todo.len(), if p.gpu { "GPU" } else { "CPU" });
        let started = std::time::Instant::now();
        let mut found = 0;
        let mut batch: Vec<(i64, i64, f32)> = Vec::new();
        // Previews are read and prepared a few at a time on the processor's
        // cores; the model looks at them one after another.
        for chunk in todo.chunks(32) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let inputs: Vec<(i64, Result<Vec<f32>>)> = self.pool.install(|| {
                chunk
                    .par_iter()
                    .map(|id| (*id, cat.open_copy(&cat.data.preview_path(*id)).map(|img| preprocess(&img))))
                    .collect()
            });
            for (id, input) in inputs {
                let looked = input.and_then(|i| model.odds_of(i)).and_then(|odds| {
                    let (turn, sure) = verdict(odds);
                    // A second look, at the photo turned that way: it must then stand upright.
                    let sure_again = turn == 0
                        || cat.open_copy(&cat.data.preview_path(id)).and_then(|img| confirms(model, &img, turn))?;
                    Ok((if sure_again { turn } else { 0 }, sure))
                });
                match looked {
                    Ok((turn, sure)) => {
                        found += usize::from(turn != 0);
                        batch.push((id, turn, sure));
                    }
                    Err(e) => log::warn!("upright: photo {id}: {e:#}"),
                }
            }
            store(cat, &mut batch)?;
            p.done += chunk.len() as u64;
            progress(&p);
        }
        log::info!(
            "upright: {} photos looked at in {:.1} s, {found} sideways or upside down",
            p.done,
            started.elapsed().as_secs_f64()
        );
        Ok(())
    }
}

/// The model's verdicts, for photos nobody decided about meanwhile.
fn store(cat: &Catalog, batch: &mut Vec<(i64, i64, f32)>) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let db = cat.db();
    let tx = db.unchecked_transaction()?;
    for (id, turn, sure) in batch.drain(..) {
        tx.execute(
            "UPDATE photos SET sideways = ?1, sideways_p = ?2 WHERE id = ?3 AND sideways IS NULL",
            params![turn, sure, id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// A photo the model would turn.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: i64,
    /// Quarter turns clockwise.
    pub turn: i64,
    /// How sure the model is (0..1).
    pub sure: f64,
}

/// Every photo the model would turn, the surest first.
pub fn suggestions(cat: &Catalog) -> Result<Vec<Suggestion>> {
    let db = cat.read()?;
    let out = db
        .prepare(&format!(
            "SELECT id, sideways, COALESCE(sideways_p, 0) FROM photos p WHERE sideways > 0 AND {}
              ORDER BY sideways_p DESC, id",
            cat.shown_sql("p")
        ))?
        .query_map([], |r| Ok(Suggestion { id: r.get(0)?, turn: r.get(1)?, sure: r.get(2)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

/// Turns these photos the way the model suggests (those it suggests turning).
pub fn accept(cat: &Catalog, ids: &[i64]) -> Result<crate::turn::TurnReport> {
    let ids = &cat.shown_ids(ids)?[..];
    let turns: Vec<(i64, i64)> = {
        let db = cat.read()?;
        let mut stmt = db.prepare(
            "SELECT id, sideways FROM photos WHERE id IN (SELECT value FROM json_each(?1)) AND sideways > 0",
        )?;
        stmt.query_map([serde_json::to_string(ids)?], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    crate::turn::turn(cat, &turns)
}

/// The user says these photos are fine as they are: never suggested again.
/// Returns how many were suggested.
pub fn leave(cat: &Catalog, ids: &[i64]) -> Result<usize> {
    let db = cat.db();
    let tx = db.unchecked_transaction()?;
    let mut n = 0;
    for id in ids {
        n += tx.execute(
            "UPDATE photos SET sideways = ?1 WHERE id = ?2 AND COALESCE(sideways, 0) >= 0",
            params![SIDEWAYS_DECIDED, id],
        )?;
    }
    tx.commit()?;
    Ok(n)
}

/// Forgets the model's verdicts (not the user's decisions), so every photo is
/// looked at again: after the model or its threshold changes. Returns how many.
pub fn look_again(cat: &Catalog) -> Result<usize> {
    Ok(cat.db().execute("UPDATE photos SET sideways = NULL, sideways_p = NULL WHERE sideways >= 0", [])?)
}

/// How well the model does on upright JPEGs in `folder` (previews of a real
/// catalog: almost all stand upright), each shown to it four ways: `seen[k][j]`
/// counts pictures turned counter-clockwise by k quarter turns for which it
/// suggested j (at [`SURE`]); `sure_wrong` lists, per threshold, how many
/// upright ones it would have turned.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Eval {
    pub pictures: usize,
    pub seen: [[usize; 4]; 4],
    pub upright_turned_at: Vec<(f32, usize)>,
    pub turned_found_at: Vec<(f32, usize)>,
    /// Upright pictures it would have turned, with its odds, and whether it
    /// still would once it looks at them turned that way.
    pub upright_turned: Vec<(String, [f32; 4], bool)>,
    /// Turned pictures it found (at [`SURE`]), and of those, how many it
    /// still finds once it looks at them turned back.
    pub turned_found: usize,
    pub turned_found_confirmed: usize,
    pub ms_per_look: f64,
    /// Of which the model itself, input ready.
    pub ms_in_model: f64,
    pub gpu: bool,
}

/// What the model suggests for the JPEGs in `folder` as they stand (one look each).
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AsTheyStand {
    pub pictures: usize,
    /// Each with the turn suggested, how sure, and whether the model finds it
    /// upright once turned that way.
    pub suggested: Vec<(String, i64, f32, bool)>,
    pub ms_per_look: f64,
}

/// The model, looking at `img` turned by `turn`, finds it upright. Asked of
/// each turn it suggests: it keeps most of the right ones and drops most of
/// the photos that have no clear way up (foliage, people lying down
/// shot from above, fireworks) - measured with `upright-eval`.
fn confirms(model: &mut Model, img: &DynamicImage, turn: i64) -> Result<bool> {
    let turned = crate::imaging::turned(img.clone(), turn.rem_euclid(4) as u8);
    let odds = model.odds(&turned)?;
    Ok(odds[0] >= SURE)
}

pub fn eval_as_they_stand(folder: &Path, limit: usize) -> Result<AsTheyStand> {
    let mut model = Model::load()?;
    let mut out = AsTheyStand::default();
    let started = std::time::Instant::now();
    for f in jpegs(folder).take(limit) {
        let Ok(img) = image::open(f.path()) else { continue };
        out.pictures += 1;
        let (turn, sure) = verdict(model.odds(&img)?);
        if turn != 0 {
            let confirmed = confirms(&mut model, &img, turn)?;
            out.suggested.push((f.file_name().to_string_lossy().into_owned(), turn, sure, confirmed));
        }
    }
    out.ms_per_look = started.elapsed().as_secs_f64() * 1000.0 / out.pictures.max(1) as f64;
    Ok(out)
}

fn jpegs(folder: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    walkdir::WalkDir::new(folder)
        .into_iter()
        .flatten()
        .filter(|f| f.file_type().is_file() && f.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("jpg")))
}

pub fn eval(folder: &Path, limit: usize) -> Result<Eval> {
    let mut model = Model::load()?;
    let mut e = Eval { gpu: model.on_gpu, ..Default::default() };
    let mut in_model = std::time::Duration::ZERO;
    let thresholds = [0.5f32, 0.6, 0.7, 0.8, 0.9, 0.95, 0.98, 0.99];
    let mut upright = vec![0usize; thresholds.len()];
    let mut found = vec![0usize; thresholds.len()];
    let started = std::time::Instant::now();
    let mut looks = 0;
    for f in jpegs(folder).take(limit) {
        let Ok(img) = image::open(f.path()) else { continue };
        e.pictures += 1;
        for k in 0..4 {
            // Turned counter-clockwise by k: it takes k clockwise to stand upright.
            let shown = crate::imaging::turned(img.clone(), ((4 - k) % 4) as u8);
            let input = preprocess(&shown);
            let t = std::time::Instant::now();
            let odds = model.odds_of(input)?;
            in_model += t.elapsed();
            looks += 1;
            let (turn, _) = verdict(odds);
            if k == 0 && turn != 0 {
                let confirmed = confirms(&mut model, &shown, turn)?;
                e.upright_turned.push((f.file_name().to_string_lossy().into_owned(), odds, confirmed));
            }
            if k != 0 && turn == k as i64 {
                e.turned_found += 1;
                e.turned_found_confirmed += usize::from(confirms(&mut model, &shown, turn)?);
            }
            let (best, sure) =
                odds.iter().copied().enumerate().fold((0, f32::MIN), |b, (i, p)| if p > b.1 { (i, p) } else { b });
            e.seen[k][verdict(odds).0 as usize] += 1;
            for (t, th) in thresholds.iter().enumerate() {
                let suggested = best != 0 && sure >= *th;
                if k == 0 && suggested {
                    upright[t] += 1;
                }
                if k != 0 && suggested && best == k {
                    found[t] += 1;
                }
            }
        }
    }
    e.ms_per_look = started.elapsed().as_secs_f64() * 1000.0 / looks.max(1) as f64;
    e.ms_in_model = in_model.as_secs_f64() * 1000.0 / looks.max(1) as f64;
    e.upright_turned_at = thresholds.iter().copied().zip(upright).collect();
    e.turned_found_at = thresholds.iter().copied().zip(found).collect();
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_suggested_only_when_the_model_is_sure() {
        assert_eq!(verdict([0.9, 0.05, 0.03, 0.02]), (0, 0.9));
        assert_eq!(verdict([0.02, 0.95, 0.02, 0.01]), (1, 0.95));
        assert_eq!(verdict([0.02, 0.01, 0.02, 0.95]), (3, 0.95));
        // Unsure: left as it is.
        assert_eq!(verdict([0.3, 0.6, 0.05, 0.05]).0, 0);
        let s = softmax([1.0, 2.0, 3.0, 4.0]);
        assert!((s.iter().sum::<f32>() - 1.0).abs() < 1e-5 && s[3] > s[2]);
    }
}
