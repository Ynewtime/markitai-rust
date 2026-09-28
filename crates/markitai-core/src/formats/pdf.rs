use crate::{Asset, Document, Error, Result};
use lopdf::{Dictionary, Object, ObjectId, Stream, content::Content};
use std::collections::{BTreeMap, BTreeSet};

const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_ASSET_BYTES: usize = 128 * 1024 * 1024;
const MAX_IMAGE_PIXELS: usize = 32 * 1024 * 1024;

fn conversion(message: impl std::fmt::Display) -> Error {
    Error::Conversion(format!("Native PDF conversion failed: {message}"))
}

fn decoded(stream: &Stream) -> std::result::Result<Vec<u8>, String> {
    if !stream.dict.has(b"Filter") {
        if stream.content.len() > MAX_STREAM_BYTES {
            return Err("image stream exceeds the size limit".into());
        }
        return Ok(stream.content.clone());
    }
    stream
        .decompressed_content_with_limit(MAX_STREAM_BYTES)
        .map_err(|e| e.to_string())
}

fn image_bytes(stream: &Stream) -> std::result::Result<(&'static str, Vec<u8>), String> {
    let dict = &stream.dict;
    if dict.has(b"SMask")
        || dict.has(b"Mask")
        || dict
            .get(b"ImageMask")
            .and_then(Object::as_bool)
            .unwrap_or(false)
    {
        return Err("image transparency or stencil masks require compositing".into());
    }
    if dict.has(b"Decode") {
        return Err("image sample remapping is not implemented".into());
    }
    let filters = if dict.has(b"Filter") {
        stream.filters().map_err(|e| e.to_string())?
    } else {
        Vec::new()
    };
    if filters.as_slice() == [b"DCTDecode".as_slice()] {
        if stream.content.len() > MAX_STREAM_BYTES {
            return Err("JPEG stream exceeds the size limit".into());
        }
        return Ok(("jpg", stream.content.clone()));
    }
    if filters.iter().any(|f| {
        !matches!(
            *f,
            b"FlateDecode"
                | b"ASCII85Decode"
                | b"ASCIIHexDecode"
                | b"LZWDecode"
                | b"RunLengthDecode"
        )
    }) {
        return Err(format!(
            "unsupported image filters: {}",
            filters
                .iter()
                .map(|name| String::from_utf8_lossy(name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let width = dict
        .get(b"Width")
        .and_then(Object::as_i64)
        .map_err(|e| e.to_string())?;
    let height = dict
        .get(b"Height")
        .and_then(Object::as_i64)
        .map_err(|e| e.to_string())?;
    let width = u32::try_from(width)
        .ok()
        .filter(|n| *n > 0)
        .ok_or("invalid image width")?;
    let height = u32::try_from(height)
        .ok()
        .filter(|n| *n > 0)
        .ok_or("invalid image height")?;
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .filter(|n| *n <= MAX_IMAGE_PIXELS)
        .ok_or("image dimensions exceed the pixel limit")?;
    if dict
        .get(b"BitsPerComponent")
        .and_then(Object::as_i64)
        .unwrap_or(0)
        != 8
    {
        return Err("only 8-bit image samples are currently encoded".into());
    }
    let (color, channels) = match dict
        .get(b"ColorSpace")
        .and_then(Object::as_name)
        .map_err(|_| "indirect, indexed or calibrated image color space is not implemented")?
    {
        b"DeviceRGB" => (png::ColorType::Rgb, 3),
        b"DeviceGray" => (png::ColorType::Grayscale, 1),
        _ => return Err("image color space requires color conversion".into()),
    };
    let samples = decoded(stream)?;
    let expected = pixels
        .checked_mul(channels)
        .ok_or("image sample count overflow")?;
    if samples.len() != expected {
        return Err(format!(
            "image sample length {} does not match expected {expected}",
            samples.len()
        ));
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer
            .write_image_data(&samples)
            .map_err(|e| e.to_string())?;
    }
    Ok(("png", bytes))
}

#[derive(Clone, Copy)]
struct GraphicsState {
    render_mode: i64,
    font_size: f32,
    white_fill: bool,
    white_stroke: bool,
    invisible_alpha: bool,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            render_mode: 0,
            font_size: 12.0,
            white_fill: false,
            white_stroke: false,
            invisible_alpha: false,
        }
    }
}

impl GraphicsState {
    fn suspicious(self) -> Option<&'static str> {
        if matches!(self.render_mode, 3 | 7) {
            Some("invisible text rendering mode")
        } else if self.invisible_alpha {
            Some("transparent text graphics state")
        } else if self.font_size.abs() <= 1.0 {
            Some("text at one point or smaller")
        } else if (matches!(self.render_mode, 0 | 4) && self.white_fill)
            || (matches!(self.render_mode, 1 | 5) && self.white_stroke)
            || (matches!(self.render_mode, 2 | 6) && self.white_fill && self.white_stroke)
        {
            Some("white text")
        } else {
            None
        }
    }
}

struct PageInspection {
    images: Vec<ObjectId>,
    signals: BTreeSet<&'static str>,
    warnings: Vec<String>,
    inspected_bytes: usize,
    inspected_streams: usize,
}

fn named_resource<'a>(
    pdf: &'a lopdf::Document,
    resources: &[&'a Dictionary],
    kind: &[u8],
    name: &[u8],
) -> Option<&'a Object> {
    resources
        .iter()
        .find_map(|resources| pdf.get_dict_in_dict(resources, kind).ok()?.get(name).ok())
}

fn inspect_content(
    pdf: &lopdf::Document,
    bytes: &[u8],
    resources: &[&Dictionary],
    mut state: GraphicsState,
    seen_forms: &mut BTreeSet<ObjectId>,
    depth: usize,
    out: &mut PageInspection,
) {
    if out.inspected_streams >= 256
        || bytes.len() > MAX_STREAM_BYTES.saturating_sub(out.inspected_bytes)
    {
        out.warnings
            .push("Expanded page/Form content exceeds the inspection budget.".into());
        return;
    }
    out.inspected_bytes += bytes.len();
    out.inspected_streams += 1;
    if depth > 32 {
        out.warnings
            .push("Form nesting exceeds the inspection limit.".into());
        return;
    }
    let content = match Content::decode(bytes) {
        Ok(content) => content,
        Err(error) => {
            out.warnings
                .push(format!("Content stream inspection failed: {error}"));
            return;
        }
    };
    let mut states = Vec::new();
    for operation in content.operations {
        let last = operation.operands.last();
        let name = last.and_then(|obj| obj.as_name().ok());
        match operation.operator.as_str() {
            "q" => {
                if states.len() >= 1024 {
                    out.warnings
                        .push("Graphics-state nesting exceeds the inspection limit.".into());
                    return;
                }
                states.push(state);
            }
            "Q" => {
                if let Some(saved) = states.pop() {
                    state = saved;
                }
            }
            "Tr" => {
                if let Some(mode) = last.and_then(|obj| obj.as_i64().ok()) {
                    state.render_mode = mode;
                }
            }
            "Tf" => {
                if let Some(size) = last.and_then(|obj| obj.as_float().ok()) {
                    state.font_size = size;
                }
            }
            "g" => {
                state.white_fill = last
                    .and_then(|obj| obj.as_float().ok())
                    .is_some_and(|v| v >= 0.98)
            }
            "G" => {
                state.white_stroke = last
                    .and_then(|obj| obj.as_float().ok())
                    .is_some_and(|v| v >= 0.98)
            }
            "rg" => {
                state.white_fill = operation.operands.len() == 3
                    && operation
                        .operands
                        .iter()
                        .all(|obj| obj.as_float().is_ok_and(|v| v >= 0.98))
            }
            "RG" => {
                state.white_stroke = operation.operands.len() == 3
                    && operation
                        .operands
                        .iter()
                        .all(|obj| obj.as_float().is_ok_and(|v| v >= 0.98))
            }
            "k" => {
                state.white_fill = operation.operands.len() == 4
                    && operation
                        .operands
                        .iter()
                        .all(|obj| obj.as_float().is_ok_and(|v| v <= 0.02))
            }
            "K" => {
                state.white_stroke = operation.operands.len() == 4
                    && operation
                        .operands
                        .iter()
                        .all(|obj| obj.as_float().is_ok_and(|v| v <= 0.02))
            }
            "scn" | "SCN" if name.is_some() => {
                out.warnings.push(
                    "Pattern paint may contain raster content; pattern streams are not inspected."
                        .into(),
                );
            }
            "gs" => {
                if let Some(obj) =
                    name.and_then(|name| named_resource(pdf, resources, b"ExtGState", name))
                {
                    let resolved = match obj {
                        Object::Reference(id) => pdf.get_object(*id).ok(),
                        other => Some(other),
                    };
                    if let Some(dict) = resolved.and_then(|obj| obj.as_dict().ok()) {
                        state.invisible_alpha = dict
                            .get(b"ca")
                            .and_then(Object::as_float)
                            .is_ok_and(|v| v <= 0.01);
                    }
                }
            }
            "Tj" | "TJ" | "'" | "\"" => {
                if let Some(reason) = state.suspicious() {
                    out.signals.insert(reason);
                }
            }
            "BI" | "ID" => {
                out.warnings
                    .push("Inline PDF images are not extracted.".into());
            }
            "Do" => {
                let Some(obj) =
                    name.and_then(|name| named_resource(pdf, resources, b"XObject", name))
                else {
                    continue;
                };
                let Ok(id) = obj.as_reference() else {
                    out.warnings
                        .push("A direct XObject cannot be extracted.".into());
                    continue;
                };
                let Ok(stream) = pdf.get_object(id).and_then(Object::as_stream) else {
                    continue;
                };
                match stream.dict.get(b"Subtype").and_then(Object::as_name).ok() {
                    Some(b"Image") => {
                        if !out.images.contains(&id) {
                            out.images.push(id);
                        }
                    }
                    Some(b"Form") if seen_forms.insert(id) => {
                        let nested = pdf.get_dict_in_dict(&stream.dict, b"Resources").ok();
                        let form_resources: Vec<_> = nested
                            .into_iter()
                            .chain(resources.iter().copied())
                            .collect();
                        match decoded(stream) {
                            Ok(bytes) => inspect_content(
                                pdf,
                                &bytes,
                                &form_resources,
                                state,
                                seen_forms,
                                depth + 1,
                                out,
                            ),
                            Err(error) => out
                                .warnings
                                .push(format!("Form stream inspection failed: {error}")),
                        }
                        seen_forms.remove(&id);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn inspect_page(pdf: &lopdf::Document, id: ObjectId) -> PageInspection {
    let mut out = PageInspection {
        images: Vec::new(),
        signals: BTreeSet::new(),
        warnings: Vec::new(),
        inspected_bytes: 0,
        inspected_streams: 0,
    };
    let (direct, ids) = match pdf.get_page_resources(id) {
        Ok(resources) => resources,
        Err(error) => {
            out.warnings
                .push(format!("Page resource inspection failed: {error}"));
            return out;
        }
    };
    let resources = direct
        .into_iter()
        .chain(ids.iter().filter_map(|id| pdf.get_dictionary(*id).ok()))
        .collect::<Vec<_>>();
    match pdf.get_page_content_with_limit(id, MAX_STREAM_BYTES) {
        Ok(bytes) => inspect_content(
            pdf,
            &bytes,
            &resources,
            GraphicsState::default(),
            &mut BTreeSet::new(),
            0,
            &mut out,
        ),
        Err(error) => out
            .warnings
            .push(format!("Page content inspection failed: {error}")),
    }
    out
}

fn page_markdown(page: &pdf_inspector::PageMarkdown, warnings: &mut Vec<String>) -> String {
    let number = page.page + 1;
    let marker = format!("<!-- Page number: {number} -->");
    if page.needs_ocr || page.markdown.trim().is_empty() {
        let reason = page
            .ocr_reason
            .as_deref()
            .unwrap_or("empty native text extraction");
        warnings.push(format!("PDF page {number}: native text was not recovered ({reason}); OCR is required for this page."));
        marker
    } else {
        format!("{marker}\n\n{}", page.markdown.trim())
    }
}

fn readable_fallback(text: &str) -> bool {
    if text.len() > 1024 * 1024
        || text.chars().any(|ch| {
            ch == '\u{fffd}'
                || (ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
                || matches!(ch as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
        })
    {
        return false;
    }
    let visible = text.chars().filter(|ch| !ch.is_whitespace()).count();
    let alphanumeric = text.chars().filter(|ch| ch.is_alphanumeric()).count();
    alphanumeric >= 20 && alphanumeric * 100 >= visible * 55
}

fn recover_plain_text(
    pdf: &lopdf::Document,
    number: u32,
    page: &mut pdf_inspector::PageMarkdown,
    inspection: &PageInspection,
    warnings: &mut Vec<String>,
) {
    // The layout API also rejects pages for unnamed formatting/GID heuristics.
    // An unused image resource can also cause a false scan verdict. Only recover
    // when executed content has no raster, hidden text or uninspected streams.
    if !page.needs_ocr
        || page
            .ocr_reason
            .as_deref()
            .is_some_and(|reason| reason != "scanned")
        || !inspection.signals.is_empty()
        || !inspection.warnings.is_empty()
        || !inspection.images.is_empty()
    {
        return;
    }
    let Some(&page_id) = pdf.get_pages().get(&number) else {
        return;
    };
    let Ok(fonts) = pdf.get_page_fonts(page_id) else {
        return;
    };
    if fonts.values().any(|font| {
        font.get(b"Subtype")
            .and_then(Object::as_name)
            .is_ok_and(|name| name == b"Type3")
    }) {
        return;
    }
    let Ok(text) = pdf.extract_text_with_limit(&[number], MAX_STREAM_BYTES) else {
        return;
    };
    if !readable_fallback(&text) {
        return;
    }
    page.markdown = super::escape(text.trim());
    page.needs_ocr = false;
    let reason = if page.ocr_reason.as_deref() == Some("scanned") {
        "a scan verdict from declared image resources, with no executed raster content"
    } else {
        "an unnamed layout/GID verdict"
    };
    warnings.push(format!("PDF page {number}: recovered bounded font-decoded text after {reason}. Reading order, paragraph boundaries and text styling may differ."));
}

pub(super) fn extract(bytes: &[u8]) -> Result<Document> {
    let pdf = lopdf::Document::load_mem(bytes).map_err(conversion)?;
    let page_ids = pdf.get_pages();
    if page_ids.is_empty() {
        return Err(conversion("document contains no pages"));
    }
    let mut document = Document::default();
    let extracted = match pdf_inspector::extract_pages_markdown_mem(bytes, None) {
        Ok(result) => result.pages,
        Err(error) => {
            document.warnings.push(format!(
                "Whole-document extraction failed ({error}); pages were retried independently."
            ));
            page_ids
                .keys()
                .map(|&page| {
                    pdf_inspector::extract_pages_markdown_mem(bytes, Some(&[page - 1]))
                        .ok()
                        .and_then(|mut result| result.pages.pop())
                        .unwrap_or(pdf_inspector::PageMarkdown {
                            page: page - 1,
                            markdown: String::new(),
                            needs_ocr: true,
                            ocr_reason: Some("page extraction failed".into()),
                        })
                })
                .collect()
        }
    };
    let mut pages = extracted
        .into_iter()
        .map(|page| (page.page + 1, page))
        .collect::<BTreeMap<_, _>>();
    let mut image_names = BTreeMap::<ObjectId, Option<String>>::new();
    let mut total_asset_bytes = 0;
    let mut sections = Vec::new();
    let mut readable_pages = 0;
    for (&number, &id) in &page_ids {
        let mut page = pages
            .remove(&number)
            .unwrap_or(pdf_inspector::PageMarkdown {
                page: number - 1,
                markdown: String::new(),
                needs_ocr: true,
                ocr_reason: Some("page absent from extraction result".into()),
            });
        let inspection = inspect_page(&pdf, id);
        recover_plain_text(&pdf, number, &mut page, &inspection, &mut document.warnings);
        readable_pages += usize::from(!page.needs_ocr && !page.markdown.trim().is_empty());
        let mut section = page_markdown(&page, &mut document.warnings);
        for warning in inspection.warnings {
            document
                .warnings
                .push(format!("PDF page {number}: {warning}"));
        }
        if !inspection.signals.is_empty() {
            document.warnings.push(format!("PDF page {number}: contains {}; the native reader applies its own visibility heuristics, and complete hidden-text filtering is not established.", inspection.signals.into_iter().collect::<Vec<_>>().join(", ")));
        }
        for image_id in inspection.images {
            let name = image_names.entry(image_id).or_insert_with(|| {
                let extracted = pdf
                    .get_object(image_id)
                    .and_then(Object::as_stream)
                    .map_err(|e| e.to_string())
                    .and_then(image_bytes);
                match extracted {
                    Ok((extension, bytes))
                        if total_asset_bytes + bytes.len() <= MAX_ASSET_BYTES =>
                    {
                        total_asset_bytes += bytes.len();
                        let name = format!("pdf-image-{}-{}.{}", image_id.0, image_id.1, extension);
                        document.assets.push(Asset {
                            name: name.clone(),
                            bytes,
                        });
                        Some(name)
                    }
                    Ok(_) => {
                        document.warnings.push(format!(
                            "PDF page {number}: image {} exceeds the total asset size limit.",
                            image_id.0
                        ));
                        None
                    }
                    Err(error) => {
                        document.warnings.push(format!(
                            "PDF page {number}: image {} was not extracted ({error}).",
                            image_id.0
                        ));
                        None
                    }
                }
            });
            if let Some(name) = name {
                section.push_str(&format!(
                    "\n\n![Image on page {number}](.markitai/assets/{name})"
                ));
            }
        }
        sections.push(section);
    }
    if readable_pages == 0 && document.assets.is_empty() {
        return Err(conversion(format!(
            "no reliable native text or extractable images; {}",
            document.warnings.join(" ")
        )));
    }
    document.markdown = sections.join("\n\n");
    document
        .metadata
        .insert("converter".into(), "pdf-inspector".into());
    if let Ok(info) = pdf
        .trailer
        .get(b"Info")
        .and_then(Object::as_reference)
        .and_then(|id| pdf.get_dictionary(id))
        && let Ok(title) = info.get(b"Title").and_then(lopdf::decode_text_string)
        && !title.trim().is_empty()
    {
        document
            .metadata
            .insert("title".into(), title.trim().into());
    }
    document.warnings.push("PDF images are appended to their source page; exact placement, page screenshots, vector graphics and local OCR are not implemented.".into());
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{content::Operation, dictionary};

    #[test]
    fn real_pdf_recovers_text_despite_unused_image_and_keeps_blank_page_warning() {
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages_id = pdf.new_object_id();
        let font = pdf.add_object(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        );
        let unused_image = pdf.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 2000, "Height" => 2000, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "DCTDecode" }, vec![0xff, 0xd8, 0xff, 0xd9]));
        let resources = pdf.add_object(dictionary! { "Font" => dictionary! { "F1" => font }, "XObject" => dictionary! { "Unused" => unused_image } });
        let content = Content { operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 12.into()]),
            Operation::new("Td", vec![40.into(), 700.into()]),
            Operation::new("Tj", vec![Object::string_literal("This page has readable native text which must survive a blank second page.")]),
            Operation::new("ET", vec![]),
        ] }.encode().unwrap();
        let text_id = pdf.add_object(Stream::new(Dictionary::new(), content));
        let first = pdf.add_object(
            dictionary! { "Type" => "Page", "Parent" => pages_id, "Contents" => text_id, "Resources" => resources },
        );
        let second = pdf.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id });
        pdf.objects.insert(pages_id, dictionary! { "Type" => "Pages", "Kids" => vec![first.into(), second.into()], "Count" => 2, "Resources" => resources, "MediaBox" => vec![0.into(),0.into(),612.into(),792.into()] }.into());
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        pdf.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        pdf.save_to(&mut bytes).unwrap();
        let initial = pdf_inspector::extract_pages_markdown_mem(&bytes, Some(&[0])).unwrap();
        assert!(initial.pages[0].needs_ocr);
        assert_eq!(initial.pages[0].ocr_reason.as_deref(), Some("scanned"));
        let result = extract(&bytes).unwrap();
        assert!(result.markdown.contains("readable native text"));
        assert!(result.assets.is_empty());
        assert!(result.warnings.iter().any(|warning| {
            warning.contains("declared image resources, with no executed raster content")
        }));
        assert!(result.markdown.contains("<!-- Page number: 2 -->"));
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.starts_with("PDF page 2: native text was not recovered"))
        );
    }
    #[test]
    fn partial_text_keeps_page_markers_and_precise_ocr_warning() {
        let mut warnings = Vec::new();
        let good = page_markdown(
            &pdf_inspector::PageMarkdown {
                page: 0,
                markdown: "Readable text".into(),
                needs_ocr: false,
                ocr_reason: None,
            },
            &mut warnings,
        );
        let missing = page_markdown(
            &pdf_inspector::PageMarkdown {
                page: 2,
                markdown: "unreliable".into(),
                needs_ocr: true,
                ocr_reason: Some("image-only".into()),
            },
            &mut warnings,
        );
        assert_eq!(good, "<!-- Page number: 1 -->\n\nReadable text");
        assert_eq!(missing, "<!-- Page number: 3 -->");
        assert_eq!(
            warnings,
            [
                "PDF page 3: native text was not recovered (image-only); OCR is required for this page."
            ]
        );
    }

    #[test]
    fn decoded_fallback_rejects_empty_control_and_unmapped_glyphs() {
        assert!(readable_fallback(
            "Visible text with enough letters to recover a rejected page."
        ));
        assert!(readable_fallback(
            "这是具有足够原生文本的页面，恢复文本内容需要保留全部字符和清晰警告。"
        ));
        assert!(!readable_fallback(""));
        assert!(!readable_fallback("Short"));
        assert!(!readable_fallback(
            "A long but damaged text with \u{fffd} replacement glyph"
        ));
        assert!(!readable_fallback(
            "A long but damaged text with \u{e042} private glyph"
        ));
        assert!(!readable_fallback(
            "A long but damaged text with \u{1} control glyph"
        ));
    }
    #[test]
    fn rgb_image_preserves_pixels_and_masks_are_explicit() {
        let mut stream = Stream::new(
            dictionary! { "Width" => 2, "Height" => 1, "BitsPerComponent" => 8, "ColorSpace" => "DeviceRGB" },
            vec![255, 0, 0, 0, 0, 255],
        );
        let (extension, bytes) = image_bytes(&stream).unwrap();
        assert_eq!(extension, "png");
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut decoded).unwrap();
        assert_eq!(decoded, stream.content);
        stream.dict.set("SMask", Object::Reference((99, 0)));
        assert!(image_bytes(&stream).unwrap_err().contains("compositing"));
    }
    #[test]
    fn invisible_text_and_used_images_are_inspected_inside_forms() {
        let mut pdf = lopdf::Document::with_version("1.7");
        let image = pdf.add_object(Stream::new(dictionary! { "Subtype" => "Image" }, vec![1]));
        let content = Content {
            operations: vec![
                Operation::new("Tr", vec![3.into()]),
                Operation::new("Tj", vec![Object::string_literal("hidden")]),
                Operation::new("Do", vec![Object::Name(b"Im".to_vec())]),
            ],
        }
        .encode()
        .unwrap();
        let form = pdf.add_object(Stream::new(dictionary! { "Subtype" => "Form", "Resources" => dictionary! { "XObject" => dictionary! { "Im" => image } } }, content));
        let resources =
            dictionary! { "XObject" => dictionary! { "Fm" => form, "Unused" => image } };
        let content = Content {
            operations: vec![Operation::new("Do", vec![Object::Name(b"Fm".to_vec())])],
        }
        .encode()
        .unwrap();
        let mut output = PageInspection {
            images: Vec::new(),
            signals: BTreeSet::new(),
            warnings: Vec::new(),
            inspected_bytes: 0,
            inspected_streams: 0,
        };
        inspect_content(
            &pdf,
            &content,
            &[&resources],
            GraphicsState::default(),
            &mut BTreeSet::new(),
            0,
            &mut output,
        );
        assert_eq!(output.images, [image]);
        assert!(output.signals.contains("invisible text rendering mode"));
        assert!(output.warnings.is_empty());
    }
}
