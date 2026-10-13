//! The JPEGs cameras embed in their RAW files, found without decoding the RAW.
//!
//! A catalog preview needs ~1080 px, and nearly every RAW already carries a
//! JPEG at that size or larger. Reading only that JPEG (a few hundred KB to a
//! few MB of a 25 MB file) and decoding it at reduced size (JPEG allows 1/2,
//! 1/4, 1/8 straight from the compressed data) is what makes scanning fast.
//!
//! Formats: TIFF-based (CR2, NEF, ARW, DNG, PEF, RW2, ...), CR3, RAF.
//! Anything else returns nothing and the caller falls back to rawler / the
//! Windows codecs. Files are only read.

use image::{DynamicImage, GrayImage, RgbImage};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub struct Found {
    /// Upright is the caller's job (EXIF orientation).
    pub image: DynamicImage,
    /// Size of the largest embedded JPEG: the photo's full size for most cameras.
    pub full_w: u32,
    pub full_h: u32,
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    offset: u64,
    len: u64,
    w: u32,
    h: u32,
}

/// The embedded JPEG best suited to a `long_edge` preview, decoded at the
/// smallest size that is still at least `long_edge` long (when possible).
pub fn preview(path: &Path, long_edge: u32) -> Option<Found> {
    let mut f = File::open(path).ok()?;
    let size = f.metadata().ok()?.len();
    let mut cands: Vec<Candidate> = locate(&mut f, size)
        .into_iter()
        .filter(|&(o, l)| l > 512 && o.checked_add(l).is_some_and(|end| end <= size))
        .filter_map(|(o, l)| jpeg_size(&mut f, o, l).map(|(w, h)| Candidate { offset: o, len: l, w, h }))
        .collect();
    if cands.is_empty() {
        return None;
    }
    cands.sort_by_key(|c| c.w.max(c.h));
    let full = *cands.last()?;
    let pick = cands.iter().find(|c| c.w.max(c.h) >= long_edge).copied().unwrap_or(full);

    let mut bytes = vec![0u8; pick.len as usize];
    f.seek(SeekFrom::Start(pick.offset)).ok()?;
    f.read_exact(&mut bytes).ok()?;
    let image = decode_scaled(&bytes, pick.w, pick.h, long_edge)?;
    Some(Found { image, full_w: full.w, full_h: full.h })
}

/// Decodes a JPEG at the smallest DCT scale (1, 1/2, 1/4, 1/8) whose long edge
/// is still >= `long_edge`.
fn decode_scaled(bytes: &[u8], w: u32, h: u32, long_edge: u32) -> Option<DynamicImage> {
    let mut dec = jpeg_decoder::Decoder::new(bytes);
    dec.read_info().ok()?;
    let long = w.max(h).max(1) as f64;
    let s = (long_edge as f64 / long).min(1.0);
    let req_w = ((w as f64 * s).ceil() as u32).clamp(1, u16::MAX as u32) as u16;
    let req_h = ((h as f64 * s).ceil() as u32).clamp(1, u16::MAX as u32) as u16;
    let (ow, oh) = dec.scale(req_w, req_h).ok()?;
    let pixels = dec.decode().ok()?;
    let info = dec.info()?;
    let (ow, oh) = (ow as u32, oh as u32);
    match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => RgbImage::from_raw(ow, oh, pixels).map(DynamicImage::ImageRgb8),
        jpeg_decoder::PixelFormat::L8 => GrayImage::from_raw(ow, oh, pixels).map(DynamicImage::ImageLuma8),
        _ => None,
    }
}

/// (offset, length) of every JPEG-looking blob the container points at.
fn locate(f: &mut File, size: u64) -> Vec<(u64, u64)> {
    let head = read_at(f, 0, 96).unwrap_or_default();
    if head.len() < 16 {
        return Vec::new();
    }
    if head.starts_with(b"FUJIFILMCCD-RAW") {
        return raf(&head);
    }
    if &head[4..8] == b"ftyp" {
        return bmff(f, size);
    }
    let le = match &head[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Vec::new(),
    };
    tiff(f, le, size)
}

fn raf(head: &[u8]) -> Vec<(u64, u64)> {
    let be = |o: usize| head.get(o..o + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as u64);
    match (be(84), be(88)) {
        (Some(o), Some(l)) if o > 0 && l > 0 => vec![(o, l)],
        _ => Vec::new(),
    }
}

/// TIFF IFD walk: IFD0 chain + SubIFDs. JPEG previews are either
/// JPEGInterchangeFormat/Length, a single JPEG-compressed strip, or
/// Panasonic's JpgFromRaw tag.
fn tiff(f: &mut File, le: bool, size: u64) -> Vec<(u64, u64)> {
    let u16_at = |b: &[u8], o: usize| -> u64 {
        let x = [b[o], b[o + 1]];
        (if le { u16::from_le_bytes(x) } else { u16::from_be_bytes(x) }) as u64
    };
    let u32_at = |b: &[u8], o: usize| -> u64 {
        let x = [b[o], b[o + 1], b[o + 2], b[o + 3]];
        (if le { u32::from_le_bytes(x) } else { u32::from_be_bytes(x) }) as u64
    };
    let Some(hdr) = read_at(f, 0, 8) else { return Vec::new() };
    let mut queue = vec![u32_at(&hdr, 4)];
    let mut seen = Vec::new();
    let mut out = Vec::new();

    while let Some(ifd) = queue.pop() {
        if ifd == 0 || ifd >= size || seen.contains(&ifd) || seen.len() > 32 {
            continue;
        }
        seen.push(ifd);
        let Some(nb) = read_at(f, ifd, 2) else { continue };
        let n = u16_at(&nb, 0).min(512);
        let Some(entries) = read_at(f, ifd + 2, n * 12 + 4) else { continue };
        let (mut jif, mut jif_len, mut strip, mut strip_len, mut compression, mut photometric) =
            (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
        for i in 0..n as usize {
            let e = i * 12;
            let tag = u16_at(&entries, e);
            let typ = u16_at(&entries, e + 2);
            let count = u32_at(&entries, e + 4);
            // SHORT values sit in the first two bytes of the value field.
            let val = if typ == 3 && count == 1 { u16_at(&entries, e + 8) } else { u32_at(&entries, e + 8) };
            match tag {
                0x0103 => compression = val,
                0x0106 => photometric = val,
                0x0111 if count == 1 => strip = val,
                0x0117 if count == 1 => strip_len = val,
                0x0201 => jif = val,
                0x0202 => jif_len = val,
                // Panasonic RW2: the whole JPEG is the tag's value.
                0x002e if typ == 7 && count > 512 => out.push((val, count)),
                0x014a => {
                    if count == 1 {
                        queue.push(val);
                    } else if let Some(b) = read_at(f, val, count.min(16) * 4) {
                        for j in 0..count.min(16) as usize {
                            queue.push(u32_at(&b, j * 4));
                        }
                    }
                }
                _ => {}
            }
        }
        if jif > 0 && jif_len > 0 {
            out.push((jif, jif_len));
        }
        // 32803 = CFA, 34892 = LinearRaw: that strip is the sensor data, not a picture.
        if matches!(compression, 6 | 7) && strip > 0 && strip_len > 0 && !matches!(photometric, 32803 | 34892) {
            out.push((strip, strip_len));
        }
        let next = u32_at(&entries, n as usize * 12);
        queue.push(next);
    }
    out
}

const CR3_PREVIEW_UUID: [u8; 16] =
    [0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16];

/// ISO BMFF (Canon CR3): the ~1620 px preview in a top-level uuid box, and the
/// full-size JPEG that is track 1's first sample.
fn bmff(f: &mut File, size: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for (typ, start, end, hdr) in boxes(f, 0, size) {
        match &typ {
            b"uuid" => {
                if read_at(f, start + hdr, 16).as_deref() == Some(&CR3_PREVIEW_UUID[..]) {
                    // The JPEG starts at its SOI marker somewhere in the first bytes of the payload.
                    if let Some(b) = read_at(f, start + hdr + 16, 256)
                        && let Some(i) = b.windows(3).position(|w| w == [0xff, 0xd8, 0xff])
                    {
                        let o = start + hdr + 16 + i as u64;
                        out.push((o, end - o));
                    }
                }
            }
            b"moov" => {
                if let Some(trak) = boxes(f, start + hdr, end).into_iter().find(|b| &b.0 == b"trak")
                    && let Some(s) = first_sample(f, trak.1 + trak.3, trak.2)
                {
                    out.push(s);
                }
            }
            _ => {}
        }
    }
    out
}

/// trak -> mdia -> minf -> stbl -> (co64|stco, stsz): first sample's offset and size.
fn first_sample(f: &mut File, start: u64, end: u64) -> Option<(u64, u64)> {
    let child = |f: &mut File, s: u64, e: u64, name: &[u8; 4]| boxes(f, s, e).into_iter().find(|b| &b.0 == name);
    let mdia = child(f, start, end, b"mdia")?;
    let minf = child(f, mdia.1 + mdia.3, mdia.2, b"minf")?;
    let stbl = child(f, minf.1 + minf.3, minf.2, b"stbl")?;
    let mut offset = None;
    let mut len = None;
    for (typ, s, _, hdr) in boxes(f, stbl.1 + stbl.3, stbl.2) {
        let body = s + hdr;
        match &typ {
            b"co64" => {
                let b = read_at(f, body + 8, 8)?;
                offset = Some(u64::from_be_bytes(b.try_into().ok()?));
            }
            b"stco" => {
                let b = read_at(f, body + 8, 4)?;
                offset = Some(u32::from_be_bytes(b.try_into().ok()?) as u64);
            }
            b"stsz" => {
                let b = read_at(f, body + 4, 12)?;
                let fixed = u32::from_be_bytes(b[0..4].try_into().ok()?) as u64;
                len = Some(if fixed != 0 { fixed } else { u32::from_be_bytes(b[8..12].try_into().ok()?) as u64 });
            }
            _ => {}
        }
    }
    Some((offset?, len?))
}

/// Child boxes in [start, end): (type, box start, box end, header length).
fn boxes(f: &mut File, start: u64, end: u64) -> Vec<([u8; 4], u64, u64, u64)> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end && out.len() < 256 {
        let Some(h) = read_at(f, pos, 16) else { break };
        let small = u32::from_be_bytes(h[0..4].try_into().unwrap()) as u64;
        let typ: [u8; 4] = h[4..8].try_into().unwrap();
        let (len, hdr) = match small {
            0 => (end - pos, 8),
            1 if h.len() >= 16 => (u64::from_be_bytes(h[8..16].try_into().unwrap()), 16),
            n => (n, 8),
        };
        if len < hdr || pos + len > end {
            break;
        }
        out.push((typ, pos, pos + len, hdr));
        pos += len;
    }
    out
}

/// Width and height from a JPEG's frame header, only for 8-bit baseline,
/// extended or progressive JPEGs. Lossless JPEG (SOF3, how CR2/DNG store the
/// sensor data) is rejected here.
fn jpeg_size(f: &mut File, offset: u64, len: u64) -> Option<(u32, u32)> {
    let b = read_at(f, offset, len.min(256 * 1024))?;
    if b.get(..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xff {
            return None;
        }
        let marker = b[i + 1];
        if marker == 0xff {
            i += 1;
            continue;
        }
        let seg = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        match marker {
            0xc0..=0xc2 => {
                let s = b.get(i + 4..i + 9)?;
                if s[0] != 8 {
                    return None;
                }
                let h = u16::from_be_bytes([s[1], s[2]]) as u32;
                let w = u16::from_be_bytes([s[3], s[4]]) as u32;
                return (w > 0 && h > 0).then_some((w, h));
            }
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => return None,
            0xd9 | 0xda => return None,
            _ => i += 2 + seg,
        }
    }
    None
}

fn read_at(f: &mut File, offset: u64, len: u64) -> Option<Vec<u8>> {
    f.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = Vec::with_capacity(len as usize);
    f.take(len).read_to_end(&mut buf).ok()?;
    (!buf.is_empty()).then_some(buf)
}
