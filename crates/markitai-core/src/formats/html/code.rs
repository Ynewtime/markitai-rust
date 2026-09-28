//! Recover code from display-oriented syntax highlighters without executing UI.
use crate::{Error, Result};
use scraper::{ElementRef, Node};

const MAX_DEPTH: usize = 256;
type CodeTarget<'a> = (ElementRef<'a>, usize);

fn has_class(element: ElementRef<'_>, name: &str) -> bool {
    element.value().classes().any(|class| class == name)
}
fn any_class(element: ElementRef<'_>, names: &[&str]) -> bool {
    names.iter().any(|name| has_class(element, name))
}

// Split actual declarations, not semicolons inside CSS strings or functions.
fn property(style: &str, wanted: &str) -> Option<String> {
    let mut declarations = Vec::new();
    let mut current = String::new();
    let mut chars = style.chars().peekable();
    let mut quote = None;
    let mut depth = 0usize;
    while let Some(ch) = chars.next() {
        if let Some(delimiter) = quote {
            current.push(ch);
            if ch == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else if ch == delimiter {
                quote = None;
            }
        } else if ch == '\\' {
            current.push(ch);
            if let Some(next) = chars.next() {
                current.push(next);
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
            current.push(' ');
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            current.push(ch);
        } else if ch == '(' {
            depth += 1;
            current.push(ch);
        } else if ch == ')' {
            depth = depth.saturating_sub(1);
            current.push(ch);
        } else if ch == ';' && depth == 0 {
            declarations.push(std::mem::take(&mut current));
        } else {
            current.push(ch);
        }
    }
    declarations.push(current);
    declarations
        .into_iter()
        .filter_map(|declaration| {
            let (key, value) = declaration.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(wanted)
                .then(|| value.trim().to_ascii_lowercase())
        })
        .next_back()
}
fn style_is(element: ElementRef<'_>, property_name: &str, values: &[&str]) -> bool {
    element
        .value()
        .attr("style")
        .and_then(|style| property(style, property_name))
        .is_some_and(|value| {
            let keyword = match value.split_once('!') {
                Some((keyword, priority)) if priority.trim() == "important" => keyword.trim(),
                Some(_) => return false,
                None => value.trim(),
            };
            values.contains(&keyword)
        })
}
fn digits(element: ElementRef<'_>) -> bool {
    let mut number = false;
    for text in element.text() {
        for ch in text.chars() {
            if ch.is_ascii_digit() {
                number = true;
            } else if !ch.is_whitespace() {
                return false;
            }
        }
    }
    number
}
fn gutter(element: ElementRef<'_>) -> bool {
    any_class(
        element,
        &[
            "gutter",
            "rouge-gutter",
            "cm-gutters",
            "cm-gutter",
            "CodeMirror-gutters",
            "CodeMirror-gutter-wrapper",
            "linenos",
            "linenodiv",
            "line-numbers-rows",
        ],
    ) || (any_class(
        element,
        &[
            "lnt",
            "ln",
            "lineno",
            "linenumber",
            "line-number",
            "CodeMirror-linenumber",
            "react-syntax-highlighter-line-number",
        ],
    ) && digits(element))
}
fn control(element: ElementRef<'_>) -> bool {
    super::is_hidden(element)
        || matches!(
            element.value().name(),
            "script"
                | "style"
                | "button"
                | "input"
                | "select"
                | "textarea"
                | "iframe"
                | "object"
                | "embed"
                | "template"
                | "noscript"
                | "svg"
        )
        || element.value().attr("role").is_some_and(|role| {
            matches!(
                role,
                "button" | "toolbar" | "listbox" | "option" | "tooltip"
            )
        })
        || any_class(
            element,
            &[
                "code__header",
                "code-header",
                "CodeBlock-header",
                "code-toolbar",
                "copy-button",
                "copy-code-button",
                "rehype-pretty-copy",
                "hover-container",
                "hover-info",
            ],
        )
        || element.value().attr("data-floating-buttons").is_some()
        || element.value().attr("data-fade-overlay").is_some()
}
fn excluded(element: ElementRef<'_>) -> bool {
    control(element) || gutter(element)
}
fn visible_between(element: ElementRef<'_>, root: ElementRef<'_>) -> bool {
    !element
        .ancestors()
        .filter_map(ElementRef::wrap)
        .take_while(|parent| *parent != root)
        .any(excluded)
}
fn code_mirror(element: ElementRef<'_>) -> bool {
    any_class(element, &["cm-content", "CodeMirror-code"])
}
fn styled_block(element: ElementRef<'_>) -> bool {
    element.value().name() == "code"
        && (style_is(element, "white-space", &["pre", "pre-wrap", "break-spaces"])
            || (has_class(element, "block") && any_class(element, &["lean", "hl"])))
}
fn target(element: ElementRef<'_>) -> bool {
    element.value().name() == "pre" || code_mirror(element) || styled_block(element)
}
fn explicit_wrapper(element: ElementRef<'_>) -> bool {
    matches!(element.value().name(), "div" | "figure" | "table" | "code")
        && (any_class(
            element,
            &[
                "highlight",
                "chroma",
                "highlighter-rouge",
                "lntable",
                "rouge-table",
                "highlighttable",
                "expressive-code",
                "code-block",
                "CodeBlock",
                "cm-editor",
                "CodeMirror",
            ],
        ) || element
            .value()
            .attr("data-rehype-pretty-code-figure")
            .is_some()
            || (element.value().name() == "code"
                && element
                    .descendants()
                    .filter_map(ElementRef::wrap)
                    .any(|child| child.value().name() == "pre")))
}
fn language_label(raw: &str) -> Option<String> {
    let raw = raw.trim().to_ascii_lowercase();
    let name = match raw.as_str() {
        "c++" => "cpp",
        "c#" => "csharp",
        "node.js" | "nodejs" => "javascript",
        "shell" | "shell script" => "bash",
        "plain text" | "plaintext" => "text",
        other => other,
    };
    matches!(
        name,
        "bash"
            | "sh"
            | "zsh"
            | "fish"
            | "python"
            | "javascript"
            | "typescript"
            | "java"
            | "kotlin"
            | "rust"
            | "go"
            | "cpp"
            | "csharp"
            | "c"
            | "ruby"
            | "php"
            | "swift"
            | "scala"
            | "sql"
            | "json"
            | "yaml"
            | "toml"
            | "xml"
            | "html"
            | "css"
            | "tsx"
            | "jsx"
            | "text"
            | "lean"
            | "nasm"
    )
    .then(|| name.to_owned())
}
fn safe_language(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 64
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+.#-".contains(&byte))
    {
        return None;
    }
    Some(language_label(raw).unwrap_or_else(|| raw.to_ascii_lowercase()))
}
fn language_attribute(element: ElementRef<'_>) -> Option<String> {
    for key in ["data-language", "data-lang", "language"] {
        if let Some(value) = element.value().attr(key).and_then(safe_language) {
            return Some(value);
        }
    }
    for class in element.value().classes() {
        if let Some(value) = class
            .strip_prefix("language-")
            .or_else(|| class.strip_prefix("lang-"))
            .and_then(safe_language)
        {
            return Some(value);
        }
    }
    if (matches!(element.value().name(), "pre" | "code")
        || any_class(element, &["highlighter-rouge", "highlight"]))
        && let Some(value) = element.value().attr("lang").and_then(safe_language)
    {
        return Some(value);
    }
    if any_class(element, &["highlight", "hl"]) {
        return element.value().classes().find_map(language_label);
    }
    None
}
fn header_label(element: ElementRef<'_>) -> Option<String> {
    let mut text = String::new();
    for node in element.descendants() {
        if let Node::Text(value) = node.value() {
            if node
                .ancestors()
                .filter_map(ElementRef::wrap)
                .take_while(|parent| *parent != element)
                .any(|parent| control(parent) || target(parent))
            {
                continue;
            }
            text.push_str(value);
        }
    }
    language_label(&text)
}
fn header_with_copy(element: ElementRef<'_>) -> bool {
    header_label(element).is_some()
        && element
            .descendants()
            .filter_map(ElementRef::wrap)
            .any(|child| child.value().name() == "button")
}
fn isolated_header(element: ElementRef<'_>) -> bool {
    !element
        .descendants()
        .filter_map(ElementRef::wrap)
        .any(target)
        && header_with_copy(element)
}
fn inferred_wrapper(element: ElementRef<'_>) -> bool {
    if element.value().name() != "div" {
        return false;
    }
    let children = element.child_elements().collect::<Vec<_>>();
    children.len() == 2
        && header_with_copy(children[0])
        && children[1]
            .descendants()
            .filter_map(ElementRef::wrap)
            .any(target)
}

fn collect_targets<'a>(
    element: ElementRef<'a>,
    depth: usize,
    targets: &mut Vec<CodeTarget<'a>>,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::Conversion(
            "HTML code nesting exceeds 256 elements".into(),
        ));
    }
    if excluded(element) {
        return Ok(());
    }
    if target(element) {
        // Some gutters use a normal <pre> containing only marked line numbers.
        let only_gutter = digits(element)
            && element
                .descendants()
                .filter_map(ElementRef::wrap)
                .any(gutter);
        if !only_gutter {
            targets.push((element, depth));
        }
        return Ok(());
    }
    for child in element.child_elements() {
        collect_targets(child, depth + 1, targets)?;
    }
    Ok(())
}
fn only_code_and_controls(
    element: ElementRef<'_>,
    targets: &[CodeTarget<'_>],
    depth: usize,
) -> Result<bool> {
    if depth > MAX_DEPTH {
        return Err(Error::Conversion(
            "HTML code nesting exceeds 256 elements".into(),
        ));
    }
    if targets.iter().any(|(target, _)| *target == element)
        || excluded(element)
        || isolated_header(element)
    {
        return Ok(true);
    }
    for child in element.children() {
        if let Some(child) = ElementRef::wrap(child) {
            if !only_code_and_controls(child, targets, depth + 1)? {
                return Ok(false);
            }
        } else if let Node::Text(text) = child.value()
            && !text.trim().is_empty()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
fn code_language(root: ElementRef<'_>, scope: ElementRef<'_>) -> Option<String> {
    // The code's own attributes outrank container labels.
    for node in root.descendants().filter_map(ElementRef::wrap) {
        if node.value().name() == "code"
            && visible_between(node, root)
            && let Some(language) = language_attribute(node)
        {
            return Some(language);
        }
    }
    for node in std::iter::once(root).chain(root.ancestors().filter_map(ElementRef::wrap).take(8)) {
        if matches!(node.value().name(), "article" | "main" | "body" | "html") {
            break;
        }
        if let Some(language) = language_attribute(node) {
            return Some(language);
        }
        if node == scope {
            break;
        }
    }
    if root != scope
        && let Some(language) = language_attribute(scope)
    {
        return Some(language);
    }
    for element in scope.descendants().filter_map(ElementRef::wrap) {
        if header_with_copy(element)
            && let Some(language) = header_label(element)
        {
            return Some(language);
        }
    }
    // CodeMirror embeds its language label above the actual editor contents.
    if root
        .descendants()
        .filter_map(ElementRef::wrap)
        .any(code_mirror)
    {
        for element in root.descendants().filter_map(ElementRef::wrap) {
            if element.child_elements().next().is_none()
                && !control(element)
                && !element
                    .ancestors()
                    .filter_map(ElementRef::wrap)
                    .any(code_mirror)
                && let Some(language) = language_label(&element.text().collect::<String>())
            {
                return Some(language);
            }
        }
    }
    None
}
fn paired_gutter(element: ElementRef<'_>) -> Option<ElementRef<'_>> {
    let children = element.child_elements().collect::<Vec<_>>();
    if children.len() != 2 || !digits(children[0]) {
        return None;
    }
    let first = children[0];
    let flex =
        has_class(element, "flex-row") || style_is(element, "display", &["flex", "inline-flex"]);
    let marked = gutter(first)
        || style_is(first, "user-select", &["none"])
        || style_is(first, "-webkit-user-select", &["none"])
        || any_class(first, &["text-end", "text-right"]);
    (flex && marked).then_some(first)
}
fn row(element: ElementRef<'_>) -> bool {
    any_class(
        element,
        &[
            "line",
            "cm-line",
            "CodeMirror-line",
            "ec-line",
            "cm-editor",
            "CodeMirror",
        ],
    ) || code_mirror(element)
        || element.value().attr("data-line").is_some()
        || paired_gutter(element).is_some()
        || (element.value().name() == "div"
            && element
                .child_elements()
                .any(|child| has_class(child, "CodeMirror-line")))
}
fn text_content(element: ElementRef<'_>, output: &mut String, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::Conversion(
            "HTML code nesting exceeds 256 elements".into(),
        ));
    }
    if excluded(element) {
        return Ok(());
    }
    if element.value().name() == "br" {
        output.push('\n');
        return Ok(());
    }
    let skip = paired_gutter(element);
    let mut saw_row = false;
    let mut previous_ends_line = false;
    let mut gap_has_line = false;
    for child in element.children() {
        if let Some(child) = ElementRef::wrap(child) {
            if Some(child) == skip || excluded(child) || isolated_header(child) {
                continue;
            }
            if row(child) {
                let mut text = String::new();
                // A sole BR in an editable CodeMirror line is a cursor placeholder.
                let placeholder = has_class(child, "cm-line")
                    && child.child_elements().count() == 1
                    && child
                        .child_elements()
                        .next()
                        .is_some_and(|v| v.value().name() == "br")
                    && child.text().all(|text| text.is_empty());
                if !placeholder {
                    text_content(child, &mut text, depth + 1)?;
                }
                if saw_row && !previous_ends_line && !gap_has_line {
                    output.push('\n');
                }
                output.push_str(&text);
                saw_row = true;
                previous_ends_line = text.ends_with('\n');
                gap_has_line = false;
            } else {
                let start = output.len();
                text_content(child, output, depth + 1)?;
                if output[start..].contains('\n') {
                    gap_has_line = true;
                }
            }
        } else if let Node::Text(text) = child.value() {
            output.push_str(text);
            if text.contains('\n') {
                gap_has_line = true;
            }
        }
    }
    Ok(())
}
fn render_target(
    root: ElementRef<'_>,
    scope: ElementRef<'_>,
    output: &mut String,
    depth: usize,
) -> Result<()> {
    let editors = root
        .descendants()
        .filter_map(ElementRef::wrap)
        .filter(|node| code_mirror(*node) && !excluded(*node) && visible_between(*node, root))
        .take(2)
        .collect::<Vec<_>>();
    // Narrowing to an editor is safe only when everything else is confirmed UI.
    // Otherwise retain the whole pre, including peripheral text or other editors.
    let content =
        if editors.len() == 1 && only_code_and_controls(root, &[(editors[0], depth)], depth)? {
            editors[0]
        } else {
            root
        };
    let skipped_depth = if content == root {
        0
    } else {
        1 + content
            .ancestors()
            .take_while(|node| node.id() != root.id())
            .count()
    };
    let mut text = String::new();
    text_content(content, &mut text, depth + skipped_depth)?;
    output.push_str("<pre><code");
    if let Some(language) = code_language(root, scope) {
        output.push_str(" class=\"language-");
        super::escaped(&language, output);
        output.push('"');
    }
    output.push('>');
    super::escaped(&text, output);
    output.push_str("</code></pre>");
    Ok(())
}

fn confirmed_targets(element: ElementRef<'_>, depth: usize) -> Result<Option<Vec<CodeTarget<'_>>>> {
    if !target(element) && !explicit_wrapper(element) && !inferred_wrapper(element) {
        return Ok(None);
    }
    let mut targets = Vec::new();
    collect_targets(element, depth, &mut targets)?;
    if targets.is_empty()
        || (!target(element) && !only_code_and_controls(element, &targets, depth)?)
    {
        return Ok(None);
    }
    Ok(Some(targets))
}

/// Shared structural classifier for the earlier footnote and math inference pass.
pub(super) fn is_block(element: ElementRef<'_>) -> bool {
    confirmed_targets(element, 0).is_ok_and(|targets| targets.is_some())
}

/// Serialize a confirmed block as literal code, leaving other DOM nodes untouched.
pub(super) fn render(element: ElementRef<'_>, output: &mut String, depth: usize) -> Result<bool> {
    let Some(targets) = confirmed_targets(element, depth)? else {
        return Ok(false);
    };
    // Build separately: malformed/deep markup must not leave a half-written block.
    let mut cleaned = String::new();
    for (target, target_depth) in targets {
        render_target(target, element, &mut cleaned, target_depth)?;
    }
    output.push_str(&cleaned);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::{Html, Selector};
    fn normalized(source: &str, selector: &str) -> (bool, String) {
        let html = Html::parse_fragment(source);
        let element = html
            .select(&Selector::parse(selector).unwrap())
            .next()
            .unwrap();
        let mut output = String::new();
        let handled = render(element, &mut output, 0).unwrap();
        (handled, output)
    }
    fn code(source: &str, selector: &str) -> String {
        let (handled, html) = normalized(source, selector);
        assert!(handled, "{source}");
        html
    }
    fn text(source: &str, selector: &str) -> String {
        let clean = code(source, selector);
        Html::parse_fragment(&clean)
            .select(&Selector::parse("code").unwrap())
            .next()
            .unwrap()
            .text()
            .collect()
    }

    #[test]
    fn ordinary_inline_code_prose_and_mixed_highlights_are_not_captured() {
        for (source, selector) in [
            ("<p>Use <code>return 1</code> today.</p>", "p"),
            ("<code class='language-rust'>x</code>", "code"),
            (
                "<div class='highlight'><p>Explanation</p><pre>x</pre></div>",
                "div",
            ),
            ("<div class='code-block'>This is prose.</div>", "div"),
        ] {
            assert_eq!(normalized(source, selector), (false, String::new()));
        }
    }
    #[test]
    fn literal_links_entities_tabs_and_blank_lines_survive_without_html_attributes() {
        let output = code(
            "<pre data-language='Rust' onclick='x()'><code>\tlet x = <a href='javascript:evil()'>a &amp; b</a>;\n\n&lt;script&gt;literal&lt;/script&gt;\n</code></pre>",
            "pre",
        );
        assert_eq!(
            output,
            "<pre><code class=\"language-rust\">\tlet x = a &amp; b;\n\n&lt;script&gt;literal&lt;/script&gt;\n</code></pre>"
        );
    }
    #[test]
    fn chroma_table_removes_only_the_number_column() {
        let source = "<div class='highlight'><table class='lntable'><tr><td class='lntd'><pre><code><span class='lnt'>1\n</span><span class='lnt'>2\n</span></code></pre></td><td class='lntd'><pre><code data-lang='go'><span class='line'><span class='cl'>package main\n</span></span><span class='line'><span class='cl'>\n</span></span><span class='line'><span class='cl'>var n = 123\n</span></span></code></pre></td></tr></table></div>";
        assert_eq!(
            code(source, ".highlight"),
            "<pre><code class=\"language-go\">package main\n\nvar n = 123\n</code></pre>"
        );
    }
    #[test]
    fn inline_number_gutters_do_not_remove_numeric_code_tokens() {
        let source = "<pre><code class='language-python'><span style='display:flex'><span style='user-select:none'>10</span><span>answer = <span class='mi'>42</span>\n</span></span><span style='display:flex'><span style='user-select:none'>11</span><span>\n</span></span><span style='display:flex'><span style='user-select:none'>12</span><span>print(answer)</span></span></code></pre>";
        assert_eq!(text(source, "pre"), "answer = 42\n\nprint(answer)");
        assert_eq!(
            text(
                "<pre><span style='display:flex'><span style='--user-select:none;content:&quot;;user-select:none&quot;'>123</span><span> + 4</span></span></pre>",
                "pre"
            ),
            "123 + 4"
        );
    }
    #[test]
    fn pygments_and_react_numbers_preserve_all_original_line_breaks() {
        let source = "<pre><code><span class='lineno'>8</span>first\n<span class='linenumber react-syntax-highlighter-line-number'>9</span>\n<span class='lineno'>10</span>  <span class='mi'>11</span>\n</code></pre>";
        assert_eq!(text(source, "pre"), "first\n\n  11\n");
    }
    #[test]
    fn line_spans_handle_missing_separators_empty_rows_and_explicit_br() {
        assert_eq!(
            text(
                "<pre><code><span data-line>a</span><span data-line></span><span data-line>b</span></code></pre>",
                "pre"
            ),
            "a\n\nb"
        );
        assert_eq!(
            text(
                "<pre><span class='line'>a</span><br><span class='line'></span><br><span class='line'>b</span><br></pre>",
                "pre"
            ),
            "a\n\nb\n"
        );
        assert_eq!(
            text(
                "<pre><span class='line'>a\n</span><span class='line'>\n</span><span class='line'>b\n</span></pre>",
                "pre"
            ),
            "a\n\nb\n"
        );
    }
    #[test]
    fn codemirror_content_excludes_editor_chrome_and_recovers_language() {
        let source = "<pre><div><div><span>Python</span><button>Run</button></div><div class='cm-editor'><div class='cm-gutters'>1 2 3</div><div class='cm-content'><div class='cm-line'>x = 1</div><div class='cm-line'><br></div><div class='cm-line'>print(x)</div></div></div></div></pre>";
        assert_eq!(
            code(source, "pre"),
            "<pre><code class=\"language-python\">x = 1\n\nprint(x)</code></pre>"
        );
    }
    #[test]
    fn nested_code_pre_and_expressive_flex_rows_become_one_block() {
        let source = "<code class='language-typescript'>\n <pre><div class='flex-row'><span class='text-end'>1</span><div>type X = {</div></div><div class='flex-row'><span class='text-end'>2</span><div>  n: number</div></div><div class='flex-row'><span class='text-end'>3</span><div>}</div></div></pre>\n</code>";
        assert_eq!(
            code(source, "code"),
            "<pre><code class=\"language-typescript\">type X = {\n  n: number\n}</code></pre>"
        );
        assert_eq!(
            text(
                "<pre><code><div class='ec-line'><div class='gutter'>1</div><div class='code'>a</div></div><div class='ec-line'><div class='gutter'>2</div><div class='code'>b</div></div></code></pre>",
                "pre"
            ),
            "a\nb"
        );
    }
    #[test]
    fn shiki_rehype_and_mintlify_keep_language_and_strip_only_controls() {
        let source = "<div class='code-block' language='tsx'><div data-floating-buttons><button>Copy</button><span>Copy</span></div><pre class='shiki'><code language='tsx'><span class='line'>&lt;Thing</span>\n<span class='line'>  n={42} /&gt;</span>\n<button class='rehype-pretty-copy'>Copy</button><style>bad CSS</style></code></pre><div data-fade-overlay>overlay</div></div>";
        assert_eq!(
            code(source, ".code-block"),
            "<pre><code class=\"language-tsx\">&lt;Thing\n  n={42} /&gt;\n</code></pre>"
        );
    }
    #[test]
    fn react_header_requires_a_language_and_a_copy_control() {
        assert_eq!(
            code(
                "<div><div><span>Java</span><button>Copy</button></div><div><pre><code class='language-java'>class A {}</code></pre></div></div>",
                "div"
            ),
            "<pre><code class=\"language-java\">class A {}</code></pre>"
        );
        assert!(
            !normalized(
                "<div><div>Explanation<button>Copy</button></div><pre>code</pre></div>",
                "div"
            )
            .0
        );
    }
    #[test]
    fn hidden_and_executable_nodes_never_enter_code_but_escaped_examples_do() {
        let source = "<pre><code>start<span hidden>secret</span><script>execute()</script><template>secret2</template><span style='visibility:hidden'>secret3</span><button>Copy</button><span class='hover-container'>metadata</span> &lt;script&gt;example&lt;/script&gt;</code></pre>";
        assert_eq!(text(source, "pre"), "start <script>example</script>");
    }
    #[test]
    fn language_attributes_are_bounded_and_html_safe() {
        assert_eq!(
            code(
                "<pre data-language='&quot; onclick=evil'><code class='language-rust'>x</code></pre>",
                "pre"
            ),
            "<pre><code class=\"language-rust\">x</code></pre>"
        );
        assert_eq!(
            code("<pre data-language='&quot; onclick=evil'>x</pre>", "pre"),
            "<pre><code>x</code></pre>"
        );
    }
    #[test]
    fn deep_code_fails_atomically_instead_of_recursing_without_a_bound() {
        let source = format!(
            "<pre>{}x{}</pre>",
            "<span>".repeat(300),
            "</span>".repeat(300)
        );
        let html = Html::parse_fragment(&source);
        let element = html
            .select(&Selector::parse("pre").unwrap())
            .next()
            .unwrap();
        let mut output = "already written".to_owned();
        assert!(render(element, &mut output, 0).is_err());
        assert_eq!(output, "already written");
    }
    #[test]
    fn classifier_and_walker_share_block_boundaries_and_global_depth_budget() {
        let html = Html::parse_fragment(
            "<div class='cm-editor'><div class='cm-content'><div class='cm-line'>1</div></div></div><div class='highlight'><p>prose</p><pre>x</pre></div>",
        );
        let editor = html
            .select(&Selector::parse(".cm-editor").unwrap())
            .next()
            .unwrap();
        assert!(is_block(editor));
        let mixed = html
            .select(&Selector::parse(".highlight").unwrap())
            .next()
            .unwrap();
        assert!(!is_block(mixed));
        let mut output = String::new();
        assert!(render(editor, &mut output, 255).is_err());
        assert!(output.is_empty());
        assert!(render(editor, &mut output, 250).unwrap());
    }
    #[test]
    fn pygments_table_gutters_and_code_numbers_have_distinct_ownership() {
        let source = "<table class='highlighttable'><tr><td class='linenos'><div class='linenodiv'><pre>1\n2</pre></div></td><td class='code'><div class='highlight'><pre><span class='mi'>123</span> + 4\n5</pre></div></td></tr></table>";
        assert_eq!(text(source, "table"), "123 + 4\n5");
        assert_eq!(code(source, "table").matches("<pre>").count(), 1);
        for style in [
            r"content:x\;user-select:none",
            "user-select:none !bogus",
            "--user-select:none",
            "content:';user-select:none'",
            "/* user-select:none; */color:red",
        ] {
            let source = format!(
                "<pre><span style='display:flex'><span style=\"{style}\">123</span><span> + 4</span></span></pre>"
            );
            assert_eq!(text(&source, "pre"), "123 + 4", "{style}");
        }
    }
    #[test]
    fn codemirror_five_wrapped_rows_preserve_blanks_without_gutter_text() {
        let source = "<div class='CodeMirror'><div class='CodeMirror-gutters'>123</div><div class='CodeMirror-code'><div><div class='CodeMirror-gutter-wrapper'><div class='CodeMirror-linenumber'>1</div></div><pre class='CodeMirror-line'><span>first</span></pre></div><div><div class='CodeMirror-gutter-wrapper'>2</div><pre class='CodeMirror-line'></pre></div><div><div class='CodeMirror-gutter-wrapper'>3</div><pre class='CodeMirror-line'><span>last</span></pre></div></div></div>";
        assert_eq!(text(source, ".CodeMirror"), "first\n\nlast");
    }
    #[test]
    fn multiple_codemirror_contents_and_peripheral_code_are_not_dropped() {
        let source = "<pre><div class='cm-content'><div class='cm-line'>first()</div></div><div class='cm-content'><div class='cm-line'>second()</div></div></pre>";
        assert_eq!(text(source, "pre"), "first()\nsecond()");
        let source = "<pre>prefix()\n<div class='cm-content'><div class='cm-line'>middle()</div></div>\nsuffix()</pre>";
        assert_eq!(text(source, "pre"), "prefix()\nmiddle()\nsuffix()");
        let source = "<pre><div><span>Python</span><button>Copy</button></div><div class='cm-editor'><div class='cm-content'><div class='cm-line'>first()</div></div></div><div class='cm-editor'><div class='cm-content'><div class='cm-line'>second()</div></div></div></pre>";
        assert_eq!(text(source, "pre"), "first()\nsecond()");
    }
    #[test]
    fn hidden_editor_preview_cannot_replace_the_visible_content() {
        let source = "<pre><div class='cm-content' hidden><div class='cm-line'>secret</div></div><div class='cm-content'><div class='cm-line'>visible</div></div></pre>";
        assert_eq!(text(source, "pre"), "visible");
    }
    #[test]
    fn consecutive_editor_breaks_survive_both_html_code_and_markdown_rendering() {
        let source = "<pre><div class='cm-content'>first()<br><br><br>second()</div></pre>";
        let cleaned = code(source, "pre");
        assert_eq!(cleaned, "<pre><code>first()\n\n\nsecond()</code></pre>");
        assert_eq!(
            super::super::render_sanitized(&cleaned).unwrap(),
            "```\nfirst()\n\n\nsecond()\n```"
        );
    }
}
