//! Running the AI models: ONNX Runtime on the GPU (DirectML: Intel Arc, AMD,
//! NVIDIA) when it can, on the CPU otherwise. The runtime DLL and the models
//! live in `models\` next to the app and are never downloaded here.
//!
//! CLIP ViT-B-32 (Immich's export): preprocessing and tokenizing follow
//! Immich's own code exactly, so search quality matches Immich's.

use anyhow::{Context, Result, anyhow};
use image::{DynamicImage, GenericImageView, imageops::FilterType};
use ort::session::Session;
use ort::tensor::TensorElementType;
use ort::value::{Tensor, ValueType};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

static RUNTIME: OnceLock<Result<bool, String>> = OnceLock::new();

/// Loads ONNX Runtime once. Returns true when it's the GPU (DirectML) build.
pub fn runtime() -> Result<bool> {
    RUNTIME
        .get_or_init(|| {
            let dir = crate::paths::models_dir();
            let gpu = dir.join("onnxruntime-directml").join("onnxruntime.dll");
            let cpu = dir.join("onnxruntime.dll");
            let (dll, is_gpu) = if cfg!(windows) && gpu.is_file() { (gpu, true) } else { (cpu, false) };
            if !dll.is_file() {
                return Err(format!("The AI runtime isn't in {}", dir.display()));
            }
            ort::init_from(dll.to_string_lossy())
                .with_name("Shoebox")
                .commit()
                .map(|_| is_gpu)
                .map_err(|e| format!("Couldn't start the AI runtime: {e}"))
        })
        .clone()
        .map_err(|e| anyhow!(e))
}

/// A model session, on the GPU when available. DirectML needs memory patterns
/// and parallel execution off (as in Argentum).
pub fn session(model: &Path) -> Result<(Session, bool)> {
    let gpu = runtime()?;
    if gpu {
        #[cfg(windows)]
        {
            use ort::execution_providers::DirectMLExecutionProvider;
            let built = Session::builder()?
                .with_parallel_execution(false)?
                .with_memory_pattern(false)?
                .with_execution_providers([DirectMLExecutionProvider::default().build().error_on_failure()])
                .and_then(|b| b.commit_from_file(model));
            match built {
                Ok(s) => return Ok((s, true)),
                Err(e) => log::warn!("GPU unavailable for {}, using the CPU: {e}", model.display()),
            }
        }
    }
    let s = Session::builder()?.commit_from_file(model).with_context(|| format!("loading {}", model.display()))?;
    Ok((s, false))
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

impl ClipVisual {
    pub fn load() -> Result<Self> {
        let (session, on_gpu) = session(&clip_dir().join("visual").join("model.onnx"))?;
        let input = session.inputs.first().map(|i| i.name.clone()).unwrap_or_else(|| "image".into());
        Ok(Self { session: Mutex::new(session), input, on_gpu })
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
