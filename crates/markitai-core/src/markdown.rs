//! Deterministic cleanup applied to normal output; pure output bypasses it.

use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("static Markdown pattern")
}

static BROKEN_LINK: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"\[([^\]]*?)\n+([^\]]*?)\]\(([^)]+)\)"));
static PLACEHOLDER_LINE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?m)^__MARKITAI_[A-Z_]+_?\d*__\s*$"));
static PLACEHOLDER_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"!\[[^\]]*\]\(__MARKITAI_[A-Z_]+_?\d*__\)\s*\n?"));
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| pattern(r"__MARKITAI_[A-Z_]+_?\d*__"));
static DOUBLE_ALT: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"!\[[^\]]*\](!\[[^\]]*\]\([^)]+\))"));
static EMPTY_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"!\[[^\]]*\]\((?:\.markitai/assets/)?\)\s*\n?"));
static EXTRA_PAREN: LazyLock<Regex> = LazyLock::new(|| pattern(r"(!\[[^\]]*\]\([^)]+\))\)+"));
static PAGE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"<!-- (?:Page|Slide) (?:number: ?)?\d+ -->"));
static SLIDE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^<!--\s*Slide\s+(number:\s*)?\d+\s*-->"));
static BLANKS: LazyLock<Regex> = LazyLock::new(|| pattern(r"\n{3,}"));

fn clean_footers(source: String) -> String {
    let markers: Vec<_> = PAGE.find_iter(&source).collect();
    if markers.len() < 3 {
        return source;
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut pages = 0;
    for (index, marker) in markers.iter().enumerate() {
        let end = markers
            .get(index + 1)
            .map_or(source.len(), |next| next.start());
        let lines: Vec<_> = source[marker.end()..end]
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        if lines.len() < 2 {
            continue;
        }
        let endings: Vec<_> = lines
            .iter()
            .rev()
            .take(4)
            .copied()
            .filter(|line| {
                line.chars().count() < 30 && !line.starts_with(['#', '!', '[', '-', '*'])
            })
            .collect();
        if !endings.is_empty() {
            pages += 1;
            for line in endings {
                *counts.entry(line).or_default() += 1;
            }
        }
    }
    if pages < 3 {
        return source;
    }
    let numeric = |line: &str| !line.is_empty() && line.chars().all(|ch| ch.is_numeric());
    let common: HashSet<_> = counts
        .into_iter()
        .filter(|(line, count)| count * 2 >= pages && !numeric(line))
        .map(|(line, _)| line)
        .collect();
    if common.is_empty() {
        return source;
    }
    let lines: Vec<_> = source.split('\n').collect();
    let mut output = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if common.contains(line.trim())
            || (numeric(line.trim())
                && (1..=2).any(|offset| {
                    index
                        .checked_sub(offset)
                        .is_some_and(|before| common.contains(lines[before].trim()))
                }))
        {
            continue;
        }
        output.push(*line);
    }
    output.join("\n")
}

pub(crate) fn normalize(source: &str) -> String {
    let mut text = source.to_owned();
    while BROKEN_LINK.is_match(&text) {
        text = BROKEN_LINK
            .replace_all(&text, |captures: &regex::Captures<'_>| {
                format!("[{}]({})", captures[1].trim(), &captures[3])
            })
            .into_owned();
    }
    text = clean_footers(text);
    if text.contains("__MARKITAI_") {
        text = PLACEHOLDER_LINE.replace_all(&text, "").into_owned();
        text = PLACEHOLDER_IMAGE.replace_all(&text, "").into_owned();
        text = PLACEHOLDER.replace_all(&text, "").into_owned();
    }
    if text.contains("![") {
        text = DOUBLE_ALT.replace_all(&text, "$1").into_owned();
        text = EMPTY_IMAGE.replace_all(&text, "").into_owned();
        text = EXTRA_PAREN.replace_all(&text, "$1").into_owned();
    }
    let lines: Vec<_> = text.split('\n').map(str::trim_end).collect();
    let mut output: Vec<&str> = Vec::with_capacity(lines.len());
    let mut fence: Option<(u8, usize)> = None;
    for (index, line) in lines.iter().enumerate() {
        let bytes = line.as_bytes();
        if let Some(&ch @ (b'`' | b'~')) = bytes.first() {
            let count = bytes.iter().take_while(|&&item| item == ch).count();
            if count >= 3 {
                match fence {
                    None => fence = Some((ch, count)),
                    Some((opening, size)) if opening == ch && count >= size => fence = None,
                    _ => (),
                }
            }
        }
        let hashes = bytes.iter().take_while(|&&ch| ch == b'#').count();
        let heading = (1..=6).contains(&hashes)
            && line[hashes..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace);
        if fence.is_none() && (heading || SLIDE.is_match(line)) {
            if output.last().is_some_and(|line| !line.is_empty()) {
                output.push("");
            }
            output.push(*line);
            if lines.get(index + 1).is_some_and(|line| !line.is_empty()) {
                output.push("");
            }
        } else {
            output.push(*line);
        }
    }
    let joined = output.join("\n");
    format!("{}\n", BLANKS.replace_all(joined.trim(), "\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_fences_keep_heading_content_and_normalize_only_outer_spacing() {
        let input = "  \n# Heading  \nText\n````md\n```rust\n# code\n```\n~~~~\n## still code\n````\n## Next\nBody\n\n\n";
        assert_eq!(
            normalize(input),
            "# Heading\n\nText\n````md\n```rust\n# code\n```\n~~~~\n## still code\n````\n\n## Next\n\nBody\n"
        );
    }

    #[test]
    fn repairs_links_placeholders_and_repeated_page_footers() {
        assert_eq!(
            normalize(
                "[Title\n\nDescription](/url)\n\n![a]![b](pic))\n![](\u{5f}_MARKITAI_IMAGE_1__)"
            ),
            "[Title](/url)\n\n![b](pic)\n"
        );
        let input = (1..=3).map(|n| format!("<!-- Page number: {n} -->\n# Page {n}\nLong content for page {n} that must remain.\nFooter\n{n}\n")).collect::<String>();
        let output = normalize(&input);
        assert!(!output.contains("Footer"));
        assert!(output.contains("Long content for page 2 that must remain."));
    }
}
