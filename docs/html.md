# Native HTML extraction

The Rust HTML reader parses a DOM with `scraper`, selects an article candidate,
serializes an allowlisted representation, and renders Markdown with `htmd`.
It does not run JavaScript, fetch images, follow links, start a browser, or call
the Python implementation. The same reader serves local HTML and fetched HTML;
fetching itself belongs to the separate HTTP layer.

## Metadata

Title selection follows the reference metadata layer: JSON-LD `headline`, then
JSON-LD `name`, document `<title>`, and Open Graph title. Twitter title is an
additional fallback. JSON-LD arrays and nested `@graph` objects are inspected;
malformed JSON is ignored. Scripts remain excluded from rendered content.

Repeated site prefixes and suffixes are removed when the site name is known.
A truncated title can be replaced by a longer matching document or Open Graph
title. Without site metadata, a ` | Site` suffix is removed only when a headline
or first heading matches the preceding text. Ordinary hyphenated subtitles stay
intact. Pathological titles are capped at 300 Unicode characters, preferably at
a word boundary, followed by an ellipsis.

Author meta tags precede JSON-LD author strings or author objects. Publication
metadata precedes JSON-LD `datePublished`, then a date-shaped `<time datetime>`.
Description and site values come from meta tags. Page canonical URLs are retained;
a homepage canonical on a deeper source URL is discarded. The local-format
dispatcher must preserve the reader's title so normal output can use it.

## Rendering and URL handling

- `<q>` retains quotation marks and nested inline markup. Source newlines remain
  soft Markdown line breaks outside code; structural block elements establish
  paragraph boundaries. This follows the source renderer's line behavior, which
  can differ from a browser's visual whitespace collapse.
- Preformatted code keeps indentation and empty lines. A `<code>` element with
  inline `white-space: pre` styling is treated as a code block. Chemical and
  mathematical `<sub>`/`<sup>` text retains those tags.
- Valid absolute HTTP, HTTPS, mail and telephone URLs retain their spelling.
  URL validation does not add a slash to a hostname-only link or change percent
  escapes. Literal angle brackets and double quotes are percent-encoded before
  Markdown emission. Relative URLs resolve against a supplied page URL. Local extraction
  without a base retains relative destinations.
- URL attributes with control characters or unsafe schemes are removed. Event
  handlers and arbitrary styles are not emitted. Script, form, frame, template,
  hidden and navigation content is excluded. A hidden ancestor also disqualifies
  an article candidate. DOM serialization stops with an error beyond 256 levels.

The generic reader is not yet the source's complete web extraction pipeline.
It does not replicate its site resolvers, browser/CSS visibility model, adaptive
content scoring, schema-body fallback, full footnotes, callouts, shadow roots,
or math standardization. Static input can therefore retain site chrome, omit
content outside the selected article, or differ in headings, tables and images.
Those differences must be measured rather than inferred from successful parsing.

## Corpus diagnostic

The reference checkout contains 209 HTML files paired with 209 upstream expected
Markdown files under `packages/markitai/tests/defuddle_fixtures`. Its quality test
suite checks nonempty text, title presence, site-chrome phrases and word-count
tolerance. Passing those heuristics does not establish exact output parity.

Run the new diagnostic from the Rust repository after a coordinator release build:

```sh
python3 scripts/audit_html.py \
  --reference /Users/example-user/work/markitai \
  --library target/release/libmarkitai_ffi.dylib \
  --output .local/audits/html-corpus-20260928-r1 \
  --jobs 4
```

Use `.so` or `.dll` on other platforms. The reference virtual environment is used
by default; `--reference-python` can select another installed environment.
`--pattern 'elements--*'` and `--limit 5` make an explicitly labelled subset.
The output directory must be empty and outside the reference checkout.

Each fixture runs through both **local file public APIs** in separate subprocesses.
The harness reuses `audit_formats.py` for worker isolation: explicit configuration,
disabled LLM/OCR/screenshot/alt/description features, disabled cache, an isolated
`MARKITAI_HOME`, and a credential-free child environment. Source files are hashed
before and after each conversion. A frozen copy of the native library, worker
responses/logs, exact diffs and a machine-readable report remain in the audit
directory. The source checkout is never edited.

A strict parity pass requires exact Markdown, metadata except the processing
timestamp, asset names/content hashes, warnings and skip reason. Matching failures
are not passes. `--require-parity` returns a failure exit code if any selected
fixture differs or errors.

Upstream expected-body equality, title equality, word ratios and chrome phrases
are reported separately for both implementations. These are **diagnostics**:
upstream expectations describe URL extraction, while this harness exercises local
file conversion, including the reference's fallback behavior. They do not prove
URL/site-resolver parity, do not count as migrated source tests, and do not turn a
strict output mismatch into a pass. The report records selected/available fixture
counts, source revisions and the exact library hash. It makes no performance claim.
