use crate::{Asset, Document, Error, Result};
use anydoc::model::{Block, CellSlot, ImageSource, Inline, LinkTarget};

fn conversion_error(error: anydoc::ConvertError) -> Error {
    Error::Conversion(format!("Native document conversion failed: {error}"))
}

fn escape(text: &str) -> String {
    let mut output = String::new();
    for ch in text.chars() {
        if matches!(ch, '\\' | '*' | '_' | '[' | ']' | '`') {
            output.push('\\');
        }
        output.push(ch);
    }
    output
}

fn destination(value: &str) -> String {
    value
        .replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace(['\r', '\n'], "")
}

fn image_extension(mime: &str, origin: &str) -> String {
    let known = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/avif" => "avif",
        _ => "",
    };
    if !known.is_empty() {
        return known.into();
    }
    std::path::Path::new(origin)
        .extension()
        .and_then(|s| s.to_str())
        .filter(|ext| ext.len() <= 12 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("bin")
        .to_ascii_lowercase()
}

struct Renderer<'a> {
    asset_names: &'a [String],
    merged_cells: bool,
}

impl Renderer<'_> {
    fn inlines(&self, values: &[Inline]) -> String {
        let mut output = String::new();
        for value in values {
            match value {
                Inline::Text { text, style } => {
                    if text.trim().is_empty() {
                        output.push_str(text);
                        continue;
                    }
                    let trimmed = text.trim();
                    let prefix = &text[..text.len() - text.trim_start().len()];
                    let suffix = &text[text.trim_end().len()..];
                    let mut rendered = if style.code {
                        let max_ticks =
                            trimmed.split(|c| c != '`').map(str::len).max().unwrap_or(0);
                        let ticks = "`".repeat(max_ticks + 1);
                        if trimmed.starts_with('`') || trimmed.ends_with('`') {
                            format!("{ticks} {trimmed} {ticks}")
                        } else {
                            format!("{ticks}{trimmed}{ticks}")
                        }
                    } else {
                        escape(trimmed)
                    };
                    if style.bold {
                        rendered = format!("**{rendered}**");
                    }
                    if style.italic {
                        rendered = format!("*{rendered}*");
                    }
                    if style.strike {
                        rendered = format!("~~{rendered}~~");
                    }
                    output.push_str(prefix);
                    output.push_str(&rendered);
                    output.push_str(suffix);
                }
                Inline::Link { content, target } => {
                    let label = self.inlines(content);
                    let target = match target {
                        LinkTarget::Anchor(anchor) => format!("#{}", destination(anchor)),
                        LinkTarget::External(url) | LinkTarget::Relative(url) => destination(url),
                    };
                    let safe = url::Url::parse(&target)
                        .map(|url| matches!(url.scheme(), "http" | "https" | "mailto" | "tel"))
                        .unwrap_or(true);
                    if target.is_empty() || !safe {
                        output.push_str(&label);
                    } else {
                        output.push_str(&format!("[{label}]({target})"));
                    }
                }
                Inline::Image { alt, source } => {
                    let target = match source {
                        ImageSource::Asset(id) => self
                            .asset_names
                            .get(id.0)
                            .map(|name| format!(".markitai/assets/{name}")),
                        ImageSource::External(url) => Some(destination(url)),
                        ImageSource::Unavailable => None,
                    };
                    if let Some(target) = target {
                        output.push_str(&format!("![{}]({target})", escape(alt)));
                    } else {
                        output.push_str(&escape(alt));
                    }
                }
                Inline::Anchor(anchor) => {
                    let anchor = anchor
                        .replace('&', "&amp;")
                        .replace('"', "&quot;")
                        .replace('<', "&lt;");
                    output.push_str(&format!("<a id=\"{anchor}\"></a>"));
                }
                Inline::NoteRef(id) => output.push_str(&format!("[^{}]", destination(id))),
                Inline::LineBreak => output.push_str("  \n"),
                Inline::Math(text) => output.push_str(&format!("${text}$")),
                Inline::Checkbox(checked) => {
                    output.push_str(if *checked { "[x] " } else { "[ ] " })
                }
            }
        }
        output
    }

    fn blocks(&mut self, blocks: &[Block]) -> String {
        let mut output = Vec::new();
        for block in blocks {
            let rendered = match block {
                Block::Heading {
                    level,
                    anchor,
                    content,
                } => {
                    let heading = format!(
                        "{} {}",
                        "#".repeat(usize::from((*level).clamp(1, 6))),
                        self.inlines(content)
                    );
                    if let Some(anchor) = anchor {
                        format!(
                            "{}\n{heading}",
                            self.inlines(&[Inline::Anchor(anchor.clone())])
                        )
                    } else {
                        heading
                    }
                }
                Block::Paragraph(values) => self.inlines(values),
                Block::CodeBlock { lang, text } => {
                    super::text::fence(text, lang.as_deref().unwrap_or(""))
                }
                Block::Math(text) => format!("$$\n{text}\n$$"),
                Block::Rule => "---".into(),
                Block::BlockQuote(blocks) => self
                    .blocks(blocks)
                    .lines()
                    .map(|line| format!("> {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Block::List(list) => list
                    .items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let marker = item.marker_label.clone().unwrap_or_else(|| {
                            if list.ordered() {
                                list.marker.label(list.start + index as u64)
                            } else {
                                "*".into()
                            }
                        });
                        let content = self.blocks(&item.blocks);
                        let indent = " ".repeat(marker.len() + 1);
                        let mut lines = content.lines();
                        let mut item_text = format!("{marker} {}", lines.next().unwrap_or(""));
                        for line in lines {
                            item_text.push_str(&format!("\n{indent}{line}"));
                        }
                        item_text
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Block::Table(table) => {
                    let rows = table
                        .grid
                        .iter()
                        .map(|row| {
                            row.iter()
                                .map(|slot| match slot {
                                    CellSlot::Covered { .. } => String::new(),
                                    CellSlot::Origin(cell) => {
                                        self.merged_cells |= cell.row_span > 1 || cell.col_span > 1;
                                        // Keep inline Markdown, escape table syntax only.
                                        self.blocks(&cell.blocks)
                                            .trim()
                                            .replace('|', "\\|")
                                            .replace('\n', "<br>")
                                    }
                                })
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    super::text::table(&rows, table.header_rows > 0)
                        .trim_end()
                        .to_owned()
                }
            };
            if !rendered.is_empty() {
                output.push(rendered);
            }
        }
        output.join("\n\n")
    }
}

pub(super) fn extract(bytes: &[u8], extension: &str) -> Result<Document> {
    let format = anydoc::Format::from_extension(extension)
        .ok_or_else(|| Error::Unsupported(format!("Unsupported format: {extension}")))?;
    if format == anydoc::Format::Pdf {
        let markdown = anydoc::to_markdown_bytes(bytes, format).map_err(conversion_error)?;
        let mut document = Document {
            markdown,
            ..Document::default()
        };
        document
            .metadata
            .insert("converter".into(), "pdf-inspector".into());
        document.warnings.push("PDF text and layout use the native pdf-inspector backend; PDF image assets, page screenshots and OCR are not implemented in this build.".into());
        return Ok(document);
    }
    let parsed = anydoc::to_document(bytes, format).map_err(conversion_error)?;
    let names = parsed
        .assets
        .iter()
        .map(|asset| {
            format!(
                "asset-{}.{}",
                asset.id.0 + 1,
                image_extension(&asset.media_type, &asset.origin_part)
            )
        })
        .collect::<Vec<_>>();
    let mut renderer = Renderer {
        asset_names: &names,
        merged_cells: false,
    };
    let mut markdown = renderer.blocks(&parsed.blocks);
    for note in &parsed.notes {
        let text = renderer.blocks(&note.blocks);
        let mut lines = text.lines();
        markdown.push_str(&format!(
            "\n\n[^{}]: {}",
            destination(&note.id),
            lines.next().unwrap_or("")
        ));
        for line in lines {
            markdown.push_str(&format!("\n    {line}"));
        }
    }
    let mut warnings = Vec::new();
    if renderer.merged_cells {
        warnings.push("Merged table cells are represented by their origin cell with empty covered cells in Markdown.".into());
    }
    let title = parsed.blocks.iter().find_map(|block| {
        if let Block::Heading { content, .. } = block {
            Some(anydoc::model::inlines_to_plain_text(content))
        } else {
            None
        }
    });
    let mut document = Document {
        markdown,
        warnings,
        ..Document::default()
    };
    document
        .metadata
        .insert("converter".into(), "anydoc".into());
    if let Some(title) = title.filter(|s| !s.is_empty()) {
        document.metadata.insert("title".into(), title.into());
    }
    for (asset, name) in parsed.assets.into_iter().zip(names) {
        document.assets.push(Asset {
            name,
            bytes: asset.bytes,
        });
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anydoc::model::{AssetId, Cell, Style, Table, TableKind};
    #[test]
    fn document_renderer_keeps_assets_math_notes_and_styles() {
        let names = vec!["asset-1.png".into()];
        let mut renderer = Renderer {
            asset_names: &names,
            merged_cells: false,
        };
        let result = renderer.blocks(&[
            Block::heading(1, vec![Inline::plain("Title")]),
            Block::Paragraph(vec![
                Inline::Text {
                    text: " bold ".into(),
                    style: Style {
                        bold: true,
                        ..Style::PLAIN
                    },
                },
                Inline::Image {
                    alt: "figure".into(),
                    source: ImageSource::Asset(AssetId(0)),
                },
                Inline::NoteRef("1".into()),
                Inline::Math("x^2".into()),
            ]),
            Block::Table(Table::from_rows(
                vec![vec![Cell::from_inlines(vec![Inline::plain("a|b")])]],
                0,
                TableKind::Data,
            )),
        ]);
        assert!(result.starts_with("# Title"));
        assert!(result.contains(" **bold** "));
        assert!(result.contains("![figure](.markitai/assets/asset-1.png)[^1]$x^2$"));
        assert!(result.contains("a\\|b"));
    }
    #[test]
    fn rtf_converts_locally_and_bad_pdf_fails() {
        let doc = extract(
            br"{\rtf1\ansi Hello \b world\b0\par Second paragraph}",
            "rtf",
        )
        .unwrap();
        assert!(doc.markdown.contains("**world**"));
        assert!(extract(b"this is not a PDF", "pdf").is_err());
    }
}
