//! Bounded raster decoding and shared asset preparation.

use crate::{Asset, Document, Error, Result, config, output_profiles};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgb, RgbImage};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

const MAX_PIXELS: u64 = 32_000_000;
const MAX_DECODED: u64 = 256 * 1024 * 1024;

pub(crate) struct VisionImage {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

pub fn is_image_extension(extension: &str) -> bool {
    matches!(
        extension
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str(),
        "jpeg"
            | "jpg"
            | "png"
            | "webp"
            | "gif"
            | "bmp"
            | "tiff"
            | "tif"
            | "svg"
            | "heic"
            | "heif"
            | "avif"
    )
}

fn error(error: impl std::fmt::Display) -> Error {
    Error::Conversion(format!("Cannot decode or encode image: {error}"))
}

fn decode(bytes: &[u8]) -> Result<(DynamicImage, ImageFormat)> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let format = reader
        .format()
        .ok_or_else(|| error("unrecognized image format"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_PIXELS as u32);
    limits.max_image_height = Some(MAX_PIXELS as u32);
    limits.max_alloc = Some(MAX_DECODED);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(error)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(error("decoded image exceeds 32 million pixels"));
    }
    let orientation = decoder.orientation().map_err(error)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(error)?;
    image.apply_orientation(orientation);
    Ok((image, format))
}

fn dimension(cfg: &Value, key: &str, fallback: u32) -> u32 {
    cfg.pointer(key)
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(fallback))
        .min(u64::from(u32::MAX)) as u32
}

fn filtered(image: &DynamicImage, cfg: &Value) -> bool {
    image.width() < dimension(cfg, "/image/filter/min_width", 50)
        || image.height() < dimension(cfg, "/image/filter/min_height", 50)
        || u64::from(image.width()) * u64::from(image.height())
            < cfg
                .pointer("/image/filter/min_area")
                .and_then(Value::as_u64)
                .unwrap_or(5000)
}

fn rgb_on_white(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let pixel = rgba.get_pixel(x, y).0;
        let alpha = u32::from(pixel[3]);
        Rgb([0, 1, 2].map(|channel| {
            ((u32::from(pixel[channel]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8
        }))
    })
}

fn encode(
    image: &DynamicImage,
    cfg: &Value,
    force_png: bool,
) -> Result<(Vec<u8>, &'static str, &'static str)> {
    let width = dimension(cfg, "/image/max_width", 1920).max(1);
    let height = dimension(cfg, "/image/max_height", 99999).max(1);
    let resized;
    let image = if !force_png && (image.width() > width || image.height() > height) {
        resized = image.resize(width, height, image::imageops::FilterType::Lanczos3);
        &resized
    } else {
        image
    };
    let format = if force_png {
        "png"
    } else {
        cfg.pointer("/image/format")
            .and_then(Value::as_str)
            .unwrap_or("jpeg")
    };
    let mut bytes = Vec::new();
    let (extension, mime) = match format {
        "png" => {
            image
                .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
                .map_err(error)?;
            ("png", "image/png")
        }
        "webp" => {
            // The Rust encoder is lossless; callers disclose this quality difference.
            image
                .write_to(&mut Cursor::new(&mut bytes), ImageFormat::WebP)
                .map_err(error)?;
            ("webp", "image/webp")
        }
        _ => {
            let quality = dimension(cfg, "/image/quality", 75).clamp(1, 100) as u8;
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
                .encode_image(&rgb_on_white(image))
                .map_err(error)?;
            ("jpg", "image/jpeg")
        }
    };
    Ok((bytes, extension, mime))
}

fn notice(doc: &mut Document, message: &str) {
    if !doc.warnings.iter().any(|existing| existing == message) {
        doc.warnings.push(message.into());
    }
}

/// Prepare embedded raster assets without touching the input or fetching URLs.
pub(crate) fn prepare_assets(doc: &mut Document, cfg: &Value) {
    let mut prepared = Vec::with_capacity(doc.assets.len());
    let mut seen: HashMap<_, String> = HashMap::new();
    let mut replacements = HashMap::with_capacity(doc.assets.len());
    for asset in std::mem::take(&mut doc.assets) {
        let from = format!(".markitai/assets/{}", asset.name);
        let extension = Path::new(&asset.name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if !is_image_extension(extension) && image::guess_format(&asset.bytes).is_err() {
            replacements.entry(from.clone()).or_insert(from);
            prepared.push(asset);
            continue;
        }
        let digest = Sha256::digest(&asset.bytes);
        if config::enabled(cfg, "/image/filter/deduplicate")
            && let Some(target) = seen.get(&digest)
        {
            replacements.entry(from).or_insert_with(|| target.clone());
            continue;
        }
        let (image, format) = match decode(&asset.bytes) {
            Ok(value) => value,
            Err(_) => {
                notice(
                    doc,
                    "An embedded image could not be decoded within native limits; its original asset was retained without filtering or compression.",
                );
                replacements.entry(from.clone()).or_insert(from);
                prepared.push(asset);
                continue;
            }
        };
        if filtered(&image, cfg) {
            replacements.entry(from).or_default();
            continue;
        }
        let asset = if config::enabled(cfg, "/image/compress") {
            match encode(&image, cfg, false) {
                Ok((bytes, extension, _)) => Asset {
                    name: format!("{}.{}", asset.name, extension),
                    bytes,
                },
                Err(_) => {
                    notice(
                        doc,
                        "An embedded image could not be encoded; its original asset was retained.",
                    );
                    asset
                }
            }
        } else {
            // Correct mislabeled embedded payloads even when preserving their bytes.
            let suffix = format.extensions_str()[0];
            Asset {
                name: format!("{}.{}", asset.name, suffix),
                ..asset
            }
        };
        if config::enabled(cfg, "/image/compress") && cfg["image"]["format"] == "webp" {
            notice(
                doc,
                "WebP output uses lossless native encoding; image.quality does not affect WebP in this build.",
            );
        }
        let target = format!(".markitai/assets/{}", asset.name);
        replacements.entry(from).or_insert_with(|| target.clone());
        seen.insert(digest, target);
        prepared.push(asset);
    }
    // Every key refers to the original document. A prepared name may also be
    // another input name, so applying replacements one by one would cascade.
    // Ambiguous duplicate source names retain the first asset's decision, even
    // when it keeps its original name or is filtered out.
    if replacements.iter().any(|(before, after)| before != after) {
        doc.markdown = output_profiles::rewrite_asset_references(&doc.markdown, &replacements);
    }
    doc.assets = prepared;
}

pub(crate) fn extract(path: &Path, cfg: &Value) -> Result<(Document, VisionImage)> {
    let bytes = std::fs::read(path)?;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let detected = image::guess_format(&bytes).ok();
    if detected == Some(ImageFormat::Avif)
        || detected.is_none() && matches!(extension.as_str(), "svg" | "heic" | "heif" | "avif")
    {
        let extension = if detected == Some(ImageFormat::Avif) {
            "avif"
        } else {
            &extension
        };
        return Err(Error::Unsupported(format!(
            "Native {extension} rasterization is not implemented yet"
        )));
    }
    if detected == Some(ImageFormat::Tiff) {
        let decoder = tiff::decoder::Decoder::new(Cursor::new(&bytes)).map_err(error)?;
        if decoder.more_images() {
            return Err(Error::Unsupported("Multi-page TIFF vision routing is not implemented yet; no pages were sent to a model".into()));
        }
    }
    let (image, format) = decode(&bytes)?;
    let preserve = !config::enabled(cfg, "/image/compress");
    let (vision_bytes, vision_extension, mime) = if preserve
        && matches!(
            format,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
        ) {
        (
            bytes.clone(),
            format.extensions_str()[0],
            match format {
                ImageFormat::Jpeg => "image/jpeg",
                ImageFormat::WebP => "image/webp",
                ImageFormat::Gif => "image/gif",
                _ => "image/png",
            },
        )
    } else {
        encode(&image, cfg, preserve)?
    };
    // An internal content name avoids interpreting source filename punctuation as Markdown.
    let asset_name = format!("image.{vision_extension}");
    let title = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(['\r', '\n'], " ");
    let alt = title
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    let mut doc = Document {
        markdown: format!("# {title}\n\n![{alt}](.markitai/assets/{asset_name})\n"),
        assets: vec![Asset {
            name: asset_name,
            bytes: vision_bytes.clone(),
        }],
        ..Default::default()
    };
    doc.metadata.insert("title".into(), title.into());
    if cfg["image"]["format"] == "webp" && config::enabled(cfg, "/image/compress") {
        notice(
            &mut doc,
            "WebP output uses lossless native encoding; image.quality does not affect WebP in this build.",
        );
    }
    Ok((
        doc,
        VisionImage {
            mime,
            bytes: vision_bytes,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    use serde_json::json;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        DynamicImage::new_rgba8(width, height)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn transparent_jpeg_has_white_background_and_respects_dimensions() {
        let cfg = config::normalize(&json!({"image":{"max_width":20,"max_height":20}})).unwrap();
        let original = DynamicImage::new_rgba8(100, 50);
        let (bytes, extension, mime) = encode(&original, &cfg, false).unwrap();
        let (output, _) = decode(&bytes).unwrap();
        assert_eq!((output.width(), output.height()), (20, 10));
        assert_eq!((extension, mime), ("jpg", "image/jpeg"));
        assert!(output.to_rgb8().get_pixel(0, 0).0.iter().all(|v| *v >= 250));
    }

    #[test]
    fn exif_orientation_is_applied_before_dimension_checks_and_encoding() {
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut bytes);
        // Little-endian TIFF header, one SHORT orientation entry (rotate 90 CW).
        encoder
            .set_exif_metadata(vec![
                73, 73, 42, 0, 8, 0, 0, 0, 1, 0, 18, 1, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
            ])
            .unwrap();
        encoder.encode_image(&RgbImage::new(60, 90)).unwrap();
        let (image, _) = decode(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (90, 60));
    }

    #[test]
    fn filtering_and_duplicates_preserve_literal_examples_and_shared_references() {
        let big = png(100, 100);
        let mut doc = Document {
            markdown: "![a](.markitai/assets/a.png)\n![b](.markitai/assets/b.png)\n![tiny](.markitai/assets/tiny.png)\n`![example](.markitai/assets/tiny.png)`\n".into(),
            assets: vec![Asset{name:"a.png".into(),bytes:big.clone()},Asset{name:"b.png".into(),bytes:big.clone()},Asset{name:"tiny.png".into(),bytes:png(2,2)}],
            ..Default::default()
        };
        let cfg = config::normalize(&json!({"image":{"compress":false}})).unwrap();
        prepare_assets(&mut doc, &cfg);
        assert_eq!(doc.assets.len(), 1);
        assert_eq!(doc.assets[0].bytes, big);
        assert!(!doc.markdown.contains("![tiny]"));
        assert!(
            doc.markdown
                .contains("`![example](.markitai/assets/tiny.png)`")
        );
        assert_eq!(
            doc.markdown.matches(".markitai/assets/a.png.png").count(),
            2
        );
    }

    #[test]
    fn chained_asset_names_keep_distinct_images_and_duplicates_point_to_final_asset() {
        for compress in [false, true] {
            let first = png(100, 100);
            let second = png(120, 100);
            let mut doc = Document {
                markdown: "![first](.markitai/assets/a.png)\n![second](.markitai/assets/a.png.png)\n![duplicate](.markitai/assets/copy.png)\n`![literal](.markitai/assets/a.png)`\n".into(),
                assets: vec![
                    Asset { name: "a.png".into(), bytes: first.clone() },
                    Asset { name: "a.png.png".into(), bytes: second },
                    Asset { name: "copy.png".into(), bytes: first },
                ],
                ..Default::default()
            };
            let cfg =
                config::normalize(&json!({"image":{"compress":compress,"format":"png"}})).unwrap();
            prepare_assets(&mut doc, &cfg);
            assert_eq!(
                doc.markdown,
                "![first](.markitai/assets/a.png.png)\n![second](.markitai/assets/a.png.png.png)\n![duplicate](.markitai/assets/a.png.png)\n`![literal](.markitai/assets/a.png)`\n"
            );
            assert_eq!(doc.assets.len(), 2);
            assert_eq!(doc.assets[0].name, "a.png.png");
            assert_eq!(doc.assets[1].name, "a.png.png.png");
            assert_eq!(decode(&doc.assets[0].bytes).unwrap().0.width(), 100);
            assert_eq!(decode(&doc.assets[1].bytes).unwrap().0.width(), 120);
        }
    }

    #[test]
    fn filtering_an_original_name_does_not_remove_a_prepared_image_or_its_duplicate() {
        let big = png(100, 100);
        let mut doc = Document {
            markdown: "![kept](.markitai/assets/a.png)\n![duplicate](.markitai/assets/copy.png)\n![tiny](.markitai/assets/a.png.png)\n`![literal](.markitai/assets/a.png.png)`\n".into(),
            assets: vec![
                Asset { name: "a.png".into(), bytes: big.clone() },
                Asset { name: "copy.png".into(), bytes: big.clone() },
                Asset { name: "a.png.png".into(), bytes: png(2, 2) },
            ],
            ..Default::default()
        };
        let cfg = config::normalize(&json!({"image":{"compress":false}})).unwrap();
        prepare_assets(&mut doc, &cfg);
        assert_eq!(doc.assets.len(), 1);
        assert_eq!(doc.assets[0].name, "a.png.png");
        assert_eq!(doc.assets[0].bytes, big);
        assert!(doc.markdown.contains("![kept](.markitai/assets/a.png.png)"));
        assert!(
            doc.markdown
                .contains("![duplicate](.markitai/assets/a.png.png)")
        );
        assert!(!doc.markdown.contains("![tiny]"));
        assert!(
            doc.markdown
                .contains("`![literal](.markitai/assets/a.png.png)`")
        );
        assert!(doc.warnings.is_empty());
    }

    #[test]
    fn duplicate_source_names_keep_the_first_filter_decision() {
        let big = png(100, 100);
        let tiny = png(2, 2);
        for keep_first in [true, false] {
            let input =
                "![same](.markitai/assets/same.png)\n`![literal](.markitai/assets/same.png)`\n";
            let bytes = if keep_first {
                [big.clone(), tiny.clone()]
            } else {
                [tiny.clone(), big.clone()]
            };
            let mut doc = Document {
                markdown: input.into(),
                assets: bytes
                    .into_iter()
                    .map(|bytes| Asset {
                        name: "same.png".into(),
                        bytes,
                    })
                    .collect(),
                ..Default::default()
            };
            let cfg = config::normalize(&json!({"image":{"compress":false}})).unwrap();
            prepare_assets(&mut doc, &cfg);
            assert_eq!(doc.assets.len(), 1);
            assert_eq!(doc.assets[0].bytes, big);
            assert_eq!(
                doc.markdown
                    .contains("![same](.markitai/assets/same.png.png)"),
                keep_first
            );
            assert!(
                doc.markdown
                    .contains("`![literal](.markitai/assets/same.png)`")
            );
            if !keep_first {
                assert!(!doc.markdown.contains("![same]"));
            }
        }
    }

    #[test]
    fn retained_first_asset_keeps_its_reference_when_a_later_namesake_is_an_image() {
        for name in ["same.png", "same.bin"] {
            let retained = vec![0, 1, 2];
            let input = format!("![same](.markitai/assets/{name})\n");
            let mut doc = Document {
                markdown: input.clone(),
                assets: vec![
                    Asset {
                        name: name.into(),
                        bytes: retained.clone(),
                    },
                    Asset {
                        name: name.into(),
                        bytes: png(100, 100),
                    },
                ],
                ..Default::default()
            };
            let cfg = config::normalize(&json!({"image":{"compress":false}})).unwrap();
            prepare_assets(&mut doc, &cfg);
            assert_eq!(doc.markdown, input);
            assert_eq!(doc.assets.len(), 2);
            assert_eq!(doc.assets[0].name, name);
            assert_eq!(doc.assets[0].bytes, retained);
            assert_eq!(doc.assets[1].name, format!("{name}.png"));
            assert_eq!(decode(&doc.assets[1].bytes).unwrap().0.width(), 100);
            assert_eq!(doc.warnings.len(), usize::from(name.ends_with(".png")));
        }
    }

    #[test]
    fn malformed_embedded_assets_are_retained_with_warning() {
        let mut doc = Document {
            assets: vec![Asset {
                name: "bad.png".into(),
                bytes: vec![0, 1, 2],
            }],
            ..Default::default()
        };
        prepare_assets(&mut doc, &config::defaults());
        assert_eq!(doc.assets.len(), 1);
        assert_eq!(doc.warnings.len(), 1);
        assert!(decode(&[0, 1, 2]).is_err());
    }

    #[test]
    fn actual_raster_content_takes_precedence_over_mislabeled_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = png(80, 80);
        let cfg = config::normalize(&json!({"image":{"compress":false}})).unwrap();
        for extension in ["tiff", "heic", "svg", "avif"] {
            let path = dir.path().join(format!("renamed.{extension}"));
            std::fs::write(&path, &bytes).unwrap();
            let (doc, vision) = extract(&path, &cfg).unwrap();
            assert_eq!(doc.assets[0].bytes, bytes);
            assert_eq!(vision.mime, "image/png");
        }
    }

    #[test]
    fn disabling_compression_preserves_vision_bytes_and_transcoded_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let cfg =
            config::normalize(&json!({"image":{"compress":false,"max_width":10,"max_height":10}}))
                .unwrap();
        for (extension, format) in [("jpg", ImageFormat::Jpeg), ("bmp", ImageFormat::Bmp)] {
            let path = dir.path().join(format!("wide.{extension}"));
            DynamicImage::new_rgb8(200, 100)
                .save_with_format(&path, format)
                .unwrap();
            let (_, vision) = extract(&path, &cfg).unwrap();
            let (image, _) = decode(&vision.bytes).unwrap();
            assert_eq!((image.width(), image.height()), (200, 100));
            if extension == "jpg" {
                assert_eq!(vision.bytes, std::fs::read(&path).unwrap());
            }
        }
    }

    #[test]
    fn oversized_header_is_rejected_before_pixel_allocation() {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 100_000, 100_000);
        encoder.set_color(png::ColorType::Rgb);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_chunk(
                png::chunk::IDAT,
                &[120, 156, 99, 96, 96, 96, 0, 0, 0, 4, 0, 1],
            )
            .unwrap();
        writer.finish().unwrap();
        let message = decode(&bytes).unwrap_err().to_string();
        assert!(
            message.contains("32 million") || message.contains("limit"),
            "{message}"
        );
    }
}
