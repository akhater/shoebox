//! The AI models Shoebox can use: what each one is for, where it comes from,
//! and whether it's here. None ships with the app. A missing one is
//! downloaded on request from its original publisher (Immich's Hugging Face
//! pages, Microsoft for the runtime) and kept only if its SHA-256 matches the
//! one pinned below. What is here is never downloaded again. Deleting one
//! frees the space; the feature using it waits until it's back.
//!
//! The pinned hashes are the publishers' own: Hugging Face lists them, and
//! Microsoft's packages were hashed once (their library inside too, which is
//! what sits in `models\`).

use crate::paths::{self, models_dir};
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const CLIP: &str = "clip";
pub const FACES: &str = "faces";
pub const RUNTIME_GPU: &str = "runtime-gpu";
pub const RUNTIME_CPU: &str = "runtime-cpu";

/// Where a file comes from.
enum Source {
    /// The file itself.
    Url(&'static str),
    /// One file inside a zip (or NuGet package) or .tgz, itself checked first.
    Archive { url: &'static str, sha256: &'static str, size: u64, member: &'static str },
}

struct FileSpec {
    /// Inside the models folder, '/'-separated.
    path: &'static str,
    size: u64,
    sha256: &'static str,
    source: Source,
}

pub struct ModelSpec {
    pub id: &'static str,
    /// The feature it gives Shoebox.
    pub feature: &'static str,
    pub name: &'static str,
    pub about: &'static str,
    pub publisher: &'static str,
    pub licence: &'static str,
    /// Deleting the model removes this whole folder (with the files it came
    /// with); otherwise just its files.
    dir: Option<&'static str>,
    files: &'static [FileSpec],
    /// Not used by Shoebox yet: listed so its space can be freed, never downloaded.
    later: bool,
}

const DIRECTML_PACKAGE: &str = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/1.22.0/microsoft.ml.onnxruntime.directml.1.22.0.nupkg";
const DIRECTML_SHA256: &str = "29f9872d786236b79aa83f94482f3a17c14297e4833768d6d0ed4883ee732e60";
const DIRECTML_SIZE: u64 = 17_898_472;

const CLIP_FILES: &[FileSpec] = &[
    FileSpec {
        path: "clip/ViT-B-32__openai/visual/model.onnx",
        size: 351_613_724,
        sha256: "33a3df41ceef21acdf371af00f6dd0456ec1f9eba24d03a7720f9c3734e40859",
        source: Source::Url("https://huggingface.co/immich-app/ViT-B-32__openai/resolve/main/visual/model.onnx"),
    },
    FileSpec {
        path: "clip/ViT-B-32__openai/textual/model.onnx",
        size: 254_193_396,
        sha256: "b80cf0af751533a6712d92247f0ddc0c95208748bc59a1a27f33e67be6864e3b",
        source: Source::Url("https://huggingface.co/immich-app/ViT-B-32__openai/resolve/main/textual/model.onnx"),
    },
    FileSpec {
        path: "clip/ViT-B-32__openai/textual/tokenizer.json",
        size: 3_642_073,
        sha256: "6d9109cc838977f3ca94a379eec36aecc7c807e1785cd729660ca2fc0171fb35",
        source: Source::Url("https://huggingface.co/immich-app/ViT-B-32__openai/resolve/main/textual/tokenizer.json"),
    },
];

const FACE_FILES: &[FileSpec] = &[
    FileSpec {
        path: "faces/buffalo_l/detection/model.onnx",
        size: 16_923_827,
        sha256: "5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91",
        source: Source::Url("https://huggingface.co/immich-app/buffalo_l/resolve/main/detection/model.onnx"),
    },
    FileSpec {
        path: "faces/buffalo_l/recognition/model.onnx",
        size: 174_383_860,
        sha256: "4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43",
        source: Source::Url("https://huggingface.co/immich-app/buffalo_l/resolve/main/recognition/model.onnx"),
    },
];

/// ONNX Runtime 1.22.0 (what the `ort` binding expects), for this system.
#[cfg(all(windows, target_arch = "x86_64"))]
const RUNTIME_GPU_FILES: &[FileSpec] = &[FileSpec {
    path: "onnxruntime-directml/onnxruntime.dll",
    size: 16_471_584,
    sha256: "95366724919f4e95ecc60010912ed538ad9804b6683fbd0aad389749102834b9",
    source: Source::Archive {
        url: DIRECTML_PACKAGE,
        sha256: DIRECTML_SHA256,
        size: DIRECTML_SIZE,
        member: "runtimes/win-x64/native/onnxruntime.dll",
    },
}];
#[cfg(all(windows, target_arch = "x86_64"))]
const RUNTIME_CPU_FILES: &[FileSpec] = &[FileSpec {
    path: "onnxruntime.dll",
    size: 12_418_080,
    sha256: "579b636403983254346a5c1d80bd28f1519cd1e284cd204f8d4ff41f8d711559",
    source: Source::Archive {
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-win-x64-1.22.0.zip",
        sha256: "174c616efc0271194488642a72f1a514e01487da4dfe84c49296d66e40ebe0da",
        size: 72_368_545,
        member: "onnxruntime-win-x64-1.22.0/lib/onnxruntime.dll",
    },
}];

#[cfg(all(windows, target_arch = "aarch64"))]
const RUNTIME_GPU_FILES: &[FileSpec] = &[FileSpec {
    path: "onnxruntime-directml/onnxruntime.dll",
    size: 17_268_792,
    sha256: "c544001fbb76c7217fce76e9c24dada4a35d263b7ae3c0024474d9d7323a888f",
    source: Source::Archive {
        url: DIRECTML_PACKAGE,
        sha256: DIRECTML_SHA256,
        size: DIRECTML_SIZE,
        member: "runtimes/win-arm64/native/onnxruntime.dll",
    },
}];
#[cfg(all(windows, target_arch = "aarch64"))]
const RUNTIME_CPU_FILES: &[FileSpec] = &[FileSpec {
    path: "onnxruntime.dll",
    size: 12_679_728,
    sha256: "79281671a386ed1baab9dbdbb09fe55f99577011472e9526cf9d0b468bb6bcc7",
    source: Source::Archive {
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-win-arm64-1.22.0.zip",
        sha256: "7008f7ff82f8e7de563a22f2b590e08e706a1289eba606b93de2b56edfb1e04b",
        size: 73_055_483,
        member: "onnxruntime-win-arm64-1.22.0/lib/onnxruntime.dll",
    },
}];

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const RUNTIME_CPU_FILES: &[FileSpec] = &[FileSpec {
    path: "libonnxruntime.dylib",
    size: 33_481_272,
    sha256: "2b885992d3d6fa4130d39ec84a80d7504ff52750027c547bb22c86165f19406a",
    source: Source::Archive {
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-osx-arm64-1.22.0.tgz",
        sha256: "cab6dcbd77e7ec775390e7b73a8939d45fec3379b017c7cb74f5b204c1a1cc07",
        size: 25_943_843,
        member: "onnxruntime-osx-arm64-1.22.0/lib/libonnxruntime.1.22.0.dylib",
    },
}];

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const RUNTIME_CPU_FILES: &[FileSpec] = &[FileSpec {
    path: "libonnxruntime.dylib",
    size: 37_411_816,
    sha256: "283e595e61cf65df7a6b1d59a1616cbd35c8b6399dd90d799d99b71a3ff83160",
    source: Source::Archive {
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-osx-x86_64-1.22.0.tgz",
        sha256: "e4ec94a7696de74fb1b12846569aa94e499958af6ffa186022cfde16c9d617f0",
        size: 27_889_590,
        member: "onnxruntime-osx-x86_64-1.22.0/lib/libonnxruntime.1.22.0.dylib",
    },
}];

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const RUNTIME_CPU_FILES: &[FileSpec] = &[FileSpec {
    path: "libonnxruntime.so",
    size: 21_042_416,
    sha256: "3da6146e14e7b8aaec625dde11d6114c7457c87a5f93d744897da8781e35c673",
    source: Source::Archive {
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-linux-x64-1.22.0.tgz",
        sha256: "8344d55f93d5bc5021ce342db50f62079daf39aaafb5d311a451846228be49b3",
        size: 7_798_730,
        member: "onnxruntime-linux-x64-1.22.0/lib/libonnxruntime.so.1.22.0",
    },
}];

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const RUNTIME_CPU_FILES: &[FileSpec] = &[FileSpec {
    path: "libonnxruntime.so",
    size: 17_643_256,
    sha256: "0afd69a0ae38c5099fd0e8604dda398ac43dee67cd9c6394b5142b19e82528de",
    source: Source::Archive {
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.22.0/onnxruntime-linux-aarch64-1.22.0.tgz",
        sha256: "bb76395092d150b52c7092dc6b8f2fe4d80f0f3bf0416d2f269193e347e24702",
        size: 6_849_865,
        member: "onnxruntime-linux-aarch64-1.22.0/lib/libonnxruntime.so.1.22.0",
    },
}];

/// DirectML is Windows only.
#[cfg(not(windows))]
const RUNTIME_GPU_FILES: &[FileSpec] = &[];
/// No runtime published for this system: AI features stay off.
#[cfg(not(any(
    all(windows, any(target_arch = "x86_64", target_arch = "aarch64")),
    all(any(target_os = "macos", target_os = "linux"), any(target_arch = "x86_64", target_arch = "aarch64"))
)))]
const RUNTIME_CPU_FILES: &[FileSpec] = &[];

const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: CLIP,
        feature: "Word search",
        name: "CLIP ViT-B-32",
        about: "Finds photos by describing them (\"sunset on the beach\", \"birthday cake\").",
        publisher: "OpenAI, exported by Immich (Hugging Face)",
        licence: "MIT",
        dir: Some("clip/ViT-B-32__openai"),
        files: CLIP_FILES,
        later: false,
    },
    ModelSpec {
        id: FACES,
        feature: "Faces",
        name: "InsightFace buffalo_l",
        about: "Finds the faces in each photo and groups them into people.",
        publisher: "InsightFace, exported by Immich (Hugging Face)",
        licence: "Non-commercial use only",
        dir: Some("faces/buffalo_l"),
        files: FACE_FILES,
        later: false,
    },
    ModelSpec {
        id: RUNTIME_GPU,
        feature: "AI runtime, graphics card",
        name: "ONNX Runtime 1.22 with DirectML",
        about: "Runs the models above on the graphics card (Intel Arc, AMD, NVIDIA): several times faster.",
        publisher: "Microsoft (NuGet)",
        licence: "MIT",
        dir: None,
        files: RUNTIME_GPU_FILES,
        later: false,
    },
    ModelSpec {
        id: RUNTIME_CPU,
        feature: if cfg!(windows) { "AI runtime, processor" } else { "AI runtime" },
        name: "ONNX Runtime 1.22",
        about: if cfg!(windows) {
            "Runs the models on the processor, when the graphics-card runtime isn't here. Slower."
        } else {
            "Runs the models above. Word search and faces need it."
        },
        publisher: "Microsoft (GitHub)",
        licence: "MIT",
        dir: None,
        files: RUNTIME_CPU_FILES,
        later: false,
    },
    ModelSpec {
        id: "ocr",
        feature: "Text in photos",
        name: "PP-OCRv5 mobile",
        about: "Not used by Shoebox yet.",
        publisher: "PaddlePaddle, exported by Immich",
        licence: "Apache 2.0",
        dir: Some("ocr"),
        files: &[],
        later: true,
    },
    ModelSpec {
        id: "digikam",
        feature: "Object and scene tags",
        name: "digiKam's models (YOLO11, EfficientNet-B7, YuNet, SFace)",
        about: "Not used by Shoebox yet.",
        publisher: "digiKam (KDE)",
        licence: "Various (YOLO11: AGPL 3.0)",
        dir: Some("digikam"),
        files: &[],
        later: true,
    },
];

/// One model as Settings > AI models shows it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub id: String,
    pub feature: String,
    pub name: String,
    pub about: String,
    pub publisher: String,
    pub licence: String,
    /// "ready", "partial" (some files missing or wrong) or "missing".
    pub state: String,
    /// On disk now.
    pub bytes_here: u64,
    /// The whole model, once here.
    pub size: u64,
    /// What a download fetches now: only what is missing.
    pub download_bytes: u64,
    /// Not used by Shoebox yet: can be deleted, not downloaded.
    pub later: bool,
    /// The runtime Shoebox has loaded right now.
    pub in_use: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelsOverview {
    pub dir: String,
    /// Word search and faces need one runtime; false = neither is here.
    pub runtime_ready: bool,
    pub models: Vec<ModelStatus>,
}

/// Every model Shoebox knows of on this system.
fn specs() -> impl Iterator<Item = &'static ModelSpec> {
    MODELS.iter().filter(|m| m.later || !m.files.is_empty())
}

fn spec(id: &str) -> Result<&'static ModelSpec> {
    specs().find(|m| m.id == id).ok_or_else(|| anyhow!("No AI model called {id:?}. Known: {}", ids()))
}

fn ids() -> String {
    specs().map(|m| m.id).collect::<Vec<_>>().join(", ")
}

fn full_path(dir: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(dir.to_path_buf(), |p, part| p.join(part))
}

/// Here, at the right size. (Hashes are checked on download and by `verify`.)
fn file_ok(dir: &Path, f: &FileSpec) -> bool {
    full_path(dir, f.path).metadata().is_ok_and(|m| m.is_file() && m.len() == f.size)
}

fn folder_bytes(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
}

/// What a download of these files transfers (a package counts once).
fn download_size(files: &[&FileSpec]) -> u64 {
    let mut seen = Vec::new();
    files
        .iter()
        .map(|f| match f.source {
            Source::Url(_) => f.size,
            Source::Archive { url, size, .. } if !seen.contains(&url) => {
                seen.push(url);
                size
            }
            Source::Archive { .. } => 0,
        })
        .sum()
}

fn status(m: &ModelSpec, dir: &Path) -> ModelStatus {
    let (state, bytes_here, size, download_bytes) = if m.later {
        let bytes = m.dir.map(|d| folder_bytes(&full_path(dir, d))).unwrap_or(0);
        (if bytes > 0 { "ready" } else { "missing" }, bytes, bytes, 0)
    } else {
        let missing: Vec<&FileSpec> = m.files.iter().filter(|f| !file_ok(dir, f)).collect();
        let here: u64 = m.files.iter().filter(|f| file_ok(dir, f)).map(|f| f.size).sum();
        let state = match missing.len() {
            0 => "ready",
            n if n == m.files.len() && !m.files.iter().any(|f| full_path(dir, f.path).exists()) => "missing",
            _ => "partial",
        };
        (state, here, m.files.iter().map(|f| f.size).sum(), download_size(&missing))
    };
    let in_use =
        crate::ai::runtime_in_use().is_some_and(|loaded| m.files.iter().any(|f| full_path(dir, f.path) == loaded));
    ModelStatus {
        id: m.id.into(),
        feature: m.feature.into(),
        name: m.name.into(),
        about: m.about.into(),
        publisher: m.publisher.into(),
        licence: m.licence.into(),
        state: state.into(),
        bytes_here,
        size,
        download_bytes,
        later: m.later,
        in_use,
    }
}

pub fn overview() -> ModelsOverview {
    let dir = models_dir();
    ModelsOverview {
        dir: dir.to_string_lossy().into_owned(),
        runtime_ready: runtime_ready(),
        models: specs().map(|m| status(m, &dir)).collect(),
    }
}

/// Every file of this model is here.
pub fn ready(id: &str) -> bool {
    let dir = models_dir();
    spec(id).is_ok_and(|m| !m.files.is_empty() && m.files.iter().all(|f| file_ok(&dir, f)))
}

pub fn runtime_ready() -> bool {
    ready(RUNTIME_GPU) || ready(RUNTIME_CPU)
}

pub const RUNTIME_MISSING: &str = if cfg!(windows) {
    "The AI runtime isn't here. Download it in Settings > AI models (or: shoebox-cli models download runtime-gpu)."
} else {
    "The AI runtime isn't here. Download it in Settings > AI models (or: shoebox-cli models download runtime-cpu)."
};

/// A clear sentence when a feature's model (or the runtime) isn't here.
pub fn require(id: &str) -> Result<()> {
    if !runtime_ready() && crate::ai::runtime_in_use().is_none() {
        bail!(RUNTIME_MISSING);
    }
    if !ready(id) {
        let m = spec(id)?;
        bail!(
            "{} needs its AI model ({}), which isn't here. Download it in Settings > AI models (or: shoebox-cli models download {id}).",
            m.feature,
            m.name
        );
    }
    Ok(())
}

/// Downloads what this model is missing, checking every file's SHA-256 before
/// it takes its place. `progress(done, total)` counts bytes transferred.
/// Returns the bytes transferred.
pub fn download(id: &str, cancel: &AtomicBool, progress: &dyn Fn(u64, u64)) -> Result<u64> {
    let m = spec(id)?;
    if m.later {
        bail!("{} isn't used by Shoebox yet: there is nothing to download.", m.feature);
    }
    let dir = models_dir();
    let todo: Vec<&FileSpec> = m.files.iter().filter(|f| !file_ok(&dir, f)).collect();
    let total = download_size(&todo);
    let agent = http_agent(30, 60);
    let mut done = 0u64;
    progress(0, total);
    for f in todo {
        let dst = full_path(&dir, f.path);
        let report = |n: u64| progress(done + n, total);
        // Part files go when dropped; this also removes the folders a stopped
        // or failed download leaves empty.
        let tidy = |path: &Path| {
            let _ = paths::remove_model_path(
                &path.with_file_name(format!("{}.part", path.file_name().unwrap_or_default().to_string_lossy())),
            );
        };
        let fetched = match f.source {
            Source::Url(url) => fetch(&agent, url, &dst, f.sha256, cancel, &report)
                .and_then(|file| file.keep().map_err(Into::into))
                .map(|_| f.size),
            Source::Archive { url, sha256, size, member } => {
                let name = url.rsplit('/').next().unwrap_or("package");
                let package_path = dir.join(".downloads").join(name);
                let unpacked = (|| -> Result<()> {
                    let package = fetch(&agent, url, &package_path, sha256, cancel, &report)?;
                    let mut inner = io::Cursor::new(open_member(package.part(), name, member)?);
                    let mut hashed = Hashed::new(&mut inner, cancel, &quiet);
                    let file = paths::model_file(&dst, &mut hashed).with_context(|| format!("unpacking {member}"))?;
                    if hashed.hex() != f.sha256 {
                        bail!("{member} in {name} isn't the file Shoebox expects (SHA-256 differs): not kept.");
                    }
                    file.keep().map_err(Into::into)
                })();
                tidy(&package_path);
                unpacked.map(|_| size)
            }
        };
        match fetched {
            Ok(bytes) => done += bytes,
            Err(e) => {
                tidy(&dst);
                return Err(e);
            }
        }
    }
    log::info!("models: {id} downloaded ({} MB)", done >> 20);
    Ok(done)
}

/// Shoebox's HTTP client, with the system's own TLS and certificates (HTTP/1.1
/// only, which every proxy understands). Timeouts in seconds.
pub(crate) fn http_agent(connect: u64, response: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_connect(Some(std::time::Duration::from_secs(connect)))
        .timeout_recv_response(Some(std::time::Duration::from_secs(response)))
        .user_agent(concat!("Shoebox/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Downloads `url` as a part file for `dst`, checked against `sha256`.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    dst: &Path,
    sha256: &str,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64),
) -> Result<paths::ModelFile> {
    log::info!("models: downloading {url}");
    let response = agent.get(url).call().map_err(|e| anyhow!("Couldn't download {url}: {e}"))?;
    let mut body = response.into_body().into_reader();
    let mut hashed = Hashed::new(&mut body, cancel, progress);
    let file = match paths::model_file(dst, &mut hashed) {
        Ok(f) => f,
        Err(_) if cancel.load(Ordering::Relaxed) => bail!("Download stopped."),
        Err(e) => return Err(anyhow!("Couldn't download {url}: {e}")),
    };
    if hashed.hex() != sha256 {
        bail!("{url} isn't the file Shoebox expects (SHA-256 differs, {} bytes): not kept.", hashed.bytes);
    }
    Ok(file)
}

/// The one file `member` inside a downloaded package.
fn open_member(package: &Path, name: &str, member: &str) -> Result<Vec<u8>> {
    let file = std::fs::File::open(package)?;
    if name.ends_with(".tgz") || name.ends_with(".tar.gz") {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
        for entry in archive.entries()? {
            let entry = entry?;
            if entry.header().entry_type().is_file() && entry.path()?.to_string_lossy() == member {
                let mut bytes = Vec::with_capacity(entry.size() as usize);
                entry.take(256 << 20).read_to_end(&mut bytes)?;
                return Ok(bytes);
            }
        }
    } else {
        let mut archive = zip::ZipArchive::new(file)?;
        let mut entry = archive.by_name(member)?;
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        (&mut entry).take(256 << 20).read_to_end(&mut bytes)?;
        return Ok(bytes);
    }
    bail!("{member} isn't in {name}")
}

fn quiet(_: u64) {}

/// Passes bytes through, hashing and counting them; stops when cancelled.
struct Hashed<'a> {
    inner: &'a mut dyn Read,
    sha: Sha256,
    bytes: u64,
    cancel: &'a AtomicBool,
    progress: &'a dyn Fn(u64),
    last: u64,
}

impl<'a> Hashed<'a> {
    fn new(inner: &'a mut dyn Read, cancel: &'a AtomicBool, progress: &'a dyn Fn(u64)) -> Self {
        Self { inner, sha: Sha256::new(), bytes: 0, cancel, progress, last: 0 }
    }

    fn hex(&self) -> String {
        self.sha.clone().finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl Read for Hashed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("Download stopped."));
        }
        let n = self.inner.read(buf)?;
        self.sha.update(&buf[..n]);
        self.bytes += n as u64;
        if self.bytes - self.last >= 1 << 20 || n == 0 {
            self.last = self.bytes;
            (self.progress)(self.bytes);
        }
        Ok(n)
    }
}

/// Deletes a model's files (or its whole folder). Returns the bytes freed.
/// The runtime Shoebox has loaded can't go until Shoebox restarts.
pub fn delete(id: &str) -> Result<u64> {
    let m = spec(id)?;
    let dir = models_dir();
    let loaded = crate::ai::runtime_in_use();
    if m.files.iter().any(|f| loaded.as_deref() == Some(full_path(&dir, f.path).as_path())) {
        bail!(
            "Shoebox is using the AI runtime right now. Restart Shoebox and delete it before any word search or face finding starts."
        );
    }
    let freed = status(m, &dir).bytes_here;
    match m.dir {
        Some(d) => paths::remove_model_path(&full_path(&dir, d)),
        None => m.files.iter().try_for_each(|f| paths::remove_model_path(&full_path(&dir, f.path))),
    }
    .with_context(|| format!("deleting {}", m.name))?;
    log::info!("models: {id} deleted ({} MB freed)", freed >> 20);
    Ok(freed)
}

/// Each file of a model that is here, hashed against its pinned SHA-256.
#[derive(Serialize, Debug)]
pub struct Verified {
    pub file: String,
    pub ok: bool,
}

pub fn verify(id: &str) -> Result<Vec<Verified>> {
    let m = spec(id)?;
    let dir = models_dir();
    let mut out = Vec::new();
    for f in m.files {
        let path = full_path(&dir, f.path);
        let ok = match std::fs::File::open(&path) {
            Ok(mut file) => {
                let never = AtomicBool::new(false);
                let mut hashed = Hashed::new(&mut file, &never, &quiet);
                io::copy(&mut hashed, &mut io::sink())?;
                hashed.hex() == f.sha256
            }
            Err(_) => false,
        };
        out.push(Verified { file: f.path.into(), ok });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_files_are_sound() {
        let mut seen = std::collections::HashSet::new();
        for m in MODELS {
            assert!(seen.insert(m.id), "{} listed twice", m.id);
            assert!(!m.later || m.files.is_empty(), "{}", m.id);
            for f in m.files {
                assert_eq!(f.sha256.len(), 64, "{}", f.path);
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{}", f.path);
                assert!(!f.path.contains("..") && !f.path.starts_with('/') && !f.path.contains('\\'), "{}", f.path);
                let url = match f.source {
                    Source::Url(u) => u,
                    Source::Archive { url, sha256, .. } => {
                        assert_eq!(sha256.len(), 64, "{url}");
                        url
                    }
                };
                assert!(url.starts_with("https://"), "{url}");
            }
        }
    }

    /// The runtime file a download puts in place is where `ai` loads it from.
    #[test]
    fn runtime_lands_where_it_is_loaded() {
        for f in RUNTIME_CPU_FILES {
            assert_eq!(f.path, crate::ai::RUNTIME_LIB);
        }
        for f in RUNTIME_GPU_FILES {
            assert_eq!(f.path, format!("onnxruntime-directml/{}", crate::ai::RUNTIME_LIB));
        }
    }

    #[test]
    fn download_counts_a_package_once() {
        let a = FileSpec { path: "a", size: 5, sha256: "", source: Source::Url("https://x/a") };
        let b = FileSpec {
            path: "b",
            size: 5,
            sha256: "",
            source: Source::Archive { url: "https://x/p.zip", sha256: "", size: 100, member: "b" },
        };
        let c = FileSpec {
            path: "c",
            size: 5,
            sha256: "",
            source: Source::Archive { url: "https://x/p.zip", sha256: "", size: 100, member: "c" },
        };
        assert_eq!(download_size(&[&a, &b, &c]), 105);
    }
}
