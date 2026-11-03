//! Argentum's rule for "this photo was edited", from the sidecar format.
//!
//! Argentum marks a photo edited when its edit names a camera profile, holds
//! AI patches or masks, crops off-centre, turns, flips or reshapes the picture,
//! or else when what its renderer is given differs from what an empty edit
//! gives it. That last test is made here on the same values, worked out the
//! way the renderer receives them (32-bit floats, scaled, clamped, behind each
//! section's eye), so an edit that only moves a value back to its default, or
//! sets one the renderer never sees, is no edit.
//!
//! Its settings are Argentum's defaults: no tone mapper forced in Settings
//! (the edit's own `toneMapper` counts).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

use crate::Kind;
use crate::read::Facts;

/// The edit's keys the rule reads, in byte order. `masks` and `aiPatches` are
/// read apart: only whether they hold anything counts.
pub(crate) const KEYS: &[&str] = &[
    "agBlackWhite",
    "agLutDeclaredInput",
    "agProviderLook",
    "agRestore",
    "agSharpen",
    "blacks",
    "brightness",
    "cameraProfile",
    "centré",
    "centréX",
    "centréY",
    "chromaticAberrationBlueYellow",
    "chromaticAberrationRedCyan",
    "clarity",
    "colorCalibration",
    "colorGrading",
    "colorNoiseReduction",
    "contrast",
    "crop",
    "curves",
    "dehaze",
    "exposure",
    "flareAmount",
    "flipHorizontal",
    "flipVertical",
    "glowAmount",
    "grainAmount",
    "grainRoughness",
    "grainSize",
    "guidedPerspective",
    "halationAmount",
    "highlights",
    "hsl",
    "hue",
    "lensBuiltin",
    "lensBuiltinParams",
    "lensDistortionAmount",
    "lensDistortionEnabled",
    "lensDistortionParams",
    "lensTcaAmount",
    "lensTcaEnabled",
    "lensVignetteAmount",
    "lensVignetteEnabled",
    "lumaNoiseReduction",
    "lutIntensity",
    "lutIsSceneReferred",
    "lutPath",
    "orientationSteps",
    "rawToneCurve",
    "rawToneCurveMode",
    "rawToneRendering",
    "rotation",
    "saturation",
    "sectionVisibility",
    "shadows",
    "sharpness",
    "sharpnessThreshold",
    "showClipping",
    "structure",
    "temperature",
    "tint",
    "toneMapper",
    "transformAspect",
    "transformDistortion",
    "transformHorizontal",
    "transformRotate",
    "transformScale",
    "transformVertical",
    "transformXOffset",
    "transformYOffset",
    "vibrance",
    "vignetteAmount",
    "vignetteCenterX",
    "vignetteCenterY",
    "vignetteFeather",
    "vignetteMidpoint",
    "vignetteRoundness",
    "whiteBalance",
    "whites",
];

/// Does the rule read `key` of an edit?
pub(crate) fn reads(key: &str) -> bool {
    KEYS.binary_search(&key).is_ok()
}

/// Argentum's mark for an edit.
pub(crate) fn kind(edit: &Facts, is_raw: bool) -> Kind {
    if !is_edited(edit, is_raw) {
        Kind::None
    } else if has_film_look(edit) {
        Kind::Film
    } else {
        Kind::Edit
    }
}

fn is_edited(edit: &Facts, is_raw: bool) -> bool {
    if !edit.object {
        return false;
    }
    if names_camera_profile(edit)
        || edit.ai_patches
        || edit.masks
        || crops_off_centre(edit)
        || turns_or_flips(edit)
        || reshapes(edit)
    {
        return true;
    }
    static UNTOUCHED: OnceLock<[Vec<u32>; 2]> = OnceLock::new();
    let untouched = UNTOUCHED.get_or_init(|| {
        let empty = Facts { object: true, ..Facts::default() };
        [render(&empty, false), render(&empty, true)]
    });
    render(edit, is_raw) != untouched[is_raw as usize]
}

// ---------------------------------------------------------------------------
// The film mark: a provider look on. `lutPath` is its marker, and the look it
// names must read as one.
// ---------------------------------------------------------------------------

const PROVIDER_LOOK_MARKER: &str = "provider-look";

/// Read only to check it reads, as Argentum does.
#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderLook {
    camera: String,
    look: String,
    #[serde(default)]
    options: BTreeMap<String, bool>,
    #[serde(default)]
    own_wb: Option<OwnWb>,
    #[serde(default)]
    before: RenderingBefore,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct OwnWb {
    option: String,
    wb: Option<LookWb>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct LookWb {
    temperature: i32,
    tint: i32,
}

#[allow(dead_code)]
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RenderingBefore {
    raw_tone_rendering: Option<String>,
    raw_tone_curve: Option<Vec<CurvePoint>>,
    raw_tone_curve_mode: Option<String>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct CurvePoint {
    x: f32,
    y: f32,
}

fn has_film_look(edit: &Facts) -> bool {
    edit.get("lutPath").as_str() == Some(PROVIDER_LOOK_MARKER)
        && ProviderLook::deserialize(edit.get("agProviderLook")).is_ok()
}

// ---------------------------------------------------------------------------
// What counts before the renderer is asked.
// ---------------------------------------------------------------------------

fn names_camera_profile(edit: &Facts) -> bool {
    edit.get("cameraProfile").as_str().is_some_and(|p| !p.trim().is_empty())
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct Crop {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// A crop counts only when it starts away from the top left corner.
fn crops_off_centre(edit: &Facts) -> bool {
    let crop = edit.get("crop");
    !crop.is_null() && Crop::deserialize(crop).is_ok_and(|c| c.x.abs() > 0.1 || c.y.abs() > 0.1)
}

fn turns_or_flips(edit: &Facts) -> bool {
    edit.get("orientationSteps").as_u64().unwrap_or(0) != 0
        || edit.get("rotation").as_f64().unwrap_or(0.0).abs() > 0.001
        || edit.get("flipHorizontal").as_bool().unwrap_or(false)
        || edit.get("flipVertical").as_bool().unwrap_or(false)
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct GuideLine {
    id: String,
    #[serde(rename = "type")]
    orientation: GuideOrientation,
    p1: GuidePoint,
    p2: GuidePoint,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum GuideOrientation {
    Vertical,
    Horizontal,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct GuidePoint {
    x: f64,
    y: f64,
}

/// The camera's own lens correction, as the edit stores it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LensCorrection {
    #[allow(dead_code)]
    source: String,
    #[allow(dead_code)]
    #[serde(default)]
    default_on: bool,
    vignetting: Option<Radial>,
    distortion: Option<Radial>,
    chromatic: Option<[Radial; 2]>,
    #[allow(dead_code)]
    #[serde(default)]
    always: Option<Parts>,
    #[serde(default)]
    centre: Option<[f32; 2]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Radial {
    knots: Vec<f32>,
    values: Vec<f32>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Parts {
    distortion: bool,
    chromatic: bool,
    vignetting: bool,
}

impl Radial {
    fn is_valid(&self) -> bool {
        let (k, v) = (&self.knots, &self.values);
        k.len() >= 2
            && k.len() == v.len()
            && k.len() <= 64
            && k.iter().chain(v).all(|x| x.is_finite())
            && k.windows(2).all(|w| w[0] < w[1])
            && k[0] >= 0.0
            && v.iter().all(|x| (0.2..=5.0).contains(x))
    }
}

impl LensCorrection {
    fn corrects(&self) -> bool {
        let radials = self.vignetting.iter().chain(&self.distortion).chain(self.chromatic.iter().flatten());
        (self.vignetting.is_some() || self.distortion.is_some() || self.chromatic.is_some())
            && self.centre.iter().flatten().all(|c| (0.0..=1.0).contains(c))
            && radials.into_iter().all(Radial::is_valid)
    }
}

/// Perspective, distortion, scale, offset, guided lines or a lens correction
/// that moves the picture.
fn reshapes(edit: &Facts) -> bool {
    let guided = edit.get("guidedPerspective");
    if guided.get("enabled").and_then(Value::as_bool).unwrap_or(false)
        && guided.get("lines").and_then(|l| Vec::<GuideLine>::deserialize(l).ok()).is_some_and(|l| l.len() >= 2)
    {
        return true;
    }
    let f32_of = |key: &str, default: f64| edit.get(key).as_f64().unwrap_or(default) as f32;
    let transforms = [
        "transformDistortion",
        "transformVertical",
        "transformHorizontal",
        "transformRotate",
        "transformAspect",
        "transformXOffset",
        "transformYOffset",
    ];
    if transforms.iter().any(|k| f32_of(k, 0.0) != 0.0) || f32_of("transformScale", 100.0) != 100.0 {
        return true;
    }

    let lens_shown = is_visible(edit, "lens");
    if lens_shown
        && edit.get("lensBuiltin").as_bool() == Some(true)
        && LensCorrection::deserialize(edit.get("lensBuiltinParams")).is_ok_and(|c| c.corrects())
    {
        return true;
    }
    let profile = edit.get("lensDistortionParams").as_object();
    let param =
        |key: &str, default: f64| profile.and_then(|p| p.get(key)).and_then(Value::as_f64).unwrap_or(default) as f32;
    let part = |enabled: &str, amount: &str| {
        let on = lens_shown && edit.get(enabled).as_bool().unwrap_or(true);
        let amount = f32_of(amount, 100.0) / 100.0;
        (on, (amount - 1.0).abs() < 1e-4)
    };
    let (on, neutral) = part("lensDistortionEnabled", "lensDistortionAmount");
    let distortion = on && !(neutral && ["k1", "k2", "k3"].iter().all(|k| param(k, 0.0).abs() < 1e-6));
    let (on, neutral) = part("lensTcaEnabled", "lensTcaAmount");
    let chromatic = on && !(neutral && ["tca_vr", "tca_vb"].iter().all(|k| (param(k, 1.0) - 1.0).abs() < 1e-6));
    let (on, neutral) = part("lensVignetteEnabled", "lensVignetteAmount");
    let vignetting = on && !(neutral && ["vig_k1", "vig_k2", "vig_k3"].iter().all(|k| param(k, 0.0).abs() < 1e-6));
    distortion || chromatic || vignetting
}

// ---------------------------------------------------------------------------
// What the renderer is given, as 32-bit words. Only compared, never drawn
// with: the order is this file's own.
// ---------------------------------------------------------------------------

/// Is the eye of `section` open? Only `false` closes it.
fn is_visible(edit: &Facts, section: &str) -> bool {
    edit.get("sectionVisibility").get(section).and_then(Value::as_bool).unwrap_or(true)
}

/// A slider: its section, key, the divisor the renderer applies, its default.
type Slider = (&'static str, &'static str, f32, f64);

const SLIDERS: [Slider; 32] = [
    ("basic", "exposure", 0.8, 0.0),
    ("basic", "brightness", 0.8, 0.0),
    ("basic", "contrast", 100.0, 0.0),
    ("basic", "highlights", 120.0, 0.0),
    ("basic", "shadows", 120.0, 0.0),
    ("basic", "whites", 40.0, 0.0),
    ("basic", "blacks", 40.0, 0.0),
    ("basic", "saturation", 100.0, 0.0),
    ("basic", "vibrance", 100.0, 0.0),
    ("basic", "centréX", 1.0, 0.5),
    ("basic", "centréY", 1.0, 0.5),
    ("basic", "clarity", 125.0, 0.0),
    ("basic", "dehaze", 750.0, 0.0),
    ("basic", "structure", 125.0, 0.0),
    ("basic", "centré", 250.0, 0.0),
    ("color", "hue", 1.0, 0.0),
    ("details", "sharpness", 50.0, 0.0),
    ("details", "lumaNoiseReduction", 100.0, 0.0),
    ("details", "colorNoiseReduction", 100.0, 0.0),
    ("details", "sharpnessThreshold", 100.0, 15.0),
    ("effects", "vignetteAmount", 100.0, 0.0),
    ("effects", "vignetteMidpoint", 100.0, 50.0),
    ("effects", "vignetteRoundness", 100.0, 0.0),
    ("effects", "vignetteFeather", 100.0, 50.0),
    ("effects", "vignetteCenterX", 1.0, 0.5),
    ("effects", "vignetteCenterY", 1.0, 0.5),
    ("effects", "grainAmount", 200.0, 0.0),
    ("effects", "grainSize", 50.0, 25.0),
    ("effects", "grainRoughness", 100.0, 50.0),
    ("effects", "glowAmount", 100.0, 0.0),
    ("effects", "halationAmount", 100.0, 0.0),
    ("effects", "flareAmount", 100.0, 0.0),
];

struct Words(Vec<u32>);

impl Words {
    fn f(&mut self, v: f32) {
        self.0.push(v.to_bits());
    }

    fn u(&mut self, v: u32) {
        self.0.push(v);
    }

    fn flag(&mut self, v: bool) {
        self.0.push(v as u32);
    }

    /// Up to 16 curve points, as the renderer stores them: a point without a
    /// number for x or y is (0, 0).
    fn points(&mut self, points: &[Value]) {
        for i in 0..16 {
            let (x, y) = points.get(i).map_or((0.0, 0.0), point);
            self.f(x);
            self.f(y);
        }
    }
}

fn point(p: &Value) -> (f32, f32) {
    match (p["x"].as_f64(), p["y"].as_f64()) {
        (Some(x), Some(y)) => (x as f32, y as f32),
        _ => (0.0, 0.0),
    }
}

fn render(edit: &Facts, is_raw: bool) -> Vec<u32> {
    let mut w = Words(Vec::with_capacity(512));
    let visible = |section| is_visible(edit, section);
    let number = |v: &Value, default: f64| v.as_f64().unwrap_or(default) as f32;

    // Sliders. Black & white also clears saturation, vibrance, hue and HSL,
    // but turning it on is itself a change (below), so that is not repeated.
    for (section, key, scale, default) in SLIDERS {
        let v = if visible(section) { number(edit.get(key), default) } else { default as f32 };
        w.f(v / scale);
    }
    // Lens fringe sliders: behind no eye.
    for key in ["chromaticAberrationRedCyan", "chromaticAberrationBlueYellow"] {
        w.f(number(edit.get(key), 0.0) / 10000.0);
    }

    w.u(clipping(edit.get("showClipping")));
    w.flag(edit.get("lutPath").is_string());
    w.f(number(edit.get("lutIntensity"), 100.0) / 100.0);
    w.flag(edit.get("lutIsSceneReferred").as_bool().unwrap_or(false));
    w.flag(edit.get("toneMapper").as_str() == Some("agx"));
    // A LUT that declares its input; its exposure counts only then.
    w.flag(edit.get("agLutDeclaredInput").as_bool() == Some(true) || edit.get("lutPath").as_str() == Some("preview"));
    w.flag(white_balance_moved(edit));

    let color = visible("color");
    let calibration = edit.get("colorCalibration");
    for (key, scale) in [
        ("shadowsTint", 400.0),
        ("redHue", 400.0),
        ("redSaturation", 120.0),
        ("greenHue", 400.0),
        ("greenSaturation", 120.0),
        ("blueHue", 400.0),
        ("blueSaturation", 120.0),
    ] {
        w.f(if color { number(&calibration[key], 0.0) / scale } else { 0.0 });
    }

    let grading = edit.get("colorGrading");
    for region in ["shadows", "midtones", "highlights", "global"] {
        let r = &grading[region];
        let (h, s, l) = if color && !r.is_null() {
            (number(&r["hue"], 0.0), number(&r["saturation"], 0.0) / 500.0, number(&r["luminance"], 0.0) / 500.0)
        } else {
            (0.0, 0.0, 0.0)
        };
        w.f(h);
        w.f(s);
        w.f(l);
    }
    w.f(if color { number(&grading["blending"], 50.0) / 100.0 } else { 0.5 });
    w.f(if color { number(&grading["balance"], 0.0) / 200.0 } else { 0.0 });
    // Lightroom's grading: behind no eye.
    w.flag(grading.get("agLightroom").and_then(Value::as_f64) == Some(1.0));

    let hsl = edit.get("hsl").as_object().filter(|_| color);
    for band in ["reds", "oranges", "yellows", "greens", "aquas", "blues", "purples", "magentas"] {
        let (h, s, l) = match hsl.and_then(|m| m.get(band)) {
            Some(b) => (
                number(&b["hue"], 0.0) * 0.3,
                number(&b["saturation"], 0.0) / 100.0,
                number(&b["luminance"], 0.0) / 100.0,
            ),
            None => (0.0, 0.0, 0.0),
        };
        w.f(h);
        w.f(s);
        w.f(l);
    }

    let curves = edit.get("curves");
    let straight_line = [serde_json::json!({"x": 0.0, "y": 0.0}), serde_json::json!({"x": 255.0, "y": 255.0})];
    for channel in ["luma", "red", "green", "blue"] {
        let points: &[Value] = if !visible("curves") {
            &[]
        } else {
            match curves.get(channel) {
                Some(v) => v.as_array().map_or(&[], Vec::as_slice),
                None => &straight_line,
            }
        };
        w.points(points);
        w.u(if is_straight(points) { 0 } else { points.len() as u32 });
    }

    raw_tone(edit, is_raw, &mut w);
    sharpening(edit, is_raw, &mut w);
    w.flag(color && edit.get("agBlackWhite").get("enabled").and_then(Value::as_bool) == Some(true));
    w.0
}

/// A curve sent as no curve: from (0, 0) to (255, 255), each point within half
/// a level of the diagonal.
fn is_straight(points: &[Value]) -> bool {
    let len = points.len();
    let at = |i: usize| points.get(i.min(15)).map_or((0.0, 0.0), point);
    len >= 2
        && (0..len).all(|i| (at(i).0 - at(i).1).abs() <= 0.5)
        && at(0).0.abs() < 0.1
        && at(0).1.abs() < 0.1
        && (at(len - 1).0 - 255.0).abs() < 0.1
        && (at(len - 1).1 - 255.0).abs() < 0.1
}

/// The clipping warning: a number up to 9 picks one, `true` all of them.
fn clipping(v: &Value) -> u32 {
    match v.as_u64() {
        Some(n) => {
            if n <= 9 {
                n as u32
            } else {
                0
            }
        }
        None => v.as_bool().unwrap_or(false) as u32,
    }
}

// ---------------------------------------------------------------------------
// White balance: `whiteBalance` (absolute) moved by `temperature` and `tint`
// (relative), behind Basic's eye. The renderer gets the change from the
// reference white as gains, which are zero only when nothing moved.
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Copy, PartialEq)]
struct WhiteBalance {
    temperature: f64,
    tint: f64,
}

/// D65 in Argentum's temperature and tint, the white an edit is compared from.
const REFERENCE: WhiteBalance = WhiteBalance {
    temperature: f64::from_bits(0x40b9_67b4_ffe0_ce15), // 6503.707029390634
    tint: f64::from_bits(0x4023_8a3a_b86d_4e98),        // 9.769979251235142
};

fn white_balance_moved(edit: &Facts) -> bool {
    if !is_visible(edit, "basic") {
        return false;
    }
    let base = WhiteBalance::deserialize(edit.get("whiteBalance")).unwrap_or(REFERENCE);
    let (temperature, tint) =
        (edit.get("temperature").as_f64().unwrap_or(0.0), edit.get("tint").as_f64().unwrap_or(0.0));
    let target = if temperature == 0.0 && tint == 0.0 {
        base
    } else {
        // 1.5 mired and 1.5 tint units a step, kept in range.
        let mired = 1.0e6 / base.temperature - temperature * 1.5;
        WhiteBalance {
            temperature: (1.0e6 / mired.max(1.0e6 / 50000.0)).clamp(2000.0, 50000.0),
            tint: (base.tint + tint * 1.5).clamp(-150.0, 150.0),
        }
    };
    target != REFERENCE
}

// ---------------------------------------------------------------------------
// A RAW's tone rendering: which one, and its curve. One that names none is in
// DNG Standard, with the curve below for a photo of no known camera. Other
// photos get none, but a curve stored in their edit still reaches the renderer.
// ---------------------------------------------------------------------------

/// The renderings, numbered as the renderer knows them.
fn rendering(name: &str) -> u32 {
    match name {
        "baseCurve" => 1,
        "autoMatched" => 2,
        "dngStandard" => 3,
        "linear" => 4,
        _ => 0,
    }
}

const DNG_STANDARD: u32 = 3;

/// DNG Standard's curve where the edit carries none and the camera is not
/// known: Argentum's, as 32-bit floats (x, y on 0 to 255).
const DNG_STANDARD_CURVE: [(f32, f32); 16] = [
    (f32::from_bits(0x00000000), f32::from_bits(0x00000000)), // 0, 0
    (f32::from_bits(0x40f16b49), f32::from_bits(0x40dcb582)), // 7.5443463, 6.8971567
    (f32::from_bits(0x41949c62), f32::from_bits(0x4186b4f5)), // 18.576359, 16.838358
    (f32::from_bits(0x41fbbff5), f32::from_bits(0x4201e06b)), // 31.468729, 32.46916
    (f32::from_bits(0x4236f621), f32::from_bits(0x425a9f49)), // 45.74036, 54.655552
    (f32::from_bits(0x4274892f), f32::from_bits(0x42a1143e)), // 61.13397, 80.539536
    (f32::from_bits(0x429af85f), f32::from_bits(0x42da48e8)), // 77.4851, 109.142395
    (f32::from_bits(0x42bd5b21), f32::from_bits(0x430c04f7)), // 94.677986, 140.0194
    (f32::from_bits(0x42e14080), f32::from_bits(0x43281f23)), // 112.62598, 168.12163
    (f32::from_bits(0x430342e7), f32::from_bits(0x433fbec7)), // 131.26134, 191.74522
    (f32::from_bits(0x4316878c), f32::from_bits(0x4352ef25)), // 150.52948, 210.93416
    (f32::from_bits(0x432a62a1), f32::from_bits(0x43624383)), // 170.38527, 226.26372
    (f32::from_bits(0x433eca6b), f32::from_bits(0x436e1940)), // 190.7907, 238.09863
    (f32::from_bits(0x4353b694), f32::from_bits(0x43765fb6)), // 211.7132, 246.37387
    (f32::from_bits(0x43691fe2), f32::from_bits(0x437bcf9f)), // 233.12454, 251.81102
    (f32::from_bits(0x437f0000), f32::from_bits(0x437effff)), // 255, 254.99998
];

fn rendering_of(edit: &Facts, is_raw: bool) -> u32 {
    if !is_raw {
        return 0;
    }
    edit.get("rawToneRendering").as_str().map_or(DNG_STANDARD, rendering)
}

fn raw_tone(edit: &Facts, is_raw: bool, w: &mut Words) {
    let mode = rendering_of(edit, is_raw);
    let fitted_for = edit.get("rawToneCurveMode").as_str().map(rendering);
    let stored: &[Value] = match fitted_for {
        Some(fitted) if fitted != mode => &[],
        _ => edit.get("rawToneCurve").as_array().map_or(&[], Vec::as_slice),
    };
    w.u(mode);
    // Another rendering is already a change; only DNG Standard's stand-in
    // curve is ever compared.
    if mode == DNG_STANDARD && stored.len() < 2 {
        w.u(16);
        for (x, y) in DNG_STANDARD_CURVE {
            w.f(x);
            w.f(y);
        }
    } else {
        w.u(if mode == 0 { 0 } else { stored.len().min(16) as u32 });
        w.points(stored);
    }
}

// ---------------------------------------------------------------------------
// Sharpening (`agSharpen`), behind Details' eye and its own switch. A RAW, or a
// photo restored from one (`agRestore`), also gets capture sharpening; a RAW in
// DNG Standard or Linear brings Camera Raw's amount and radius as defaults.
// ---------------------------------------------------------------------------

fn sharpening(edit: &Facts, is_raw: bool, w: &mut Words) {
    let s = edit.get("agSharpen");
    let number = |key: &str, default: f32| {
        s.get(key).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite()).unwrap_or(default)
    };
    let flag = |key: &str, default: bool| s.get(key).and_then(Value::as_bool).unwrap_or(default);

    if !is_visible(edit, "details") || !flag("enabled", true) {
        w.0.extend([0; 10]);
        return;
    }
    let restored_radius = edit
        .get("agRestore")
        .get("captureRadius")
        .and_then(Value::as_f64)
        .map(|r| r as f32)
        .filter(|r| r.is_finite() && *r > 0.0);
    let raw_like = is_raw || restored_radius.is_some();
    let capture = raw_like && flag("capture", true);
    let capture_radius =
        if flag("autoRadius", true) { restored_radius.unwrap_or(0.75) } else { number("radius", 0.75).clamp(0.4, 2.0) };
    let mut flags = 0;
    if flag("autoContrast", true) {
        flags |= 1;
    }
    if flag("iterCheck", true) {
        flags |= 2;
    }
    if !raw_like || flag("usmOwnMask", false) {
        flags |= 4;
        if flag("usmAutoContrast", true) {
            flags |= 8;
        }
    }
    let camera_raw = matches!(rendering_of(edit, is_raw), 3 | 4);
    let (amount, radius) = if camera_raw { (25.0, 1.0) } else { (0.0, 2.0) };

    w.f(if capture { (number("captureAmount", 100.0) / 100.0).clamp(0.0, 1.0) } else { 0.0 });
    w.f(capture_radius);
    w.f(number("cornerBoost", 0.0).clamp(-0.5, 0.5));
    w.u(number("iterations", 20.0).clamp(1.0, 100.0) as u32);
    w.f((number("amount", amount) / 100.0).clamp(0.0, 2.0));
    w.f(number("usmRadius", radius).clamp(0.1, 8.0));
    w.f(number("threshold", 0.5).clamp(0.0, 100.0));
    w.f(number("contrast", 10.0).clamp(0.0, 200.0));
    w.u(flags);
    w.f(number("usmContrast", 10.0).clamp(0.0, 200.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_in_byte_order_for_the_binary_search() {
        assert!(KEYS.windows(2).all(|w| w[0] < w[1]), "{:?}", KEYS.windows(2).find(|w| w[0] >= w[1]));
    }
}
