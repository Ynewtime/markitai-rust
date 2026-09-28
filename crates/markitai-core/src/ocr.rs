//! Local, bounded OCR. Platform engines receive pixels, never source paths.

use crate::{Error, Result};
use serde_json::Value;

#[cfg(any(target_os = "macos", test))]
mod pixels;
#[cfg(target_os = "macos")]
mod vision;

#[derive(Debug)]
pub(crate) struct OcrResult {
    pub text: String,
    /// Mean confidence of nonempty recognized lines.
    #[cfg(any(target_os = "macos", test))]
    pub confidence: f32,
    /// Pixel rectangles [left, top, right, bottom], after orientation correction.
    #[cfg(any(target_os = "macos", test))]
    pub boxes: Vec<[f32; 4]>,
}

pub(crate) fn available() -> bool {
    #[cfg(target_os = "macos")]
    return objc2::available!(macos = 11.0);
    #[cfg(not(target_os = "macos"))]
    false
}

pub(crate) fn backend() -> &'static str {
    if available() { "vision" } else { "unavailable" }
}

pub(crate) fn recognize(bytes: &[u8], cfg: &Value) -> Result<OcrResult> {
    if !available() {
        return Err(Error::Unsupported(
            "Local OCR requires macOS 11 or later; no local OCR backend is available on this platform"
                .into(),
        ));
    }
    #[cfg(target_os = "macos")]
    {
        let language = language(cfg)?;
        let image = pixels::prepare(bytes)?;
        let lines = vision::recognize(&image, &language)?;
        assemble(lines, image.width, image.height)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (bytes, cfg);
        Err(Error::Unsupported(
            "Local OCR backend is unavailable".into(),
        ))
    }
}

#[cfg(any(target_os = "macos", test))]
const MAX_LINES: usize = 10_000;
#[cfg(any(target_os = "macos", test))]
const MAX_TEXT: usize = 8 * 1024 * 1024;

#[cfg(any(target_os = "macos", test))]
#[derive(Debug)]
struct Line {
    text: String,
    confidence: f32,
    bounds: [f32; 4],
}

#[cfg(any(target_os = "macos", test))]
fn failure(message: &str) -> Error {
    Error::Conversion(format!("Local OCR: {message}"))
}

#[cfg(any(target_os = "macos", test))]
fn language(cfg: &Value) -> Result<String> {
    let configured = cfg
        .pointer("/ocr/lang")
        .and_then(Value::as_str)
        .unwrap_or("en");
    if configured.len() > 48 {
        return Err(Error::Config(
            "ocr.lang is not a supported language identifier".into(),
        ));
    }
    let normalized = configured.trim().to_ascii_lowercase().replace('_', "-");
    if normalized.is_empty()
        || !normalized
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    {
        return Err(Error::Config(
            "ocr.lang is not a supported language identifier".into(),
        ));
    }
    Ok(match normalized.as_str() {
        "zh" | "zh-cn" | "cn" | "ch" | "zh-hans" => "zh-Hans",
        "zh-tw" | "cht" | "chinese-cht" | "zh-hant" => "zh-Hant",
        "ja" | "jp" | "japan" => "ja-JP",
        "ko" | "korean" => "ko-KR",
        "ar" | "arabic" => "ar-SA",
        "en" => "en-US",
        "fr" | "french" => "fr-FR",
        "de" | "german" => "de-DE",
        "es" => "es-ES",
        "it" => "it-IT",
        "pt" => "pt-BR",
        _ => &normalized,
    }
    .to_owned())
}

#[cfg(any(target_os = "macos", test))]
fn supported_language(requested: &str, supported: &[String]) -> Result<String> {
    if let Some(exact) = supported
        .iter()
        .find(|tag| tag.eq_ignore_ascii_case(requested))
    {
        return Ok(exact.clone());
    }
    // Short ISO codes can select the engine's regional spelling; an explicitly
    // requested region/script is never silently replaced by another language.
    if !requested.contains('-') {
        let prefix = format!("{}-", requested.to_ascii_lowercase());
        if let Some(regional) = supported
            .iter()
            .find(|tag| tag.to_ascii_lowercase().starts_with(&prefix))
        {
            return Ok(regional.clone());
        }
    }
    Err(Error::Unsupported(
        "ocr.lang is not supported by the installed macOS Vision text recognizer".into(),
    ))
}

#[cfg(any(target_os = "macos", test))]
fn pixel_bounds(rectangle: [f64; 4], width: u32, height: u32) -> Result<[f32; 4]> {
    let [x, y, w, h] = rectangle;
    if rectangle.iter().any(|n| !n.is_finite()) || w < 0.0 || h < 0.0 {
        return Err(failure("recognizer returned invalid text geometry"));
    }
    Ok([
        (x.clamp(0.0, 1.0) * f64::from(width)) as f32,
        ((1.0 - y - h).clamp(0.0, 1.0) * f64::from(height)) as f32,
        ((x + w).clamp(0.0, 1.0) * f64::from(width)) as f32,
        ((1.0 - y).clamp(0.0, 1.0) * f64::from(height)) as f32,
    ])
}

#[cfg(any(target_os = "macos", test))]
fn assemble(mut lines: Vec<Line>, width: u32, height: u32) -> Result<OcrResult> {
    if lines.len() > MAX_LINES {
        return Err(failure("recognized line count exceeds the safety limit"));
    }
    lines.retain(|line| !line.text.trim().is_empty());
    let mut bytes = 0usize;
    for line in &mut lines {
        let [left, top, right, bottom] = line.bounds;
        if !line.confidence.is_finite()
            || !(0.0..=1.0).contains(&line.confidence)
            || line.bounds.iter().any(|n| !n.is_finite())
            || left < 0.0
            || top < 0.0
            || right < left
            || bottom < top
            || right > width as f32
            || bottom > height as f32
        {
            return Err(failure("recognizer returned invalid text observations"));
        }
        // Text is ordinary document content; do not synthesize Markdown markup.
        line.text = line.text.trim().to_owned();
        bytes = bytes
            .checked_add(line.text.len() + 2)
            .filter(|total| *total <= MAX_TEXT)
            .ok_or_else(|| failure("recognized text exceeds the safety limit"))?;
    }
    lines.sort_by(|a, b| {
        a.bounds[1]
            .total_cmp(&b.bounds[1])
            .then_with(|| a.bounds[0].total_cmp(&b.bounds[0]))
    });
    let mut text = String::with_capacity(bytes);
    let mut boxes = Vec::with_capacity(lines.len());
    let mut confidence = 0.0;
    let mut index = 0;
    let mut previous: Option<[f32; 4]> = None;
    while index < lines.len() {
        let first = lines[index].bounds;
        let mut end = index + 1;
        // Group only lines sharing most of the same vertical band. Each line is
        // visited once; wide multi-column pages do not trigger pairwise scans.
        while end < lines.len() {
            let next = lines[end].bounds;
            let overlap = first[3].min(next[3]) - first[1].max(next[1]);
            let min_height = (first[3] - first[1]).min(next[3] - next[1]);
            if overlap <= 0.0 || overlap < min_height * 0.6 {
                break;
            }
            end += 1;
        }
        lines[index..end].sort_by(|a, b| a.bounds[0].total_cmp(&b.bounds[0]));
        let mut row = first;
        if let Some(last) = previous {
            text.push('\n');
            if first[1] - last[3] > 0.8 * (first[3] - first[1]).max(last[3] - last[1]) {
                text.push('\n');
            }
        }
        for (offset, line) in lines[index..end].iter().enumerate() {
            if offset > 0 {
                text.push(' ');
            }
            text.push_str(&line.text);
            boxes.push(line.bounds);
            confidence += line.confidence;
            row[1] = row[1].min(line.bounds[1]);
            row[3] = row[3].max(line.bounds[3]);
        }
        previous = Some(row);
        index = end;
    }
    let confidence = if boxes.is_empty() {
        0.0
    } else {
        confidence / boxes.len() as f32
    };
    let result = OcrResult {
        text,
        confidence,
        boxes,
    };
    // Keep diagnostics private to the native API, while validating the engine's
    // blank-image distinction before exposing text to the conversion pipeline.
    if (result.text.is_empty() != result.boxes.is_empty()) || !result.confidence.is_finite() {
        return Err(failure(
            "recognizer returned inconsistent text observations",
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn language_aliases_preserve_script_and_reject_unavailable_languages() {
        for (input, expected) in [
            ("zh_cn", "zh-Hans"),
            ("chinese_cht", "zh-Hant"),
            ("JP", "ja-JP"),
            ("korean", "ko-KR"),
            ("french", "fr-FR"),
        ] {
            assert_eq!(language(&json!({"ocr":{"lang":input}})).unwrap(), expected);
        }
        let supported = vec!["en-US".into(), "zh-Hans".into(), "vi-VN".into()];
        assert_eq!(supported_language("vi", &supported).unwrap(), "vi-VN");
        assert!(supported_language("zh-Hant", &supported).is_err());
        assert!(supported_language("auto", &supported).is_err());
        assert!(language(&json!({"ocr":{"lang":"../../en"}})).is_err());
        assert!(language(&json!({"ocr":{"lang":""}})).is_err());
    }

    #[test]
    fn normalized_boxes_use_top_left_pixels_and_reject_invalid_geometry() {
        assert_eq!(
            pixel_bounds([0.1, 0.6, 0.5, 0.2], 100, 200).unwrap(),
            [10., 40., 60., 80.]
        );
        assert!(pixel_bounds([f64::NAN, 0., 1., 1.], 10, 10).is_err());
        assert!(pixel_bounds([0., 0., -1., 1.], 10, 10).is_err());
    }

    #[test]
    fn lines_keep_rows_paragraphs_confidence_and_blank_image_distinction() {
        let line = |text: &str, bounds, confidence| Line {
            text: text.into(),
            bounds,
            confidence,
        };
        let result = assemble(
            vec![
                line(" second paragraph ", [0., 70., 90., 80.], 0.8),
                line("right", [55., 10., 95., 20.], 0.6),
                line("left", [0., 11., 40., 21.], 1.0),
                line("next line", [0., 25., 90., 35.], 0.6),
            ],
            100,
            100,
        )
        .unwrap();
        assert_eq!(result.text, "left right\nnext line\n\nsecond paragraph");
        assert!((result.confidence - 0.75).abs() < 0.001);
        assert_eq!(result.boxes.len(), 4);
        let blank = assemble(Vec::new(), 100, 100).unwrap();
        assert!(blank.text.is_empty() && blank.boxes.is_empty());
        assert_eq!(blank.confidence, 0.0);
        assert!(assemble(vec![line("bad", [0., 0., 1000., 1.], 1.)], 10, 10).is_err());
        assert!(assemble(vec![line("bad", [0., 0., 1., 1.], f32::NAN)], 10, 10).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn vision_recognizes_authored_pixels_and_reports_a_real_blank_image() {
        let config = json!({"ocr":{"lang":"en"}});
        let result = recognize(include_bytes!("ocr/fixtures/english.png"), &config).unwrap();
        let expected = include_str!("ocr/fixtures/english.txt");
        assert_eq!(
            result.text.split_whitespace().collect::<Vec<_>>(),
            expected.split_whitespace().collect::<Vec<_>>()
        );
        assert!(result.confidence > 0.3 && result.confidence <= 1.0);
        assert!(result.boxes.len() >= 3);
        let blank = pixels::encode_test_image(image::DynamicImage::ImageRgb8(
            image::RgbImage::from_pixel(400, 200, image::Rgb([255; 3])),
        ));
        let result = recognize(&blank, &config).unwrap();
        assert!(result.text.is_empty() && result.boxes.is_empty());
        assert_eq!(result.confidence, 0.0);
        assert!(matches!(
            recognize(b"not an image", &config),
            Err(Error::Conversion(_))
        ));
        assert!(matches!(
            recognize(
                include_bytes!("ocr/fixtures/english.png"),
                &json!({"ocr":{"lang":"not-a-language"}})
            ),
            Err(Error::Unsupported(_))
        ));
    }
}
