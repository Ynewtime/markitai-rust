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

fn hidden_inline_style(style: &str) -> bool {
    // Parse declaration boundaries before inspecting property names. Semicolons
    // inside strings, comments or function/block values are not separators.
    fn record(declaration: &str, properties: &mut [Option<(bool, bool)>; 2]) {
        let Some((property, value)) = declaration.split_once(':') else {
            return;
        };
        let property = property.trim();
        let index = if property.eq_ignore_ascii_case("display") {
            0
        } else if property.eq_ignore_ascii_case("visibility") {
            1
        } else {
            return;
        };
        let value = value.trim();
        let (value, important) = value
            .rsplit_once('!')
            .filter(|(_, priority)| priority.trim().eq_ignore_ascii_case("important"))
            .map_or((value, false), |(value, _)| (value.trim(), true));
        // Strings and other invalid keyword values cannot create a hiding rule
        // or override an earlier valid declaration. CSS variable evaluation and
        // stylesheets belong to a browser's computed-style model.
        let keyword = value.to_ascii_lowercase();
        let hidden = match (index, keyword.as_str()) {
            (0, "none") | (1, "hidden" | "collapse") => true,
            (
                0,
                "block" | "inline" | "inline-block" | "flow-root" | "flex" | "inline-flex" | "grid"
                | "inline-grid" | "table" | "inline-table" | "table-row" | "table-cell"
                | "table-caption" | "table-column" | "table-column-group" | "table-row-group"
                | "table-header-group" | "table-footer-group" | "list-item" | "contents",
            )
            | (1, "visible")
            | (_, "initial" | "inherit" | "unset" | "revert" | "revert-layer") => false,
            _ => return,
        };
        if properties[index].is_none_or(|(_, previous_important)| important || !previous_important)
        {
            properties[index] = Some((hidden, important));
        }
    }

    let mut properties = [None; 2];
    let mut declaration = String::new();
    let mut quote = None;
    let mut nesting = 0usize;
    let mut characters = style.chars().peekable();
    while let Some(character) = characters.next() {
        if let Some(delimiter) = quote {
            declaration.push(character);
            if character == '\\' {
                if let Some(escaped) = characters.next() {
                    declaration.push(escaped);
                }
            } else if character == delimiter {
                quote = None;
            }
            continue;
        }
        if character == '/' && characters.peek() == Some(&'*') {
            characters.next();
            while let Some(comment) = characters.next() {
                if comment == '*' && characters.peek() == Some(&'/') {
                    characters.next();
                    break;
                }
            }
            declaration.push(' ');
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' | '[' | '{' => nesting += 1,
            ')' | ']' | '}' => nesting = nesting.saturating_sub(1),
            ';' if nesting == 0 => {
                record(&declaration, &mut properties);
                declaration.clear();
                continue;
            }
            '\\' => {
                declaration.push(character);
                if let Some(escaped) = characters.next() {
                    declaration.push(escaped);
                }
                continue;
            }
            _ => {}
        }
        declaration.push(character);
    }
    if quote.is_none() && nesting == 0 {
        record(&declaration, &mut properties);
    }
    properties.into_iter().flatten().any(|(hidden, _)| hidden)
}

fn is_hidden(element: ElementRef<'_>) -> bool {
    let value = element.value();
    value.attr("hidden").is_some() || value.attr("style").is_some_and(hidden_inline_style)
}

fn has_class(element: ElementRef<'_>, class: &str) -> bool {
    element
        .value()
        .attr("class")
        .is_some_and(|classes| classes.split_whitespace().any(|value| value == class))
}

fn tex_script(element: ElementRef<'_>) -> Option<bool> {
    if element.value().name() != "script" {
        return None;
    }
    let mut parts = element.value().attr("type")?.split(';');
    if !parts.next()?.trim().eq_ignore_ascii_case("math/tex") {
        return None;
    }
    Some(parts.any(|part| {
        part.split_once('=').is_some_and(|(key, value)| {
            key.trim().eq_ignore_ascii_case("mode") && value.trim().eq_ignore_ascii_case("display")
        })
    }))
}

fn math_symbol(value: &str) -> String {
    let symbol = match value {
        "α" => r"\alpha",
        "β" => r"\beta",
        "γ" => r"\gamma",
        "δ" => r"\delta",
        "ε" => r"\epsilon",
        "ζ" => r"\zeta",
        "η" => r"\eta",
        "θ" => r"\theta",
        "ι" => r"\iota",
        "κ" => r"\kappa",
        "λ" => r"\lambda",
        "μ" => r"\mu",
        "ν" => r"\nu",
        "ξ" => r"\xi",
        "π" => r"\pi",
        "ρ" => r"\rho",
        "σ" => r"\sigma",
        "τ" => r"\tau",
        "υ" => r"\upsilon",
        "ϕ" => r"\phi",
        "φ" => r"\varphi",
        "χ" => r"\chi",
        "ψ" => r"\psi",
        "ω" => r"\omega",
        "Γ" => r"\Gamma",
        "Δ" => r"\Delta",
        "Θ" => r"\Theta",
        "Λ" => r"\Lambda",
        "Ξ" => r"\Xi",
        "Π" => r"\Pi",
        "Σ" => r"\Sigma",
        "Φ" => r"\Phi",
        "Ψ" => r"\Psi",
        "Ω" => r"\Omega",
        "≠" => r"\neq",
        "≤" => r"\leq",
        "≥" => r"\geq",
        "±" => r"\pm",
        "∓" => r"\mp",
        "×" => r"\times",
        "÷" => r"\div",
        "∞" => r"\infty",
        "∑" => r"\sum",
        "∏" => r"\prod",
        "∫" => r"\int",
        "∮" => r"\oint",
        "∂" => r"\partial",
        "∇" => r"\nabla",
        "→" => r"\rightarrow",
        "←" => r"\leftarrow",
        "⇒" => r"\Rightarrow",
        "⇐" => r"\Leftarrow",
        "↦" => r"\mapsto",
        "∈" => r"\in",
        "∉" => r"\notin",
        "⊂" => r"\subset",
        "⊃" => r"\supset",
        "⊆" => r"\subseteq",
        "⊇" => r"\supseteq",
        "∪" => r"\cup",
        "∩" => r"\cap",
        "∧" => r"\wedge",
        "∨" => r"\vee",
        "¬" => r"\neg",
        "∅" => r"\emptyset",
        "∀" => r"\forall",
        "∃" => r"\exists",
        "⟹" => r"\implies",
        "⟸" => r"\impliedby",
        "⟺" => r"\iff",
        "⟨" => r"\langle",
        "⟩" => r"\rangle",
        "⌊" => r"\lfloor",
        "⌋" => r"\rfloor",
        "⌈" => r"\lceil",
        "⌉" => r"\rceil",
        "⋯" => r"\cdots",
        "…" => r"\ldots",
        "⋅" | "·" => r"\cdot",
        "−" => "-",
        "⁡" | "⁢" | "⁣" => "",
        _ => value,
    };
    symbol.to_owned()
}

fn inert_math_node(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "annotation-xml" | "iframe" | "object" | "embed" | "template"
    )
}

fn math_text(element: ElementRef<'_>, depth: usize) -> Result<String> {
    if inert_math_node(element.value().name()) {
        return Ok(String::new());
    }
    if depth > 64 {
        return Err(Error::Conversion(
            "MathML nesting exceeds 64 elements".into(),
        ));
    }
    let mut result = String::new();
    for child in element.children() {
        if let Some(child) = ElementRef::wrap(child) {
            result.push_str(&math_text(child, depth + 1)?);
        } else if let scraper::Node::Text(text) = child.value() {
            result.push_str(text);
        }
    }
    Ok(result)
}

fn mathml(element: ElementRef<'_>, depth: usize) -> Result<String> {
    if inert_math_node(element.value().name()) {
        return Ok(String::new());
    }
    if depth > 64 {
        return Err(Error::Conversion(
            "MathML nesting exceeds 64 elements".into(),
        ));
    }
    let name = element.value().name();
    let children = element.child_elements().collect::<Vec<_>>();
    let mut parts = Vec::new();
    for child in element.children() {
        if let Some(child) = ElementRef::wrap(child) {
            if !matches!(child.value().name(), "annotation" | "annotation-xml") {
                parts.push(mathml(child, depth + 1)?);
            }
        } else if let scraper::Node::Text(text) = child.value()
            && !text.trim().is_empty()
        {
            parts.push(text.trim().to_owned());
        }
    }
    let joined = parts
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let text = if matches!(name, "mi" | "mn" | "mo" | "mtext" | "ms") {
        math_text(element, depth)?
    } else {
        String::new()
    };
    let text = text.trim();
    let at = |index: usize| parts.get(index).map(String::as_str).unwrap_or("");
    Ok(match name {
        "semantics" => parts.into_iter().next().unwrap_or_default(),
        "annotation" | "annotation-xml" | "none" | "mprescripts" => String::new(),
        "mi" if text.chars().count() > 1 && text.chars().all(char::is_alphabetic) => {
            format!(r"\mathrm{{{text}}}")
        }
        "mi" | "mn" | "mo" => math_symbol(text),
        "mtext" | "ms" => {
            let mut escaped = String::new();
            for character in text.chars() {
                match character {
                    '\\' => escaped.push_str(r"\textbackslash{}"),
                    '{' | '}' | '$' | '&' | '%' | '#' | '_' => {
                        escaped.push('\\');
                        escaped.push(character);
                    }
                    '^' => escaped.push_str(r"\textasciicircum{}"),
                    '~' => escaped.push_str(r"\textasciitilde{}"),
                    _ => escaped.push(character),
                }
            }
            format!(r"\text{{{escaped}}}")
        }
        "mspace" => r"\;".into(),
        "msup" if parts.len() >= 2 => format!("{}^{{{}}}", at(0), at(1)),
        "msub" if parts.len() >= 2 => format!("{}_{{{}}}", at(0), at(1)),
        "msubsup" if parts.len() >= 3 => format!("{}_{{{}}}^{{{}}}", at(0), at(1), at(2)),
        "mfrac" if parts.len() >= 2 => format!(r"\frac{{{}}}{{{}}}", at(0), at(1)),
        "msqrt" => format!(r"\sqrt{{{joined}}}"),
        "mroot" if parts.len() >= 2 => format!(r"\sqrt[{}]{{{}}}", at(1), at(0)),
        "mover" if parts.len() >= 2 => {
            let raw = children
                .get(1)
                .map(|element| math_text(*element, depth + 1))
                .transpose()?
                .unwrap_or_default();
            let accent = match raw.trim() {
                "˙" | "̇" => Some("dot"),
                "¨" | "̈" => Some("ddot"),
                "¯" | "‾" => Some("overline"),
                "^" | "̂" => Some("hat"),
                "~" | "̃" | "˜" => Some("tilde"),
                "→" | "⃗" => Some("vec"),
                "⏞" => Some("overbrace"),
                _ => None,
            };
            accent.map_or_else(
                || format!(r"\overset{{{}}}{{{}}}", at(1), at(0)),
                |accent| format!("\\{accent}{{{}}}", at(0)),
            )
        }
        "munder" if parts.len() >= 2 => format!(r"\underset{{{}}}{{{}}}", at(1), at(0)),
        "munderover" if parts.len() >= 3 => format!("{}_{{{}}}^{{{}}}", at(0), at(1), at(2)),
        "mtable" => format!(r"\begin{{aligned}}{}\end{{aligned}}", parts.join(r" \\ ")),
        "mtr" | "mlabeledtr" => parts.join(" & "),
        "mfenced" => {
            let open = element.value().attr("open").unwrap_or("(");
            let close = element.value().attr("close").unwrap_or(")");
            let separators = element
                .value()
                .attr("separators")
                .unwrap_or(",")
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<Vec<_>>();
            let mut content = String::new();
            for (index, part) in parts.iter().enumerate() {
                if index > 0 && !separators.is_empty() {
                    content.push(separators[(index - 1).min(separators.len() - 1)]);
                    content.push(' ');
                }
                content.push_str(part);
            }
            let delimiter = |value: &str| match value {
                "" => ".".into(),
                "{" => r"\{".into(),
                "}" => r"\}".into(),
                value => math_symbol(value),
            };
            format!(
                r"\left{}{content}\right{}",
                delimiter(open),
                delimiter(close)
            )
        }
        "menclose" if element.value().attr("notation") == Some("top") => {
            format!(r"\overline{{{joined}}}")
        }
        "mphantom" => format!(r"\phantom{{{joined}}}"),
        _ => joined,
    })
}

fn math_expression(element: ElementRef<'_>) -> Result<Option<(String, bool)>> {
    if let Some(block) = tex_script(element) {
        let text = element.text().collect::<String>();
        return Ok((!text.trim().is_empty()).then(|| (text.trim().to_owned(), block)));
    }
    let recognized = element.value().name() == "math"
        || element.value().name() == "mjx-container"
        || [
            "katex",
            "katex-display",
            "math-inline",
            "math-block",
            "hurmet-tex",
            "mwe-math-element",
        ]
        .into_iter()
        .any(|class| has_class(element, class));
    if !recognized {
        return Ok(None);
    }
    let inner = if element.value().name() == "math" {
        Some(element)
    } else {
        element.select(&selector("math")).next()
    };
    let block = has_class(element, "katex-display")
        || has_class(element, "math-block")
        || matches!(element.value().attr("display"), Some("block" | "true"))
        || inner.is_some_and(|math| math.value().attr("display") == Some("block"));
    let attribute = |element: ElementRef<'_>| {
        ["data-latex", "data-math", "data-entry", "alttext"]
            .into_iter()
            .find_map(|key| {
                element
                    .value()
                    .attr(key)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
            })
    };
    let mut latex = attribute(element).or_else(|| inner.and_then(attribute));
    if latex.is_none() {
        latex = element
            .select(&selector("annotation"))
            .find(|node| {
                node.value().attr("encoding").is_some_and(|encoding| {
                    matches!(
                        encoding.to_ascii_lowercase().as_str(),
                        "application/x-tex" | "application/x-latex" | "text/tex"
                    )
                })
            })
            .map(|node| math_text(node, 0))
            .transpose()?
            .filter(|text| !text.trim().is_empty());
    }
    if latex.is_none()
        && let Some(math) = inner
    {
        latex = Some(mathml(math, 0)?);
    }
    if latex.is_none() && has_class(element, "mwe-math-element") {
        latex = element.select(&selector("img[alt]")).find_map(|image| {
            image
                .value()
                .attr("alt")
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        });
    }
    Ok(latex
        .filter(|text| !text.trim().is_empty())
        .map(|text| (text.trim().to_owned(), block)))
}

fn duplicate_math_preview(element: ElementRef<'_>) -> bool {
    [
        "MathJax_Preview",
        "MathJax",
        "MathJax_Display",
        "MathJax_SVG",
        "MathJax_MathML",
    ]
    .into_iter()
    .any(|class| has_class(element, class))
        && element
            .parent()
            .and_then(ElementRef::wrap)
            .is_some_and(|parent| {
                parent.child_elements().any(|sibling| {
                    tex_script(sibling).is_some()
                        && sibling.text().any(|text| !text.trim().is_empty())
                })
            })
}

fn emit_math(latex: &str, block: bool, output: &mut String) {
    output.push_str("<markitai-math data-latex=\"");
    escaped(latex, output);
    output.push_str(if block {
        "\" data-display=\"block\"></markitai-math>"
    } else {
        "\"></markitai-math>"
    });
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
    if is_hidden(element) || duplicate_math_preview(element) {
        return Ok(());
    }
    if !element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .any(|ancestor| matches!(ancestor.value().name(), "pre" | "code"))
        && let Some((latex, block)) = math_expression(element)?
    {
        emit_math(&latex, block, output);
        return Ok(());
    }
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
    ) {
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
            vec!["markitai-math"],
            |_: &dyn htmd::element_handler::Handlers, element: htmd::Element| {
                let latex = element
                    .attrs
                    .iter()
                    .find(|attr| attr.name.local.as_ref() == "data-latex")?
                    .value
                    .as_ref();
                // Emit TeX syntax as data, never HTML supplied in an annotation.
                let latex = latex.replace('<', r"\lt ").replace('>', r"\gt ");
                let block = element.attrs.iter().any(|attr| {
                    attr.name.local.as_ref() == "data-display" && attr.value.as_ref() == "block"
                });
                Some(
                    if block {
                        format!("\n\n$${latex}$$\n\n")
                    } else {
                        format!("${latex}$")
                    }
                    .into(),
                )
            },
        )
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

#[derive(Debug)]
enum BbNode {
    Text(String),
    Tag {
        name: String,
        argument: Option<String>,
        children: Vec<BbNode>,
    },
}

struct BbFrame {
    name: String,
    argument: Option<String>,
    children: Vec<BbNode>,
}

fn bbcode_tree(source: &str) -> Result<Vec<BbNode>> {
    fn close(stack: &mut Vec<BbFrame>) {
        let frame = stack.pop().expect("a non-root BBCode frame");
        stack
            .last_mut()
            .expect("BBCode root frame")
            .children
            .push(BbNode::Tag {
                name: frame.name,
                argument: frame.argument,
                children: frame.children,
            });
    }
    let mut stack = vec![BbFrame {
        name: String::new(),
        argument: None,
        children: Vec::new(),
    }];
    let mut offset = 0;
    while offset < source.len() {
        let Some(relative_start) = source[offset..].find('[') else {
            stack
                .last_mut()
                .unwrap()
                .children
                .push(BbNode::Text(source[offset..].to_owned()));
            break;
        };
        let start = offset + relative_start;
        if start > offset {
            stack
                .last_mut()
                .unwrap()
                .children
                .push(BbNode::Text(source[offset..start].to_owned()));
        }
        let mut quote = None;
        let mut end = None;
        // Backslash-quoted delimiters occur in older event-store payloads even
        // after JSON decoding; the token normalizer below handles that form.
        for (index, character) in source[start + 1..].char_indices() {
            if character == '\'' || character == '"' {
                if quote == Some(character) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(character);
                }
            } else if character == ']' && quote.is_none() {
                end = Some(start + 1 + index);
                break;
            }
        }
        let Some(end) = end else {
            stack
                .last_mut()
                .unwrap()
                .children
                .push(BbNode::Text(source[start..].to_owned()));
            break;
        };
        offset = end + 1;
        let token = source[start + 1..end]
            .replace(r"\/", "/")
            .replace("\\\"", "\"");
        let token = token.trim();
        let closing = token.starts_with('/');
        let token = token.strip_prefix('/').unwrap_or(token);
        let (name, argument) = token
            .split_once('=')
            .map_or((token, None), |(name, value)| {
                (
                    name,
                    Some(value.trim().trim_matches(['\'', '"']).to_owned()),
                )
            });
        let name = name.trim().to_ascii_lowercase();
        let name = if name == "*" { "li" } else { &name };
        if !matches!(
            name,
            "p" | "b"
                | "i"
                | "u"
                | "s"
                | "url"
                | "img"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "list"
                | "li"
                | "code"
                | "quote"
                | "previewyoutube"
        ) {
            stack
                .last_mut()
                .unwrap()
                .children
                .push(BbNode::Text(source[start..offset].to_owned()));
            continue;
        }
        if closing {
            if let Some(position) = stack.iter().rposition(|frame| frame.name == name) {
                while stack.len() > position {
                    close(&mut stack);
                }
            } else {
                stack
                    .last_mut()
                    .unwrap()
                    .children
                    .push(BbNode::Text(source[start..offset].to_owned()));
            }
            continue;
        }
        if name == "code" {
            let tail = &source[offset..];
            let lower = tail.to_ascii_lowercase();
            let close = lower
                .find("[/code]")
                .map(|index| (index, 7))
                .or_else(|| lower.find(r"[\/code]").map(|index| (index, 8)));
            let (length, closing_length) = close.unwrap_or((tail.len(), 0));
            stack.last_mut().unwrap().children.push(BbNode::Tag {
                name: name.into(),
                argument,
                children: vec![BbNode::Text(tail[..length].to_owned())],
            });
            offset += length + closing_length;
            continue;
        }
        if name == "li"
            && let Some(list) = stack.iter().rposition(|frame| frame.name == "list")
            && let Some(item) = stack
                .iter()
                .enumerate()
                .skip(list + 1)
                .find_map(|(index, frame)| (frame.name == "li").then_some(index))
        {
            while stack.len() > item {
                close(&mut stack);
            }
        }
        if stack.len() > 64 {
            return Err(Error::Conversion("BBCode nesting exceeds 64 tags".into()));
        }
        stack.push(BbFrame {
            name: name.into(),
            argument,
            children: Vec::new(),
        });
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    Ok(stack.pop().unwrap().children)
}

fn bbcode_text(nodes: &[BbNode]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            BbNode::Text(text) => text.clone(),
            BbNode::Tag { children, .. } => bbcode_text(children),
        })
        .collect()
}

fn escaped_bbcode_text(text: &str, output: &mut String) {
    // Keep angle brackets encoded after the HTML renderer's entity decoding,
    // so all displayed BBCode text stays literal in Markdown. Code nodes use
    // a separate fenced path; destinations use URL validation instead.
    escaped(&text.replace('<', "&lt;").replace('>', "&gt;"), output);
}

fn bbcode_html(nodes: &[BbNode], base: Option<&Url>, output: &mut String) {
    for node in nodes {
        match node {
            BbNode::Text(text) => {
                for (index, line) in text.split('\n').enumerate() {
                    if index > 0 {
                        output.push_str("<br>");
                    }
                    escaped_bbcode_text(line, output);
                }
            }
            BbNode::Tag {
                name,
                argument,
                children,
            } => {
                let argument = argument.as_deref().unwrap_or("");
                if name == "code" {
                    output.push_str("<pre><code");
                    if !argument.is_empty()
                        && argument
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
                    {
                        output.push_str(" class=\"language-");
                        escaped(argument, output);
                        output.push('"');
                    }
                    output.push('>');
                    escaped(&bbcode_text(children), output);
                    output.push_str("</code></pre>");
                    continue;
                }
                if matches!(name.as_str(), "url" | "img") {
                    let text = bbcode_text(children);
                    let destination = if argument.is_empty() {
                        text.as_str()
                    } else {
                        argument
                    };
                    let destination = destination.replace(r"\/", "/");
                    if let Some(url) = safe_url(&destination, base) {
                        output.push_str(if name == "img" {
                            "<img src=\""
                        } else {
                            "<a href=\""
                        });
                        escaped(&url, output);
                        output.push_str("\">");
                        if name == "url" {
                            bbcode_html(children, base, output);
                            output.push_str("</a>");
                        }
                    } else {
                        bbcode_html(children, base, output);
                    }
                    continue;
                }
                if name == "previewyoutube" {
                    let id = argument.split(';').next().unwrap_or("").trim();
                    if id.len() == 11
                        && id
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
                    {
                        output.push_str("<img src=\"https://www.youtube.com/watch?v=");
                        escaped(id, output);
                        output.push_str("\">");
                    } else {
                        bbcode_html(children, base, output);
                    }
                    continue;
                }
                let tag = match name.as_str() {
                    "b" => "strong",
                    "i" => "em",
                    "s" => "del",
                    "list" if !argument.is_empty() => "ol",
                    "list" => "ul",
                    "quote" => "blockquote",
                    name => name,
                };
                output.push('<');
                output.push_str(tag);
                output.push('>');
                if name == "quote" && !argument.is_empty() {
                    output.push_str("<p><strong>");
                    escaped_bbcode_text(argument, output);
                    output.push_str("</strong></p>");
                }
                bbcode_html(children, base, output);
                output.push_str("</");
                output.push_str(tag);
                output.push('>');
            }
        }
    }
}

struct Announcement {
    markdown: String,
    metadata: Map<String, Value>,
}

fn structured_announcement(document: &Html, base: Option<&Url>) -> Result<Option<Announcement>> {
    for carrier in document.select(&selector("[data-partnereventstore]")) {
        let raw = carrier.value().attr("data-partnereventstore").unwrap_or("");
        let Ok(data) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let entries = match &data {
            Value::Array(entries) => entries.iter().collect::<Vec<_>>(),
            Value::Object(_) => vec![&data],
            _ => continue,
        };
        for entry in entries {
            let Some(body) = entry.get("announcement_body") else {
                continue;
            };
            let Some(text) = body
                .get("body")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
            else {
                continue;
            };
            let nodes = bbcode_tree(text)?;
            let mut html = String::new();
            bbcode_html(&nodes, base, &mut html);
            let tree = Html::parse_fragment(&html);
            let markdown = render_clean(tree.root_element(), base)?;
            if markdown.is_empty() {
                continue;
            }
            let mut metadata = Map::new();
            if let Some(title) = body
                .get("headline")
                .and_then(Value::as_str)
                .and_then(|value| clean_title(value, None))
            {
                metadata.insert("title".into(), title.into());
            }
            if let Some(time) = body
                .get("posttime")
                .and_then(Value::as_i64)
                .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            {
                metadata.insert("published".into(), time.to_rfc3339().into());
            }
            if let Some(group) = carrier
                .value()
                .attr("data-groupvanityinfo")
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .and_then(|value| {
                    value
                        .as_array()
                        .and_then(|array| array.first())
                        .and_then(|group| group.get("group_name"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .filter(|group| !group.trim().is_empty())
            {
                metadata.insert("author".into(), group.trim().into());
            }
            return Ok(Some(Announcement { markdown, metadata }));
        }
    }
    Ok(None)
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
    let markdown = if let Some(announcement) = structured_announcement(&document, base.as_ref())? {
        metadata.extend(announcement.metadata);
        announcement.markdown
    } else {
        render_clean(root, base.as_ref())?
    };
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

    #[test]
    fn css_custom_properties_preserve_visible_article_content() {
        let doc = extract_html(r#"<div style="--footer-display: none; --graph-controls-display: none;"><article><h1>开发者</h1><p>如果你熟悉 TypeScript 或 CSS，可以开发插件。</p></article></div>"#, None).unwrap();
        assert!(doc.markdown.contains("开发者"));
        assert!(doc.markdown.contains("可以开发插件"));
    }

    #[test]
    fn hidden_style_checks_property_tokens_not_embedded_strings_or_comments() {
        for style in [
            "--foo-display:none; --foo-visibility:hidden",
            "content: 'display:none; visibility:hidden'",
            r#"content: "escaped \"; display:none"; color: red"#,
            "/* display:none; visibility:hidden */ color:red",
            "background:url('x;display:none'); color:red",
            "--theme: { display:none; visibility:hidden }; color:red",
            "display:'none'; visibility:\"hidden\"",
            "dis/**/play:none; visibility:hiddenish",
            "content: 'unfinished; display:none",
            "color:red; /* unfinished comment; display:none",
        ] {
            assert!(!hidden_inline_style(style), "unexpectedly hidden: {style}");
        }
        for style in [
            "DISPLAY : NoNe",
            "display: /* explanation */ none ! IMPORTANT",
            "/* declaration */ visibility: hidden",
            "visibility:collapse",
            "content:'display:block;'; display:none",
            "--theme:{display:block;}; visibility:hidden",
        ] {
            assert!(hidden_inline_style(style), "expected hidden: {style}");
        }
    }

    #[test]
    fn inline_hidden_properties_follow_order_and_importance() {
        for style in [
            "display:none; display:block",
            "visibility:hidden!important; visibility:visible!important",
            "display:none; display:block!important; display:none",
        ] {
            assert!(!hidden_inline_style(style), "unexpectedly hidden: {style}");
        }
        for style in [
            "display:block; display:none",
            "display:none!important; display:block",
            "visibility:hidden; display:block",
            "display:none; display:'block'",
        ] {
            assert!(hidden_inline_style(style), "expected hidden: {style}");
        }
        let doc = extract_html(r#"<main><p style="content:'display:none'">Visible</p><p style="display:/* comment */none">Hidden</p><p hidden style="display:block">Also hidden</p></main>"#, None).unwrap();
        assert_eq!(doc.markdown, "Visible");
    }

    #[test]
    fn mathjax_tex_sources_replace_only_their_duplicate_previews() {
        let doc = extract_html(r#"<main><p>Energy <span class="MathJax_Preview">duplicate preview</span><span class="MathJax">duplicate glyphs</span><script type="math/tex">E_u</script> remains.</p><script type="math/tex; mode=display">\frac{a}{b}</script><script type="application/javascript">alert('bad')</script><script type="math/tex-evil">not math</script><p><span class="MathJax">Visible unannotated text</span></p></main>"#, None).unwrap();
        assert!(doc.markdown.contains("Energy $E_u$ remains."));
        assert!(doc.markdown.contains(r"$$\frac{a}{b}$$"));
        assert!(doc.markdown.contains("Visible unannotated text"));
        for text in ["duplicate", "alert", "not math"] {
            assert!(!doc.markdown.contains(text));
        }
    }

    #[test]
    fn katex_annotations_and_data_sources_are_emitted_once() {
        let doc = extract_html(r#"<main><p>A <span class="katex"><span class="katex-mathml"><math><semantics><mrow><mi>x</mi><mo>+</mo><mi>y</mi></mrow><annotation encoding="application/x-tex">x+y</annotation></semantics></math></span><span class="katex-html">duplicate</span></span> term.</p><span class="katex-display"><span class="katex"><math><semantics><mi>z</mi><annotation encoding="application/x-tex">z^2</annotation></semantics></math><span class="katex-html">other duplicate</span></span></span><p><span class="math-inline" data-math="\forall x"><span class="katex">visual x</span></span> and <span class="hurmet-tex" data-entry="\vec{F}"><math><mi>F</mi></math></span>.</p></main>"#, None).unwrap();
        assert!(doc.markdown.contains("A $x+y$ term."));
        assert_eq!(doc.markdown.matches("z^2").count(), 1);
        assert!(doc.markdown.contains("$$z^2$$"));
        assert!(doc.markdown.contains(r"$\forall x$ and $\vec{F}$."));
        assert!(!doc.markdown.contains("duplicate"));
        assert!(!doc.markdown.contains("visual"));
    }

    #[test]
    fn hidden_assistive_math_is_recovered_inside_known_visual_wrappers() {
        let doc = extract_html(r#"<p>Quadratic <span class="mwe-math-element"><span style="display:none"><math alttext="ax^2+bx+c=0"><mi>fallback</mi></math></span><img src="https://example.test/equation.svg" alt="ax^2+bx+c=0"></span>.</p><mjx-container display="true"><svg><path></path></svg><mjx-assistive-mml style="display:none"><math><msub><mi>r</mi><mtext>perf</mtext></msub></math></mjx-assistive-mml></mjx-container>"#, None).unwrap();
        assert!(doc.markdown.contains("Quadratic $ax^2+bx+c=0$."));
        assert!(doc.markdown.contains(r"$$r_{\text{perf}}$$"));
        assert!(!doc.markdown.contains("equation.svg"));
        assert!(!doc.markdown.contains("fallback"));
    }

    #[test]
    fn nested_presentation_mathml_keeps_fraction_roots_scripts_and_accents() {
        let doc = extract_html(r#"<math display="block"><mrow><mfrac><msubsup><mi>x</mi><mn>1</mn><mn>2</mn></msubsup><mroot><mi>y</mi><mn>3</mn></mroot></mfrac><mo>+</mo><msqrt><mrow><mi>α</mi><mo>×</mo><mi>β</mi></mrow></msqrt><mo>=</mo><mover><mi>F</mi><mo>→</mo></mover></mrow></math>"#, None).unwrap();
        assert_eq!(
            doc.markdown,
            r"$$\frac{x_{1}^{2}}{\sqrt[3]{y}} + \sqrt{\alpha \times \beta} = \vec{F}$$"
        );
        let doc = extract_html(r#"<math><mfenced open="[" close="]"><mi>a</mi><mfrac><mn>1</mn><mi>b</mi></mfrac></mfenced></math>"#, None).unwrap();
        assert_eq!(doc.markdown, r"$\left[a, \frac{1}{b}\right]$");
    }

    #[test]
    fn mathml_tables_preserve_rows_and_columns() {
        let doc = extract_html(r#"<math display="block"><mtable><mtr><mtd><mi>x</mi></mtd><mtd><mo>=</mo></mtd><mtd><mi>y</mi></mtd></mtr><mtr><mtd><mi>u</mi></mtd><mtd><mo>=</mo></mtd><mtd><mi>v</mi></mtd></mtr></mtable></math>"#, None).unwrap();
        assert_eq!(
            doc.markdown,
            r"$$\begin{aligned}x & = & y \\ u & = & v\end{aligned}$$"
        );
    }

    #[test]
    fn math_annotations_cannot_emit_raw_html_or_execute_active_scripts() {
        let doc = extract_html(r#"<p><span class="math-inline" data-math="x &lt;img src=x onerror=alert(1)&gt;">visual</span></p><script>alert(2)</script><pre>&lt;math&gt;$x_1$&lt;/math&gt;</pre>"#, None).unwrap();
        assert!(
            doc.markdown
                .contains(r"$x \lt img src=x onerror=alert(1)\gt $")
        );
        assert!(!doc.markdown.contains("alert(2)"));
        assert!(!doc.markdown.contains("<img"));
        assert!(doc.markdown.contains("<math>$x_1$</math>"));
    }

    #[test]
    fn deep_mathml_fails_explicitly_instead_of_overflowing() {
        let html = format!(
            "<math>{}<mi>x</mi>{}</math>",
            "<mrow>".repeat(66),
            "</mrow>".repeat(66)
        );
        assert!(
            matches!(extract_html(&html, None), Err(Error::Conversion(message)) if message.contains("MathML nesting"))
        );
    }

    #[test]
    fn mathml_skips_active_nodes_and_xml_annotations_inside_nested_text() {
        let doc = extract_html(r#"<math><mrow><mi>x</mi><script>secret_script()</script><style>secret_style</style><annotation-xml encoding="text/html"><mtext>secret_xml</mtext></annotation-xml><mo>+</mo><mtext>safe<script>secret_nested()</script> text</mtext></mrow></math>"#, None).unwrap();
        assert!(doc.markdown.contains("x +"));
        assert!(doc.markdown.contains(r"\text{safe text}"));
        assert!(!doc.markdown.contains("secret"));
    }

    fn announcement_html(body: &str) -> String {
        let data = serde_json::json!([{"announcement_body": {"body": body, "headline": "Patch notes", "posttime": 1736942400}}]);
        let mut html = String::from("<title>Generic title</title><div data-partnereventstore=\"");
        escaped(&data.to_string(), &mut html);
        html.push_str("\" data-groupvanityinfo='[{\"group_name\":\"Example Game\"}]'></div>");
        html
    }

    #[test]
    fn structured_announcement_recovers_metadata_links_and_video_from_json() {
        let html = announcement_html(
            r#"[p]Patch [b]released[/b]. Read [url=\"https:\/\/example.test\/notes\"]the notes[\/url].[\/p][previewyoutube=\"dQw4w9WgXcQ;full\"][\/previewyoutube]"#,
        );
        let doc = extract_html(&html, None).unwrap();
        assert_eq!(doc.metadata["title"], "Patch notes");
        assert_eq!(doc.metadata["author"], "Example Game");
        assert_eq!(doc.metadata["published"], "2025-01-15T12:00:00+00:00");
        assert!(doc.markdown.contains("Patch **released**."));
        assert!(
            doc.markdown
                .contains("[the notes](https://example.test/notes)")
        );
        assert!(
            doc.markdown
                .contains("![](https://www.youtube.com/watch?v=dQw4w9WgXcQ)")
        );
    }

    #[test]
    fn nested_bbcode_lists_quotes_and_literal_code_keep_structure() {
        let html = announcement_html(
            "[quote=Ada][list][*]First [b]bold [i]inner[/i][/b][list][*]Nested[/list][*]Second[/list][/quote][code=rust]let s = \"[b]literal[/b]\";\n  // <script> data\n[/code]",
        );
        let doc = extract_html(&html, None).unwrap();
        for text in [
            "Ada",
            "First",
            "**bold *inner***",
            "Nested",
            "Second",
            "```rust",
            "[b]literal[/b]",
            "  // <script> data",
        ] {
            assert!(
                doc.markdown.contains(text),
                "missing {text}: {}",
                doc.markdown
            );
        }
        assert!(doc.markdown.contains(">"));
    }

    #[test]
    fn bbcode_escapes_html_and_sanitizes_destinations_without_dropping_labels() {
        let html = announcement_html(
            r#"[p]<script>alert(1)</script> [url=javascript:alert(2)]safe label[/url] [img]data:text/html,bad[/img] [unknown]keep me[/unknown][/p]"#,
        );
        let doc = extract_html(&html, None).unwrap();
        assert!(doc.markdown.contains("safe label"));
        assert!(doc.markdown.contains("keep me"));
        assert!(
            doc.markdown
                .contains("&lt;script&gt;alert(1)&lt;/script&gt;")
        );
        assert!(!doc.markdown.contains("<script>"));
        assert!(!doc.markdown.contains("](javascript:"));
        assert!(!doc.markdown.contains("](data:"));
    }

    #[test]
    fn bbcode_protects_all_displayed_text_paths_and_keeps_image_alt_empty() {
        let html = announcement_html(
            r#"[h2]<img src=x onerror=heading>[/h2][quote="<img src=x onerror=author>"]Quoted body[/quote][url=https://example.test]<svg onload=label>Link label</svg>[/url][img]https://example.test/image.png[/img]"#,
        );
        let doc = extract_html(&html, None).unwrap();
        for expected in [
            "## &lt;img src=x onerror=heading&gt;",
            "&lt;img src=x onerror=author&gt;",
            "&lt;svg onload=label&gt;Link label&lt;/svg&gt;",
            "Quoted body",
            "![](https://example.test/image.png)",
        ] {
            assert!(
                doc.markdown.contains(expected),
                "missing {expected}: {}",
                doc.markdown
            );
        }
        assert!(!doc.markdown.contains("<img"));
        assert!(!doc.markdown.contains("<svg"));
    }

    #[test]
    fn invalid_event_data_falls_back_and_depth_limit_is_explicit() {
        let doc = extract_html(
            "<div data-partnereventstore='invalid'></div><p>Visible fallback</p>",
            None,
        )
        .unwrap();
        assert_eq!(doc.markdown, "Visible fallback");
        let body = format!("{}deep{}", "[b]".repeat(66), "[/b]".repeat(66));
        assert!(
            matches!(extract_html(&announcement_html(&body), None), Err(Error::Conversion(message)) if message.contains("BBCode nesting"))
        );
    }
}
