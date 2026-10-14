//! Reading a photo: its metadata, and a picture of it for the catalog.
//!
//! RAW: rawler's embedded full-size JPEG first (fast, reads a few MB), then the
//! Windows codecs, then a full RAW develop as the last resort.
//! Everything else: Windows codecs first, the `image` crate as fallback.
//! Shoebox only ever opens photos for reading.

use crate::formats::Kind;
use anyhow::{Result, anyhow};
use chrono::NaiveDateTime;
use image::{DynamicImage, GenericImageView, codecs::jpeg::JpegEncoder, imageops::FilterType};
use rawler::formats::tiff::{GenericTiffReader, IFD, Value as TiffValue, reader::TiffReader};
use rawler::rawsource::RawSource;
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

/// Version of how lenses are read. Photos an older version read without
/// finding their lens are read again ([`crate::gear::find_lenses`]).
pub const LENS_READER: i64 = 1;

#[derive(Default, Debug, Clone)]
pub struct Meta {
    pub taken: Option<NaiveDateTime>,
    pub subsec: Option<String>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub serial: Option<String>,
    pub image_number: Option<u32>,
    pub iso: Option<u32>,
    pub fnumber: Option<f32>,
    pub exposure: Option<String>,
    pub focal: Option<f32>,
    pub orientation: u16,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl Meta {
    /// Fills gaps from a second source without overwriting what is known.
    fn fill_from(&mut self, o: Meta) {
        macro_rules! fill {
            ($($f:ident),*) => { $( if self.$f.is_none() { self.$f = o.$f; } )* };
        }
        fill!(taken, subsec, make, model, lens, serial, image_number, iso, fnumber, exposure, focal, width, height);
        if self.orientation == 0 {
            self.orientation = o.orientation;
        }
    }
}

pub struct Loaded {
    pub meta: Meta,
    /// Already rotated upright.
    pub image: Result<DynamicImage, String>,
}

pub fn load(path: &Path, kind: Kind, long_edge: u32) -> Loaded {
    let mut loaded = match kind {
        Kind::Raw => load_raw(path, long_edge),
        Kind::Image => load_image(path, long_edge),
    };
    loaded.meta.lens = loaded.meta.lens.as_deref().and_then(crate::gear::file_name);
    loaded
}

/// The lens a photo was taken with, from its metadata only - never the
/// picture: a few KB of the file, through a memory map. rawler's lens table
/// (which sets up a whole decoder) only when the file doesn't name it.
pub fn read_lens(path: &Path, kind: Kind) -> Option<String> {
    let lens = match kind {
        Kind::Raw => catch_unwind(AssertUnwindSafe(|| {
            let src = RawSource::new(path).ok()?;
            file_lens(&src).or_else(|| rawler_lens(&src))
        }))
        .ok()
        .flatten(),
        Kind::Image => exif_meta(path).and_then(|m| m.lens),
    };
    lens.as_deref().and_then(crate::gear::file_name)
}

/// A RAW's lens: as the file records it, else rawler's lens table. The
/// table's name is kept when it is the same lens, for its maker's name
/// ("Canon EF 70-200mm f/4L IS USM" for "EF70-200mm f/4L IS USM"); when they
/// disagree the file wins (lenses share ids, so the table can guess wrong).
fn raw_lens(src: &RawSource, table: Option<String>) -> Option<String> {
    match (file_lens(src), table) {
        (Some(f), Some(t)) if crate::gear::same_lens(&f, &t) => Some(t),
        (Some(f), _) => Some(f),
        (None, t) => t,
    }
}

/// rawler's name for the lens, from its lens table (by the lens's name or id).
fn rawler_lens(src: &RawSource) -> Option<String> {
    let decoder = rawler::get_decoder(src).ok()?;
    let md = decoder.raw_metadata(src, &rawler::decoders::RawDecodeParams::default()).ok()?;
    md.lens.as_ref().and_then(|l| non_empty(&format!("{} {}", l.lens_make, l.lens_model)))
}

const MAKE: u16 = 0x010F;
const EXIF_IFD: u16 = 0x8769;
const MAKER_NOTE: u16 = 0x927C;
const LENS_MODEL: u16 = 0xA434;
/// Canon's maker note: the lens's name (cameras since about 2008), and the
/// camera settings (lens id at 22, its focal range at 23-25).
const CANON_LENS_MODEL: u16 = 0x0095;
const CANON_CAMERA_SETTINGS: u16 = 0x0001;
/// The same camera settings in a CRW (CIFF) file.
const CIFF_CAMERA_SETTINGS: u16 = 0x102D;

/// The lens as a RAW file records it, read where each format keeps it: EXIF
/// LensModel in TIFF-based RAWs (CR2, NEF, ARW, ORF, PEF, DNG...), in a CR3's
/// boxes, in the JPEG inside a Fuji RAF; else Canon's maker note (the lens's
/// name, or its id), or a CRW's lens id.
fn file_lens(src: &RawSource) -> Option<String> {
    let buf = src.buf();
    let head = buf.get(..16)?;
    if &head[4..8] == b"ftyp" {
        let bmff = rawler::formats::bmff::Bmff::new(src.reader()).ok()?;
        let cr3 = bmff.filebox.moov.cr3desc.as_ref()?;
        return ascii(cr3.cmt2.tiff.root_ifd(), LENS_MODEL).or_else(|| canon_note_lens(cr3.cmt3.tiff.root_ifd()));
    }
    if &head[6..14] == b"HEAPCCDR" {
        return ciff_lens(buf);
    }
    if head.starts_with(b"FUJIFILM") {
        // The camera's JPEG: offset and length at 84, big-endian.
        let at = |i: usize| buf.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
        let (off, len) = (at(84)?, at(88)?);
        let jpeg = buf.get(off..off.checked_add(len)?)?;
        let ex = exif::Reader::new().read_from_container(&mut Cursor::new(jpeg)).ok()?;
        return exif_lens(&ex);
    }
    if head.starts_with(b"II") || head.starts_with(b"MM") {
        let mut r = src.reader();
        let tiff = GenericTiffReader::new(&mut r, 0, 0, None, &[]).ok()?;
        return tiff_lens(&tiff, &mut r);
    }
    None
}

/// The lens in a parsed EXIF block (a JPEG's, HEIC's...): LensModel, else
/// Canon's maker note.
fn exif_lens(ex: &exif::Exif) -> Option<String> {
    let buf = ex.buf();
    let tiff = GenericTiffReader::new_with_buffer(buf, 0, 0, None).ok()?;
    tiff_lens(&tiff, &mut Cursor::new(buf))
}

/// EXIF LensModel, else - for a Canon - its maker note. Other makers' maker
/// notes are their own formats; rawler's lens table covers those.
fn tiff_lens<R: Read + Seek>(tiff: &GenericTiffReader, r: &mut R) -> Option<String> {
    let root = tiff.root_ifd();
    let exif = root.get_sub_ifd(EXIF_IFD);
    if let Some(name) = exif.and_then(|e| ascii(e, LENS_MODEL)).or_else(|| ascii(root, LENS_MODEL)) {
        return Some(name);
    }
    if !ascii(root, MAKE)?.to_ascii_lowercase().starts_with("canon") {
        return None;
    }
    let exif = exif?;
    let at = exif.get_entry(MAKER_NOTE)?.offset()? as u32;
    let note = IFD::new(r, at, 0, 0, exif.endian, &[]).ok()?;
    canon_note_lens(&note)
}

/// Canon's maker note: the lens's name, else its id.
fn canon_note_lens(note: &IFD) -> Option<String> {
    ascii(note, CANON_LENS_MODEL).or_else(|| match &note.get_entry(CANON_CAMERA_SETTINGS)?.value {
        TiffValue::Short(settings) => canon_lens_by_id(settings),
        _ => None,
    })
}

fn ascii(ifd: &IFD, tag: u16) -> Option<String> {
    match &ifd.get_entry(tag)?.value {
        TiffValue::Ascii(a) => a.strings().first().and_then(|s| non_empty(s)),
        _ => None,
    }
}

/// Canon's lens id (camera settings 22) in rawler's lens table. Lenses share
/// ids (174 is the EF 135mm f/2L and eight Sigma zooms), so the lens's focal
/// range (23-25) must leave exactly one.
fn canon_lens_by_id(s: &[u16]) -> Option<String> {
    let (id, long, short) = (*s.get(22)? as u32, *s.get(23)? as f32, *s.get(24)? as f32);
    if id == 0 || id == 0xFFFF {
        return None;
    }
    let units = s.get(25).copied().filter(|&u| u > 0).unwrap_or(1) as f32;
    let (short, long) = (short / units, long / units);
    let mm = |r: &rawler::formats::tiff::Rational| if r.d == 0 { -1.0 } else { r.n as f32 / r.d as f32 };
    let fits = |l: &rawler::lens::LensDescription| {
        long <= 0.0 || ((mm(&l.focal_range[0]) - short).abs() < 0.6 && (mm(&l.focal_range[1]) - long).abs() < 0.6)
    };
    let names: std::collections::BTreeSet<String> = rawler::lens::get_lenses()
        .iter()
        .filter(|l| matches!(l.mount.as_str(), "ef-mount" | "rf-mount" | "cn-mount"))
        .filter(|l| l.identifiers.id.is_some_and(|(i, _)| i == id))
        .filter(|l| fits(l))
        .map(|l| format!("{} {}", l.lens_make, l.lens_model))
        .collect();
    if names.len() == 1 { names.into_iter().next() } else { None }
}

/// A CRW's lens: it records no name, only the lens id, in the camera settings
/// somewhere in its heap of directories.
fn ciff_lens(buf: &[u8]) -> Option<String> {
    let start = le32(buf, 2)? as usize;
    let data = ciff_find(buf, start, buf.len(), CIFF_CAMERA_SETTINGS, 0)?;
    let settings: Vec<u16> = data.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    canon_lens_by_id(&settings)
}

/// A CIFF heap runs from `start` to `end`; its last 4 bytes give where its
/// directory is: a count, then 10-byte entries (tag, size, offset). Tags with
/// type 0x2800 or 0x3000 are heaps themselves.
fn ciff_find(buf: &[u8], start: usize, end: usize, want: u16, depth: u8) -> Option<&[u8]> {
    if depth > 4 || end > buf.len() || end < start + 4 {
        return None;
    }
    let dir = start.checked_add(le32(buf, end - 4)? as usize)?;
    let count = le16(buf, dir)? as usize;
    for i in 0..count.min(1024) {
        let e = dir + 2 + i * 10;
        let tag = le16(buf, e)?;
        // Values kept in the entry itself (0x4000) are never a heap or the settings.
        if tag & 0xC000 != 0 {
            continue;
        }
        let (size, off) = (le32(buf, e + 2)? as usize, start.checked_add(le32(buf, e + 6)? as usize)?);
        let Some(data) = off.checked_add(size).and_then(|to| buf.get(off..to)) else { continue };
        if tag & 0x3FFF == want {
            return Some(data);
        }
        if matches!(tag & 0x3800, 0x2800 | 0x3000)
            && let Some(found) = ciff_find(buf, off, off + size, want, depth + 1)
        {
            return Some(found);
        }
    }
    None
}

fn le16(b: &[u8], i: usize) -> Option<u16> {
    b.get(i..i + 2).map(|x| u16::from_le_bytes([x[0], x[1]]))
}

fn le32(b: &[u8], i: usize) -> Option<u32> {
    b.get(i..i + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

fn load_raw(path: &Path, long_edge: u32) -> Loaded {
    let mut errors = Vec::new();
    let mut meta = match catch_unwind(AssertUnwindSafe(|| rawler_meta(path))) {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => {
            errors.push(format!("rawler: {e}"));
            Meta::default()
        }
        Err(_) => {
            errors.push("rawler: crashed reading this file".into());
            Meta::default()
        }
    };
    if (meta.taken.is_none() || meta.model.is_none())
        && let Some(m) = exif_meta(path)
    {
        meta.fill_from(m);
    }

    // An embedded preview smaller than ~3/4 of the target is a last resort only.
    let good_enough = (long_edge.min(1600) as f32 * 0.75) as u32;
    let mut image = None;
    let mut embedded = None;

    // 1. The camera's own JPEG, read on its own and decoded at the size needed.
    match catch_unwind(AssertUnwindSafe(|| crate::embedded::preview(path, long_edge))) {
        Ok(Some(found)) => {
            meta.width = meta.width.or(Some(found.full_w));
            meta.height = meta.height.or(Some(found.full_h));
            if long_of(&found.image) >= good_enough {
                image = Some(orient(found.image, meta.orientation));
            } else {
                embedded = Some(found.image);
            }
        }
        Ok(None) => {}
        Err(_) => errors.push("embedded preview: crashed".into()),
    }

    // 2. rawler's own preview extraction, for containers the reader above doesn't know.
    if image.is_none() {
        match catch_unwind(AssertUnwindSafe(|| rawler_image(path))) {
            Ok(Some(img)) => {
                meta.width = meta.width.or(Some(img.width()));
                meta.height = meta.height.or(Some(img.height()));
                if long_of(&img) >= good_enough {
                    image = Some(orient(img, meta.orientation));
                } else if embedded.as_ref().is_none_or(|e| long_of(e) < long_of(&img)) {
                    embedded = Some(img);
                }
            }
            Ok(None) => {}
            Err(_) => errors.push("rawler preview: crashed".into()),
        }
    }

    #[cfg(windows)]
    if image.is_none() {
        match crate::wic::decode(path, long_edge, true) {
            // The Windows RAW codec returns pixels already upright.
            Ok(d) => {
                meta.width = meta.width.or(Some(d.full_w));
                meta.height = meta.height.or(Some(d.full_h));
                image = Some(d.image);
            }
            Err(e) => errors.push(format!("windows codec: {e}")),
        }
    }

    if image.is_none()
        && let Some(img) = embedded
    {
        image = Some(orient(img, meta.orientation));
    }

    if image.is_none() {
        let params = rawler::decoders::RawDecodeParams::default();
        match catch_unwind(AssertUnwindSafe(|| rawler::analyze::raw_to_srgb(path, &params))) {
            Ok(Ok(img)) => image = Some(orient(img, meta.orientation)),
            Ok(Err(e)) => errors.push(format!("raw develop: {e}")),
            Err(_) => errors.push("raw develop: crashed".into()),
        }
    }

    Loaded { meta, image: image.ok_or_else(|| errors.join("; ")) }
}

/// The largest embedded image rawler can hand over (decoded at full size, so slow).
fn rawler_image(path: &Path) -> Option<DynamicImage> {
    let src = rawler::rawsource::RawSource::new(path).ok()?;
    let decoder = rawler::get_decoder(&src).ok()?;
    let params = rawler::decoders::RawDecodeParams::default();
    decoder.full_image(&src, &params).ok().flatten().or_else(|| decoder.preview_image(&src, &params).ok().flatten())
}

/// Camera, lens, exposure and capture time from the RAW's own metadata.
fn rawler_meta(path: &Path) -> Result<Meta> {
    use rawler::decoders::RawDecodeParams;
    let src = rawler::rawsource::RawSource::new(path)?;
    let decoder = rawler::get_decoder(&src)?;
    let params = RawDecodeParams::default();

    let mut meta = Meta::default();
    let mut table_lens = None;
    if let Ok(md) = decoder.raw_metadata(&src, &params) {
        let ex = &md.exif;
        meta.make = non_empty(&md.make);
        meta.model = non_empty(&md.model);
        table_lens = md.lens.as_ref().and_then(|l| non_empty(&format!("{} {}", l.lens_make, l.lens_model)));
        meta.taken = ex.date_time_original.as_deref().or(ex.create_date.as_deref()).and_then(parse_exif_date);
        meta.subsec = ex.sub_sec_time_original.as_deref().and_then(non_empty);
        meta.serial = ex.serial_number.as_deref().and_then(non_empty);
        meta.image_number = ex.image_number;
        meta.iso =
            ex.iso_speed_ratings.map(u32::from).or(ex.iso_speed).or(ex.recommended_exposure_index).filter(|&v| v > 0);
        meta.fnumber = ex.fnumber.as_ref().and_then(|r| ratio(r.n, r.d));
        meta.exposure = ex.exposure_time.as_ref().and_then(|r| shutter(r.n, r.d));
        meta.focal = ex.focal_length.as_ref().and_then(|r| ratio(r.n, r.d));
        meta.orientation = ex.orientation.unwrap_or(0);
    }
    meta.lens = raw_lens(&src, table_lens);
    Ok(meta)
}

// Only the Windows codecs decode at a chosen size.
#[cfg_attr(not(windows), allow(unused_variables))]
fn load_image(path: &Path, long_edge: u32) -> Loaded {
    let meta = exif_meta(path).unwrap_or_default();
    let ext = crate::formats::extension(&path.to_string_lossy());
    // HEIF/AVIF decoders apply the container's own rotation; EXIF orientation
    // there is informational and applying it again would double-rotate.
    let exif_rotates = !matches!(ext.as_str(), "heic" | "heif" | "hif" | "avif");
    let mut meta = meta;
    let mut errors = Vec::new();

    #[cfg(windows)]
    match crate::wic::decode(path, long_edge, false) {
        Ok(d) => {
            meta.width = meta.width.or(Some(d.full_w));
            meta.height = meta.height.or(Some(d.full_h));
            let img = if exif_rotates { orient(d.image, meta.orientation) } else { d.image };
            return Loaded { meta, image: Ok(img) };
        }
        Err(e) => errors.push(format!("windows codec: {e}")),
    }

    match catch_unwind(AssertUnwindSafe(|| image::open(path))) {
        Ok(Ok(img)) => {
            meta.width = meta.width.or(Some(img.width()));
            meta.height = meta.height.or(Some(img.height()));
            let img = if exif_rotates { orient(img, meta.orientation) } else { img };
            Loaded { meta, image: Ok(img) }
        }
        Ok(Err(e)) => {
            errors.push(format!("image: {e}"));
            Loaded { meta, image: Err(errors.join("; ")) }
        }
        Err(_) => {
            errors.push("image: crashed".into());
            Loaded { meta, image: Err(errors.join("; ")) }
        }
    }
}

/// EXIF via kamadak-exif: JPEG, TIFF-based RAW, HEIF, PNG, WebP.
fn exif_meta(path: &Path) -> Option<Meta> {
    use exif::{In, Tag, Value};
    let mut reader = BufReader::new(File::open(path).ok()?);
    let ex = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = |t: Tag| ex.get_field(t, In::PRIMARY);
    let ascii = |t: Tag| -> Option<String> {
        match &field(t)?.value {
            Value::Ascii(v) => v.first().and_then(|b| non_empty(&String::from_utf8_lossy(b))),
            _ => None,
        }
    };
    let rational = |t: Tag| -> Option<(u32, u32)> {
        match &field(t)?.value {
            Value::Rational(v) => v.first().map(|r| (r.num, r.denom)),
            _ => None,
        }
    };
    let uint = |t: Tag| field(t).and_then(|f| f.value.get_uint(0));

    Some(Meta {
        taken: ascii(Tag::DateTimeOriginal)
            .or_else(|| ascii(Tag::DateTimeDigitized))
            .or_else(|| ascii(Tag::DateTime))
            .as_deref()
            .and_then(parse_exif_date),
        subsec: ascii(Tag::SubSecTimeOriginal),
        make: ascii(Tag::Make),
        model: ascii(Tag::Model),
        lens: ascii(Tag::LensModel).or_else(|| exif_lens(&ex)),
        serial: ascii(Tag::BodySerialNumber),
        image_number: None,
        iso: uint(Tag::PhotographicSensitivity).filter(|&v| v > 0),
        fnumber: rational(Tag::FNumber).and_then(|(n, d)| ratio(n, d)),
        exposure: rational(Tag::ExposureTime).and_then(|(n, d)| shutter(n, d)),
        focal: rational(Tag::FocalLength).and_then(|(n, d)| ratio(n, d)),
        orientation: uint(Tag::Orientation).unwrap_or(0) as u16,
        width: uint(Tag::PixelXDimension),
        height: uint(Tag::PixelYDimension),
    })
}

fn parse_exif_date(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim().trim_end_matches('\0');
    NaiveDateTime::parse_from_str(s.get(..19)?, "%Y:%m:%d %H:%M:%S").ok().filter(|d| d.and_utc().timestamp() > 0)
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim().trim_end_matches('\0').trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn ratio(n: u32, d: u32) -> Option<f32> {
    (d != 0 && n != 0).then(|| n as f32 / d as f32)
}

fn shutter(n: u32, d: u32) -> Option<String> {
    if n == 0 || d == 0 {
        return None;
    }
    let secs = n as f64 / d as f64;
    Some(if secs >= 0.3 { format!("{}s", (secs * 10.0).round() / 10.0) } else { format!("1/{}", (1.0 / secs).round()) })
}

fn long_of(img: &DynamicImage) -> u32 {
    img.width().max(img.height())
}

/// EXIF orientation 1-8 to upright pixels.
pub fn orient(img: DynamicImage, o: u16) -> DynamicImage {
    match o {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

pub struct Rendered {
    pub preview: Vec<u8>,
    pub thumb: Vec<u8>,
    pub thumb_w: u32,
    pub thumb_h: u32,
}

/// The two JPEGs Shoebox keeps: a preview (long edge `preview_long`) and a grid
/// thumbnail (height `thumb_h`). Never upscales.
pub fn render(img: &DynamicImage, preview_long: u32, quality: u8, thumb_h: u32) -> Result<Rendered> {
    let preview = shrink(img, preview_long);
    let (pw, ph) = preview.dimensions();
    let th = thumb_h.min(ph);
    let mut tw = ((pw as f64 * th as f64 / ph as f64).round() as u32).max(1);
    let mut th = th.max(1);
    // Very wide panoramas: cap the width, which lowers the height.
    let max_w = thumb_h * 4;
    if tw > max_w {
        th = ((th as f64 * max_w as f64 / tw as f64).round() as u32).max(1);
        tw = max_w;
    }
    let thumb = resize(&preview, tw, th);
    Ok(Rendered { preview: encode(&preview, quality)?, thumb: encode(&thumb, 80)?, thumb_w: tw, thumb_h: th })
}

fn shrink(img: &DynamicImage, long: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if w.max(h) <= long {
        return DynamicImage::ImageRgb8(img.to_rgb8());
    }
    let s = long as f64 / w.max(h) as f64;
    let nw = ((w as f64 * s).round() as u32).max(1);
    let nh = ((h as f64 * s).round() as u32).max(1);
    resize(img, nw, nh)
}

fn resize(img: &DynamicImage, nw: u32, nh: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    if (w, h) == (nw, nh) {
        return DynamicImage::ImageRgb8(img.to_rgb8());
    }
    // A fast box pre-shrink to twice the size, then a sharp filter for the last step.
    if w > nw * 3 {
        let half = img.thumbnail_exact(nw * 2, nh * 2);
        DynamicImage::ImageRgb8(half.resize_exact(nw, nh, FilterType::CatmullRom).to_rgb8())
    } else {
        DynamicImage::ImageRgb8(img.resize_exact(nw, nh, FilterType::CatmullRom).to_rgb8())
    }
}

pub fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    encode(img, quality)
}

/// The viewer's sharp picture, read from the original (read-only) when its
/// disk is plugged in: long edge `long`, or full size for 0. Never upscaled,
/// never stored - previews stay the only pictures Shoebox keeps.
pub fn sharp(path: &Path, kind: Kind, long: u32) -> Result<Vec<u8>> {
    // Up to 15% short of the asked size is fine on screen, and lets a JPEG be
    // decoded at a cheaper scale (1/2, 1/4) instead of full size then shrunk.
    let loaded = load(path, kind, if long == 0 { u32::MAX } else { long * 85 / 100 });
    let img = loaded.image.map_err(|e| anyhow!(e))?;
    let img = if long == 0 { DynamicImage::ImageRgb8(img.to_rgb8()) } else { shrink(&img, long) };
    encode(&img, 90)
}

fn encode(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let rgb = img.as_rgb8().ok_or_else(|| anyhow!("expected RGB8"))?;
    let mut out = Vec::with_capacity(rgb.len() / 8);
    JpegEncoder::new_with_quality(&mut out, quality).encode_image(rgb)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canon_lens_ids_need_the_focal_range() {
        let mut s = [0u16; 26];
        (s[22], s[23], s[24], s[25]) = (174, 135, 135, 1);
        assert_eq!(canon_lens_by_id(&s).as_deref(), Some("Canon EF 135mm f/2L USM"));
        // Without its range, id 174 is ten lenses: none is guessed.
        (s[23], s[24]) = (0, 0);
        assert_eq!(canon_lens_by_id(&s), None);
        s[22] = 0xFFFF;
        assert_eq!(canon_lens_by_id(&s), None);
    }

    #[test]
    fn a_crw_lens_is_found_in_its_heap() {
        // Header ("II", header length, "HEAPCCDR"), then a heap holding a
        // heap (0x300A) holding the camera settings.
        let mut settings = [0u16; 26];
        (settings[22], settings[23], settings[24], settings[25]) = (174, 135, 135, 1);
        let settings: Vec<u8> = settings.iter().flat_map(|v| v.to_le_bytes()).collect();
        let dir = |tag: u16, size: usize| {
            let mut d = 1u16.to_le_bytes().to_vec();
            d.extend(tag.to_le_bytes());
            d.extend((size as u32).to_le_bytes());
            d.extend(0u32.to_le_bytes());
            d
        };
        let mut inner = settings.clone();
        inner.extend(dir(0x102D, settings.len()));
        inner.extend((settings.len() as u32).to_le_bytes());
        let mut root = inner.clone();
        root.extend(dir(0x300A, inner.len()));
        root.extend((inner.len() as u32).to_le_bytes());
        let mut file = b"II".to_vec();
        file.extend(26u32.to_le_bytes());
        file.extend(b"HEAPCCDR");
        file.resize(26, 0);
        file.extend(root);
        assert_eq!(ciff_lens(&file).as_deref(), Some("Canon EF 135mm f/2L USM"));
        // A truncated file finds nothing, and doesn't panic.
        assert_eq!(ciff_lens(&file[..60]), None);
    }
}
