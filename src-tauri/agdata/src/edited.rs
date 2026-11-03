//! The edited rule: whether an edit changes its photo, and whether it has a
//! film look on.
//!
//! An edit changes its photo when it names something that always shows (a
//! camera profile, masks, AI patches, an off-centre crop, a turn, a flip, a
//! reshape, a lens correction), or else when the values a renderer is given
//! for it differ from those given for an empty edit, compared bit for bit as
//! 32-bit floats and integers.

use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

use crate::Kind;
use crate::read::Facts;

/// The edit's top-level keys the rule reads, in byte order for a binary search.
/// `masks` and `aiPatches` are not here: the reader keeps them as flags.
const READ_KEYS: [&str; 79] = [
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

/// Whether the rule reads the edit's top-level key `key`.
pub(crate) fn reads(key: &str) -> bool {
    READ_KEYS.binary_search(&key).is_ok()
}

/// The mark an edit gets. `is_raw`: whether its photo is a RAW file.
pub(crate) fn kind(edit: &Facts, is_raw: bool) -> Kind {
    if !edit.object {
        return Kind::None;
    }
    let eyes = Eyes::of(edit);
    let edited = shows_directly(edit, &eyes) || Rendered::of(edit, &eyes, is_raw) != *untouched(is_raw);
    match edited {
        false => Kind::None,
        true if film_look(edit) => Kind::Film,
        true => Kind::Edit,
    }
}

/// What a renderer is given for an empty edit, worked out once per kind of photo.
fn untouched(is_raw: bool) -> &'static Rendered {
    static UNTOUCHED: [OnceLock<Rendered>; 2] = [OnceLock::new(), OnceLock::new()];
    UNTOUCHED[usize::from(is_raw)].get_or_init(|| {
        let empty = Facts { object: true, ..Facts::default() };
        Rendered::of(&empty, &Eyes::of(&empty), is_raw)
    })
}

// ---------------------------------------------------------------------------
// Reading values

/// `v` as a number, or `default` when it is not one.
fn number(v: &Value, default: f64) -> f64 {
    v.as_f64().unwrap_or(default)
}

/// `v` as a number, or `default`, as a 32-bit float.
fn float(v: &Value, default: f64) -> f32 {
    number(v, default) as f32
}

/// Whether `v` is the boolean `true`.
fn is_true(v: &Value) -> bool {
    v.as_bool() == Some(true)
}

/// The editor's sections, each with an eye that hides its values.
#[derive(Clone, Copy)]
enum Section {
    Basic,
    Color,
    Curves,
    Details,
    Effects,
    Lens,
}

/// Which sections are shown. Only an explicit `false` hides one.
struct Eyes([bool; 6]);

impl Eyes {
    fn of(edit: &Facts) -> Self {
        let v = edit.get("sectionVisibility");
        let shown = |name: &str| v[name].as_bool() != Some(false);
        Eyes([shown("basic"), shown("color"), shown("curves"), shown("details"), shown("effects"), shown("lens")])
    }

    fn shown(&self, section: Section) -> bool {
        self.0[section as usize]
    }
}

// ---------------------------------------------------------------------------
// What always shows

/// Keys whose value, as a 32-bit float, reshapes the picture unless it is 0.
const RESHAPES: [&str; 7] = [
    "transformDistortion",
    "transformVertical",
    "transformHorizontal",
    "transformRotate",
    "transformAspect",
    "transformXOffset",
    "transformYOffset",
];

/// A lens profile correction: its switch, its amount, and its parameters in
/// `lensDistortionParams`, with the value each one rests at.
struct LensPart {
    switch: &'static str,
    amount: &'static str,
    params: &'static [&'static str],
    rest: f32,
}

const LENS_PARTS: [LensPart; 3] = [
    LensPart {
        switch: "lensDistortionEnabled",
        amount: "lensDistortionAmount",
        params: &["k1", "k2", "k3"],
        rest: 0.0,
    },
    LensPart { switch: "lensTcaEnabled", amount: "lensTcaAmount", params: &["tca_vr", "tca_vb"], rest: 1.0 },
    LensPart {
        switch: "lensVignetteEnabled",
        amount: "lensVignetteAmount",
        params: &["vig_k1", "vig_k2", "vig_k3"],
        rest: 0.0,
    },
];

/// Whether the edit names something that changes the photo whatever its values.
fn shows_directly(edit: &Facts, eyes: &Eyes) -> bool {
    edit.get("cameraProfile").as_str().is_some_and(|p| !p.trim().is_empty())
        || edit.ai_patches
        || edit.masks
        || crops_off_centre(edit.get("crop"))
        || edit.get("orientationSteps").as_u64().is_some_and(|n| n != 0)
        || number(edit.get("rotation"), 0.0).abs() > 0.001
        || is_true(edit.get("flipHorizontal"))
        || is_true(edit.get("flipVertical"))
        || guides_perspective(edit.get("guidedPerspective"))
        || RESHAPES.iter().any(|key| float(edit.get(key), 0.0) != 0.0)
        || float(edit.get("transformScale"), 100.0) != 100.0
        || corrects_built_in_lens(edit, eyes)
        || LENS_PARTS.iter().any(|part| corrects_lens_part(edit, eyes, part))
}

fn crops_off_centre(crop: &Value) -> bool {
    !crop.is_null() && shape::Rect::deserialize(crop).is_ok_and(|r| r.x.abs() > 0.1 || r.y.abs() > 0.1)
}

fn guides_perspective(guided: &Value) -> bool {
    is_true(&guided["enabled"]) && Vec::<shape::GuideLine>::deserialize(&guided["lines"]).is_ok_and(|l| l.len() >= 2)
}

fn corrects_built_in_lens(edit: &Facts, eyes: &Eyes) -> bool {
    eyes.shown(Section::Lens)
        && is_true(edit.get("lensBuiltin"))
        && shape::LensTable::deserialize(edit.get("lensBuiltinParams")).is_ok_and(|t| t.has_part() && t.is_valid())
}

fn corrects_lens_part(edit: &Facts, eyes: &Eyes, part: &LensPart) -> bool {
    let on = eyes.shown(Section::Lens) && edit.get(part.switch).as_bool().unwrap_or(true);
    if !on {
        return false;
    }
    let amount = float(edit.get(part.amount), 100.0) / 100.0;
    let neutral = (amount - 1.0).abs() < 1e-4;
    let params = edit.get("lensDistortionParams");
    let at_rest = part.params.iter().all(|key| (float(&params[*key], f64::from(part.rest)) - part.rest).abs() < 1e-6);
    !(neutral && at_rest)
}

// ---------------------------------------------------------------------------
// What the renderer is given

/// The sliders: section, key, divisor, default.
const SLIDERS: [(Section, &str, f32, f64); 32] = [
    (Section::Basic, "exposure", 0.8, 0.0),
    (Section::Basic, "brightness", 0.8, 0.0),
    (Section::Basic, "contrast", 100.0, 0.0),
    (Section::Basic, "highlights", 120.0, 0.0),
    (Section::Basic, "shadows", 120.0, 0.0),
    (Section::Basic, "whites", 40.0, 0.0),
    (Section::Basic, "blacks", 40.0, 0.0),
    (Section::Basic, "saturation", 100.0, 0.0),
    (Section::Basic, "vibrance", 100.0, 0.0),
    (Section::Basic, "centréX", 1.0, 0.5),
    (Section::Basic, "centréY", 1.0, 0.5),
    (Section::Basic, "clarity", 125.0, 0.0),
    (Section::Basic, "dehaze", 750.0, 0.0),
    (Section::Basic, "structure", 125.0, 0.0),
    (Section::Basic, "centré", 250.0, 0.0),
    (Section::Color, "hue", 1.0, 0.0),
    (Section::Details, "sharpness", 50.0, 0.0),
    (Section::Details, "lumaNoiseReduction", 100.0, 0.0),
    (Section::Details, "colorNoiseReduction", 100.0, 0.0),
    (Section::Details, "sharpnessThreshold", 100.0, 15.0),
    (Section::Effects, "vignetteAmount", 100.0, 0.0),
    (Section::Effects, "vignetteMidpoint", 100.0, 50.0),
    (Section::Effects, "vignetteRoundness", 100.0, 0.0),
    (Section::Effects, "vignetteFeather", 100.0, 50.0),
    (Section::Effects, "vignetteCenterX", 1.0, 0.5),
    (Section::Effects, "vignetteCenterY", 1.0, 0.5),
    (Section::Effects, "grainAmount", 200.0, 0.0),
    (Section::Effects, "grainSize", 50.0, 25.0),
    (Section::Effects, "grainRoughness", 100.0, 50.0),
    (Section::Effects, "glowAmount", 100.0, 0.0),
    (Section::Effects, "halationAmount", 100.0, 0.0),
    (Section::Effects, "flareAmount", 100.0, 0.0),
];

/// Colour calibration, in `colorCalibration`: key and divisor.
const CALIBRATION: [(&str, f32); 7] = [
    ("shadowsTint", 400.0),
    ("redHue", 400.0),
    ("redSaturation", 120.0),
    ("greenHue", 400.0),
    ("greenSaturation", 120.0),
    ("blueHue", 400.0),
    ("blueSaturation", 120.0),
];

const GRADING_REGIONS: [&str; 4] = ["shadows", "midtones", "highlights", "global"];

const HSL_BANDS: [&str; 8] = ["reds", "oranges", "yellows", "greens", "aquas", "blues", "purples", "magentas"];

const CURVE_CHANNELS: [&str; 4] = ["luma", "red", "green", "blue"];

/// A white balance as temperature and tint.
#[derive(Deserialize)]
struct White {
    temperature: f64,
    tint: f64,
}

/// The white an edit without one is given.
const REFERENCE_WHITE: White =
    White { temperature: f64::from_bits(0x40b967b4ffe0ce15), tint: f64::from_bits(0x40238a3ab86d4e98) };

/// The DNG Standard rendering, as tone renderings are numbered.
const DNG_STANDARD: u32 = 3;

/// The curve a RAW photo in DNG Standard gets when its edit has none: the bits
/// of each point's x and y.
const DNG_STANDARD_CURVE: [[u32; 2]; 16] = [
    [0x00000000, 0x00000000],
    [0x40f16b49, 0x40dcb582],
    [0x41949c62, 0x4186b4f5],
    [0x41fbbff5, 0x4201e06b],
    [0x4236f621, 0x425a9f49],
    [0x4274892f, 0x42a1143e],
    [0x429af85f, 0x42da48e8],
    [0x42bd5b21, 0x430c04f7],
    [0x42e14080, 0x43281f23],
    [0x430342e7, 0x433fbec7],
    [0x4316878c, 0x4352ef25],
    [0x432a62a1, 0x43624383],
    [0x433eca6b, 0x436e1940],
    [0x4353b694, 0x43765fb6],
    [0x43691fe2, 0x437bcf9f],
    [0x437f0000, 0x437effff],
];

/// A tone rendering's number; 0 for a name that is none.
fn rendering(name: &str) -> u32 {
    match name {
        "baseCurve" => 1,
        "autoMatched" => 2,
        "dngStandard" => DNG_STANDARD,
        "linear" => 4,
        _ => 0,
    }
}

/// The tone rendering of a RAW photo (DNG Standard unless named), none for any
/// other photo.
fn raw_rendering(edit: &Facts, is_raw: bool) -> u32 {
    if !is_raw {
        return 0;
    }
    edit.get("rawToneRendering").as_str().map_or(DNG_STANDARD, rendering)
}

/// A curve as a renderer holds it: its first 16 points, (0, 0) where a point
/// is not two numbers, and how many points it had.
struct Curve {
    slots: [[f32; 2]; 16],
    len: usize,
}

impl Curve {
    /// The curve a channel without one gets: a straight line.
    const STRAIGHT: Curve = {
        let mut slots = [[0.0; 2]; 16];
        slots[1] = [255.0, 255.0];
        Curve { slots, len: 2 }
    };

    fn of(points: &[Value]) -> Self {
        let mut slots = [[0.0; 2]; 16];
        for (slot, point) in slots.iter_mut().zip(points) {
            if let (Some(x), Some(y)) = (point["x"].as_f64(), point["y"].as_f64()) {
                *slot = [x as f32, y as f32];
            }
        }
        Curve { slots, len: points.len() }
    }

    /// Whether the curve leaves tones as they are: a diagonal from (0, 0) to
    /// (255, 255). Points past the 16th are read as the 16th.
    fn is_straight(&self) -> bool {
        let slot = |i: usize| self.slots[i.min(15)];
        let [x0, y0] = slot(0);
        let [xn, yn] = slot(self.len.wrapping_sub(1));
        self.len >= 2
            && (0..self.len.min(16)).all(|i| (slot(i)[0] - slot(i)[1]).abs() <= 0.5)
            && x0.abs() < 0.1
            && y0.abs() < 0.1
            && (xn - 255.0).abs() < 0.1
            && (yn - 255.0).abs() < 0.1
    }
}

/// `v` as an array, empty when it is not one.
fn items(v: &Value) -> &[Value] {
    v.as_array().map_or(&[][..], Vec::as_slice)
}

/// How many words `Rendered` holds.
const WORDS: usize = 264;

/// What a renderer is given for an edit: each value's bits, in a fixed order.
#[derive(PartialEq, Eq)]
struct Rendered {
    words: [u32; WORDS],
    len: usize,
}

impl Rendered {
    fn of(edit: &Facts, eyes: &Eyes, is_raw: bool) -> Self {
        let mut r = Rendered { words: [0; WORDS], len: 0 };
        r.sliders(edit, eyes);
        r.ungated(edit);
        r.flag(eyes.shown(Section::Basic) && white_balance_moved(edit));
        r.colour(edit, eyes);
        r.curves(edit, eyes);
        let mode = raw_rendering(edit, is_raw);
        r.raw_tone(edit, mode);
        r.sharpening(edit, eyes, is_raw, mode);
        r.flag(eyes.shown(Section::Color) && is_true(&edit.get("agBlackWhite")["enabled"]));
        debug_assert_eq!(r.len, WORDS);
        r
    }

    fn word(&mut self, word: u32) {
        self.words[self.len] = word;
        self.len += 1;
    }

    fn float(&mut self, v: f32) {
        self.word(v.to_bits());
    }

    fn flag(&mut self, on: bool) {
        self.word(u32::from(on));
    }

    fn sliders(&mut self, edit: &Facts, eyes: &Eyes) {
        for (section, key, divisor, default) in SLIDERS {
            let v = if eyes.shown(section) { float(edit.get(key), default) } else { default as f32 };
            self.float(v / divisor);
        }
    }

    /// Values no eye hides.
    fn ungated(&mut self, edit: &Facts) {
        self.float(float(edit.get("chromaticAberrationRedCyan"), 0.0) / 10000.0);
        self.float(float(edit.get("chromaticAberrationBlueYellow"), 0.0) / 10000.0);
        let clipping = edit.get("showClipping");
        self.word(match clipping.as_u64() {
            Some(n) if n <= 9 => n as u32,
            Some(_) => 0,
            None => u32::from(is_true(clipping)),
        });
        let lut = edit.get("lutPath");
        self.flag(lut.is_string());
        self.float(float(edit.get("lutIntensity"), 100.0) / 100.0);
        self.flag(is_true(edit.get("lutIsSceneReferred")));
        self.flag(edit.get("toneMapper").as_str() == Some("agx"));
        self.flag(is_true(edit.get("agLutDeclaredInput")) || lut.as_str() == Some("preview"));
        self.flag(edit.get("colorGrading")["agLightroom"].as_f64() == Some(1.0));
    }

    /// Calibration, grading and the HSL table, all behind the colour eye.
    fn colour(&mut self, edit: &Facts, eyes: &Eyes) {
        let shown = eyes.shown(Section::Color);

        let calibration = edit.get("colorCalibration");
        for (key, divisor) in CALIBRATION {
            self.float(if shown { float(&calibration[key], 0.0) / divisor } else { 0.0 });
        }

        let grading = edit.get("colorGrading");
        for region in GRADING_REGIONS {
            let r = &grading[region];
            if shown && !r.is_null() {
                self.float(float(&r["hue"], 0.0));
                self.float(float(&r["saturation"], 0.0) / 500.0);
                self.float(float(&r["luminance"], 0.0) / 500.0);
            } else {
                self.zeros(3);
            }
        }
        self.float(if shown { float(&grading["blending"], 50.0) / 100.0 } else { 0.5 });
        self.float(if shown { float(&grading["balance"], 0.0) / 200.0 } else { 0.0 });

        let hsl = edit.get("hsl").as_object();
        for band in HSL_BANDS {
            match hsl.and_then(|m| m.get(band)) {
                Some(b) if shown => {
                    self.float(float(&b["hue"], 0.0) * 0.3);
                    self.float(float(&b["saturation"], 0.0) / 100.0);
                    self.float(float(&b["luminance"], 0.0) / 100.0);
                }
                _ => self.zeros(3),
            }
        }
    }

    fn curves(&mut self, edit: &Facts, eyes: &Eyes) {
        let curves = edit.get("curves").as_object();
        for channel in CURVE_CHANNELS {
            let curve = match curves.and_then(|m| m.get(channel)) {
                _ if !eyes.shown(Section::Curves) => Curve::of(&[]),
                Some(points) => Curve::of(items(points)),
                None => Curve::STRAIGHT,
            };
            self.points(&curve.slots);
            self.word(if curve.is_straight() { 0 } else { curve.len as u32 });
        }
    }

    /// The RAW tone rendering `mode` and its curve.
    fn raw_tone(&mut self, edit: &Facts, mode: u32) {
        let points: &[Value] = match edit.get("rawToneCurveMode").as_str() {
            Some(name) if rendering(name) != mode => &[],
            _ => items(edit.get("rawToneCurve")),
        };
        self.word(mode);
        if mode == DNG_STANDARD && points.len() < 2 {
            self.word(16);
            for [x, y] in DNG_STANDARD_CURVE {
                self.word(x);
                self.word(y);
            }
        } else {
            self.word(if mode == 0 { 0 } else { points.len().min(16) as u32 });
            self.points(&Curve::of(points).slots);
        }
    }

    /// Sharpening (`agSharpen`), behind the details eye. `mode`: the RAW tone
    /// rendering.
    fn sharpening(&mut self, edit: &Facts, eyes: &Eyes, is_raw: bool, mode: u32) {
        let s = edit.get("agSharpen");
        let num =
            |key: &str, default: f32| s[key].as_f64().map(|v| v as f32).filter(|v| v.is_finite()).unwrap_or(default);
        let flag = |key: &str, default: bool| s[key].as_bool().unwrap_or(default);
        if !eyes.shown(Section::Details) || !flag("enabled", true) {
            self.zeros(10);
            return;
        }
        let restored =
            edit.get("agRestore")["captureRadius"].as_f64().map(|v| v as f32).filter(|v| v.is_finite() && *v > 0.0);
        let raw_like = is_raw || restored.is_some();
        let capture = raw_like && flag("capture", true);
        let camera_raw = is_raw && matches!(mode, 3 | 4);
        let (amount, usm_radius) = if camera_raw { (25.0, 1.0) } else { (0.0, 2.0) };

        self.float(if capture { (num("captureAmount", 100.0) / 100.0).clamp(0.0, 1.0) } else { 0.0 });
        self.float(if flag("autoRadius", true) {
            restored.unwrap_or(0.75)
        } else {
            num("radius", 0.75).clamp(0.4, 2.0)
        });
        self.float(num("cornerBoost", 0.0).clamp(-0.5, 0.5));
        self.word(num("iterations", 20.0).clamp(1.0, 100.0) as u32);
        self.float((num("amount", amount) / 100.0).clamp(0.0, 2.0));
        self.float(num("usmRadius", usm_radius).clamp(0.1, 8.0));
        self.float(num("threshold", 0.5).clamp(0.0, 100.0));
        self.float(num("contrast", 10.0).clamp(0.0, 200.0));
        let mut flags = 0;
        if flag("autoContrast", true) {
            flags += 1;
        }
        if flag("iterCheck", true) {
            flags += 2;
        }
        if !raw_like || flag("usmOwnMask", false) {
            flags += 4;
            if flag("usmAutoContrast", true) {
                flags += 8;
            }
        }
        self.word(flags);
        self.float(num("usmContrast", 10.0).clamp(0.0, 200.0));
    }

    fn points(&mut self, slots: &[[f32; 2]; 16]) {
        for [x, y] in slots {
            self.float(*x);
            self.float(*y);
        }
    }

    fn zeros(&mut self, n: usize) {
        for _ in 0..n {
            self.word(0);
        }
    }
}

/// Whether the white balance ends somewhere other than the reference white.
fn white_balance_moved(edit: &Facts) -> bool {
    let base = White::deserialize(edit.get("whiteBalance")).unwrap_or(REFERENCE_WHITE);
    let t = number(edit.get("temperature"), 0.0);
    let n = number(edit.get("tint"), 0.0);
    let target = if t == 0.0 && n == 0.0 {
        base
    } else {
        let mired = 1_000_000.0 / base.temperature - 1.5 * t;
        White {
            temperature: (1_000_000.0 / mired.max(20.0)).clamp(2000.0, 50000.0),
            tint: (base.tint + 1.5 * n).clamp(-150.0, 150.0),
        }
    };
    target.temperature != REFERENCE_WHITE.temperature || target.tint != REFERENCE_WHITE.tint
}

// ---------------------------------------------------------------------------
// Film look

/// Whether the edit has a provider's film look on.
fn film_look(edit: &Facts) -> bool {
    edit.get("lutPath").as_str() == Some("provider-look")
        && shape::Look::deserialize(edit.get("agProviderLook")).is_ok()
}

/// Shapes a value must read as for the rule to count it. Most fields are only
/// checked by reading them, never used.
///
/// Each shape also reads from an array of its fields in order. There a field
/// with `#[serde(default)]` (one that may be absent) may be missing from the
/// end, while a plain `Option` (may be null, not absent) must be there.
#[allow(dead_code)]
mod shape {
    use std::fmt;

    use serde::Deserialize;
    use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};

    /// A JSON string, checked and not kept.
    pub struct Text;

    impl<'de> Deserialize<'de> for Text {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl Visitor<'_> for V {
                type Value = Text;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a string")
                }
                fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Text, E> {
                    Ok(Text)
                }
            }
            deserializer.deserialize_str(V)
        }
    }

    /// A JSON object whose values are all booleans, checked and not kept.
    #[derive(Default)]
    pub struct Switches;

    impl<'de> Deserialize<'de> for Switches {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct V;
            impl<'de> Visitor<'de> for V {
                type Value = Switches;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("an object of booleans")
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Switches, A::Error> {
                    while map.next_entry::<IgnoredAny, bool>()?.is_some() {}
                    Ok(Switches)
                }
            }
            deserializer.deserialize_map(V)
        }
    }

    /// The crop.
    #[derive(Deserialize)]
    pub struct Rect {
        pub x: f64,
        pub y: f64,
        width: f64,
        height: f64,
    }

    #[derive(Deserialize)]
    struct Point {
        x: f64,
        y: f64,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum Direction {
        Vertical,
        Horizontal,
    }

    /// A guide line of guided perspective.
    #[derive(Deserialize)]
    pub struct GuideLine {
        id: Text,
        #[serde(rename = "type")]
        direction: Direction,
        p1: Point,
        p2: Point,
    }

    /// A camera's built-in lens correction.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct LensTable {
        source: Text,
        #[serde(default)]
        default_on: bool,
        vignetting: Option<LensCurve>,
        distortion: Option<LensCurve>,
        chromatic: Option<[LensCurve; 2]>,
        #[serde(default)]
        always: Option<Always>,
        #[serde(default)]
        centre: Option<[f32; 2]>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Always {
        distortion: bool,
        chromatic: bool,
        vignetting: bool,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LensCurve {
        knots: Vec<f32>,
        values: Vec<f32>,
    }

    impl LensTable {
        /// Whether it corrects anything.
        pub fn has_part(&self) -> bool {
            self.vignetting.is_some() || self.distortion.is_some() || self.chromatic.is_some()
        }

        /// Whether a renderer accepts it.
        pub fn is_valid(&self) -> bool {
            self.centre.is_none_or(|c| c.iter().all(|v| (0.0..=1.0).contains(v)))
                && self
                    .vignetting
                    .iter()
                    .chain(&self.distortion)
                    .chain(self.chromatic.iter().flatten())
                    .all(LensCurve::is_valid)
        }
    }

    impl LensCurve {
        fn is_valid(&self) -> bool {
            let (knots, values) = (&self.knots, &self.values);
            (2..=64).contains(&knots.len())
                && values.len() == knots.len()
                && knots.iter().chain(values).all(|v| v.is_finite())
                && knots[0] >= 0.0
                && knots.windows(2).all(|w| w[0] < w[1])
                && values.iter().all(|v| (0.2..=5.0).contains(v))
        }
    }

    /// A provider's film look (`agProviderLook`).
    #[derive(Deserialize)]
    pub struct Look {
        camera: Text,
        look: Text,
        #[serde(default)]
        options: Switches,
        #[serde(default, rename = "ownWb")]
        own_wb: Option<OwnWhite>,
        #[serde(default)]
        before: Before,
    }

    #[derive(Deserialize)]
    struct OwnWhite {
        option: Text,
        // May be left out of an object, but not of an array (measured).
        wb: Option<WhiteInts>,
    }

    #[derive(Deserialize)]
    struct WhiteInts {
        temperature: i32,
        tint: i32,
    }

    /// The tone rendering the edit had before the look.
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Before {
        #[serde(rename = "rawToneRendering")]
        rendering: Option<Text>,
        #[serde(rename = "rawToneCurve")]
        curve: Option<Vec<Point32>>,
        #[serde(rename = "rawToneCurveMode")]
        curve_mode: Option<Text>,
    }

    #[derive(Deserialize)]
    struct Point32 {
        x: f32,
        y: f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_keys_are_sorted_for_the_binary_search() {
        assert!(READ_KEYS.windows(2).all(|w| w[0] < w[1]));
        assert!(reads("centréX") && reads("whites") && reads("agBlackWhite"));
        assert!(!reads("masks") && !reads("aiPatches") && !reads("version"));
    }

    #[test]
    fn an_empty_edit_renders_as_untouched() {
        let empty = Facts { object: true, ..Facts::default() };
        for is_raw in [false, true] {
            assert_eq!(kind(&empty, is_raw), Kind::None);
        }
    }
}
