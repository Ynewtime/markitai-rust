//! Output profiles transform references and metadata after conversion finishes.

use crate::VERSION;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

pub(crate) fn apply(markdown: &mut String, metadata: &mut Map<String, Value>, cfg: &Value) {
    match cfg.pointer("/output/profile").and_then(Value::as_str) {
        Some("rag") => *markdown = transform(markdown, true, false, true, Replacements::None),
        Some("obsidian") => {
            let wiki = cfg.pointer("/output/wikilinks").and_then(Value::as_bool) == Some(true);
            *markdown = transform(markdown, true, wiki, false, Replacements::None);
        }
        Some("okf") => okf(metadata),
        _ => (),
    }
}

/// Replace a complete asset destination, retaining link titles and wiki aliases.
/// An empty replacement removes the reference (a filtered image).
/// Literal code and unrelated paths remain untouched.
pub(crate) fn rewrite_asset_target(markdown: &str, previous: &str, next: &str) -> String {
    transform(
        markdown,
        false,
        false,
        false,
        Replacements::Single(previous, next),
    )
}

/// Rewrite original destinations through one map, without visiting inserted text.
/// Keys are decoded paths; percent-encoded references are decoded exactly once.
pub(crate) fn rewrite_asset_references(
    source: &str,
    replacements: &HashMap<String, String>,
) -> String {
    if replacements.is_empty() {
        return source.to_owned();
    }
    transform(
        source,
        false,
        false,
        false,
        Replacements::Many(replacements),
    )
}

#[derive(Clone, Copy)]
enum Replacements<'a> {
    None,
    Single(&'a str, &'a str),
    Many(&'a HashMap<String, String>),
}

#[derive(Clone, Copy)]
enum TargetSyntax {
    Markdown,
    Html,
    Wiki,
}

impl TargetSyntax {
    fn decode(self, value: &str) -> String {
        match self {
            Self::Markdown => unescape(value),
            Self::Html => html_unescape(value),
            Self::Wiki => value.to_owned(),
        }
    }

    fn suffix_start(self, value: &str) -> usize {
        let mut index = 0;
        while index < value.len() {
            let rest = &value[index..];
            let character = rest.chars().next().unwrap();
            if matches!(character, '#' | '?') {
                return index;
            }
            if matches!(self, Self::Markdown) && character == '\\' {
                if let Some(next) = rest[1..].chars().next() {
                    if matches!(next, '#' | '?') {
                        return index;
                    }
                    index += 1 + next.len_utf8();
                    continue;
                }
            } else if matches!(self, Self::Html) && character == '&' {
                // Stop at another '&' or non-entity character, so an unmatched
                // prefix cannot repeatedly scan the remainder of a long tag.
                let mut end = None;
                for (offset, byte) in rest.bytes().enumerate().skip(1) {
                    if byte == b';' {
                        end = Some(offset);
                        break;
                    }
                    if !byte.is_ascii_alphanumeric() && byte != b'#' {
                        break;
                    }
                }
                if let Some(end) = end {
                    let entity = &rest[..=end];
                    let decoded = html_unescape(entity);
                    if matches!(decoded.as_str(), "#" | "?") {
                        return index;
                    }
                    if decoded != entity {
                        index += entity.len();
                        continue;
                    }
                }
            }
            index += character.len_utf8();
        }
        value.len()
    }
}

/// Keep URI syntax separate from a filesystem name. A suffix is still in its
/// original Markdown/HTML/wiki spelling and must not be percent-decoded again.
#[derive(Clone, Copy)]
struct TargetRewrite<'map, 'source> {
    path: &'map str,
    suffix: &'source str,
}

impl TargetRewrite<'_, '_> {
    fn markdown(self, angle: bool) -> String {
        let mut result = destination(self.path, angle);
        result.push_str(self.suffix);
        result
    }

    fn html(self) -> String {
        let mut result = html_escape(&uri_file_path(self.path, false));
        // This span came from the same parsed attribute and retains its entity
        // escaping and percent escapes. Escaping it again changes URI data.
        result.push_str(self.suffix);
        result
    }

    fn wiki(self) -> String {
        let mut result = uri_file_path(self.path, true);
        result.push_str(self.suffix);
        result
    }
}

fn uri_file_path(value: &str, wiki: bool) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '%' => result.push_str("%25"),
            '#' => result.push_str("%23"),
            '?' => result.push_str("%3F"),
            character
                if wiki
                    && (matches!(character, '|' | '[' | ']') || character.is_ascii_control()) =>
            {
                use std::fmt::Write;
                write!(result, "%{:02X}", character as u32).unwrap();
            }
            character => result.push(character),
        }
    }
    result
}

impl<'a> Replacements<'a> {
    fn get(self, path: &str) -> Option<&'a str> {
        match self {
            Self::None => None,
            Self::Single(previous, next) => (previous == path).then_some(next),
            Self::Many(paths) => paths.get(path).map(String::as_str),
        }
        .filter(|next| *next != path)
    }

    fn removes(self) -> bool {
        match self {
            Self::None => false,
            Self::Single(_, next) => next.is_empty(),
            Self::Many(paths) => paths.values().any(String::is_empty),
        }
    }

    fn decoded<'source>(
        self,
        target: &'source str,
        syntax: TargetSyntax,
    ) -> Option<TargetRewrite<'a, 'source>> {
        if matches!(self, Self::None) {
            None
        } else {
            let split = syntax.suffix_start(target);
            self.get(&unquote(&syntax.decode(&target[..split])))
                .map(|path| TargetRewrite {
                    path,
                    suffix: &target[split..],
                })
        }
    }
}

/// Recognize actual image references while excluding Markdown and HTML literals.
pub(crate) fn has_image_references(markdown: &str) -> bool {
    let definitions = definitions(markdown);
    let mut context = LiteralContext::default();
    let mut cursor = 0;
    while let Some((line, literal)) = next_content(markdown, &mut cursor, &mut context) {
        if !literal && definition(line).is_none() && inline_has_images(line, &definitions) {
            return true;
        }
    }
    false
}

// Keep multiline HTML tags together without allocating or treating Markdown
// fences and literal examples as HTML attributes.
fn next_content<'a>(
    source: &'a str,
    cursor: &mut usize,
    context: &mut LiteralContext,
) -> Option<(&'a str, bool)> {
    let start = *cursor;
    if start == source.len() {
        return None;
    }
    let end_of_line = |offset: usize| {
        source[offset..]
            .find('\n')
            .map_or(source.len(), |end| offset + end + 1)
    };
    let mut end = end_of_line(start);
    let literal = context.literal(&source[start..end]);
    if !literal {
        let mut index = start;
        while index < end {
            let tail = &source[index..end];
            if let Some(rest) = tail.strip_prefix('\\') {
                index += 1 + rest.chars().next().map_or(0, char::len_utf8);
            } else if let Some(length) =
                code_span_end(&source[index..]).or_else(|| html_literal_end(&source[index..]))
            {
                index += length;
                if index > end {
                    end = end_of_line(index);
                }
            } else if let Some(reference) = html_reference(&source[index..]) {
                index += reference.tag_end;
                if index > end {
                    end = end_of_line(index);
                }
            } else if let Some(length) = html_tag_end(&source[index..]) {
                index += length;
                if index > end {
                    end = end_of_line(index);
                }
            } else {
                index += tail.chars().next().unwrap().len_utf8();
            }
        }
    }
    *cursor = end;
    Some((&source[start..end], literal))
}

struct Definition<'a> {
    label: String,
    target: &'a str,
    start: usize,
    end: usize,
    angle: bool,
}

fn label(value: &str) -> String {
    unescape(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn closing_bracket(text: &str, start: usize) -> Option<usize> {
    let mut depth = 1;
    let mut index = start;
    while index < text.len() {
        let ch = text[index..].chars().next()?;
        if ch == '\\' {
            index += 1;
            index += text[index..].chars().next()?.len_utf8();
            continue;
        }
        if ch == '[' {
            depth += 1;
        }
        if ch == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
        index += ch.len_utf8();
    }
    None
}

fn definition(line: &str) -> Option<Definition<'_>> {
    let mut index = line.len() - line.trim_start_matches([' ', '\t', '>']).len();
    if !line[index..].starts_with('[') {
        return None;
    }
    let label_start = index + 1;
    index = closing_bracket(line, label_start)?;
    let name = label(&line[label_start..index]);
    if name.is_empty() || !line[index..].starts_with("]:") {
        return None;
    }
    index += 2;
    while line[index..].starts_with([' ', '\t']) {
        index += 1;
    }
    let angle = line[index..].starts_with('<');
    if angle {
        index += 1;
    }
    let start = index;
    let mut depth = 0;
    while index < line.len() {
        let ch = line[index..].chars().next()?;
        if ch == '\\' {
            index += 1;
            index += line[index..].chars().next()?.len_utf8();
            continue;
        }
        if angle && ch == '>' || !angle && ch.is_whitespace() {
            break;
        }
        if ch == '\n' || ch == '\r' {
            return None;
        }
        if !angle && ch == '(' {
            depth += 1;
        }
        if !angle && ch == ')' {
            if depth == 0 {
                return None;
            }
            depth -= 1;
        }
        index += ch.len_utf8();
    }
    if index == start || depth != 0 || angle && !line[index..].starts_with('>') {
        return None;
    }
    let remainder = line[index + usize::from(angle)..].trim();
    if !remainder.is_empty() {
        let opening = remainder.chars().next()?;
        let closing = match opening {
            '\'' => '\'',
            '"' => '"',
            '(' => ')',
            _ => return None,
        };
        if remainder.len() < 2 || !remainder.ends_with(closing) {
            return None;
        }
    }
    Some(Definition {
        label: name,
        target: &line[start..index],
        start,
        end: index,
        angle,
    })
}

fn definitions(markdown: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let mut context = LiteralContext::default();
    let mut cursor = 0;
    while let Some((line, literal)) = next_content(markdown, &mut cursor, &mut context) {
        if !literal && let Some(definition) = definition(line) {
            result
                .entry(definition.label)
                .or_insert_with(|| unquote(&unescape(definition.target)));
        }
    }
    result
}

fn reference_use(text: &str) -> Option<(String, &str, usize, bool)> {
    let (image, start) = if text.starts_with("![") {
        (true, 2)
    } else if text.starts_with('[') {
        (false, 1)
    } else {
        return None;
    };
    let close = closing_bracket(text, start)?;
    let alt = &text[start..close];
    let mut end = close + 1;
    if text[end..].starts_with('(') {
        return None;
    }
    let name = if text[end..].starts_with('[') {
        let close = closing_bracket(text, end + 1)?;
        let explicit = &text[end + 1..close];
        end = close + 1;
        if explicit.is_empty() { alt } else { explicit }
    } else {
        alt
    };
    Some((label(name), alt, end, image))
}

fn code_span_end(text: &str) -> Option<usize> {
    if !text.starts_with('`') {
        return None;
    }
    let width = text.bytes().take_while(|byte| *byte == b'`').count();
    let mut index = width;
    while index < text.len() {
        if text[index..].starts_with('`') {
            let closing = text[index..]
                .bytes()
                .take_while(|byte| *byte == b'`')
                .count();
            index += closing;
            if closing == width {
                return Some(index);
            }
        } else {
            index += text[index..].chars().next()?.len_utf8();
        }
    }
    Some(text.len())
}

fn html_literal_end(text: &str) -> Option<usize> {
    if text.starts_with("<!--") {
        return Some(text.find("-->").map_or(text.len(), |end| end + 3));
    }
    if !text.starts_with('<') {
        return None;
    }
    for tag in ["pre", "code", "script", "style"] {
        if text
            .get(1..tag.len() + 1)
            .is_some_and(|name| name.eq_ignore_ascii_case(tag))
            && text[tag.len() + 1..].starts_with(|ch: char| ch == '>' || ch.is_whitespace())
        {
            let closing = format!("</{tag}>");
            return Some(
                text.to_ascii_lowercase()
                    .find(&closing)
                    .map_or(text.len(), |end| end + closing.len()),
            );
        }
    }
    None
}

struct HtmlReference<'a> {
    target: &'a str,
    start: usize,
    end: usize,
    tag_end: usize,
    attribute_start: usize,
    attribute_end: usize,
    image: bool,
}

fn html_reference(text: &str) -> Option<HtmlReference<'_>> {
    if !text.starts_with('<') {
        return None;
    }
    let bytes = text.as_bytes();
    let mut index = 1;
    while bytes.get(index).is_some_and(u8::is_ascii_alphanumeric) {
        index += 1;
    }
    let image = text[1..index].eq_ignore_ascii_case("img");
    if !image && !text[1..index].eq_ignore_ascii_case("a") {
        return None;
    }
    let wanted = if image { "src" } else { "href" };
    let mut destination = None;
    while index < bytes.len() {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if text[index..].starts_with("/>") {
            index += 1;
        }
        if bytes.get(index) == Some(&b'>') {
            let (start, end, attribute_start, attribute_end) = destination?;
            return Some(HtmlReference {
                target: &text[start..end],
                start,
                end,
                tag_end: index + 1,
                attribute_start,
                attribute_end,
                image,
            });
        }
        let name_start = index;
        while bytes
            .get(index)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !b"=<>/\"'".contains(byte))
        {
            index += 1;
        }
        if index == name_start {
            return None;
        }
        let name = &text[name_start..index];
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index) != Some(&b'=') {
            continue;
        }
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        let quote = bytes
            .get(index)
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'));
        if quote.is_some() {
            index += 1;
        }
        let start = index;
        while let Some(byte) = bytes.get(index) {
            if quote == Some(*byte)
                || quote.is_none() && (byte.is_ascii_whitespace() || *byte == b'>')
            {
                break;
            }
            index += 1;
        }
        let end = index;
        if quote.is_some() {
            if bytes.get(index).copied() != quote {
                return None;
            }
            index += 1;
        }
        if name.eq_ignore_ascii_case(wanted) && destination.is_none() {
            destination = Some((start, end, name_start, index));
        }
    }
    None
}

fn html_tag_end(text: &str) -> Option<usize> {
    let after = text.strip_prefix('<')?;
    let after = after.strip_prefix('/').unwrap_or(after);
    if !after
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
    {
        return None;
    }
    let mut quote = None;
    for (index, byte) in text.bytes().enumerate().skip(1) {
        match (quote, byte) {
            (Some(opening), closing) if opening == closing => quote = None,
            (None, b'\'' | b'"') => quote = Some(byte),
            (None, b'>') => return Some(index + 1),
            (None, b'<') => return None,
            _ => (),
        }
    }
    None
}

fn html_unescape(value: &str) -> String {
    let mut result = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';') else {
            break;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "num" => Some('#'),
            "quest" => Some('?'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|n| u32::from_str_radix(n, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok()))
                .and_then(char::from_u32),
        };
        if let Some(ch) = decoded {
            result.push(ch);
            rest = &rest[end + 1..];
        } else {
            result.push('&');
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}

fn html_escape(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '&' => "&amp;".into(),
            '"' => "&quot;".into(),
            '\'' => "&#39;".into(),
            '<' => "&lt;".into(),
            '>' => "&gt;".into(),
            ch if ch.is_ascii_whitespace() || ch == '`' || ch == '=' => format!("&#{};", ch as u32),
            ch => ch.to_string(),
        })
        .collect()
}

fn destination(value: &str, angle: bool) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '%' => result.push_str("%25"),
            '#' => result.push_str("%23"),
            '?' => result.push_str("%3F"),
            character
                if character.is_ascii_whitespace()
                    || matches!(character, '<' | '>' | '\\')
                    || !angle && matches!(character, '(' | ')' | '[' | ']' | '"' | '\'') =>
            {
                use std::fmt::Write;
                write!(result, "%{:02X}", character as u32).unwrap();
            }
            character => result.push(character),
        }
    }
    result
}

fn inline_has_images(line: &str, definitions: &HashMap<String, String>) -> bool {
    inline_has_images_nested(line, definitions, 0)
}

fn inline_has_images_nested(
    line: &str,
    definitions: &HashMap<String, String>,
    depth: usize,
) -> bool {
    if depth >= 64 {
        return false;
    }
    let mut index = 0;
    while index < line.len() {
        let tail = &line[index..];
        if let Some(rest) = tail.strip_prefix('\\') {
            index += 1 + rest.chars().next().map_or(0, char::len_utf8);
        } else if let Some(end) = code_span_end(tail) {
            index += end;
        } else if let Some(end) = html_literal_end(tail) {
            index += end;
        } else if let Some(reference) = html_reference(tail) {
            if reference.image && !reference.target.is_empty() {
                return true;
            }
            index += reference.tag_end;
        } else if let Some(end) = html_tag_end(tail) {
            index += end;
        } else if tail.starts_with("![[") && tail.contains("]]") {
            return true;
        } else if let Some(reference) = reference(tail) {
            if reference.image || inline_has_images_nested(reference.alt, definitions, depth + 1) {
                return true;
            }
            index += reference.end;
        } else if let Some((name, _, _, true)) = reference_use(tail)
            && definitions.contains_key(&name)
        {
            return true;
        } else {
            index += tail.chars().next().unwrap().len_utf8();
        }
    }
    false
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

fn yaml_quote_end(text: &str, start: usize, quote: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = start;
    while let Some(&byte) = bytes.get(index) {
        if quote == b'"' && byte == b'\\' {
            index += 2;
        } else if byte == quote {
            if quote == b'\'' && bytes.get(index + 1) == Some(&quote) {
                index += 2;
            } else {
                return Some(index + 1);
            }
        } else {
            index += 1;
        }
    }
    None
}

fn yaml_colon(text: &str, start: usize, flow: bool) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = start;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\'' | b'"' => index = yaml_quote_end(text, index + 1, byte)?,
            b':' if flow || bytes.get(index + 1).is_none_or(u8::is_ascii_whitespace) => {
                return Some(index);
            }
            b'[' | b'{' | b']' | b'}' | b',' if flow => return None,
            b'#' if index == start || bytes[index - 1].is_ascii_whitespace() => return None,
            _ => index += 1,
        }
    }
    None
}

fn quoted_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/._~-".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(encoded, "%{byte:02X}").unwrap();
        }
    }
    encoded
}

fn yaml_flow_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0;
    let mut index = start;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\'' | b'"' => {
                index = yaml_quote_end(text, index + 1, byte)?;
                continue;
            }
            b'#' if index == start || bytes[index - 1].is_ascii_whitespace() => {
                index += text[index..].find('\n')?;
            }
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => (),
        }
        index += 1;
    }
    None
}

fn yaml_value_start(text: &str, mut index: usize) -> usize {
    let bytes = text.as_bytes();
    loop {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if !matches!(bytes.get(index), Some(b'!' | b'&')) {
            return index;
        }
        if text[index..].starts_with("!<") {
            let Some(end) = text[index..].find('>') else {
                return text.len();
            };
            index += end + 1;
        } else {
            while bytes
                .get(index)
                .is_some_and(|byte| !byte.is_ascii_whitespace())
            {
                index += 1;
            }
        }
    }
}

// Keep the source spelling around each scalar, including keys, comments and
// quoting. Block scalars and multiline prose are deliberately not path fields.
fn yaml_value_edits(
    text: &str,
    mut index: usize,
    flow: bool,
    replacement: Replacements<'_>,
    edits: &mut Vec<(usize, usize, String)>,
    depth: usize,
) -> usize {
    let bytes = text.as_bytes();
    index = yaml_value_start(text, index);
    if depth >= 64 {
        return text.len();
    }
    let start = index;
    let Some(&opening) = bytes.get(index) else {
        return index;
    };
    if opening == b'[' || opening == b'{' {
        let closing = if opening == b'[' { b']' } else { b'}' };
        index += 1;
        loop {
            while bytes
                .get(index)
                .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b',')
            {
                index += 1;
            }
            if bytes.get(index) == Some(&b'#') {
                if let Some(end) = text[index..].find('\n') {
                    index += end + 1;
                    continue;
                }
                return text.len();
            }
            if bytes.get(index) == Some(&closing) {
                return index + 1;
            }
            if opening == b'{' {
                let Some(colon) = yaml_colon(text, index, true) else {
                    return text.len();
                };
                index = colon + 1;
            }
            let end = yaml_value_edits(text, index, true, replacement, edits, depth + 1);
            if end <= index || end == text.len() {
                return text.len();
            }
            index = end;
        }
    }
    if matches!(opening, b'|' | b'>' | b'!' | b'&' | b'*' | b'#') {
        return text.len();
    }
    let quoted = matches!(opening, b'\'' | b'"');
    let end = if quoted {
        let Some(end) = yaml_quote_end(text, index + 1, opening) else {
            return text.len();
        };
        end
    } else {
        while let Some(&byte) = bytes.get(index) {
            if flow && matches!(byte, b',' | b']' | b'}')
                || byte == b'#' && (index == start || bytes[index - 1].is_ascii_whitespace())
            {
                break;
            }
            index += 1;
        }
        start + text[start..index].trim_end().len()
    };
    let token = &text[start..end];
    let value = if quoted {
        serde_yaml::from_str::<String>(token).ok()
    } else {
        Some(token.to_owned())
    };
    if let Some(value) = value {
        let decoded = unquote(&value);
        if let Some(next) = replacement.get(&decoded) {
            let next = if decoded == value {
                next.to_owned()
            } else {
                quoted_path(next)
            };
            let rendered = match opening {
                b'\'' => format!("'{}'", next.replace('\'', "''")),
                b'"' => serde_json::to_string(&next).unwrap(),
                _ if (!flow || !next.contains([',', '[', ']', '{', '}']))
                    && serde_yaml::from_str::<String>(&next).is_ok_and(|value| value == next) =>
                {
                    next
                }
                _ => serde_json::to_string(&next).unwrap(),
            };
            edits.push((start, end, rendered));
        }
    }
    end
}

fn frontmatter_prefix(source: &str, replacement: Replacements<'_>) -> Option<(String, usize)> {
    if matches!(replacement, Replacements::None) {
        return None;
    }
    let first = source.split_inclusive('\n').next()?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }
    let mut end = first.len();
    let mut closing = None;
    for line in source[first.len()..].split_inclusive('\n') {
        if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            closing = Some((end, end + line.len()));
            break;
        }
        end += line.len();
    }
    let (content_end, prefix_end) = closing?;
    let content = &source[first.len()..content_end];
    if !matches!(
        serde_yaml::from_str::<serde_yaml::Value>(content),
        Ok(serde_yaml::Value::Mapping(_))
    ) {
        return Some((source[..prefix_end].to_owned(), prefix_end));
    }
    let mut output = first.to_owned();
    let mut block_indent = None;
    let mut multiline_quote = None;
    let mut cursor = 0;
    while cursor < content.len() {
        let line_start = cursor;
        cursor += content[cursor..]
            .find('\n')
            .map_or(content.len() - cursor, |end| end + 1);
        let mut line = &content[line_start..cursor];
        let indent = line.len() - line.trim_start_matches(' ').len();
        if let Some(quote) = multiline_quote {
            if yaml_quote_end(line, 0, quote).is_some() {
                multiline_quote = None;
            }
            output.push_str(line);
            continue;
        }
        if let Some(previous) = block_indent {
            if line.trim().is_empty() || indent > previous {
                output.push_str(line);
                continue;
            }
            block_indent = None;
        }
        let mut start = indent;
        let sequence = line[start..].starts_with("- ");
        while line[start..].starts_with("- ") {
            start += 2;
        }
        let value_start = if line[start..].starts_with(['[', '{']) {
            Some(start)
        } else {
            yaml_colon(line, start, false)
                .map(|colon| colon + 1)
                .or(sequence.then_some(start))
        };
        let Some(start) = value_start else {
            output.push_str(line);
            continue;
        };
        let start = yaml_value_start(line, start);
        if !line[start..].trim().is_empty()
            && !line[start..].starts_with(['\'', '"', '[', '{', '|', '>'])
            && content[cursor..]
                .lines()
                .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
                .is_some_and(|next| next.len() - next.trim_start_matches(' ').len() > indent)
        {
            block_indent = Some(indent);
            output.push_str(line);
            continue;
        }
        if line[start..].starts_with(['[', '{'])
            && let Some(end) = yaml_flow_end(&content[line_start..], start)
            && end > line.len()
        {
            let tail = &content[line_start + end..];
            cursor = line_start + end + tail.find('\n').map_or(tail.len(), |end| end + 1);
            line = &content[line_start..cursor];
        }
        match line.as_bytes().get(start).copied() {
            Some(b'|' | b'>') => block_indent = Some(indent),
            Some(quote @ (b'\'' | b'"')) if yaml_quote_end(line, start + 1, quote).is_none() => {
                multiline_quote = Some(quote);
            }
            _ => (),
        }
        let mut edits = Vec::new();
        yaml_value_edits(line, start, false, replacement, &mut edits, 0);
        let mut copied = 0;
        for (start, end, next) in edits {
            output.push_str(&line[copied..start]);
            output.push_str(&next);
            copied = end;
        }
        output.push_str(&line[copied..]);
    }
    output.push_str(&source[content_end..prefix_end]);
    Some((output, prefix_end))
}

fn transform(
    source: &str,
    visible: bool,
    wiki: bool,
    rag: bool,
    replacement: Replacements<'_>,
) -> String {
    let mut context = LiteralContext::default();
    let mut output = String::with_capacity(source.len());
    let mut removed = HashSet::new();
    if replacement.removes() {
        let mut context = LiteralContext::default();
        let mut cursor = 0;
        let mut seen = HashSet::new();
        while let Some((line, literal)) = next_content(source, &mut cursor, &mut context) {
            if !literal
                && let Some(definition) = definition(line)
                && seen.insert(definition.label.clone())
                && replacement
                    .decoded(definition.target, TargetSyntax::Markdown)
                    .is_some_and(|next| next.path.is_empty())
            {
                removed.insert(definition.label);
            }
        }
    }
    let mut cursor = 0;
    if let Some((prefix, end)) = frontmatter_prefix(source, replacement) {
        output.push_str(&prefix);
        cursor = end;
    }
    while let Some((line, literal)) = next_content(source, &mut cursor, &mut context) {
        if literal {
            output.push_str(line);
        } else if let Some(definition) = definition(line) {
            if removed.contains(&definition.label) {
                if line.ends_with('\n') {
                    output.push('\n');
                }
                continue;
            }
            let next = replacement
                .decoded(definition.target, TargetSyntax::Markdown)
                .map(|next| next.markdown(definition.angle))
                .or_else(|| visible.then(|| visible_target(definition.target)).flatten());
            if let Some(next) = next {
                output.push_str(&line[..definition.start]);
                output.push_str(&next);
                output.push_str(&line[definition.end..]);
            } else {
                output.push_str(line);
            }
        } else {
            output.push_str(&inline(line, visible, wiki, rag, replacement, &removed));
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
    replacement: Replacements<'_>,
    removed: &HashSet<String>,
) -> String {
    inline_nested(line, visible, wiki, rag, replacement, removed, 0)
}

fn inline_nested(
    line: &str,
    visible: bool,
    wiki: bool,
    rag: bool,
    replacement: Replacements<'_>,
    removed: &HashSet<String>,
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
        if let Some(end) = code_span_end(tail) {
            output.push_str(&tail[..end]);
            index += end;
            continue;
        }
        if let Some(end) = html_literal_end(tail) {
            if rag && tail.starts_with("<!--") && tail[..end].ends_with("-->") {
                let comment = tail[4..end - 3].trim();
                if let Some(number) = comment.strip_prefix("Page number:").map(str::trim)
                    && !number.is_empty()
                    && number.bytes().all(|byte| byte.is_ascii_digit())
                {
                    output.push_str(&format!("<!-- page: {number} -->"));
                    index += end;
                    continue;
                }
            }
            output.push_str(&tail[..end]);
            index += end;
            continue;
        }
        if let Some(reference) = html_reference(tail) {
            let replaced = replacement.decoded(reference.target, TargetSyntax::Html);
            let next = replaced
                .map(|next| {
                    if next.path.is_empty() {
                        String::new()
                    } else {
                        next.html()
                    }
                })
                .or_else(|| {
                    visible
                        .then(|| visible_target(&html_unescape(reference.target)))
                        .flatten()
                        .map(|next| html_escape(&next))
                });
            if let Some(next) = next {
                if next.is_empty() {
                    if !reference.image {
                        output.push_str(&tail[..reference.attribute_start]);
                        output.push_str(&tail[reference.attribute_end..reference.tag_end]);
                    }
                } else {
                    output.push_str(&tail[..reference.start]);
                    output.push_str(&next);
                    output.push_str(&tail[reference.end..reference.tag_end]);
                }
            } else {
                output.push_str(&tail[..reference.tag_end]);
            }
            index += reference.tag_end;
            continue;
        }
        if let Some(end) = html_tag_end(tail) {
            output.push_str(&tail[..end]);
            index += end;
            continue;
        }
        if let Some((rest, opening, image)) = tail
            .strip_prefix("![[")
            .map(|rest| (rest, "![[", true))
            .or_else(|| tail.strip_prefix("[[").map(|rest| (rest, "[[", false)))
            && let Some(end) = rest.find("]]")
        {
            let body = &rest[..end];
            let (target, alias) = body
                .split_once('|')
                .map_or((body, None), |(a, b)| (a, Some(b)));
            let replaced = replacement.decoded(target, TargetSyntax::Wiki);
            if replaced.is_some_and(|next| next.path.is_empty()) {
                if !image {
                    output.push_str(alias.unwrap_or(target));
                }
                index += end + opening.len() + 2;
                continue;
            }
            let new = replaced
                .map(TargetRewrite::wiki)
                .or_else(|| visible.then(|| visible_target(target)).flatten())
                .unwrap_or_else(|| target.to_owned());
            output.push_str(&format!(
                "{opening}{new}{}]]",
                alias.map(|value| format!("|{value}")).unwrap_or_default()
            ));
            index += end + opening.len() + 2;
            continue;
        }
        if !removed.is_empty()
            && let Some((name, alt, end, image)) = reference_use(tail)
            && removed.contains(&name)
        {
            if !image {
                output.push_str(alt);
            }
            index += end;
            continue;
        }
        if let Some(reference) = reference(tail) {
            let replaced = replacement.decoded(reference.target, TargetSyntax::Markdown);
            if replaced.is_some_and(|next| next.path.is_empty()) {
                if !reference.image {
                    output.push_str(reference.alt);
                }
                index += reference.end;
                continue;
            }
            let target = replaced
                .map(|next| {
                    next.markdown(
                        reference.target_start > 0
                            && tail.as_bytes()[reference.target_start - 1] == b'<',
                    )
                })
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
                        removed,
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

    fn paths(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(from, to)| ((*from).into(), (*to).into()))
            .collect()
    }

    #[test]
    fn bulk_references_use_original_destinations_and_keep_titles_aliases_and_prose() {
        let replacements = paths(&[
            ("assets/a.png", "assets/b.png"),
            ("assets/b.png", "assets/c.png"),
        ]);
        let input = "![a](assets/a.png \"assets/a.png\") [b](assets/b.png)\n![ref][picture]\n[picture]: <assets/a.png> 'Keep title'\n![[assets/a.png|Alias]] [[assets/b.png|Download]]\n<img src='assets/a.png' alt='assets/a.png'> <a href=\"assets/b.png\">assets/a.png</a>\nPlain assets/a.png; ![long](assets/a.png.extra)\n";
        let expected = "![a](assets/b.png \"assets/a.png\") [b](assets/c.png)\n![ref][picture]\n[picture]: <assets/b.png> 'Keep title'\n![[assets/b.png|Alias]] [[assets/c.png|Download]]\n<img src='assets/b.png' alt='assets/a.png'> <a href=\"assets/c.png\">assets/a.png</a>\nPlain assets/a.png; ![long](assets/a.png.extra)\n";
        assert_eq!(rewrite_asset_references(input, &replacements), expected);
    }

    #[test]
    fn encoded_destinations_decode_once_and_matching_is_case_sensitive() {
        let replacements = paths(&[
            ("assets/a b.png", "assets/空 格.png"),
            ("assets/a%20b.png", "assets/literal-percent.png"),
            ("assets/a&b.png", "assets/entity.png"),
        ]);
        let input = "![](<assets/a b.png>) ![](assets/a%20b.png) ![](assets/a%2520b.png) ![](assets/A%20b.png)\n![[assets/a%20b.png|保留]]\n<img src='assets/a&amp;b.png'>\n";
        let result = rewrite_asset_references(input, &replacements);
        assert_eq!(
            result,
            "![](<assets/空%20格.png>) ![](assets/空%20格.png) ![](assets/literal-percent.png) ![](assets/A%20b.png)\n![[assets/空 格.png|保留]]\n<img src='assets/entity.png'>\n"
        );
    }

    #[test]
    fn frontmatter_paths_rewrite_without_reserializing_keys_comments_or_examples() {
        let replacements = paths(&[
            ("assets/a.png", "assets/b.png"),
            ("assets/b.png", "assets/c.png"),
            ("assets/a b.png", "assets/新 图.png"),
        ]);
        let input = "---\r\nscreenshot: assets/a.png # keep\r\nquoted: 'assets/a.png'\r\nencoded: \"assets/a%20b.png\"\r\nassets/a.png: unchanged\r\ncaption: \"Use assets/a.png here\"\r\nnested:\r\n  image: assets/b.png\r\nimages:\r\n  - assets/a.png\r\nflow: [assets/a.png, {image: 'assets/b.png'}]\r\nmulti: [assets/a.png, # keep comment\r\n  assets/b.png]\r\n---\r\nscreenshot: assets/a.png\r\n";
        let expected = "---\r\nscreenshot: assets/b.png # keep\r\nquoted: 'assets/b.png'\r\nencoded: \"assets/%E6%96%B0%20%E5%9B%BE.png\"\r\nassets/a.png: unchanged\r\ncaption: \"Use assets/a.png here\"\r\nnested:\r\n  image: assets/c.png\r\nimages:\r\n  - assets/b.png\r\nflow: [assets/b.png, {image: 'assets/c.png'}]\r\nmulti: [assets/b.png, # keep comment\r\n  assets/c.png]\r\n---\r\nscreenshot: assets/a.png\r\n";
        let result = rewrite_asset_references(input, &replacements);
        assert_eq!(result, expected);
        let yaml = result
            .strip_prefix("---\r\n")
            .unwrap()
            .split_once("\r\n---")
            .unwrap()
            .0;
        let parsed: Value = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed["screenshot"], "assets/b.png");
    }

    #[test]
    fn yaml_literals_and_multiline_prose_remain_byte_exact() {
        let input = "---\nliteral: |\n  image: assets/a.png\n  ![](assets/a.png)\nfolded: >-\n  assets/a.png\nanchored: &example |\n  image: assets/a.png\nquoted: \"first line\n  image: assets/a.png\n  final line\"\nplain: first line\n  assets/a.png\ncontinued: assets/a.png\n  is an example\nactual: assets/a.png\n---\n";
        let expected = input.replacen("actual: assets/a.png", "actual: assets/b.png", 1);
        assert_eq!(
            rewrite_asset_references(input, &paths(&[("assets/a.png", "assets/b.png")])),
            expected
        );
        let malformed = "---\nimage: [assets/a.png\n---\n![](assets/a.png)\n";
        assert_eq!(
            rewrite_asset_references(malformed, &paths(&[("assets/a.png", "assets/b.png")])),
            "---\nimage: [assets/a.png\n---\n![](assets/b.png)\n"
        );
    }

    #[test]
    fn unchanged_mappings_preserve_encoded_and_escaped_destination_spelling() {
        let input = "---\nimage: 'assets/a%2epng' # spelling\n---\n![](<assets/a%2epng>) ![](assets/a\\(b\\).png)\n[ref]: assets/a%2epng \"title\"\n![[assets/a%2epng|Alias]]\n<img src='assets/a&#38;b.png'>\n";
        let replacements = paths(&[
            ("assets/a.png", "assets/a.png"),
            ("assets/a(b).png", "assets/a(b).png"),
            ("assets/a&b.png", "assets/a&b.png"),
        ]);
        assert_eq!(rewrite_asset_references(input, &replacements), input);
    }

    #[test]
    fn replacement_percent_names_remain_distinct_from_space_names_in_each_syntax() {
        let replacements = paths(&[
            ("assets/percent.png", "assets/a%20b.png"),
            ("assets/space.png", "assets/a b.png"),
            ("assets/a%20b.png", "assets/a%20b.png"),
            ("assets/a b.png", "assets/a b.png"),
        ]);
        let input = "![](assets/percent.png) ![](assets/space.png)\n[percent]: <assets/percent.png>\n![[assets/percent.png|Percent]] [[assets/space.png|Space]]\n<img src='assets/percent.png'> <a href='assets/space.png'>Space</a>\n![](assets/%61%2520b.png) ![](assets/a%20b.png)\n";
        let expected = "![](assets/a%2520b.png) ![](assets/a%20b.png)\n[percent]: <assets/a%2520b.png>\n![[assets/a%2520b.png|Percent]] [[assets/a b.png|Space]]\n<img src='assets/a%2520b.png'> <a href='assets/a&#32;b.png'>Space</a>\n![](assets/%61%2520b.png) ![](assets/a%20b.png)\n";
        assert_eq!(rewrite_asset_references(input, &replacements), expected);
        assert_eq!(
            profile(
                "![](.markitai/assets/a%20b.png) <img src='.markitai/assets/a%20b.png'>",
                "rag",
                false
            ),
            "![](assets/a%20b.png) <img src='assets/a%20b.png'>"
        );
    }

    #[test]
    fn uri_suffixes_survive_bulk_collisions_without_chained_mapping() {
        let replacements = paths(&[
            ("assets/a.svg", "assets/b.svg"),
            ("assets/b.svg", "assets/c.svg"),
        ]);
        let input = "![crop](assets/a.svg?size=2%20x#view \"Keep title\") [next](assets/b.svg#page=2)\n[ref]: <assets/a.svg#view> 'Title'\n![[assets/a.svg#view|Alias]] [[assets/b.svg?dl=1#top|File]]\n<img src='assets/a.svg?x=1&amp;y=%23keep#v' alt='assets/a.svg'> <a href=\"assets/b.svg#top\">Download</a>\n![](assets/a.svg.extra#view)\n`![](assets/a.svg#view)`\n";
        let expected = "![crop](assets/b.svg?size=2%20x#view \"Keep title\") [next](assets/c.svg#page=2)\n[ref]: <assets/b.svg#view> 'Title'\n![[assets/b.svg#view|Alias]] [[assets/c.svg?dl=1#top|File]]\n<img src='assets/b.svg?x=1&amp;y=%23keep#v' alt='assets/a.svg'> <a href=\"assets/c.svg#top\">Download</a>\n![](assets/a.svg.extra#view)\n`![](assets/a.svg#view)`\n";
        assert_eq!(rewrite_asset_references(input, &replacements), expected);
    }

    #[test]
    fn literal_uri_marker_filenames_are_encoded_before_original_suffixes() {
        let replacements = paths(&[("assets/a#b?.png", "assets/c#d?%.png")]);
        let input = "![](<assets/a%23b%3F.png?raw=%2523#view>)\n[ref]: assets/a%23b%3F.png#part \"Title\"\n![[assets/a%23b%3F.png#view|Alias]]\n<img src='assets/a%23b%3F.png?x=%26&amp;y=2#v'>\n";
        let expected = "![](<assets/c%23d%3F%25.png?raw=%2523#view>)\n[ref]: assets/c%23d%3F%25.png#part \"Title\"\n![[assets/c%23d%3F%25.png#view|Alias]]\n<img src='assets/c%23d%3F%25.png?x=%26&amp;y=2#v'>\n";
        assert_eq!(rewrite_asset_references(input, &replacements), expected);
        assert_eq!(
            rewrite_asset_target(input, "assets/a#b?.png", "assets/c#d?%.png"),
            expected
        );
    }

    #[test]
    fn encoded_filename_markers_are_distinct_from_uri_delimiters() {
        let replacements = paths(&[
            ("assets/a.png", "assets/base.png"),
            ("assets/a.png#view", "assets/literal#view"),
            ("assets/a.png?query", "assets/literal?query"),
            ("assets/a.png%23view", "assets/percent%23view"),
        ]);
        let input = "![](assets/a.png#view) ![](assets/a.png%23view) ![](assets/a.png?query) ![](assets/a.png%3Fquery) ![](assets/a.png%2523view)\n";
        let expected = "![](assets/base.png#view) ![](assets/literal%23view) ![](assets/base.png?query) ![](assets/literal%3Fquery) ![](assets/percent%2523view)\n";
        assert_eq!(rewrite_asset_references(input, &replacements), expected);
    }

    #[test]
    fn syntax_escaped_uri_separators_keep_original_suffix_bytes() {
        let replacements = paths(&[("assets/a&b.png", "assets/c&d#?.png")]);
        let input = "<img src='assets/a&amp;b.png&#35;view?x=%23'>\n<a href=\"assets/a&#38;b.png&#0000000000000000063;x=1&amp;y=2#v\">x</a>\n<img src='assets/a&amp;b.png&quest;x=1&num;view'>\n![](assets/a&b.png\\#view) ![](assets/a&b.png\\?x=%3F#v)\n";
        let expected = "<img src='assets/c&amp;d%23%3F.png&#35;view?x=%23'>\n<a href=\"assets/c&amp;d%23%3F.png&#0000000000000000063;x=1&amp;y=2#v\">x</a>\n<img src='assets/c&amp;d%23%3F.png&quest;x=1&num;view'>\n![](assets/c&d%23%3F.png\\#view) ![](assets/c&d%23%3F.png\\?x=%3F#v)\n";
        assert_eq!(rewrite_asset_references(input, &replacements), expected);
    }

    #[test]
    fn uri_identity_mappings_and_raw_frontmatter_filepaths_keep_their_semantics() {
        let identity = "---\nimage: 'assets/a#b?.png'\n---\n![](assets/a%23b%3f.png?x=%2523#v) ![[assets/a%23b%3F.png#part|Alias]]\n<img src='assets/a%23b%3f.png&#35;v?x=1&#38;y=2'>\n";
        assert_eq!(
            rewrite_asset_references(identity, &paths(&[("assets/a#b?.png", "assets/a#b?.png")])),
            identity
        );
        let input = "---\nraw: 'assets/a#b?.png' # literal filesystem name\nencoded: \"assets/a%23b%3F.png\"\nunrelated: 'assets/a.png#view'\n---\n![](assets/a%23b%3F.png#view)\n";
        let replacements = paths(&[
            ("assets/a#b?.png", "assets/c#d?%.png"),
            ("assets/a.png", "assets/base.png"),
        ]);
        let expected = "---\nraw: 'assets/c#d?%.png' # literal filesystem name\nencoded: \"assets/c%23d%3F%25.png\"\nunrelated: 'assets/a.png#view'\n---\n![](assets/c%23d%3F%25.png#view)\n";
        assert_eq!(rewrite_asset_references(input, &replacements), expected);
    }

    #[test]
    fn removal_of_suffixed_links_uses_the_file_part_in_all_reference_forms() {
        let input = "![photo][ref] [download][ref]\n[ref]: assets/a.png#view \"Title\"\n![direct](assets/a.png?size=2) ![[assets/a.png#view|Photo]]\n<img src='assets/a.png?x=1&amp;y=2'> <a href='assets/a.png#v'>link</a>\n![](assets/a%23b.png#view)\n";
        let replacements = paths(&[("assets/a.png", "")]);
        let result = rewrite_asset_references(input, &replacements);
        assert_eq!(result, rewrite_asset_target(input, "assets/a.png", ""));
        assert_eq!(
            result,
            " download\n\n \n <a >link</a>\n![](assets/a%23b.png#view)\n"
        );
    }

    #[test]
    fn multiline_literals_and_unrelated_html_attributes_are_not_destinations() {
        let literals = "```md\n![](assets/a.png)\n```\n    ![](assets/a.png)\n`code starts\n![](assets/a.png)\nends`\n<!-- comment\n![](assets/a.png)\n[id]: assets/a.png\n-->\n<div title=\"![example](assets/a.png)\"\n data-text='![[assets/a.png]]'>literal</div>\n<code>\n![](assets/a.png)\n</code>\n";
        let input = format!("{literals}![](assets/a.png)\n");
        let replacements = paths(&[("assets/a.png", "assets/b.png")]);
        assert_eq!(
            rewrite_asset_references(&input, &replacements),
            format!("{literals}![](assets/b.png)\n")
        );
        assert_eq!(
            rewrite_asset_target(&input, "assets/a.png", "assets/b.png"),
            rewrite_asset_references(&input, &replacements)
        );
    }

    #[test]
    fn bulk_removals_keep_reference_labels_and_single_target_contract() {
        let input = "![photo][id] [download][id]\n[id]: assets/a.png \"Title\"\n<img src='assets/a.png'> <a href='assets/a.png'>Download</a>\n![[assets/a.png|Photo]] [[assets/a.png|Alias]]\n`![literal](assets/a.png)`\n";
        let replacements = paths(&[("assets/a.png", "")]);
        let result = rewrite_asset_references(input, &replacements);
        assert_eq!(result, rewrite_asset_target(input, "assets/a.png", ""));
        assert!(result.contains(" download\n\n"));
        assert!(result.contains("<a >Download</a>"));
        assert!(result.contains(" Alias\n"));
        assert!(result.contains("`![literal](assets/a.png)`"));
    }

    #[test]
    fn large_mapping_does_not_cascade_or_substitute_path_prefixes() {
        let mut replacements = HashMap::new();
        let mut input = String::new();
        let mut expected = String::new();
        for index in 0..1024 {
            replacements.insert(
                format!("assets/{index}.png"),
                format!("assets/{}.png", index + 1),
            );
            input.push_str(&format!(
                "![](assets/{index}.png) ![](assets/{index}.png.extra)\n"
            ));
            expected.push_str(&format!(
                "![](assets/{}.png) ![](assets/{index}.png.extra)\n",
                index + 1
            ));
        }
        assert_eq!(rewrite_asset_references(&input, &replacements), expected);
        assert_eq!(rewrite_asset_references(&input, &HashMap::new()), input);
    }

    #[test]
    fn multiline_html_attributes_remain_complete_and_literals_stay_unchanged() {
        let input = "Before <img\n src=\".markitai/assets/a.png\"\n alt=\"title\"> after\n<a\n href='.markitai/assets/a.png'>download</a>\n";
        let rewritten = rewrite_asset_target(input, ".markitai/assets/a.png", "assets/hash.png");
        assert_eq!(rewritten.matches("assets/hash.png").count(), 2);
        assert!(!rewritten.contains(".markitai"));
        assert!(has_image_references(input));
        let literal = "```html\n<img\n src=\".markitai/assets/a.png\">\n```\n`<img`\n";
        assert_eq!(
            rewrite_asset_target(literal, ".markitai/assets/a.png", "assets/hash.png"),
            literal
        );
        assert!(!has_image_references(literal));
    }

    #[test]
    fn reference_definitions_keep_titles_and_match_complete_decoded_targets() {
        let input = "![one][image]\n![image][]\n![image]\n[image]: <.markitai/assets/a%20b.png> \"A title\"\n[other]: .markitai/assets/a%20b.png.extra 'Keep'\n";
        let result = rewrite_asset_target(
            input,
            ".markitai/assets/a b.png",
            ".markitai/assets/new name.png",
        );
        assert!(result.contains("[image]: <.markitai/assets/new%20name.png> \"A title\""));
        assert!(result.contains("[other]: .markitai/assets/a%20b.png.extra 'Keep'"));
        let visible = profile(&result, "rag", false);
        assert!(visible.contains("[image]: <assets/new%20name.png> \"A title\""));
        assert!(has_image_references(&visible));
    }

    #[test]
    fn html_asset_attributes_preserve_other_attributes_and_escape_replacements() {
        let input = "<IMG class='photo' SRC=\".markitai/assets/a&amp;b.png\" data-src='unchanged' title='tip'> <a href='.markitai/assets/a&amp;b.png'>Download</a> <img src='.markitai/assets/a&amp;b.png.extra'>";
        let result = rewrite_asset_target(
            input,
            ".markitai/assets/a&b.png",
            ".markitai/assets/new & name.png",
        );
        assert!(result.contains("SRC=\".markitai/assets/new&#32;&amp;&#32;name.png\""));
        assert!(result.contains("data-src='unchanged' title='tip'"));
        assert!(result.contains("href='.markitai/assets/new&#32;&amp;&#32;name.png'"));
        assert!(result.contains("src='.markitai/assets/a&amp;b.png.extra'"));
        assert!(has_image_references(&result));
        assert!(!has_image_references(
            "<img data-src='not-loaded.png'> <a href='image.png'>link</a>"
        ));
    }

    #[test]
    fn filtered_reference_images_disappear_but_download_text_and_literals_remain() {
        let input = "![one][id] ![id][] ![id] [download][id]\n[id]: assets/a.png \"Title\"\n<img src='assets/a.png'> <a href='assets/a.png' class='download'>Download</a>\n`![id]`\n";
        let result = rewrite_asset_target(input, "assets/a.png", "");
        assert!(!result.contains("[one]"));
        assert!(!result.contains("[id]:"));
        assert!(result.contains("download"));
        assert!(!result.contains("<img"));
        assert!(!result.contains("href="));
        assert!(result.contains("class='download'>Download</a>"));
        assert!(result.contains("`![id]`"));
        assert!(!has_image_references(&result));
    }

    #[test]
    fn image_detection_and_rewrites_ignore_literal_code_and_comments() {
        let literals = "```md\n![example](assets/a.png)\n[id]: assets/a.png\n<img src='assets/a.png'>\n```\n    ![indented](assets/a.png)\n`<img src='assets/a.png'>` and `![example](assets/a.png)`\n<pre>![pre](assets/a.png)</pre>\nText <code><img src='assets/a.png'></code>\n<!-- ![comment](assets/a.png) -->\n";
        assert!(!has_image_references(literals));
        assert_eq!(
            rewrite_asset_target(literals, "assets/a.png", "assets/hash.png"),
            literals
        );
        for actual in [
            "![actual](assets/a.png)",
            "![[assets/a.png|Caption]]",
            "<img src='assets/a.png'>",
            "![id]\n[id]: assets/a.png",
            "[![photo](assets/a.png)](https://example.test)",
        ] {
            assert!(
                has_image_references(&format!("{literals}\n{actual}")),
                "{actual}"
            );
        }
        assert!(!has_image_references("![unresolved]"));
        assert!(!has_image_references("\\![escaped](assets/a.png)"));
        assert!(!has_image_references("[unused]: assets/a.png"));
    }

    #[test]
    fn written_enhanced_markdown_keeps_reference_and_html_assets_resolvable() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::normalize(&json!({"output":{"profile":"rag"}})).unwrap();
        let mut result = crate::ConversionOutput {
            markdown: "![base](.markitai/assets/image.png)".into(),
            llm_markdown: Some("![photo][img]\n\n[img]: .markitai/assets/image.png \"Original\"\n<img src=\".markitai/assets/image.png\">\n<a href=\".markitai/assets/image.png\">Original</a>\n".into()),
            ..Default::default()
        };
        crate::output::apply_profiles(&mut result, &cfg);
        crate::output::write(
            dir.path(),
            "scan.png",
            &mut result,
            &[crate::Asset {
                name: "image.png".into(),
                bytes: b"fixture-image-bytes".to_vec(),
            }],
            &cfg,
        )
        .unwrap();
        let filename = result.assets[0].file_name().unwrap().to_str().unwrap();
        let written = std::fs::read_to_string(result.llm_output_path.unwrap()).unwrap();
        assert_eq!(written.matches(&format!("assets/{filename}")).count(), 3);
        assert!(!written.contains("assets/image.png"));
        assert!(result.assets[0].is_file());
    }

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
