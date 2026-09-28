//! Local, deterministic format adapters. No adapter performs network requests.

mod html;
mod markup;
mod msg;
mod native;
mod text;

pub use html::extract_html;
#[cfg(test)]
pub(crate) use native::pdf::extract_pages as extract_pdf_pages;
pub(crate) use native::pdf::{
    PdfPages, extract_pages_bounded as extract_pdf_pages_bounded,
    screenshot_reference as pdf_screenshot_reference,
};

use crate::{Document, Error, Result};
use std::path::Path;

pub(crate) fn extract_pdf(bytes: &[u8]) -> Result<Document> {
    native::extract(bytes, "pdf")
}

/// Extensions with an implemented local reader (leading dots are accepted).
pub fn supports_extension(extension: &str) -> bool {
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "txt"
            | "md"
            | "markdown"
            | "html"
            | "htm"
            | "xhtml"
            | "csv"
            | "tsv"
            | "ipynb"
            | "json"
            | "xml"
            | "eml"
            | "msg"
            | "rst"
            | "org"
            | "tex"
            | "latex"
    ) || anydoc::Format::from_extension(&extension).is_some()
}

/// Extract a local document without modifying the input or writing assets.
pub fn extract(path: &Path) -> Result<Document> {
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !supports_extension(&extension) {
        return Err(Error::Unsupported(format!(
            "Unsupported file format: '{}'. This Rust build supports text, Markdown, HTML, CSV/TSV, JSON/XML, notebooks, EML/MSG email, PDF, Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, Org, RST and TeX. Image OCR is available through the conversion API on supported platforms; the Numbers reader is not implemented yet.",
            extension
        )));
    }
    let bytes = std::fs::read(path)?;
    let mut result = match extension.as_str() {
        "txt" | "md" | "markdown" => Document {
            markdown: text::decode(&bytes)?,
            ..Document::default()
        },
        "html" | "htm" | "xhtml" => extract_html(&text::decode(&bytes)?, None)?,
        "csv" | "tsv" => text::delimited(
            &text::decode(&bytes)?,
            if extension == "tsv" { b'\t' } else { b',' },
        )?,
        "ipynb" => text::notebook(&text::decode(&bytes)?)?,
        "json" => text::json(&text::decode(&bytes)?)?,
        "xml" => text::xml(&text::decode(&bytes)?)?,
        "eml" => text::email(&bytes)?,
        "msg" => msg::extract(&bytes)?,
        "rst" | "org" | "tex" | "latex" => markup::extract(&text::decode(&bytes)?, &extension)?,
        _ => native::extract(&bytes, &extension)?,
    };
    result
        .metadata
        .insert("source".into(), path.to_string_lossy().as_ref().into());
    result
        .metadata
        .insert("format".into(), extension.to_uppercase().into());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_support_does_not_promise_unimplemented_readers() {
        for extension in [
            ".DOCX", "pdf", "pptx", "xls", "ods", "eml", "markdown", "tsv", "org", "rst", "tex",
            "latex", "msg",
        ] {
            assert!(supports_extension(extension), "{extension}");
        }
        for extension in ["", "exe", "png", "heic", "numbers"] {
            assert!(!supports_extension(extension), "{extension}");
        }
    }

    #[test]
    fn text_preserves_frontmatter_and_non_ascii() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hello.MD");
        std::fs::write(&path, "---\ntitle: 原题\n---\n\n# 原题\n\n正文\n").unwrap();
        let result = extract(&path).unwrap();
        assert_eq!(result.markdown, std::fs::read_to_string(&path).unwrap());
        assert!(!result.metadata.contains_key("title"));
    }

    #[test]
    fn unsupported_and_corrupt_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let unknown = dir.path().join("file.unknown");
        assert!(matches!(extract(&unknown), Err(Error::Unsupported(_))));
        let broken = dir.path().join("file.docx");
        std::fs::write(&broken, "not a document").unwrap();
        assert!(matches!(extract(&broken), Err(Error::Conversion(_))));
    }
}
