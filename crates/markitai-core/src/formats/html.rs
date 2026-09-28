use crate::{Document, Error, Result};
use scraper::{ElementRef, Html, Selector};
use serde_json::Map;
use url::Url;

fn selector(query: &str) -> Selector {
    Selector::parse(query).expect("static selector")
}

fn plain(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn meta(document: &Html, keys: &[&str]) -> Option<String> {
    for key in keys {
        for element in document.select(&selector("meta")) {
            let name = element
                .value()
                .attr("property")
                .or_else(|| element.value().attr("name"));
            if name.is_some_and(|name| name.eq_ignore_ascii_case(key))
                && let Some(value) = element
                    .value()
                    .attr("content")
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
            {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn escaped(value: &str, output: &mut String) {
    for ch in value.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            _ => output.push(ch),
        }
    }
}

fn safe_url(value: &str, base: Option<&Url>) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with('#') {
        return Some(value.to_owned());
    }
    if let Ok(url) = Url::parse(value) {
        return matches!(url.scheme(), "http" | "https" | "mailto" | "tel")
            .then(|| url.to_string());
    }
    if value.chars().any(|c| c.is_control()) {
        return None;
    }
    if let Some(base) = base {
        return base
            .join(value)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| url.to_string());
    }
    (!value.contains(':')).then(|| value.to_owned())
}

fn is_hidden(element: ElementRef<'_>) -> bool {
    let value = element.value();
    let style = value
        .attr("style")
        .unwrap_or("")
        .to_ascii_lowercase()
        .replace(char::is_whitespace, "");
    value.attr("hidden").is_some()
        || style.contains("display:none")
        || style.contains("visibility:hidden")
}

fn serialize_clean(
    element: ElementRef<'_>,
    base: Option<&Url>,
    output: &mut String,
    depth: usize,
) -> Result<()> {
    if depth > 256 {
        return Err(Error::Conversion(
            "HTML nesting exceeds 256 elements".into(),
        ));
    }
    let value = element.value();
    let name = value.name();
    if matches!(
        name,
        "script"
            | "style"
            | "nav"
            | "footer"
            | "form"
            | "button"
            | "input"
            | "select"
            | "textarea"
            | "iframe"
            | "object"
            | "embed"
            | "head"
            | "template"
    ) || is_hidden(element)
    {
        return Ok(());
    }
    // Canonicalize lazy images without retaining arbitrary event or style attributes.
    let src = value
        .attr("data-src")
        .or_else(|| value.attr("data-original"))
        .or_else(|| value.attr("src"));
    output.push('<');
    output.push_str(name);
    for attribute in [
        "href", "src", "alt", "title", "colspan", "rowspan", "start", "class", "id",
    ] {
        let raw = if attribute == "src" && name == "img" {
            src
        } else {
            value.attr(attribute)
        };
        if let Some(raw) = raw {
            let normalized = if matches!(attribute, "href" | "src") {
                safe_url(raw, base)
            } else {
                Some(raw.to_owned())
            };
            if let Some(normalized) = normalized {
                output.push(' ');
                output.push_str(attribute);
                output.push_str("=\"");
                escaped(&normalized, output);
                output.push('"');
            }
        }
    }
    output.push('>');
    for child in element.children() {
        if let Some(child) = ElementRef::wrap(child) {
            serialize_clean(child, base, output, depth + 1)?;
        } else if let scraper::Node::Text(text) = child.value() {
            escaped(text, output);
        }
    }
    if !matches!(
        name,
        "area" | "base" | "br" | "col" | "hr" | "img" | "link" | "meta" | "source" | "wbr"
    ) {
        output.push_str("</");
        output.push_str(name);
        output.push('>');
    }
    Ok(())
}

fn render_clean(root: ElementRef<'_>, base: Option<&Url>) -> Result<String> {
    let mut cleaned = String::new();
    serialize_clean(root, base, &mut cleaned, 0)?;
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "head"])
        .build()
        .convert(&cleaned)
        .map(|markdown| markdown.trim().to_owned())
        .map_err(|error| Error::Conversion(format!("HTML rendering failed: {error}")))
}

pub(super) fn fragment(source: &str) -> Result<String> {
    let document = Html::parse_fragment(source);
    render_clean(document.root_element(), None)
}

/// Extract an article candidate, metadata and Markdown without fetching links.
pub fn extract_html(source: &str, base_url: Option<&str>) -> Result<Document> {
    let document = Html::parse_document(source);
    let base = base_url.and_then(|value| Url::parse(value).ok());
    let candidates = selector("article, main, [role=main]");
    let links = selector("a");
    let paragraphs = selector("p");
    let root = document
        .select(&candidates)
        .filter(|candidate| !is_hidden(*candidate))
        .max_by_key(|element| {
            let length: usize = element.text().map(str::len).sum();
            let link_length: usize = element
                .select(&links)
                .flat_map(|link| link.text())
                .map(str::len)
                .sum();
            length
                .saturating_sub(link_length)
                .saturating_add(element.select(&paragraphs).count() * 40)
        })
        .or_else(|| document.select(&selector("body")).next())
        .unwrap_or_else(|| document.root_element());
    let mut metadata = Map::new();
    let title = meta(&document, &["og:title", "twitter:title"])
        .or_else(|| root.select(&selector("h1")).next().map(plain))
        .or_else(|| document.select(&selector("title")).next().map(plain));
    if let Some(title) = title.filter(|value| !value.is_empty()) {
        metadata.insert("title".into(), title.into());
    }
    for (key, candidates) in [
        ("author", &["author", "article:author"][..]),
        (
            "description",
            &["description", "og:description", "twitter:description"][..],
        ),
        ("site", &["og:site_name"][..]),
        (
            "published",
            &["article:published_time", "date", "datePublished"][..],
        ),
    ] {
        if let Some(value) = meta(&document, candidates) {
            metadata.insert(key.into(), value.into());
        }
    }
    if let Some(canonical) = document
        .select(&selector("link[rel=canonical]"))
        .next()
        .and_then(|node| node.value().attr("href"))
        .and_then(|value| safe_url(value, base.as_ref()))
    {
        metadata.insert("canonical_url".into(), canonical.into());
    }
    if let Some(base) = &base {
        metadata.insert("source".into(), base.as_str().into());
        if let Some(host) = base.host_str() {
            metadata.insert("domain".into(), host.into());
        }
    }
    let markdown = render_clean(root, base.as_ref())?;
    if markdown.is_empty() {
        return Err(Error::Conversion(
            "HTML contains no extractable content".into(),
        ));
    }
    metadata.insert("converter".into(), "native-html".into());
    Ok(Document {
        markdown,
        metadata,
        ..Document::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn article_drops_navigation_and_resolves_links() {
        let doc = extract_html(r#"<title>Fallback</title><meta property="og:title" content="Article"><nav>menu</nav><article><h1>Article</h1><p>Some <strong>important</strong> text <a href="../next">next</a>.</p><img data-src="/image.png" alt="Photo"><p hidden>hidden</p><script>alert(1)</script></article><footer>footer</footer>"#, Some("https://example.test/posts/one")).unwrap();
        assert_eq!(doc.metadata["title"], "Article");
        assert!(doc.markdown.contains("**important**"));
        assert!(doc.markdown.contains("https://example.test/next"));
        assert!(doc.markdown.contains("https://example.test/image.png"));
        for excluded in ["menu", "hidden", "alert", "footer"] {
            assert!(!doc.markdown.contains(excluded));
        }
    }
    #[test]
    fn article_keeps_code_table_unicode_and_safe_text() {
        let doc = extract_html(r#"<main><h1>中文</h1><pre><code class="language-rust">let n = 1 &lt; 2;</code></pre><table><tr><th>A</th><th>B</th></tr><tr><td>x</td><td>y</td></tr></table><a href="javascript:alert(1)">safe label</a></main>"#, None).unwrap();
        assert!(doc.markdown.contains("let n = 1 < 2;"));
        assert!(doc.markdown.contains("|"));
        assert!(doc.markdown.contains("safe label"));
        assert!(!doc.markdown.contains("javascript:"));
    }
}
