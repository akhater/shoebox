//! Running the AI models: ONNX Runtime on the GPU (DirectML: Intel Arc, AMD,
//! NVIDIA) when it can, on the CPU otherwise. The runtime library and the
//! models live in `models\`; `models.rs` lists them and downloads missing
//! ones when asked (Settings > AI models).
//!
//! CLIP ViT-B-32 (Immich's export): preprocessing and tokenizing follow
//! Immich's own code exactly, so search quality matches Immich's.

use anyhow::{Context, Result, anyhow};
use image::{DynamicImage, GenericImageView, imageops::FilterType};
use ort::session::Session;
use ort::tensor::TensorElementType;
use ort::value::{Tensor, ValueType};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

/// The runtime library loaded into Shoebox, and whether it's the GPU build.
/// Loaded once; until then every use tries again (it may have just been
/// downloaded).
static RUNTIME: Mutex<Option<(PathBuf, bool)>> = Mutex::new(None);

/// ONNX Runtime's library file on this system.
pub(crate) const RUNTIME_LIB: &str = if cfg!(windows) {
    "onnxruntime.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime.dylib"
} else {
    "libonnxruntime.so"
};

/// Loads ONNX Runtime once. Returns true when it's the GPU (DirectML) build.
pub fn runtime() -> Result<bool> {
    let mut loaded = RUNTIME.lock().unwrap();
    if let Some((_, gpu)) = &*loaded {
        return Ok(*gpu);
    }
    let dir = crate::paths::models_dir();
    let gpu = dir.join("onnxruntime-directml").join(RUNTIME_LIB);
    let cpu = dir.join(RUNTIME_LIB);
    let (dll, is_gpu) = if cfg!(windows) && gpu.is_file() { (gpu, true) } else { (cpu, false) };
    if !dll.is_file() {
        return Err(anyhow!(crate::models::RUNTIME_MISSING));
    }
    ort::init_from(dll.to_string_lossy())
        .with_name("Shoebox")
        .commit()
        .map_err(|e| anyhow!("Couldn't start the AI runtime: {e}"))?;
    *loaded = Some((dll, is_gpu));
    Ok(is_gpu)
}

/// The runtime library Shoebox has loaded (it can't be deleted until Shoebox
/// restarts).
pub fn runtime_in_use() -> Option<PathBuf> {
    RUNTIME.lock().unwrap().as_ref().map(|(p, _)| p.clone())
}

/// A model session, on the GPU when available. DirectML needs memory patterns
/// and parallel execution off (as in Argentum).
///
/// A GPU can run a model and still get its answers wrong: seen on an Intel
/// Arc, where the orientation model's answers all came out washed out while
/// the CPU's were right. So the GPU only gets a model once its answer to a
/// test input matches the CPU's ([`check`]); otherwise the model runs on the
/// CPU.
pub fn session(model: &Path) -> Result<(Session, bool)> {
    if runtime()? {
        match gpu_session(model) {
            Ok(mut s) => {
                let mut cpu = cpu_session(model)?;
                match check(model, &mut s, &mut cpu) {
                    Ok(()) => return Ok((s, true)),
                    Err(e) => {
                        log::warn!("{}: {e:#}, using the CPU", model.display());
                        return Ok((cpu, false));
                    }
                }
            }
            Err(e) => log::warn!("GPU unavailable for {}, using the CPU: {e:#}", model.display()),
        }
    }
    Ok((cpu_session(model)?, false))
}

fn gpu_session(model: &Path) -> Result<Session> {
    #[cfg(windows)]
    {
        use ort::execution_providers::DirectMLExecutionProvider;
        Ok(Session::builder()?
            .with_parallel_execution(false)?
            .with_memory_pattern(false)?
            .with_execution_providers([DirectMLExecutionProvider::default().build().error_on_failure()])?
            .commit_from_file(model)?)
    }
    #[cfg(not(windows))]
    {
        let _ = model;
        Err(anyhow!("no GPU runtime on this system"))
    }
}

fn cpu_session(model: &Path) -> Result<Session> {
    Session::builder()?.commit_from_file(model).with_context(|| format!("loading {}", model.display()))
}

/// One model's GPU check (`shoebox-cli gpu-check`).
#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GpuCheck {
    pub model: &'static str,
    /// How far the GPU's answer to the test input is from the CPU's.
    pub off: Option<f32>,
    /// Close enough: Shoebox runs this model on the GPU.
    pub right: bool,
    pub error: Option<String>,
}

/// Runs the GPU check on every model that is here. Empty without the GPU runtime.
pub fn gpu_check() -> Result<Vec<GpuCheck>> {
    if !runtime()? {
        return Ok(Vec::new());
    }
    let dir = crate::paths::models_dir();
    let faces = dir.join("faces").join("buffalo_l");
    let models = [
        ("orientation", crate::models::upright_model()),
        ("clip-visual", clip_visual()),
        ("clip-text", clip_dir().join("textual").join("model.onnx")),
        ("face-detection", faces.join("detection").join("model.onnx")),
        ("face-recognition", faces.join("recognition").join("model.onnx")),
    ];
    Ok(models
        .into_iter()
        .filter(|(_, path)| path.is_file())
        .map(|(model, path)| {
            let off = cpu_session(&path)
                .and_then(|mut s| answer(&mut s))
                .and_then(|expected| Ok(off_by(&expected, &answer(&mut gpu_session(&path)?)?)));
            match off {
                Ok(off) => GpuCheck { model, off: Some(off), right: off <= GPU_OFF_MAX, error: None },
                Err(e) => GpuCheck { model, off: None, right: false, error: Some(format!("{e:#}")) },
            }
        })
        .collect())
}

/// How far the GPU's answer to the test input may be from the CPU's: the size
/// of the difference over the size of the CPU's answer. Working, the two
/// differ by rounding (measured on an Intel Arc: 3e-7 to 7e-6 for word
/// search's and faces' models); the GPU gone wrong on the orientation model
/// was 0.6 off.
const GPU_OFF_MAX: f32 = 1e-2;

/// Each model's answer on the CPU to its test input, to check the GPU by
/// again while Shoebox runs ([`still_right`]).
static ON_CPU: Mutex<Vec<(PathBuf, Vec<Vec<f32>>)>> = Mutex::new(Vec::new());

/// Runs the test input on the GPU and on the CPU: an error when the GPU's
/// answer is off. The CPU's answer is kept for [`still_right`].
fn check(model: &Path, gpu: &mut Session, cpu: &mut Session) -> Result<()> {
    let expected = answer(cpu).context("the CPU couldn't run the test input")?;
    let got = answer(gpu).context("the GPU couldn't run the test input")?;
    let off = off_by(&expected, &got);
    {
        let mut kept = ON_CPU.lock().unwrap();
        kept.retain(|(p, _)| p != model);
        kept.push((model.to_path_buf(), expected));
    }
    log::info!("{}: the GPU's answer is {off:.1e} off the CPU's", model.display());
    if off > GPU_OFF_MAX {
        anyhow::bail!("the GPU's answer is {:.0}% off the CPU's", off * 100.0);
    }
    Ok(())
}

/// Checks a model on the GPU again (a GPU can go wrong while Shoebox runs,
/// e.g. after the computer slept): false when its answer to the test input
/// no longer matches the CPU's, and the model must be loaded again.
pub fn still_right(model: &Path, s: &mut Session) -> bool {
    let expected = ON_CPU.lock().unwrap().iter().find(|(p, _)| p == model).map(|(_, e)| e.clone());
    let Some(expected) = expected else { return true };
    let off = match answer(s) {
        Ok(got) => off_by(&expected, &got),
        Err(e) => {
            log::warn!("{}: the GPU couldn't run the test input: {e:#}", model.display());
            return false;
        }
    };
    if off > GPU_OFF_MAX {
        log::warn!("{}: the GPU's answer is now {:.0}% off the CPU's", model.display(), off * 100.0);
    }
    off <= GPU_OFF_MAX
}

/// What a test input of a size the model takes (an unknown size: 1 for the
/// first, as one picture; 640 for the others) makes the model answer, each
/// output as numbers.
fn answer(s: &mut Session) -> Result<Vec<Vec<f32>>> {
    let mut inputs = Vec::new();
    for i in &s.inputs {
        let ValueType::Tensor { ty, shape, .. } = &i.input_type else {
            anyhow::bail!("input {} is not a tensor", i.name)
        };
        let dims: Vec<usize> = shape
            .iter()
            .enumerate()
            .map(|(k, &d)| {
                if d > 0 {
                    d as usize
                } else if k == 0 {
                    1
                } else {
                    640
                }
            })
            .collect();
        let value = match ty {
            TensorElementType::Float32 => Tensor::from_array((dims.clone(), test_pixels(&dims)))?.into_dyn(),
            TensorElementType::Int32 => Tensor::from_array((dims.clone(), test_tokens(&dims)))?.into_dyn(),
            TensorElementType::Int64 => {
                Tensor::from_array((dims.clone(), test_tokens(&dims).into_iter().map(i64::from).collect::<Vec<_>>()))?
                    .into_dyn()
            }
            other => anyhow::bail!("input {} holds {other:?}", i.name),
        };
        inputs.push((i.name.clone(), value));
    }
    let out = s.run(inputs)?;
    let mut answers = Vec::new();
    for (_, v) in out.iter() {
        if let Ok((_, data)) = v.try_extract_tensor::<f32>() {
            answers.push(data.to_vec());
        }
    }
    Ok(answers)
}

/// A picture-like input: smooth waves of different sizes per channel, about
/// the range of normalised pixels.
fn test_pixels(dims: &[usize]) -> Vec<f32> {
    let n: usize = dims.iter().product();
    let (w, h) = match dims {
        [.., h, w] => (*w.max(&1), *h.max(&1)),
        _ => (n.max(1), 1),
    };
    (0..n)
        .map(|i| {
            let (x, y, c) = ((i % w) as f32, ((i / w) % h) as f32, (i / (w * h)) as f32);
            (x * 0.11 + c).sin() * (y * 0.07).cos() + 0.5 * ((x + y) * 0.031 + c * 2.0).sin()
        })
        .collect()
}

/// Token-like input: small whole numbers, valid in any vocabulary of 1,000 or more.
fn test_tokens(dims: &[usize]) -> Vec<i32> {
    let n: usize = dims.iter().product();
    (0..n).map(|i| 1 + (i * 37 % 997) as i32).collect()
}

/// How far `got` is from `expected`: the size of the difference over the size
/// of `expected`, every output together. Outputs that don't match up count as
/// infinitely far.
fn off_by(expected: &[Vec<f32>], got: &[Vec<f32>]) -> f32 {
    if expected.len() != got.len() || expected.iter().zip(got).any(|(e, g)| e.len() != g.len()) {
        return f32::INFINITY;
    }
    let (mut diff, mut size) = (0f64, 0f64);
    for (e, g) in expected.iter().zip(got) {
        for (a, b) in e.iter().zip(g) {
            diff += f64::from(a - b).powi(2);
            size += f64::from(*a).powi(2);
        }
    }
    let off = (diff.sqrt() / size.sqrt().max(1e-12)) as f32;
    if off.is_nan() { f32::INFINITY } else { off }
}

pub fn clip_dir() -> PathBuf {
    crate::paths::models_dir().join("clip").join("ViT-B-32__openai")
}

pub const CLIP_MODEL: &str = "ViT-B-32__openai";
pub const CLIP_DIM: usize = 512;
const CLIP_SIZE: u32 = 224;
// OpenAI's published CLIP normalisation, kept digit for digit.
#[allow(clippy::excessive_precision)]
const CLIP_MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
#[allow(clippy::excessive_precision)]
const CLIP_STD: [f32; 3] = [0.268_629_54, 0.261_302_58, 0.275_777_11];
const CONTEXT: usize = 77;

/// The image half of CLIP (loaded by the indexer).
pub struct ClipVisual {
    session: Mutex<Session>,
    input: String,
    pub on_gpu: bool,
}

fn clip_visual() -> PathBuf {
    clip_dir().join("visual").join("model.onnx")
}

impl ClipVisual {
    pub fn load() -> Result<Self> {
        crate::models::require(crate::models::CLIP)?;
        let (session, on_gpu) = session(&clip_visual())?;
        let input = session.inputs.first().map(|i| i.name.clone()).unwrap_or_else(|| "image".into());
        Ok(Self { session: Mutex::new(session), input, on_gpu })
    }

    /// False when it runs on the GPU and that has gone wrong since it was loaded.
    pub fn still_right(&self) -> bool {
        !self.on_gpu || still_right(&clip_visual(), &mut self.session.lock().unwrap())
    }

    /// A unit-length 512-number description of what the picture shows.
    pub fn embed(&self, img: &DynamicImage) -> Result<Vec<f32>> {
        let pixels = preprocess(img);
        let tensor = Tensor::from_array(([1usize, 3, CLIP_SIZE as usize, CLIP_SIZE as usize], pixels))?;
        let mut s = self.session.lock().unwrap();
        let out = s.run(ort::inputs![self.input.as_str() => tensor])?;
        let (_, v) = out[0].try_extract_tensor::<f32>()?;
        Ok(normalize(v.to_vec()))
    }
}

/// The text half of CLIP (loaded on the first search).
pub struct ClipText {
    session: Mutex<Session>,
    input: String,
    int64: bool,
    tokenizer: Tokenizer,
}

impl ClipText {
    pub fn load() -> Result<Self> {
        crate::models::require(crate::models::CLIP)?;
        let dir = clip_dir().join("textual");
        let (session, _) = session(&dir.join("model.onnx"))?;
        let first = session.inputs.first().ok_or_else(|| anyhow!("text model has no input"))?;
        let input = first.name.clone();
        let int64 = matches!(first.input_type, ValueType::Tensor { ty: TensorElementType::Int64, .. });

        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| anyhow!("tokenizer: {e}"))?;
        // As Immich: pad with the end-of-text token to 77, truncate at 77.
        let pad_token = "<|endoftext|>".to_string();
        let pad_id = tokenizer.token_to_id(&pad_token).unwrap_or(49407);
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::Fixed(CONTEXT),
            pad_id,
            pad_token,
            ..Default::default()
        }));
        tokenizer
            .with_truncation(Some(TruncationParams { max_length: CONTEXT, ..Default::default() }))
            .map_err(|e| anyhow!("tokenizer: {e}"))?;
        Ok(Self { session: Mutex::new(session), input, int64, tokenizer })
    }

    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let enc = self.tokenizer.encode(text, true).map_err(|e| anyhow!("tokenizer: {e}"))?;
        let ids = enc.get_ids();
        let mut s = self.session.lock().unwrap();
        let out = if self.int64 {
            let t = Tensor::from_array(([1usize, CONTEXT], ids.iter().map(|&i| i as i64).collect::<Vec<_>>()))?;
            s.run(ort::inputs![self.input.as_str() => t])?
        } else {
            let t = Tensor::from_array(([1usize, CONTEXT], ids.iter().map(|&i| i as i32).collect::<Vec<_>>()))?;
            s.run(ort::inputs![self.input.as_str() => t])?
        };
        let (_, v) = out[0].try_extract_tensor::<f32>()?;
        Ok(normalize(v.to_vec()))
    }
}

/// Shortest side to 224 (bicubic), centre crop 224x224, CLIP's mean/std, CHW.
fn preprocess(img: &DynamicImage) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let s = CLIP_SIZE as f32 / w.min(h).max(1) as f32;
    let (nw, nh) = (((w as f32 * s).round() as u32).max(CLIP_SIZE), ((h as f32 * s).round() as u32).max(CLIP_SIZE));
    let resized = img.resize_exact(nw, nh, FilterType::CatmullRom).to_rgb8();
    let (x0, y0) = ((nw - CLIP_SIZE) / 2, (nh - CLIP_SIZE) / 2);
    let plane = (CLIP_SIZE * CLIP_SIZE) as usize;
    let mut out = vec![0f32; plane * 3];
    for y in 0..CLIP_SIZE {
        for x in 0..CLIP_SIZE {
            let p = resized.get_pixel(x0 + x, y0 + y);
            let i = (y * CLIP_SIZE + x) as usize;
            for c in 0..3 {
                out[c * plane + i] = (p[c] as f32 / 255.0 - CLIP_MEAN[c]) / CLIP_STD[c];
            }
        }
    }
    out
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_compared_by_how_far_apart_they_are() {
        let cpu = vec![vec![3.0, 4.0], vec![0.0]];
        assert_eq!(off_by(&cpu, &cpu), 0.0);
        // A rounding difference passes, an answer gone soft doesn't.
        assert!(off_by(&cpu, &[vec![3.0001, 4.0], vec![0.0]]) < GPU_OFF_MAX);
        assert!(off_by(&cpu, &[vec![2.0, 3.0], vec![0.0]]) > GPU_OFF_MAX);
        // Answers that don't match up, or aren't numbers, are as far as can be.
        assert_eq!(off_by(&cpu, &[vec![3.0, 4.0]]), f32::INFINITY);
        assert_eq!(off_by(&cpu, &[vec![3.0], vec![0.0]]), f32::INFINITY);
        assert_eq!(off_by(&cpu, &[vec![f32::NAN, 4.0], vec![0.0]]), f32::INFINITY);
    }

    #[test]
    fn test_inputs_look_like_pictures_and_tokens() {
        let px = test_pixels(&[1, 3, 8, 16]);
        assert_eq!(px.len(), 3 * 8 * 16);
        assert!(px.iter().all(|v| v.abs() <= 1.5), "about the range of normalised pixels");
        assert_ne!(px[0], px[8 * 16], "each channel its own");
        assert_eq!(px, test_pixels(&[1, 3, 8, 16]), "the same every time");
        let tokens = test_tokens(&[1, 77]);
        assert_eq!(tokens.len(), 77);
        assert!(tokens.iter().all(|t| (1..1000).contains(t)));
    }
}
