//! Reading a photo: its metadata, and a picture of it for the catalog.
//!
//! RAW: rawler's embedded full-size JPEG first (fast, reads a few MB), then the
//! Windows codecs, then a full RAW develop as the last resort.
//! Everything else: Windows codecs first, the `image` crate as fallback.
//! A Photoshop file, whatever its name, is read by [`crate::psd`].
//! Shoebox only ever opens photos for reading.

use crate::formats::Kind;
use anyhow::{Result, anyhow};
use chrono::NaiveDateTime;
use image::{DynamicImage, GenericImageView, codecs::jpeg::JpegEncoder, imageops::FilterType};
use rawler::bits::Endian;
use rawler::formats::tiff::{GenericTiffReader, IFD, Value as TiffValue, reader::TiffReader};
use rawler::rawsource::RawSource;
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

/// Version of how lenses are read. Photos an older version read without
/// finding their lens are read again ([`crate::gear::find_lenses`]).
/// 2: a lens nothing names goes by what it is (focal range and aperture),
/// from the standard tags or its maker's note, for every maker.
pub const LENS_READER: i64 = 2;

/// Version of what Shoebox can read (formats, readers). A photo it gave up on,
/// still unreadable when retried at a plug-in, is retried by itself again
/// only by a newer version: raise it when Shoebox learns to read more. The
/// folders listed by an older one are listed again once, for the files it
/// left out.
/// 2: Photoshop files (.psd, .psb, or any name).
pub const PICTURE_READER: i64 = 2;

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
    // A Photoshop file is told by its content, whatever its name says.
    let mut loaded = if crate::psd::is_psd(path) {
        crate::psd::load(path, long_edge)
    } else {
        match kind {
            Kind::Raw => load_raw(path, long_edge),
            Kind::Image => load_image(path, long_edge),
        }
    };
    loaded.meta.lens = loaded.meta.lens.as_deref().and_then(crate::gear::file_name);
    loaded
}

/// The lens a photo was taken with, from its metadata only - never the
/// picture: a few KB of the file, through a memory map. rawler's lens table
/// (which sets up a whole decoder) only when the file names no lens.
pub fn read_lens(path: &Path, kind: Kind) -> Option<String> {
    if crate::psd::is_psd(path) {
        return crate::psd::meta(path)?.lens.as_deref().and_then(crate::gear::file_name);
    }
    let lens = match kind {
        Kind::Raw => catch_unwind(AssertUnwindSafe(|| {
            let src = RawSource::new(path).ok()?;
            let clues = file_clues(&src);
            // rawler's CRW reader reads the whole file and never names a lens.
            let crw = src.buf().get(6..14) == Some(b"HEAPCCDR");
            if clues.name.is_some() || crw { clues.name.or(clues.spec) } else { rawler_lens(&src).or(clues.spec) }
        }))
        .ok()
        .flatten(),
        Kind::Image => exif_meta(path).and_then(|m| m.lens),
    };
    lens.as_deref().and_then(crate::gear::file_name)
}

/// A RAW's lens: its name as the file records it, else rawler's lens table,
/// else what the file says the lens is. The table's name is kept when it is
/// the same lens, for its maker's name ("Canon EF 70-200mm f/4L IS USM" for
/// "EF70-200mm f/4L IS USM"); when they disagree the file wins (lenses share
/// ids, so the table can guess wrong).
fn raw_lens(src: &RawSource, table: Option<String>) -> Option<String> {
    let clues = file_clues(src);
    match (clues.name, table) {
        (Some(f), Some(t)) if crate::gear::same_lens(&f, &t) => Some(t),
        (Some(f), _) => Some(f),
        (None, t) => t.or(clues.spec),
    }
}

/// A lens of rawler's table by its maker and model; some models already
/// start with the maker ("Sigma" + "Sigma 35mm F1.4 DG HSM").
fn table_name(l: &rawler::lens::LensDescription) -> Option<String> {
    let starts = l.lens_model.to_lowercase().starts_with(&l.lens_make.to_lowercase());
    non_empty(&if starts { l.lens_model.clone() } else { format!("{} {}", l.lens_make, l.lens_model) })
}

/// rawler's name for the lens, from its lens table (by the lens's name or id).
fn rawler_lens(src: &RawSource) -> Option<String> {
    let decoder = rawler::get_decoder(src).ok()?;
    let md = decoder.raw_metadata(src, &rawler::decoders::RawDecodeParams::default()).ok()?;
    md.lens.as_ref().and_then(table_name)
}

const MAKE: u16 = 0x010F;
const EXIF_IFD: u16 = 0x8769;
const MAKER_NOTE: u16 = 0x927C;
/// Standard EXIF (2.3): the lens's name, and what it is - min and max focal
/// length, widest aperture at each (any maker may write it).
const LENS_MODEL: u16 = 0xA434;
const LENS_SPECIFICATION: u16 = 0xA432;
/// DNG's own copy of LensSpecification, and where a DNG keeps the camera's
/// original maker note.
const DNG_LENS_INFO: u16 = 0xC630;
const DNG_PRIVATE_DATA: u16 = 0xC634;
/// Canon's maker note: the lens's name (cameras since about 2008), and the
/// camera settings (lens id at 22, focal range at 23-25, aperture at 26).
const CANON_LENS_MODEL: u16 = 0x0095;
const CANON_CAMERA_SETTINGS: u16 = 0x0001;
/// The same camera settings in a CRW (CIFF) file.
const CIFF_CAMERA_SETTINGS: u16 = 0x102D;
/// Nikon's maker note: what the lens is, as LensSpecification.
const NIKON_LENS: u16 = 0x0084;
/// Pentax's maker note: the lens's id (series, number).
const PENTAX_LENS_REC: u16 = 0x003F;

/// What a file says about its lens: its name, and what it is ("24-70mm
/// f/2.8", from its focal range and aperture) for when nothing names it.
#[derive(Default)]
struct Clues {
    name: Option<String>,
    spec: Option<String>,
}

/// The lens as a RAW file records it, read where each format keeps it: the
/// TIFF-based RAWs (CR2, NEF, ARW, ORF, PEF, RW2, DNG...), a CR3's boxes, the
/// JPEG inside a Fuji RAF, a CRW's heap.
fn file_clues(src: &RawSource) -> Clues {
    let buf = src.buf();
    let Some(head) = buf.get(..16) else { return Clues::default() };
    if &head[4..8] == b"ftyp" {
        let Ok(bmff) = rawler::formats::bmff::Bmff::new(src.reader()) else { return Clues::default() };
        let Some(cr3) = bmff.filebox.moov.cr3desc.as_ref() else { return Clues::default() };
        let ifds = [cr3.cmt2.tiff.root_ifd(), cr3.cmt1.tiff.root_ifd()];
        return clues("canon", &ifds, Some(cr3.cmt3.tiff.root_ifd()));
    }
    if &head[6..14] == b"HEAPCCDR" {
        return ciff_clues(buf);
    }
    if head.starts_with(b"FUJIFILM") {
        // The camera's JPEG: offset and length at 84, big-endian. Its EXIF is
        // at its start, so a file cut short still has it.
        let at = |i: usize| buf.get(i..i + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
        let jpeg = at(84).zip(at(88)).and_then(|(off, len)| buf.get(off..off.saturating_add(len).min(buf.len())));
        return jpeg
            .and_then(|j| exif::Reader::new().read_from_container(&mut Cursor::new(j)).ok())
            .map(|ex| exif_clues(&ex))
            .unwrap_or_default();
    }
    if head.starts_with(b"II") || head.starts_with(b"MM") {
        let mut r = src.reader();
        if let Ok(tiff) = GenericTiffReader::new(&mut r, 0, 0, Some(0), &[]) {
            return tiff_clues(&tiff, &mut r);
        }
    }
    Clues::default()
}

/// The lens in a parsed EXIF block (a JPEG's, a HEIC's...).
fn exif_lens(ex: &exif::Exif) -> Option<String> {
    let c = exif_clues(ex);
    c.name.or(c.spec)
}

fn exif_clues(ex: &exif::Exif) -> Clues {
    let buf = ex.buf();
    match GenericTiffReader::new_with_buffer(buf, 0, 0, Some(0)) {
        Ok(tiff) => tiff_clues(&tiff, &mut Cursor::new(buf)),
        Err(_) => Clues::default(),
    }
}

fn tiff_clues<R: Read + Seek>(tiff: &GenericTiffReader, r: &mut R) -> Clues {
    let root = tiff.root_ifd();
    let exif = root.get_sub_ifd(EXIF_IFD);
    let make = ascii(root, MAKE).unwrap_or_default().to_ascii_lowercase();
    let note = exif.and_then(|e| maker_note(e, r)).or_else(|| dng_maker_note(root, r));
    let ifds: Vec<&IFD> = exif.into_iter().chain(std::iter::once(root)).collect();
    clues(&make, &ifds, note.as_ref())
}

/// The lens from a file's EXIF / TIFF directories and its maker's note: by
/// name first (EXIF LensModel, the maker note's name or id), then by what it
/// is (EXIF LensSpecification, DNG LensInfo, the maker note's focal range).
fn clues(make: &str, ifds: &[&IFD], note: Option<&IFD>) -> Clues {
    let from_note = note.map(|n| note_clues(make, n)).unwrap_or_default();
    Clues {
        name: ifds.iter().find_map(|i| ascii(i, LENS_MODEL)).or(from_note.name),
        spec: ifds
            .iter()
            .find_map(|i| spec_of_tag(i, LENS_SPECIFICATION).or_else(|| spec_of_tag(i, DNG_LENS_INFO)))
            .or(from_note.spec),
    }
}

/// What a maker's note says about the lens, for the makers whose notes are
/// known; the others write the standard tags, and rawler's table reads their ids.
fn note_clues(make: &str, note: &IFD) -> Clues {
    if make.starts_with("canon") {
        let mut c = match &note.get_entry(CANON_CAMERA_SETTINGS).map(|e| &e.value) {
            Some(TiffValue::Short(settings)) => canon_settings(settings),
            _ => Clues::default(),
        };
        c.name = ascii(note, CANON_LENS_MODEL).or(c.name);
        c
    } else if make.starts_with("nikon") {
        Clues { name: None, spec: spec_of_tag(note, NIKON_LENS) }
    } else if make.starts_with("pentax") || make.starts_with("ricoh") {
        let name = match note.get_entry(PENTAX_LENS_REC).map(|e| &e.value) {
            Some(TiffValue::Byte(id)) if id.len() >= 2 => pentax_lens_by_id(id[0] as u32, id[1] as u32),
            _ => None,
        };
        Clues { name, spec: None }
    } else {
        Clues::default()
    }
}

/// A maker note's directory, wherever its maker puts it: Nikon's has its own
/// TIFF header (offsets from there), Pentax's a name and a byte order, Canon's
/// is a plain directory. `corr` moves offsets written for another file (a
/// maker note copied into a DNG).
fn note_ifd<R: Read + Seek>(r: &mut R, at: u32, data: &[u8], endian: Endian, corr: i32) -> Option<IFD> {
    let order = |b: &[u8]| match b {
        b"II" => Some(Endian::Little),
        b"MM" => Some(Endian::Big),
        _ => None,
    };
    if data.starts_with(b"Nikon\0") {
        return IFD::new(r, 8, at + 10, 0, order(data.get(10..12)?)?, &[]).ok();
    }
    if data.starts_with(b"AOC\0") {
        return IFD::new(r, at + 6, 0, corr, order(data.get(4..6)?).unwrap_or(endian), &[]).ok();
    }
    if data.starts_with(b"PENTAX ") {
        // Offsets from the note's own start.
        return IFD::new(r, at + 10, 0, at as i32, order(data.get(8..10)?)?, &[]).ok();
    }
    IFD::new(r, at, 0, corr, endian, &[]).ok()
}

fn maker_note<R: Read + Seek>(exif: &IFD, r: &mut R) -> Option<IFD> {
    let entry = exif.get_entry(MAKER_NOTE)?;
    let TiffValue::Undefined(data) = &entry.value else { return None };
    note_ifd(r, entry.offset()? as u32, data, exif.endian, 0)
}

/// The camera's maker note a DNG kept: "Adobe\0", "MakN", its length, its
/// byte order, its offset in the original file, then the note.
fn dng_maker_note<R: Read + Seek>(root: &IFD, r: &mut R) -> Option<IFD> {
    let entry = root.get_entry(DNG_PRIVATE_DATA)?;
    let data = match &entry.value {
        TiffValue::Byte(d) | TiffValue::Undefined(d) => d,
        _ => return None,
    };
    // A Pentax's own DNG keeps its note there as it is.
    if data.starts_with(b"PENTAX ") {
        return note_ifd(r, entry.offset()? as u32, data, root.endian, 0);
    }
    if !data.starts_with(b"Adobe\0MakN") {
        return None;
    }
    let endian = match data.get(14..16)? {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return None,
    };
    let original = u32::from_be_bytes(data.get(16..20)?.try_into().ok()?);
    let at = entry.offset()? as u32 + 20;
    note_ifd(r, at, data.get(20..)?, endian, at as i32 - original as i32)
}

fn ascii(ifd: &IFD, tag: u16) -> Option<String> {
    match &ifd.get_entry(tag)?.value {
        TiffValue::Ascii(a) => a.strings().first().and_then(|s| non_empty(s)),
        _ => None,
    }
}

/// A LensSpecification-shaped tag (min and max focal length, widest
/// aperture at each) as a lens named by what it is: "18-55mm f/3.5-5.6".
/// All zeros (a lens that reports nothing) says nothing.
fn spec_of_tag(ifd: &IFD, tag: u16) -> Option<String> {
    let TiffValue::Rational(v) = &ifd.get_entry(tag)?.value else { return None };
    let f = |i: usize| v.get(i).filter(|r| r.d != 0).map(|r| r.n as f32 / r.d as f32).unwrap_or(0.0);
    let aperture = (f(2) > 0.0).then(|| (f(2), f(3).max(f(2))));
    crate::gear::spec_name(f(0), f(1), aperture)
}

/// The lens in Canon's camera settings: its name from rawler's lens table
/// when its id (22) leaves exactly one there - lenses share ids (174 is the
/// EF 135mm f/2L and eight Sigma zooms), so the focal range (23-25) must
/// agree; and what it is - older bodies like the 10D record no id (65535) -
/// from its focal range and widest aperture (26): "24-70mm f/2.8".
fn canon_settings(s: &[u16]) -> Clues {
    let (Some(&id), Some(&long), Some(&short)) = (s.get(22), s.get(23), s.get(24)) else { return Clues::default() };
    let units = s.get(25).copied().filter(|&u| u > 0).unwrap_or(1) as f32;
    let (short, long) = (short as f32 / units, long as f32 / units);
    let aperture =
        s.get(26).and_then(|&code| canon_aperture(code as i16)).filter(|&a| a_lens_has(short, long, a)).map(|a| (a, a));
    Clues { name: canon_lens_by_id(id as u32, short, long), spec: crate::gear::spec_name(short, long, aperture) }
}

/// Some lens of rawler's table - every maker's - has this focal range and
/// this widest aperture (at either end). A lens that reports nonsense to the
/// camera fails it (a Sigma 12-24 on a 30D says f/1.8): its aperture is dropped.
fn a_lens_has(short: f32, long: f32, aperture: f32) -> bool {
    let v = |r: &rawler::formats::tiff::Rational| if r.d == 0 { -1.0 } else { r.n as f32 / r.d as f32 };
    let close = |a: f32, b: f32| (a / b).ln().abs() < 0.06;
    rawler::lens::get_lenses().iter().any(|l| {
        (v(&l.focal_range[0]) - short).abs() < 0.6
            && (v(&l.focal_range[1]) - long).abs() < 0.6
            && (close(v(&l.aperture_range[0]), aperture) || close(v(&l.aperture_range[1]), aperture))
    })
}

/// Canon's aperture code: an EV in 1/32 steps, thirds written 0x0C / 0x14.
fn canon_aperture(code: i16) -> Option<f32> {
    if code <= 0 {
        return None;
    }
    let frac = code & 0x1F;
    let thirds = match frac {
        0x0C => 32.0 / 3.0,
        0x14 => 64.0 / 3.0,
        f => f as f32,
    };
    let ev = ((code - frac) as f32 + thirds) / 32.0;
    Some(2f32.powf(ev / 2.0))
}

/// The one lens of these mounts with this id in rawler's lens table, if only one.
fn lens_by_id(mounts: &[&str], fits: impl Fn(&rawler::lens::LensDescription) -> bool) -> Option<String> {
    let names: std::collections::BTreeSet<String> = rawler::lens::get_lenses()
        .iter()
        .filter(|l| mounts.contains(&l.mount.as_str()) && fits(l))
        .filter_map(table_name)
        .collect();
    if names.len() == 1 { names.into_iter().next() } else { None }
}

fn canon_lens_by_id(id: u32, short: f32, long: f32) -> Option<String> {
    if id == 0 || id == 0xFFFF {
        return None;
    }
    let mm = |r: &rawler::formats::tiff::Rational| if r.d == 0 { -1.0 } else { r.n as f32 / r.d as f32 };
    lens_by_id(&["ef-mount", "rf-mount", "cn-mount"], |l| {
        l.identifiers.id.is_some_and(|(i, _)| i == id)
            && (long <= 0.0
                || ((mm(&l.focal_range[0]) - short).abs() < 0.6 && (mm(&l.focal_range[1]) - long).abs() < 0.6))
    })
}

/// Pentax ids are (series, number); series 0-2 are manual lenses (M42, K, M,
/// A) that can't tell themselves apart.
fn pentax_lens_by_id(series: u32, number: u32) -> Option<String> {
    if series <= 2 {
        return None;
    }
    lens_by_id(&["k-mount"], |l| l.identifiers.id == Some((series, number)))
}

/// A CRW's lens: it records no name, only the camera settings (lens id,
/// focal range, aperture) somewhere in its heap of directories.
fn ciff_clues(buf: &[u8]) -> Clues {
    let Some(start) = le32(buf, 2) else { return Clues::default() };
    match ciff_find(buf, start as usize, buf.len(), CIFF_CAMERA_SETTINGS, 0) {
        Some(data) => {
            canon_settings(&data.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect::<Vec<_>>())
        }
        None => Clues::default(),
    }
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
        table_lens = md.lens.as_ref().and_then(table_name);
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
    let mut reader = BufReader::new(File::open(path).ok()?);
    let ex = exif::Reader::new().read_from_container(&mut reader).ok()?;
    Some(meta_of(&ex))
}

/// What EXIF says about the photo (also the EXIF a Photoshop file carries).
pub(crate) fn meta_of(ex: &exif::Exif) -> Meta {
    use exif::{In, Tag, Value};
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

    Meta {
        taken: ascii(Tag::DateTimeOriginal)
            .or_else(|| ascii(Tag::DateTimeDigitized))
            .or_else(|| ascii(Tag::DateTime))
            .as_deref()
            .and_then(parse_exif_date),
        subsec: ascii(Tag::SubSecTimeOriginal),
        make: ascii(Tag::Make),
        model: ascii(Tag::Model),
        lens: ascii(Tag::LensModel).or_else(|| exif_lens(ex)),
        serial: ascii(Tag::BodySerialNumber),
        image_number: None,
        iso: uint(Tag::PhotographicSensitivity).filter(|&v| v > 0),
        fnumber: rational(Tag::FNumber).and_then(|(n, d)| ratio(n, d)),
        exposure: rational(Tag::ExposureTime).and_then(|(n, d)| shutter(n, d)),
        focal: rational(Tag::FocalLength).and_then(|(n, d)| ratio(n, d)),
        orientation: uint(Tag::Orientation).unwrap_or(0) as u16,
        width: uint(Tag::PixelXDimension),
        height: uint(Tag::PixelYDimension),
    }
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

    fn lens_of(s: &[u16]) -> Option<String> {
        let c = canon_settings(s);
        c.name.or(c.spec)
    }

    #[test]
    fn canon_settings_name_the_lens() {
        let mut s = [0u16; 27];
        (s[22], s[23], s[24], s[25]) = (174, 135, 135, 1);
        assert_eq!(lens_of(&s).as_deref(), Some("Canon EF 135mm f/2L USM"));
        // Without its range, id 174 is ten lenses: none is guessed, and nothing says what it is.
        (s[23], s[24]) = (0, 0);
        assert_eq!(lens_of(&s), None);
        // A 10D records no id (65535): the lens goes by what it is.
        for (long, short, aperture, name) in
            [(70, 24, 95, "24-70mm f/2.8"), (24, 12, 128, "12-24mm f/4"), (50, 50, 54, "50mm f/1.8")]
        {
            (s[22], s[23], s[24], s[26]) = (0xFFFF, long, short, aperture);
            assert_eq!(lens_of(&s).as_deref(), Some(name));
        }
        s[26] = 0;
        assert_eq!(lens_of(&s).as_deref(), Some("50mm"));
        // A lens reporting nonsense (a Sigma 12-24 on a 30D: 54, f/1.8): no such lens, no aperture.
        (s[23], s[24], s[26]) = (24, 12, 54);
        assert_eq!(lens_of(&s).as_deref(), Some("12-24mm"));
    }

    fn ciff_lens_of(buf: &[u8]) -> Option<String> {
        let c = ciff_clues(buf);
        c.name.or(c.spec)
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
        assert_eq!(ciff_lens_of(&file).as_deref(), Some("Canon EF 135mm f/2L USM"));
        // A truncated file finds nothing, and doesn't panic.
        assert_eq!(ciff_lens_of(&file[..60]), None);
    }

    /// A TIFF entry: tag, type, count, value bytes.
    type E = (u16, u16, u32, Vec<u8>);

    /// A small little-endian TIFF: IFD0 with `root`, and an EXIF IFD with
    /// `exif` when given; values over 4 bytes go after the directories.
    fn tiff(root: &[E], exif: &[E]) -> Vec<u8> {
        let ifd_len = |n: usize| 2 + n * 12 + 4;
        let mut root = root.to_vec();
        let exif_at = 8 + ifd_len(root.len() + usize::from(!exif.is_empty()));
        if !exif.is_empty() {
            root.push((EXIF_IFD, 4, 1, (exif_at as u32).to_le_bytes().to_vec()));
        }
        root.sort_by_key(|e| e.0);
        let data_at = exif_at + if exif.is_empty() { 0 } else { ifd_len(exif.len()) };
        let mut out = b"II*\0".to_vec();
        out.extend(8u32.to_le_bytes());
        let mut data: Vec<u8> = Vec::new();
        for entries in [&root[..], exif].into_iter().filter(|e| !e.is_empty()) {
            out.extend((entries.len() as u16).to_le_bytes());
            for (tag, typ, count, bytes) in entries {
                out.extend(tag.to_le_bytes());
                out.extend(typ.to_le_bytes());
                out.extend(count.to_le_bytes());
                if bytes.len() <= 4 {
                    out.extend(bytes.iter().copied().chain(std::iter::repeat(0)).take(4));
                } else {
                    out.extend(((data_at + data.len()) as u32).to_le_bytes());
                    data.extend(bytes);
                }
            }
            out.extend(0u32.to_le_bytes());
        }
        out.extend(data);
        out
    }

    fn text(tag: u16, s: &str) -> E {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        (tag, 2, b.len() as u32, b)
    }

    /// Four rationals, as LensSpecification: min and max focal, widest aperture at each.
    fn spec(tag: u16, v: [(u32, u32); 4]) -> E {
        (tag, 5, 4, v.iter().flat_map(|(n, d)| n.to_le_bytes().into_iter().chain(d.to_le_bytes())).collect())
    }

    fn lens_in(buf: &[u8]) -> Option<String> {
        let t = GenericTiffReader::new_with_buffer(buf, 0, 0, Some(0)).unwrap();
        let c = tiff_clues(&t, &mut Cursor::new(buf));
        c.name.or(c.spec)
    }

    #[test]
    fn any_maker_says_what_its_lens_is() {
        // The standard tag, any maker.
        let sony = text(MAKE, "SONY");
        let fe = spec(LENS_SPECIFICATION, [(16, 1), (35, 1), (28, 10), (28, 10)]);
        assert_eq!(
            lens_in(&tiff(std::slice::from_ref(&sony), std::slice::from_ref(&fe))).as_deref(),
            Some("16-35mm f/2.8")
        );
        // A name beats what it is.
        let named = [fe, text(LENS_MODEL, "FE 16-35mm F2.8 GM")];
        assert_eq!(lens_in(&tiff(std::slice::from_ref(&sony), &named)).as_deref(), Some("FE 16-35mm F2.8 GM"));
        // All zeros: a lens that reports nothing.
        let none = spec(LENS_SPECIFICATION, [(0, 1), (0, 1), (0, 1), (0, 1)]);
        assert_eq!(lens_in(&tiff(&[sony], &[none])), None);
        // A DNG's own copy, with the apertures of a zoom.
        let dng = [text(MAKE, "Leica"), spec(DNG_LENS_INFO, [(18, 1), (55, 1), (35, 10), (56, 10)])];
        assert_eq!(lens_in(&tiff(&dng, &[])).as_deref(), Some("18-55mm f/3.5-5.6"));
    }

    #[test]
    fn maker_notes_say_it_too() {
        // Nikon: a TIFF of its own inside the note, offsets from there.
        let mut nikon = b"Nikon\0\x02\x10\0\0".to_vec();
        nikon.extend(tiff(&[spec(NIKON_LENS, [(24, 1), (70, 1), (28, 10), (28, 10)])], &[]));
        let note = (MAKER_NOTE, 7, nikon.len() as u32, nikon);
        assert_eq!(lens_in(&tiff(&[text(MAKE, "NIKON CORPORATION")], &[note])).as_deref(), Some("24-70mm f/2.8"));

        // Pentax's own DNG: the note as it is in DNGPrivateData, the lens id
        // (8, 29) in it - rawler's table says which lens that is.
        let mut pentax = b"PENTAX \0II".to_vec();
        pentax.extend(1u16.to_le_bytes());
        pentax.extend(PENTAX_LENS_REC.to_le_bytes());
        pentax.extend([1, 0, 4, 0, 0, 0, 8, 29, 0, 0, 0, 0, 0, 0]);
        let private = (DNG_PRIVATE_DATA, 1, pentax.len() as u32, pentax);
        let k3 = [text(MAKE, "RICOH IMAGING COMPANY, LTD."), private];
        assert_eq!(lens_in(&tiff(&k3, &[])).as_deref(), Some("Sigma 35mm F1.4 DG HSM"));
    }
}
