//! Windows Imaging Component: decodes whatever Windows itself can open.
//! With the Microsoft RAW (LibRaw-based) and HEIF extensions installed this
//! covers nearly every RAW format plus HEIC/HIF/AVIF, so it is Shoebox's safety net
//! behind rawler. For JPEG it also downscales during decode, which is fast.

use anyhow::{Result, anyhow};
use image::{DynamicImage, RgbImage};
use std::path::Path;
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat24bppRGB, IWICBitmapSource, IWICImagingFactory,
    WICBitmapInterpolationModeFant, WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::core::{Interface, PCWSTR};

thread_local! {
    static COM_READY: () = unsafe {
        // Fails harmlessly if the thread already chose a COM mode.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    };
}

pub struct Decoded {
    pub image: DynamicImage,
    /// Size of the full image, before any downscaling.
    pub full_w: u32,
    pub full_h: u32,
}

/// Decodes `path` so its long edge is at most `max_long`.
/// `prefer_preview` asks the codec for its embedded preview first (RAW files).
pub fn decode(path: &Path, max_long: u32, prefer_preview: bool) -> Result<Decoded> {
    COM_READY.with(|_| {});
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let decoder = factory.CreateDecoderFromFilename(
            PCWSTR(wide.as_ptr()),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let (mut full_w, mut full_h) = (0u32, 0u32);
        frame.GetSize(&mut full_w, &mut full_h)?;

        let source: IWICBitmapSource = match prefer_preview {
            true => match decoder.GetPreview() {
                Ok(p) => p,
                Err(_) => frame.cast()?,
            },
            false => frame.cast()?,
        };
        let (mut sw, mut sh) = (0u32, 0u32);
        source.GetSize(&mut sw, &mut sh)?;
        if sw == 0 || sh == 0 {
            return Err(anyhow!("empty image"));
        }

        let (tw, th) = fit(sw, sh, max_long);
        let sized: IWICBitmapSource = if (tw, th) != (sw, sh) {
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(&source, tw, th, WICBitmapInterpolationModeFant)?;
            scaler.cast()?
        } else {
            source
        };

        let rgb = WICConvertBitmapSource(&GUID_WICPixelFormat24bppRGB, &sized)?;
        let stride = tw * 3;
        let mut buf = vec![0u8; stride as usize * th as usize];
        rgb.CopyPixels(std::ptr::null(), stride, &mut buf)?;
        let img = RgbImage::from_raw(tw, th, buf).ok_or_else(|| anyhow!("bad pixel buffer"))?;
        Ok(Decoded { image: DynamicImage::ImageRgb8(img), full_w, full_h })
    }
}

fn fit(w: u32, h: u32, max_long: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max_long {
        return (w, h);
    }
    let s = max_long as f64 / long as f64;
    (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1))
}
