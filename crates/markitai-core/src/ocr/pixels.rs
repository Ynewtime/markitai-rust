use super::{Result, failure};
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Rgb, RgbImage};
use std::io::{Cursor, Write};

const MAX_INPUT: usize = 64 * 1024 * 1024;
const MAX_PIXELS: u64 = 32_000_000;
const MAX_DECODED: u64 = 256 * 1024 * 1024;
const MAX_ENCODED: usize = 128 * 1024 * 1024;

pub(super) struct Prepared {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

struct Bounded(Vec<u8>);

impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_ENCODED.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other(
                "OCR image exceeds the encoded size limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn prepare(bytes: &[u8]) -> Result<Prepared> {
    if bytes.is_empty() || bytes.len() > MAX_INPUT {
        return Err(failure("image input is empty or exceeds 64 MiB"));
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| failure("cannot identify image encoding"))?;
    if reader.format() == Some(ImageFormat::Tiff) {
        let tiff = tiff::decoder::Decoder::new(Cursor::new(bytes))
            .map_err(|_| failure("cannot decode TIFF image"))?;
        if tiff.more_images() {
            return Err(crate::Error::Unsupported(
                "Local OCR of multi-page TIFF images is not implemented".into(),
            ));
        }
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PIXELS as u32);
    limits.max_image_height = Some(MAX_PIXELS as u32);
    limits.max_alloc = Some(MAX_DECODED);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| failure("cannot decode image"))?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(failure("decoded image exceeds 32 million pixels"));
    }
    let orientation = decoder
        .orientation()
        .map_err(|_| failure("cannot read image orientation"))?;
    let mut image =
        DynamicImage::from_decoder(decoder).map_err(|_| failure("cannot decode image pixels"))?;
    image.apply_orientation(orientation);
    let rgb = if image.color().has_alpha() {
        let rgba = image.into_rgba8();
        RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
            let pixel = rgba.get_pixel(x, y).0;
            let alpha = u32::from(pixel[3]);
            Rgb([0, 1, 2].map(|channel| {
                ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8
            }))
        })
    } else {
        image.into_rgb8()
    };
    let (width, height) = rgb.dimensions();
    let mut png = Bounded(Vec::new());
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(rgb.as_raw(), width, height, image::ExtendedColorType::Rgb8)
    .map_err(|_| failure("cannot prepare bounded OCR image"))?;
    Ok(Prepared {
        png: png.0,
        width,
        height,
    })
}

#[cfg(test)]
pub(super) fn encode_test_image(image: DynamicImage) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    image.write_to(&mut buffer, ImageFormat::Png).unwrap();
    buffer.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_preserves_resolution_and_composites_alpha_on_white() {
        let rgba = image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([0, 0, 0, 0])
            } else {
                image::Rgba([200, 10, 20, 128])
            }
        });
        let result = prepare(&encode_test_image(DynamicImage::ImageRgba8(rgba))).unwrap();
        assert_eq!((result.width, result.height), (2, 1));
        let decoded = image::load_from_memory(&result.png).unwrap().into_rgb8();
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 255, 255]);
        assert_eq!(decoded.get_pixel(1, 0).0, [227, 132, 137]);
        let text = prepare(include_bytes!("fixtures/english.png")).unwrap();
        assert_eq!((text.width, text.height), (819, 301));
    }

    #[test]
    fn malformed_and_oversized_images_fail_before_pixel_allocation() {
        assert!(prepare(&[]).is_err());
        assert!(prepare(b"no image").is_err());
        let mut header = vec![0u8; 54];
        header[..2].copy_from_slice(b"BM");
        header[10..14].copy_from_slice(&54u32.to_le_bytes());
        header[14..18].copy_from_slice(&40u32.to_le_bytes());
        header[18..22].copy_from_slice(&100_000i32.to_le_bytes());
        header[22..26].copy_from_slice(&100_000i32.to_le_bytes());
        header[26..28].copy_from_slice(&1u16.to_le_bytes());
        header[28..30].copy_from_slice(&24u16.to_le_bytes());
        assert!(prepare(&header).is_err());
    }
}
