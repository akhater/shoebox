//! Photoshop files: `.psd`, `.psb` (large documents), or any other name - a
//! Photoshop file is told by its first bytes ("8BPS"), not by its name.
//!
//! Shoebox shows the flattened picture Photoshop keeps inside the file (saved
//! with "Maximize compatibility", Photoshop's default) and reads the EXIF it
//! carries over from the original photo. Layers are never composed. Channels
//! are stored one after the other, so each is read front to back (an external
//! disk never seeks back and forth); a huge document is read every few rows
//! and columns, as the preview needs no more. Only ever opened for reading.

use crate::imaging::{Loaded, Meta};
use anyhow::{Result, bail};
use image::{DynamicImage, RgbImage};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

const SIGNATURE: &[u8; 4] = b"8BPS";

/// A Photoshop file, whatever its name.
pub fn is_psd(path: &Path) -> bool {
    let mut head = [0u8; 4];
    File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == SIGNATURE
}

/// Its metadata and a picture of it, at least `long_edge` px on its long side
/// when the document is that big.
pub fn load(path: &Path, long_edge: u32) -> Loaded {
    let fail = |e: String| Loaded { meta: Meta::default(), image: Err(format!("photoshop: {e}")) };
    let mut r = match File::open(path) {
        Ok(f) => BufReader::with_capacity(1 << 20, f),
        Err(e) => return fail(e.to_string()),
    };
    let doc = match read_doc(&mut r) {
        Ok(d) => d,
        Err(e) => return fail(format!("{e:#}")),
    };
    let mut meta = doc.meta();
    // The picture is upright as Photoshop shows it, whatever the EXIF says.
    meta.orientation = 0;
    let long = doc.width.max(doc.height);
    let step = if long_edge == u32::MAX { 1 } else { (long / long_edge.saturating_mul(2).max(1)).max(1) };
    let image = match catch_unwind(AssertUnwindSafe(|| composite(&mut r, &doc, step))) {
        Ok(Ok(img)) => Ok(DynamicImage::ImageRgb8(img)),
        Ok(Err(e)) => Err(format!("photoshop: {e:#}")),
        Err(_) => Err("photoshop: crashed".into()),
    };
    Loaded { meta, image }
}

/// What the file says about the photo (from its EXIF), without its picture.
pub fn meta(path: &Path) -> Option<Meta> {
    let mut r = BufReader::new(File::open(path).ok()?);
    Some(read_doc(&mut r).ok()?.meta())
}

/// A document's sections, up to its flattened picture.
struct Doc {
    /// A large document (.psb): wider lengths and row sizes.
    big: bool,
    channels: usize,
    height: u32,
    width: u32,
    depth: u16,
    mode: u16,
    /// Indexed colour: 256 reds, then greens, then blues.
    palette: Vec<u8>,
    exif: Option<Vec<u8>>,
    /// False when saved without "Maximize compatibility": what is there is not the picture.
    real_composite: bool,
    picture_at: u64,
}

impl Doc {
    fn meta(&self) -> Meta {
        let ex = self.exif.clone().and_then(|e| exif::Reader::new().read_raw(e).ok());
        let mut meta = ex.as_ref().map(crate::imaging::meta_of).unwrap_or_default();
        meta.width = Some(self.width);
        meta.height = Some(self.height);
        meta
    }
}

const MODE_GRAY: u16 = 1;
const MODE_INDEXED: u16 = 2;
const MODE_RGB: u16 = 3;
const MODE_CMYK: u16 = 4;
const MODE_DUOTONE: u16 = 8;
/// Image resources Shoebox reads: which version wrote the file (and whether
/// its flattened picture is real), and the EXIF.
const RES_VERSION: u16 = 1057;
const RES_EXIF: u16 = 1058;

fn read_doc(r: &mut BufReader<File>) -> Result<Doc> {
    let mut h = [0u8; 26];
    r.read_exact(&mut h)?;
    if &h[0..4] != SIGNATURE {
        bail!("not a Photoshop file");
    }
    let big = match be16(&h[4..6]) {
        1 => false,
        2 => true,
        v => bail!("Photoshop file version {v}"),
    };
    let mut doc = Doc {
        big,
        channels: be16(&h[12..14]) as usize,
        height: be32(&h[14..18]),
        width: be32(&h[18..22]),
        depth: be16(&h[22..24]),
        mode: be16(&h[24..26]),
        palette: Vec::new(),
        exif: None,
        real_composite: true,
        picture_at: 0,
    };
    if doc.width == 0 || doc.height == 0 {
        bail!("empty Photoshop document");
    }
    // Colour mode data: the palette of an indexed-colour document.
    let len = u32_of(r)? as u64;
    if doc.mode == MODE_INDEXED && len >= 768 {
        doc.palette = vec![0; 768];
        r.read_exact(&mut doc.palette)?;
        r.seek_relative(len as i64 - 768)?;
    } else {
        r.seek_relative(len as i64)?;
    }
    // Image resources.
    let len = u32_of(r)? as u64;
    let end = r.stream_position()? + len;
    while r.stream_position()? + 12 <= end {
        let mut sig = [0u8; 4];
        r.read_exact(&mut sig)?;
        let id = u16_of(r)?;
        // Its name: a length byte and the name, padded to an even size.
        let name = u8_of(r)? as i64;
        r.seek_relative(name + (name + 1) % 2)?;
        let size = u32_of(r)? as u64;
        let at = r.stream_position()?;
        match id {
            RES_EXIF if size > 0 && size < 16 << 20 => {
                let mut d = vec![0; size as usize];
                r.read_exact(&mut d)?;
                doc.exif = Some(d);
            }
            RES_VERSION if size >= 5 => {
                let mut d = [0u8; 5];
                r.read_exact(&mut d)?;
                doc.real_composite = d[4] != 0;
            }
            _ => {}
        }
        r.seek(SeekFrom::Start(at + size + (size & 1)))?;
    }
    r.seek(SeekFrom::Start(end))?;
    // Layers and masks: never composed.
    let len = if big { u64_of(r)? } else { u32_of(r)? as u64 };
    r.seek_relative(len as i64)?;
    doc.picture_at = r.stream_position()?;
    Ok(doc)
}

/// The flattened picture, every `step`th row and column.
fn composite(r: &mut BufReader<File>, d: &Doc, step: u32) -> Result<RgbImage> {
    let bytes = match d.depth {
        8 => 1,
        16 => 2,
        n => bail!("{n}-bit Photoshop documents aren't read yet"),
    };
    let used = match d.mode {
        MODE_GRAY | MODE_DUOTONE | MODE_INDEXED => 1,
        MODE_RGB => 3,
        MODE_CMYK => 4,
        m => bail!("Photoshop colour mode {m} isn't read yet"),
    };
    if d.channels < used {
        bail!("{} channels for colour mode {}", d.channels, d.mode);
    }
    if d.mode == MODE_INDEXED && d.palette.len() < 768 {
        bail!("indexed colour without its palette");
    }
    if !d.real_composite {
        bail!("saved without \"Maximize compatibility\": no flattened picture inside");
    }
    r.seek(SeekFrom::Start(d.picture_at))?;
    let compression = u16_of(r)?;
    let (w, h, step) = (d.width as usize, d.height as usize, step as usize);
    let (ow, oh) = (w.div_ceil(step), h.div_ceil(step));
    let row_len = w * bytes;
    let mut planes = vec![vec![0u8; ow * oh]; used];
    let mut row = vec![0u8; row_len];
    // One row's samples kept: the high byte of 16-bit ones.
    let keep = |row: &[u8], plane: &mut [u8], y: usize| {
        for (x, out) in plane[y * ow..(y + 1) * ow].iter_mut().enumerate() {
            *out = row[x * step * bytes];
        }
    };
    match compression {
        // Uncompressed, or PackBits (run-length) rows listed with their sizes.
        0 | 1 => {
            let sizes: Vec<u64> = if compression == 1 {
                (0..d.channels * h)
                    .map(|_| if d.big { u32_of(r).map(u64::from) } else { u16_of(r).map(u64::from) })
                    .collect::<std::io::Result<_>>()?
            } else {
                vec![row_len as u64; d.channels * h]
            };
            let mut packed = Vec::new();
            for (c, plane) in planes.iter_mut().enumerate() {
                for y in 0..h {
                    let size = sizes[c * h + y];
                    if y % step != 0 {
                        r.seek_relative(size as i64)?;
                        continue;
                    }
                    if compression == 1 {
                        // PackBits never takes much more room than the row itself.
                        if size > 2 * row_len as u64 + 64 {
                            bail!("damaged: a row of {size} bytes");
                        }
                        packed.resize(size as usize, 0);
                        r.read_exact(&mut packed)?;
                        unpack_bits(&packed, &mut row);
                    } else {
                        r.read_exact(&mut row)?;
                    }
                    keep(&row, plane, y / step);
                }
            }
        }
        // Zip, without or with prediction (each sample stored as the
        // difference from the one before it in its row).
        2 | 3 => {
            let mut all = Vec::new();
            flate2::read::ZlibDecoder::new(r.by_ref()).take((used * h * row_len) as u64).read_to_end(&mut all)?;
            if all.len() < used * h * row_len {
                bail!("flattened picture cut short");
            }
            for (c, plane) in planes.iter_mut().enumerate() {
                for y in (0..h).step_by(step) {
                    row.copy_from_slice(&all[(c * h + y) * row_len..][..row_len]);
                    if compression == 3 {
                        undo_prediction(&mut row, bytes);
                    }
                    keep(&row, plane, y / step);
                }
            }
        }
        n => bail!("Photoshop compression {n} isn't read yet"),
    }

    let mut img = RgbImage::new(ow as u32, oh as u32);
    for (i, px) in img.pixels_mut().enumerate() {
        px.0 = match d.mode {
            MODE_RGB => [planes[0][i], planes[1][i], planes[2][i]],
            MODE_INDEXED => {
                let v = planes[0][i] as usize;
                [d.palette[v], d.palette[256 + v], d.palette[512 + v]]
            }
            // Stored inverted: 255 is no ink.
            MODE_CMYK => {
                let k = planes[3][i] as u32;
                [0, 1, 2].map(|c| (planes[c][i] as u32 * k / 255) as u8)
            }
            _ => [planes[0][i]; 3],
        };
    }
    Ok(img)
}

/// PackBits: a count byte, then that many bytes as they are (0..=127: n + 1)
/// or one byte repeated (-1..=-127: 1 - n times). What is missing stays 0.
fn unpack_bits(src: &[u8], out: &mut [u8]) {
    out.fill(0);
    let (mut i, mut o) = (0, 0);
    while i < src.len() && o < out.len() {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let k = (n as usize + 1).min(src.len() - i).min(out.len() - o);
            out[o..o + k].copy_from_slice(&src[i..i + k]);
            i += n as usize + 1;
            o += k;
        } else if n != -128 && i < src.len() {
            let k = (1 - n as isize) as usize;
            let k = k.min(out.len() - o);
            out[o..o + k].fill(src[i]);
            i += 1;
            o += k;
        }
    }
}

/// Zip with prediction: each sample was stored as its difference from the
/// one before it in the row (16-bit samples big-endian).
fn undo_prediction(row: &mut [u8], bytes: usize) {
    if bytes == 1 {
        for x in 1..row.len() {
            row[x] = row[x].wrapping_add(row[x - 1]);
        }
    } else {
        let mut prev = 0u16;
        for s in row.as_chunks_mut::<2>().0 {
            prev = prev.wrapping_add(u16::from_be_bytes(*s));
            *s = prev.to_be_bytes();
        }
    }
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn u8_of(r: &mut impl Read) -> std::io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn u16_of(r: &mut impl Read) -> std::io::Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn u32_of(r: &mut impl Read) -> std::io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn u64_of(r: &mut impl Read) -> std::io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_be_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits_runs_and_literals() {
        let mut out = [9u8; 8];
        // 3 literal bytes, then 4 times 7, then a no-op, then 1 literal.
        unpack_bits(&[2, 1, 2, 3, (-3i8) as u8, 7, 0x80, 0, 5], &mut out);
        assert_eq!(out, [1, 2, 3, 7, 7, 7, 7, 5]);
        // Cut short: the rest stays 0, nothing is read past the end.
        unpack_bits(&[5, 1, 2], &mut out);
        assert_eq!(out, [1, 2, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn prediction_is_undone() {
        let mut row = [10u8, 1, 1, 254];
        undo_prediction(&mut row, 1);
        assert_eq!(row, [10, 11, 12, 10]);
        let mut row = [0x01, 0x00, 0x00, 0x10];
        undo_prediction(&mut row, 2);
        assert_eq!(row, [0x01, 0x00, 0x01, 0x10]);
    }
}
