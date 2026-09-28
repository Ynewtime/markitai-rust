//! Optional native page rasterization, independent of OCR and disk publication.

use crate::{Error, Result};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::PdfRasterSession;

pub(crate) fn available() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(not(target_os = "macos"))]
pub(crate) struct PdfRasterSession;

#[cfg(not(target_os = "macos"))]
impl PdfRasterSession {
    pub(crate) fn open(_bytes: &[u8]) -> Result<Self> {
        Err(Error::Unsupported(
            "Native PDF page rendering is available only on macOS in this build".into(),
        ))
    }

    pub(crate) fn pages(&self) -> usize {
        0
    }

    pub(crate) fn dimensions(&self, _page_1_based: usize, _dpi: f64) -> Result<(u32, u32)> {
        Err(Error::Unsupported(
            "Native PDF page rendering is available only on macOS in this build".into(),
        ))
    }

    pub(crate) fn render(&self, _page_1_based: usize, _dpi: f64) -> Result<image::RgbImage> {
        Err(Error::Unsupported(
            "Native PDF page rendering is available only on macOS in this build".into(),
        ))
    }
}

#[cfg(any(target_os = "macos", test))]
fn failure(message: &str) -> Error {
    Error::Conversion(format!("Native PDF rendering: {message}"))
}

#[cfg(any(target_os = "macos", test))]
const MAX_PIXELS: usize = 32_000_000;

#[cfg(any(target_os = "macos", test))]
struct RasterSize {
    width: u32,
    height: u32,
    row_bytes: usize,
    rgba_bytes: usize,
    rgb_bytes: usize,
}

#[cfg(any(target_os = "macos", test))]
fn raster_size(width: f64, height: f64, dpi: f64) -> Result<RasterSize> {
    if [width, height, dpi]
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.)
    {
        return Err(failure(
            "page dimensions and DPI must be positive and finite",
        ));
    }
    // Multiply before dividing so exact point/DPI products such as 792*150
    // remain the integer 1650 rather than rounding just above it before ceil.
    let width = (width * dpi / 72.).ceil();
    let height = (height * dpi / 72.).ceil();
    if !width.is_finite()
        || !height.is_finite()
        || width < 1.
        || height < 1.
        || width > MAX_PIXELS as f64
        || height > MAX_PIXELS as f64
    {
        return Err(failure("page dimensions exceed the 32 million pixel limit"));
    }
    let width = width as usize;
    let height = height as usize;
    let pixels = width
        .checked_mul(height)
        .filter(|pixels| *pixels <= MAX_PIXELS)
        .ok_or_else(|| failure("page exceeds the 32 million pixel limit"))?;
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| failure("row size overflow"))?;
    let rgba_bytes = row_bytes
        .checked_mul(height)
        .ok_or_else(|| failure("bitmap size overflow"))?;
    let rgb_bytes = pixels
        .checked_mul(3)
        .ok_or_else(|| failure("RGB size overflow"))?;
    Ok(RasterSize {
        width: width as u32,
        height: height as u32,
        row_bytes,
        rgba_bytes,
        rgb_bytes,
    })
}

#[cfg(test)]
mod limits_tests {
    use super::*;

    #[test]
    fn dimensions_reject_invalid_or_oversized_frames_before_allocation() {
        for (width, height, dpi) in [
            (0., 100., 150.),
            (-1., 100., 150.),
            (100., 0., 150.),
            (100., 100., 0.),
            (100., 100., f64::NAN),
            (f64::INFINITY, 100., 150.),
            (f64::MAX, 100., 150.),
            (8001., 4000., 72.),
        ] {
            assert!(raster_size(width, height, dpi).is_err());
        }
        let maximum = raster_size(8000., 4000., 72.).unwrap();
        assert_eq!((maximum.width, maximum.height), (8000, 4000));
        assert_eq!(maximum.row_bytes, 32_000);
        assert_eq!(maximum.rgba_bytes, 128_000_000);
        assert_eq!(maximum.rgb_bytes, 96_000_000);
        let a4 = raster_size(600., 800., 150.).unwrap();
        assert_eq!((a4.width, a4.height), (1250, 1667));
        let letter = raster_size(612., 792., 150.).unwrap();
        assert_eq!((letter.width, letter.height), (1275, 1650));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unavailable_platform_is_explicit_without_falling_back_to_a_runtime() {
        assert!(!available());
        assert!(matches!(
            PdfRasterSession::open(b"%PDF-1.7"),
            Err(Error::Unsupported(_))
        ));
    }
}
