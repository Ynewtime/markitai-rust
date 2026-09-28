//! Output profiles transform references and metadata after conversion finishes.

use crate::VERSION;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use serde_json::{Map, Value, json};

pub(crate) fn apply(markdown: &mut String, metadata: &mut Map<String, Value>, cfg: &Value) {
    match cfg.pointer("/output/profile").and_then(Value::as_str) {
        Some("rag") => *markdown = transform(markdown, true, false, true, None),
        Some("obsidian") => {
            let wiki = cfg.pointer("/output/wikilinks").and_then(Value::as_bool) == Some(true);
            *markdown = transform(markdown, true, wiki, false, None);
        }
        Some("okf") => okf(metadata),
        _ => (),
    }
}

/// Replace a complete asset destination, retaining link titles and wiki aliases.
/// Literal code and unrelated paths remain untouched.
pub(crate) fn rewrite_asset_target(markdown: &str, previous: &str, next: &str) -> String {
    transform(markdown, false, false, false, Some((previous, next)))
}

fn okf(metadata: &mut Map<String, Value>) {
    metadata.entry("type").or_insert_with(|| json!("Document"));
    if let Some(source) = metadata.remove("source") {
        metadata.entry("resource").or_insert(source);
    }
    let mut generated = json!({"by": format!("markitai/{VERSION}")});
    if let Some(value) = metadata.remove("markitai_processed")
        && let Some(timestamp) = value.as_str().and_then(utc_timestamp)
    {
        generated["at"] = timestamp.into();
    }
    // Existing unknown fields keep their values, matching the public mapping.
    // This also makes repeated profile application stable.
    metadata.entry("generated").or_insert(generated);
}

fn utc_timestamp(value: &str) -> Option<String> {
    let aware = DateTime::parse_from_rfc3339(value).ok().or_else(|| {
        ["%Y-%m-%d %H:%M:%S%.f%:z", "%Y-%m-%dT%H:%M:%S%.f%z"]
            .into_iter()
            .find_map(|format| DateTime::parse_from_str(value, format).ok())
    });
    if let Some(aware) = aware {
        return Some(
            aware
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        );
    }
    let naive = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
        .into_iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
        })?;
    let local = Local.from_local_datetime(&naive).earliest()?;
    Some(
        local
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

#[derive(Default)]
struct LiteralContext {
    fence: Option<(u8, usize)>,
    html: Option<&'static str>,
}

impl LiteralContext {
    /// Opening and closing fence lines themselves are literal too.
    fn literal(&mut self, line: &str) -> bool {
        let mut content = line.trim_start_matches([' ', '\t']);
        while let Some(rest) = content.strip_prefix('>') {
            content = rest.trim_start_matches(' ');
        }
        if let Some(tag) = self.html {
            if content.to_ascii_lowercase().contains(&format!("</{tag}>")) {
                self.html = None;
            }
            return true;
        }
        if self.fence.is_none() {
            let lower = content.to_ascii_lowercase();
            for tag in ["pre", "code", "script", "style"] {
                if lower.starts_with(&format!("<{tag}")) {
                    if !lower.contains(&format!("</{tag}>")) {
                        self.html = Some(tag);
                    }
                    return true;
                }
            }
        }
        let bytes = content.as_bytes();
        if let Some(&marker @ (b'`' | b'~')) = bytes.first() {
            let width = bytes.iter().take_while(|&&byte| byte == marker).count();
            let remaining = &content[width..];
            if let Some((opening, minimum)) = self.fence {
                if marker == opening && width >= minimum && remaining.trim().is_empty() {
                    self.fence = None;
                }
                return true;
            }
            if width >= 3 && (marker != b'`' || !remaining.contains('`')) {
                self.fence = Some((marker, width));
                return true;
            }
        }
        self.fence.is_some() || line.starts_with("    ") || line.starts_with('\t')
    }
}

fn transform(
    source: &str,
    visible: bool,
    wiki: bool,
    rag: bool,
    replacement: Option<(&str, &str)>,
) -> String {
    let mut context = LiteralContext::default();
    let mut output = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        if context.literal(line) {
            output.push_str(line);
        } else {
            output.push_str(&inline(line, visible, wiki, rag, replacement));
        }
    }
    output
}

struct Reference<'a> {
    alt: &'a str,
    target: &'a str,
    target_start: usize,
    target_end: usize,
    end: usize,
    image: bool,
    has_title: bool,
}

fn reference(text: &str) -> Option<Reference<'_>> {
    let (image, start) = if text.starts_with("![") {
        (true, 2)
    } else if text.starts_with('[') {
        (false, 1)
    } else {
        return None;
    };
    let mut index = start;
    let mut brackets = 1;
    while index < text.len() {
        let ch = text[index..].chars().next()?;
        if ch == '\\' {
            index += 1;
            if let Some(ch) = text[index..].chars().next() {
                index += ch.len_utf8();
            }
            continue;
        }
        if ch == '[' {
            brackets += 1;
        }
        if ch == ']' {
            brackets -= 1;
            if brackets == 0 {
                break;
            }
        }
        index += ch.len_utf8();
    }
    if brackets != 0 || !text[index..].starts_with("](") {
        return None;
    }
    let alt = &text[start..index];
    index += 2;
    while text[index..].starts_with([' ', '\t']) {
        index += 1;
    }
    let angle = text[index..].starts_with('<');
    if angle {
        index += 1;
    }
    let target_start = index;
    let mut parens = 0;
    while index < text.len() {
        let ch = text[index..].chars().next()?;
        if ch == '\\' {
            let after = index + 1;
            if let Some(next) = text[after..].chars().next()
                && (next.is_ascii_punctuation() || next.is_whitespace())
            {
                index = after + next.len_utf8();
                continue;
            }
        }
        if angle && ch == '>' || !angle && parens == 0 && (ch == ')' || ch.is_whitespace()) {
            break;
        }
        if !angle && ch == '(' {
            parens += 1;
        }
        if !angle && ch == ')' {
            if parens == 0 {
                break;
            }
            parens -= 1;
        }
        if ch == '\n' {
            return None;
        }
        index += ch.len_utf8();
    }
    let target_end = index;
    if angle {
        if !text[index..].starts_with('>') {
            return None;
        }
        index += 1;
    }
    let suffix_start = index;
    let mut quote = None;
    while index < text.len() {
        let ch = text[index..].chars().next()?;
        if ch == '\\' {
            index += 1;
            if let Some(next) = text[index..].chars().next() {
                index += next.len_utf8();
            }
            continue;
        }
        if let Some(opening) = quote {
            if ch == opening {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch == ')' {
            return Some(Reference {
                alt,
                target: &text[target_start..target_end],
                target_start,
                target_end,
                end: index + 1,
                image,
                has_title: !text[suffix_start..index].trim().is_empty(),
            });
        } else if !ch.is_whitespace() {
            return None;
        }
        if ch == '\n' {
            return None;
        }
        index += ch.len_utf8();
    }
    None
}

fn unescape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && chars
                .peek()
                .is_some_and(|ch| matches!(ch, '[' | ']' | '\\' | '(' | ')' | ' '))
        {
            output.push(chars.next().unwrap());
        } else {
            output.push(ch);
        }
    }
    output
}

fn unquote(value: &str) -> String {
    let mut output = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let a = (bytes[i + 1] as char).to_digit(16);
            let b = (bytes[i + 2] as char).to_digit(16);
            if let (Some(a), Some(b)) = (a, b) {
                output.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        output.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn visible_target(target: &str) -> Option<String> {
    target
        .strip_prefix(".markitai/assets/")
        .map(|name| format!("assets/{name}"))
        .or_else(|| {
            target
                .strip_prefix(".markitai/assets\\")
                .map(|name| format!("assets\\{name}"))
        })
}

fn inline(
    line: &str,
    visible: bool,
    wiki: bool,
    rag: bool,
    replacement: Option<(&str, &str)>,
) -> String {
    inline_nested(line, visible, wiki, rag, replacement, 0)
}

fn inline_nested(
    line: &str,
    visible: bool,
    wiki: bool,
    rag: bool,
    replacement: Option<(&str, &str)>,
    depth: usize,
) -> String {
    if depth >= 64 {
        return line.to_owned();
    }
    let mut output = String::with_capacity(line.len());
    let mut index = 0;
    while index < line.len() {
        let tail = &line[index..];
        if let Some(rest) = tail.strip_prefix('\\') {
            let amount = 1 + rest.chars().next().map(char::len_utf8).unwrap_or(0);
            output.push_str(&tail[..amount]);
            index += amount;
            continue;
        }
        if tail.starts_with('`') {
            let width = tail.bytes().take_while(|byte| *byte == b'`').count();
            let mut end = width;
            while end < tail.len() {
                if tail[end..].starts_with('`') {
                    let closing = tail[end..].bytes().take_while(|byte| *byte == b'`').count();
                    end += closing;
                    if closing == width {
                        break;
                    }
                } else {
                    end += tail[end..].chars().next().unwrap().len_utf8();
                }
            }
            output.push_str(&tail[..end]);
            index += end;
            continue;
        }
        if let Some(rest) = tail.strip_prefix("![[")
            && let Some(end) = rest.find("]]")
        {
            let body = &rest[..end];
            let (target, alias) = body
                .split_once('|')
                .map_or((body, None), |(a, b)| (a, Some(b)));
            let new = replacement
                .filter(|(from, _)| *from == unquote(target))
                .map(|(_, to)| to.to_owned())
                .or_else(|| visible.then(|| visible_target(target)).flatten())
                .unwrap_or_else(|| target.to_owned());
            output.push_str(&format!(
                "![[{new}{}]]",
                alias.map(|value| format!("|{value}")).unwrap_or_default()
            ));
            index += end + 5;
            continue;
        }
        if let Some(reference) = reference(tail) {
            let target = replacement
                .filter(|(from, _)| *from == unquote(&unescape(reference.target)))
                .map(|(_, to)| to.to_owned())
                .or_else(|| visible.then(|| visible_target(reference.target)).flatten())
                .unwrap_or_else(|| reference.target.to_owned());
            if wiki && reference.image && target.starts_with("assets/") && !reference.has_title {
                let decoded = unquote(&unescape(&target))
                    .replace('|', "%7C")
                    .replace(']', "%5D");
                let alt = unescape(reference.alt);
                let alt = alt.trim();
                output.push_str(&format!(
                    "![[{decoded}{}]]",
                    if alt.is_empty() {
                        String::new()
                    } else {
                        format!("|{alt}")
                    }
                ));
            } else {
                if !reference.image && reference.alt.contains("![") {
                    output.push('[');
                    output.push_str(&inline_nested(
                        reference.alt,
                        visible,
                        wiki,
                        rag,
                        replacement,
                        depth + 1,
                    ));
                    output.push_str(&tail[1 + reference.alt.len()..reference.target_start]);
                } else {
                    output.push_str(&tail[..reference.target_start]);
                }
                output.push_str(&target);
                output.push_str(&tail[reference.target_end..reference.end]);
            }
            index += reference.end;
            continue;
        }
        if rag
            && tail.starts_with("<!--")
            && let Some(end) = tail.find("-->")
        {
            let comment = tail[4..end].trim();
            if let Some(number) = comment.strip_prefix("Page number:").map(str::trim)
                && !number.is_empty()
                && number.bytes().all(|byte| byte.is_ascii_digit())
            {
                output.push_str(&format!("<!-- page: {number} -->"));
                index += end + 3;
                continue;
            }
        }
        let ch = tail.chars().next().unwrap();
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

/// Report inconsistent pipe-table columns without changing the table itself.
pub(crate) fn table_warnings(markdown: &str) -> Vec<String> {
    let mut context = LiteralContext::default();
    let mut warnings = Vec::new();
    let mut start = None;
    let mut width = 0;
    let mut mismatch = Vec::new();
    let flush = |start: &mut Option<usize>,
                 width: usize,
                 mismatch: &mut Vec<(usize, usize)>,
                 warnings: &mut Vec<String>| {
        if let Some(start) = start.take()
            && !mismatch.is_empty()
        {
            let detail = mismatch
                .iter()
                .take(3)
                .map(|(line, cells)| format!("line {line}: {cells}"))
                .collect::<Vec<_>>()
                .join(", ");
            warnings.push(format!("pipe table at line {start} has {width} header column(s) but inconsistent row(s) ({detail})"));
        }
        mismatch.clear();
    };
    for (index, line) in markdown.lines().enumerate() {
        if context.literal(line) || !line.trim_start().starts_with('|') {
            flush(&mut start, width, &mut mismatch, &mut warnings);
            continue;
        }
        let cells = table_cells(line);
        let count = cells.len();
        let delimiter = cells.iter().all(|cell| {
            let cell = cell.trim().trim_matches(':');
            !cell.is_empty() && cell.bytes().all(|byte| byte == b'-')
        });
        if start.is_none() {
            start = Some(index + 1);
            width = count;
        } else if width != count && !delimiter {
            mismatch.push((index + 1, count));
        }
    }
    flush(&mut start, width, &mut mismatch, &mut warnings);
    warnings
}

fn table_cells(line: &str) -> Vec<String> {
    let line = line.trim().strip_prefix('|').unwrap_or(line.trim());
    let line = line.strip_suffix('|').unwrap_or(line);
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in line.chars() {
        if ch == '|' && !escaped {
            cells.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
        if ch == '\\' {
            escaped = !escaped;
        } else {
            escaped = false;
        }
    }
    cells.push(current);
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(source: &str, name: &str, wiki: bool) -> String {
        let mut source = source.to_owned();
        apply(
            &mut source,
            &mut Map::new(),
            &json!({"output":{"profile":name,"wikilinks":wiki}}),
        );
        source
    }

    #[test]
    fn rag_rewrites_only_asset_destinations_and_page_markers() {
        let input = "![A](.markitai/assets/image.png)\n[Download](.markitai/assets/manual.pdf)\nPlain .markitai/assets/path and https://example.invalid/.markitai/assets/a.png\n<!-- Page number: 012 -->\n";
        let output = profile(input, "rag", false);
        assert!(output.contains("![A](assets/image.png)"));
        assert!(output.contains("[Download](assets/manual.pdf)"));
        assert!(output.contains("Plain .markitai/assets/path"));
        assert!(output.contains("https://example.invalid/.markitai/assets/a.png"));
        assert!(output.contains("<!-- page: 012 -->"));
        assert_eq!(profile(&output, "rag", false), output);
    }

    #[test]
    fn obsidian_preserves_alt_unquotes_unicode_and_handles_brackets() {
        let input = r#"![A \] caption](.markitai/assets/a%20b.png)
![](.markitai/assets/%E5%9B%BE%E7%89%87.png)
![text](.markitai/assets/p.png "tooltip")"#;
        let output = profile(input, "obsidian", true);
        assert!(output.contains("![[assets/a b.png|A ] caption]]"));
        assert!(output.contains("![[assets/图片.png]]"));
        // Link titles have no wiki equivalent, so retain their Markdown form.
        assert!(
            profile(
                "![alt](.markitai/assets/a.png \"tooltip\")",
                "obsidian",
                true
            )
            .contains("![alt](assets/a.png \"tooltip\")")
        );
        assert_eq!(
            profile("![Alt](.markitai/assets/a.png)", "obsidian", false),
            "![Alt](assets/a.png)"
        );
    }

    #[test]
    fn profiles_leave_literal_code_unchanged() {
        let input = "````md\n![code](.markitai/assets/a.png)\n```\n<!-- Page number: 1 -->\n````\n    ![indented](.markitai/assets/b.png)\n`![inline](.markitai/assets/c.png)`\n<pre>\n![html](.markitai/assets/d.png)\n</pre>\n![real](.markitai/assets/e.png)\n";
        let output = profile(input, "obsidian", true);
        assert!(output.contains("![code](.markitai/assets/a.png)"));
        assert!(output.contains("<!-- Page number: 1 -->"));
        assert!(output.contains("    ![indented](.markitai/assets/b.png)"));
        assert!(output.contains("`![inline](.markitai/assets/c.png)`"));
        assert!(output.contains("![html](.markitai/assets/d.png)"));
        assert!(output.contains("![[assets/e.png|real]]"));
    }

    #[test]
    fn asset_remapping_retains_titles_aliases_and_does_not_replace_prefixes() {
        let input = "![alt](assets/a.png \"title\") ![[assets/a.png|alias]] ![other](assets/a.png.extra) `![code](assets/a.png)`";
        let output = rewrite_asset_target(input, "assets/a.png", "assets/hash.png");
        assert_eq!(
            output,
            "![alt](assets/hash.png \"title\") ![[assets/hash.png|alias]] ![other](assets/a.png.extra) `![code](assets/a.png)`"
        );
        assert_eq!(
            rewrite_asset_target("![](<assets/a b.png>)", "assets/a b.png", "assets/hash.png"),
            "![](<assets/hash.png>)"
        );
    }

    #[test]
    fn linked_images_keep_the_outer_link_and_rewrite_the_image() {
        let source = "[![Alt](.markitai/assets/a.png)](https://example.invalid/gallery)";
        let visible = profile(source, "rag", false);
        assert_eq!(
            visible,
            "[![Alt](assets/a.png)](https://example.invalid/gallery)"
        );
        let hashed = rewrite_asset_target(&visible, "assets/a.png", "assets/hash.png");
        assert_eq!(
            hashed,
            "[![Alt](assets/hash.png)](https://example.invalid/gallery)"
        );
        assert_eq!(
            profile(source, "obsidian", true),
            "[![[assets/a.png|Alt]]](https://example.invalid/gallery)"
        );
    }

    #[test]
    fn okf_maps_metadata_to_utc_seconds_and_is_idempotent() {
        let mut metadata=json!({"title":"Doc","source":"sample.pdf","description":"A doc","tags":["a","b"],"markitai_processed":"2026-08-25T09:42:09.460+08:00","fetch_strategy":"static"}).as_object().unwrap().clone();
        let cfg = json!({"output":{"profile":"okf"}});
        let mut body = "# Content\n".to_owned();
        apply(&mut body, &mut metadata, &cfg);
        assert_eq!(metadata["type"], "Document");
        assert_eq!(metadata["resource"], "sample.pdf");
        assert_eq!(metadata["generated"]["at"], "2026-08-25T01:42:09Z");
        assert_eq!(metadata["generated"]["by"], format!("markitai/{VERSION}"));
        assert_eq!(metadata["fetch_strategy"], "static");
        assert!(!metadata.contains_key("source"));
        assert!(!metadata.contains_key("markitai_processed"));
        let once = metadata.clone();
        apply(&mut body, &mut metadata, &cfg);
        assert_eq!(metadata, once);
        assert_eq!(body, "# Content\n");
        let mut invalid = json!({"markitai_processed":"not-a-date"})
            .as_object()
            .unwrap()
            .clone();
        apply(&mut body, &mut invalid, &cfg);
        assert!(invalid["generated"].get("at").is_none());
    }

    #[test]
    fn rag_table_warnings_ignore_fences_and_escaped_pipes() {
        let source = "| A | B |\n| --- | --- |\n| a\\|b | c |\n| 1 | 2 | 3 |\n\n````md\n| A |\n```\n| B | C |\n````\n";
        let warnings = table_warnings(source);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("line 1"));
        assert!(warnings[0].contains("line 4: 3"));
    }
}
