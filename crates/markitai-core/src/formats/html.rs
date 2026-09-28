use crate::{Document, Error, Result};
use scraper::{ElementRef, Html, Selector};
use serde_json::{Map, Value};
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

fn jsonld_documents(document: &Html) -> Vec<Value> {
    fn collect(value: Value, documents: &mut Vec<Value>, depth: usize) {
        if depth > 64 {
            return;
        }
        match value {
            Value::Object(mut object) => {
                let graph = object.remove("@graph");
                documents.push(Value::Object(object));
                if let Some(graph) = graph {
                    collect(graph, documents, depth + 1);
                }
            }
            Value::Array(values) => {
                for value in values {
                    collect(value, documents, depth + 1);
                }
            }
            _ => {}
        }
    }
    let mut documents = Vec::new();
    for script in document.select(&selector("script")) {
        if script
            .value()
            .attr("type")
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/ld+json"))
            && let Ok(value) = serde_json::from_str(&script.text().collect::<String>())
        {
            collect(value, &mut documents, 0);
        }
    }
    documents
}

fn jsonld_text(documents: &[Value], key: &str) -> Option<String> {
    documents.iter().find_map(|document| {
        document
            .get(key)?
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    })
}

fn clean_title(value: &str, site: Option<&str>) -> Option<String> {
    let mut title = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(site) = site {
        let site = site.split_whitespace().collect::<Vec<_>>().join(" ");
        loop {
            let mut changed = false;
            for separator in [" | ", " - ", " -- ", " · ", " — ", " – "] {
                if let Some(trimmed) = title.strip_suffix(&format!("{separator}{site}")) {
                    title = trimmed.to_owned();
                    changed = true;
                    break;
                }
                if let Some(trimmed) = title.strip_prefix(&format!("{site}{separator}")) {
                    title = trimmed.to_owned();
                    changed = true;
                    break;
                }
            }
            if !changed {
                break;
            }
        }
    }
    if let Some((end, _)) = title.char_indices().nth(300) {
        title.truncate(end);
        if let Some(boundary) = title.rfind(' ').filter(|index| *index > 0) {
            title.truncate(boundary);
        }
        title.push('…');
    }
    (!title.is_empty()).then_some(title)
}

fn page_title(document: &Html, jsonld: &[Value], site: Option<&str>) -> Option<String> {
    let headline = jsonld_text(jsonld, "headline");
    let document_title = document.select(&selector("title")).next().map(plain);
    let social = meta(document, &["og:title", "twitter:title"]);
    let mut title = [
        headline.clone(),
        jsonld_text(jsonld, "name"),
        document_title.clone(),
        social.clone(),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| clean_title(&value, site))?;
    if let Some(prefix) = title
        .strip_suffix('…')
        .or_else(|| title.strip_suffix("..."))
    {
        let prefix = prefix.trim_end();
        if let Some(candidate) = [document_title, social]
            .into_iter()
            .flatten()
            .filter_map(|value| clean_title(&value, site))
            .find(|value| {
                !value.ends_with('…')
                    && !value.ends_with("...")
                    && value.len() > title.len()
                    && value.starts_with(prefix)
            })
        {
            title = candidate;
        }
    }
    if title.contains(" | ") {
        let heading = document.select(&selector("h1")).next().map(plain);
        for candidate in [headline, heading].into_iter().flatten() {
            let candidate = candidate.split_whitespace().collect::<Vec<_>>().join(" ");
            if !candidate.is_empty() && title.starts_with(&format!("{candidate} | ")) {
                return Some(candidate);
            }
        }
    }
    Some(title)
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
    fn destination(value: &str) -> String {
        value
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace('"', "%22")
    }
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    if value.starts_with('#') {
        return Some(destination(value));
    }
    if let Ok(url) = Url::parse(value) {
        return matches!(url.scheme(), "http" | "https" | "mailto" | "tel")
            .then(|| destination(value));
    }
    if let Some(base) = base {
        return base
            .join(value)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https"))
            .map(|url| url.to_string());
    }
    (!value.contains(':')).then(|| destination(value))
}

fn block_tag(name: &str) -> bool {
    matches!(
        name,
        "html"
            | "body"
            | "article"
            | "main"
            | "section"
            | "div"
            | "p"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "pre"
            | "ul"
            | "ol"
            | "li"
            | "dl"
            | "dt"
            | "dd"
            | "table"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "td"
            | "th"
            | "blockquote"
            | "figure"
            | "figcaption"
            | "details"
            | "summary"
            | "hr"
            | "header"
            | "footer"
    )
}

fn escaped_text(value: &str, output: &mut String) {
    // HTML source line boundaries remain soft Markdown line breaks. The marker
    // prevents the renderer from collapsing them with ordinary inline spaces.
    let mut start = 0;
    for (index, ch) in value.char_indices() {
        if ch == '\n' || ch == '\r' {
            escaped(value[start..index].trim_matches([' ', '\t']), output);
            if !output.ends_with("</markitai-soft-break>") {
                output.push_str("<markitai-soft-break></markitai-soft-break>");
            }
            start = index + ch.len_utf8();
        }
    }
    escaped(
        if start > 0 {
            value[start..].trim_start_matches([' ', '\t'])
        } else {
            value
        },
        output,
    );
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
            | "noscript"
    ) || is_hidden(element)
    {
        return Ok(());
    }
    // Canonicalize lazy images without retaining arbitrary event or style attributes.
    let src = value
        .attr("data-src")
        .or_else(|| value.attr("data-original"))
        .or_else(|| value.attr("src"));
    let preformatted = matches!(name, "pre" | "code")
        || element
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|parent| matches!(parent.value().name(), "pre" | "code"));
    let styled_code = name == "code"
        && value.attr("style").is_some_and(|style| {
            style
                .to_ascii_lowercase()
                .replace(char::is_whitespace, "")
                .contains("white-space:pre")
        })
        && !element
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|parent| parent.value().name() == "pre");
    if styled_code {
        output.push_str("<pre>");
    }
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
            if preformatted {
                escaped(text, output);
            } else {
                let previous = child
                    .prev_siblings()
                    .find(|node| !matches!(node.value(), scraper::Node::Comment(_)));
                let next = child
                    .next_siblings()
                    .find(|node| !matches!(node.value(), scraper::Node::Comment(_)));
                let trim_start = previous.map_or(block_tag(name), |node| {
                    ElementRef::wrap(node).is_some_and(|element| block_tag(element.value().name()))
                });
                let trim_end = next.map_or(block_tag(name), |node| {
                    ElementRef::wrap(node).is_some_and(|element| block_tag(element.value().name()))
                });
                let text = if trim_start {
                    text.trim_start()
                } else {
                    text.as_ref()
                };
                let text = if trim_end { text.trim_end() } else { text };
                escaped_text(text, output);
            }
        }
    }
    if styled_code {
        output.push_str("</pre>");
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
        .add_handler(
            vec!["markitai-soft-break"],
            |_: &dyn htmd::element_handler::Handlers, _: htmd::Element| Some("\n".into()),
        )
        .add_handler(
            vec!["q"],
            |handlers: &dyn htmd::element_handler::Handlers, element: htmd::Element| {
                Some(format!("\"{}\"", handlers.walk_children(element.node).content).into())
            },
        )
        .add_handler(
            vec!["sub", "sup"],
            |handlers: &dyn htmd::element_handler::Handlers, element: htmd::Element| {
                Some(
                    format!(
                        "<{0}>{1}</{0}>",
                        element.tag,
                        handlers.walk_children(element.node).content.trim()
                    )
                    .into(),
                )
            },
        )
        .add_handler(
            vec![
                "div",
                "section",
                "article",
                "main",
                "figure",
                "figcaption",
                "dl",
                "dt",
                "dd",
                "details",
                "summary",
            ],
            |handlers: &dyn htmd::element_handler::Handlers, element: htmd::Element| {
                Some(
                    format!(
                        "\n\n{}\n\n",
                        handlers.walk_children(element.node).content.trim()
                    )
                    .into(),
                )
            },
        )
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
        .filter(|candidate| {
            !is_hidden(*candidate)
                && !candidate
                    .ancestors()
                    .filter_map(ElementRef::wrap)
                    .any(is_hidden)
        })
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
    let jsonld = jsonld_documents(&document);
    let site = meta(&document, &["og:site_name", "application-name"]);
    let title = page_title(&document, &jsonld, site.as_deref());
    if let Some(title) = title.filter(|value| !value.is_empty()) {
        metadata.insert("title".into(), title.into());
    }
    let author = meta(&document, &["author", "article:author"]).or_else(|| {
        jsonld.iter().find_map(|doc| {
            let value = doc.get("author")?;
            value
                .as_str()
                .or_else(|| value.get("name")?.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
    });
    let published = meta(&document, &["article:published_time"])
        .or_else(|| jsonld_text(&jsonld, "datePublished"))
        .or_else(|| {
            document
                .select(&selector("time[datetime]"))
                .find_map(|time| {
                    let value = time.value().attr("datetime")?.trim();
                    let bytes = value.as_bytes();
                    (bytes.len() >= 7
                        && bytes[..4].iter().all(u8::is_ascii_digit)
                        && bytes[4] == b'-'
                        && bytes[5..7].iter().all(u8::is_ascii_digit))
                    .then(|| value.to_owned())
                })
        });
    for (key, value) in [
        ("author", author),
        ("site", site),
        ("published", published),
        (
            "description",
            meta(
                &document,
                &["description", "og:description", "twitter:description"],
            ),
        ),
    ] {
        if let Some(value) = value {
            metadata.insert(key.into(), value.into());
        }
    }
    if let Some(canonical) = document
        .select(&selector("link[rel]"))
        .find(|node| {
            node.value().attr("rel").is_some_and(|rel| {
                rel.split_whitespace()
                    .any(|token| token.eq_ignore_ascii_case("canonical"))
            })
        })
        .and_then(|node| node.value().attr("href"))
        .and_then(|value| safe_url(value, base.as_ref()))
        .filter(|value| {
            !base.as_ref().is_some_and(|source| {
                !source.path().trim_matches('/').is_empty()
                    && Url::parse(value)
                        .is_ok_and(|canonical| canonical.path().trim_matches('/').is_empty())
            })
        })
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
        assert_eq!(doc.metadata["title"], "Fallback");
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

    #[test]
    fn structured_metadata_uses_graph_headline_and_author() {
        let doc = extract_html(r#"<title>Wrong title</title><meta property="og:title" content="Wrong social title"><meta name="og:site_name" content="Journal"><script type="application/ld+json">{"@graph":[{"name":"Website"},{"headline":"Journal - Structured headline · Journal","author":{"name":"Ada"},"datePublished":"2026-02-03"}]}</script><main><p>Article text.</p></main>"#, None).unwrap();
        assert_eq!(doc.metadata["title"], "Structured headline");
        assert_eq!(doc.metadata["author"], "Ada");
        assert_eq!(doc.metadata["published"], "2026-02-03");
        assert!(!doc.markdown.contains("Structured headline"));
    }

    #[test]
    fn title_layers_recover_truncation_and_keep_real_subtitles() {
        let doc = extract_html(r#"<title>A complete long headline</title><script type="application/ld+json">{"headline":"A complete…"}</script><p>Body</p>"#, None).unwrap();
        assert_eq!(doc.metadata["title"], "A complete long headline");
        for (title, expected) in [
            ("Article | Site", "Article"),
            ("Article - a retrospective", "Article - a retrospective"),
        ] {
            let doc = extract_html(
                &format!("<title>{title}</title><h1>Article</h1><p>Body</p>"),
                None,
            )
            .unwrap();
            assert_eq!(doc.metadata["title"], expected);
        }
        let long = "界".repeat(350);
        assert_eq!(clean_title(&long, None).unwrap().chars().count(), 301);
    }

    #[test]
    fn malformed_jsonld_and_meta_author_do_not_hide_valid_layers() {
        let doc = extract_html(r#"<meta name="author" content="Meta author"><meta property="article:published_time" content="2026-01-01"><title>Document title</title><meta property="og:title" content="Social title"><script type="application/ld+json">invalid</script><script type="application/ld+json">[{"author":"Schema author","datePublished":"2025-01-01"}]</script><p>Body</p>"#, None).unwrap();
        assert_eq!(doc.metadata["title"], "Document title");
        assert_eq!(doc.metadata["author"], "Meta author");
        assert_eq!(doc.metadata["published"], "2026-01-01");
        let doc = extract_html(
            "<time datetime=PT2H>Duration</time><time datetime=2026-03-04>Today</time>",
            None,
        )
        .unwrap();
        assert_eq!(doc.metadata["published"], "2026-03-04");
    }

    #[test]
    fn canonical_homepage_is_not_used_for_article() {
        let html = r#"<link rel="alternate canonical" href="https://example.test"><p>Body</p>"#;
        assert!(
            !extract_html(html, Some("https://example.test/post"))
                .unwrap()
                .metadata
                .contains_key("canonical_url")
        );
        assert_eq!(
            extract_html(html, Some("https://example.test/"))
                .unwrap()
                .metadata["canonical_url"],
            "https://example.test"
        );
    }

    #[test]
    fn inline_quotes_and_source_lines_preserve_reference_spelling() {
        let doc = extract_html("<p>WWF's goal is to:\n    <q>Build a <em>better</em> future.</q>\n</p>\n<a href=\"https://example.com\">Example Link</a>\n<img src=\"https://example.com/test.jpg\" alt=\"Test Image\">", None).unwrap();
        assert_eq!(
            doc.markdown,
            "WWF's goal is to:\n\"Build a *better* future.\"\n\n[Example Link](https://example.com)\n![Test Image](https://example.com/test.jpg)"
        );
        assert_eq!(
            fragment("<p>One <q>two <q>three</q></q> four.</p>").unwrap(),
            "One \"two \"three\"\" four."
        );
    }

    #[test]
    fn urls_are_validated_without_rewriting_absolute_spelling() {
        let base = Url::parse("https://example.test/posts/one").unwrap();
        for value in [
            "https://EXAMPLE.test",
            "https://example.test/%2f?q=a%2Bb#part",
            "mailto:name@example.test",
            "tel:+12025550123",
        ] {
            assert_eq!(safe_url(value, Some(&base)).as_deref(), Some(value));
        }
        assert_eq!(
            safe_url("../two", Some(&base)).as_deref(),
            Some("https://example.test/two")
        );
        assert_eq!(
            safe_url("https://example.test/<tag>\"", None).as_deref(),
            Some("https://example.test/%3Ctag%3E%22")
        );
        for value in [
            "javascript:alert(1)",
            "data:text/html,body",
            "java\nscript:alert(1)",
            "https://exam\tple.test/",
        ] {
            assert!(safe_url(value, Some(&base)).is_none());
        }
    }

    #[test]
    fn block_boundaries_code_and_chemical_notation_survive() {
        let doc = extract_html("<main><div>First</div><div>Second</div><p>H<sub>2</sub>O and 10<sup>n</sup>.</p><pre><code>one\n  two\n\nthree</code></pre><code style=\"white-space: pre\">a\n  b</code></main>", None).unwrap();
        assert!(
            doc.markdown
                .starts_with("First\n\nSecond\n\nH<sub>2</sub>O and 10<sup>n</sup>.")
        );
        assert!(doc.markdown.contains("one\n  two\n\nthree"));
        assert!(doc.markdown.contains("```\na\n  b\n```"));
        assert!(!doc.markdown.contains("markitai-soft-break"));
    }

    #[test]
    fn candidate_inside_hidden_ancestor_cannot_become_article() {
        let doc = extract_html("<div hidden><article><p>Hidden text that outweighs the visible article by length.</p></article></div><main><p>Visible</p></main>", None).unwrap();
        assert_eq!(doc.markdown, "Visible");
    }
}
