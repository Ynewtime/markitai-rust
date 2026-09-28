# Document formats and conversion contracts

The Rust engine parses documents locally and returns a `Document`: Markdown,
metadata, embedded asset bytes and explicit warnings. It never writes assets or
fetches external content inside a format adapter. Output naming, profiles,
network policy and optional model enhancement belong to the orchestration layer.

## Implemented readers

| Inputs | Reader | Current behavior |
| --- | --- | --- |
| TXT, MD, MARKDOWN | Rust text decoder | Preserves text and existing frontmatter; accepts UTF-8, BOM-marked UTF-16 and Windows-1252 |
| HTML, HTM, XHTML | scraper + htmd | Selects an article/main candidate, extracts metadata, removes navigation/scripts/hidden content, resolves relative HTTP links and images |
| CSV, TSV | csv | CSV preserves the reference's header width and raw cells; TSV retains the widest row and escapes table delimiters |
| IPYNB | serde_json | Markdown cells, fenced code and raw cells; metadata title and code language; code fences sized to protect embedded backticks |
| JSON | serde_json | Validated, pretty-printed fenced JSON; an additive Rust format |
| XML | quick-xml | Validated, fenced source; document types are rejected; content-specific XML rendering remains pending |
| EML | mail-parser | Decoded subject, body and MIME attachments; attachment bytes returned separately |
| DOC, DOCX, DOCM | anydoc document model | Headings, styled text, lists, tables, links, formulas, notes and embedded assets |
| PPT, PPS, POT, PPTX, PPTM, PPSX, PPSM | anydoc document model | Native presentation content through the same Markdown renderer |
| XLS, XLSX, XLSM, XLSB | anydoc document model | Native sheet content; exact cell-format compatibility has not been established |
| ODT, ODS, ODP, RTF, EPUB | anydoc document model | Native structured documents through the same Markdown renderer |
| PDF | anydoc / pdf-inspector | Native text/layout Markdown; scanned or image-only PDFs fail with a conversion error explaining the OCR requirement |

The native Office renderer reads the document once and preserves its embedded
bytes. References use `.markitai/assets/{name}` until the output layer assigns
final paths. A merged table's origin contains its content; covered cells are
empty, and a warning records this Markdown representation.

PDF extraction currently returns a warning because PDF image assets, screenshots,
local OCR and the Python implementation's page-marker contract are not yet
implemented. The browser runtime is outside this module. No Python interpreter,
Node runtime, Office installation, LibreOffice or hosted extraction service is
used by the readers above.

## Explicit remaining compatibility work

Image formats (JPEG/JPG/PNG/WEBP/SVG/GIF/BMP/TIFF/HEIC/HEIF/AVIF), MSG, Numbers,
Org, RST and TeX have no local adapter yet. They return an unsupported-format
error. `supports_extension` reports implemented adapters rather than a desired
future format list. Image routing with vision enhancement is an orchestration
capability and must not imply local OCR exists.

The HTML reader implements general article extraction. It does not yet implement
the original engine's X/GitHub/Hacker News/Reddit/YouTube/Substack/Bilibili/Steam
resolvers, structured conversation threads, four-stage adaptive recovery,
schema.org fallback, advanced footnotes, mathematical reconstruction, CSS media
visibility, or content-pattern scoring. These need independent tests before
advertising parity. Basic success on an HTML fixture is not evidence that its
full extraction contract matches.

The email reader preserves the body and attachments, but full Python header and
layout parity is pending. The XML reader preserves the original XML in a fenced
block; it does not yet reproduce the original dialect-aware Markdown adapter.
Office conversion can differ in whitespace, table header selection, numbering,
anchors, font-driven headings and metadata. Such differences must remain visible
in differential reports rather than being normalized away.

## Error and output principles

- A malformed or unsupported input returns an error. An error description is
  never persisted as a successful Markdown document.
- Native document parsers do not call remote OCR automatically.
- HTML links are restricted to ordinary HTTP(S), mail and telephone references.
  Relative references remain relative for a local file and become absolute when
  the caller supplies a source URL. Script links lose the destination while
  keeping their visible label.
- Format adapters return embedded bytes rather than downloading remote images.
- Text readers retain original Markdown frontmatter for the output layer to
  merge according to its public contract.

## Reference audit and acceptance corpus

The reference implementation was inspected read-only at
`/Users/example-user/work/markitai`. At the audit it contained 209 Python source modules
and 278 test modules. Converter, web extraction, LLM and provider code accounted
for approximately 8.8k, 15k, 9.8k and 5.5k lines respectively. These figures
describe migration scope and are not performance measurements.

The existing project supplies these useful compatibility inputs:

| Reference path (relative to `packages/markitai`) | Purpose |
| --- | --- |
| `tests/fixtures/` | Office/PDF/email/markup/structured text and legacy format examples |
| `benchmarks/docs_snapshots/expected/` | Four normalized end-to-end Markdown snapshots: PDF, DOCX, PPTX, XLSX |
| `tests/defuddle_fixtures/` | 209 paired HTML and expected Markdown cases |
| `tests/fixtures/web/*.expected.json` | Semantic thread and required/forbidden text contracts |
| `benchmarks/local_fixtures/` | Two additional website fixtures |
| `tests/unit/test_structured_text_parity.py` | Encoding, CSV quoting/ragged rows and notebook failure cases |

The recorded reference web benchmark dated 2026-09-22 had a mean score of 96.2
for 209 cases. Its guardrail floor of 86.58 is a tolerated regression boundary,
not the reference quality. The separate two-case corpus scored 99.47. These are
reference records only; this Rust engine has not passed those full gates.

The reference repository's `scripts/benchmarks/` also contains a 49-case cold/warm
performance harness and a 40-case quality audit of Markdown, metadata, image
hashes, dimensions, frontmatter and output assets. Use a frozen source identity
and isolated output directories. Run timing sequentially without concurrent
builds. Record failures and unsupported cases in the denominator. Compare speed
only after a case's content and asset checks pass.

## Dependency choices

The native parser dependency is [anydoc](https://github.com/firecrawl/anydoc),
whose Rust API exposes both Markdown and a structured document with embedded
assets. It already backed legacy DOC/PPT in the reference implementation. Its
native PDF reader cannot do OCR. This project adds its own renderer to avoid
discarding embedded images and to control Markitai's output contract.

HTML uses [scraper](https://docs.rs/scraper/) for an HTML5 DOM and selectors and
[htmd](https://docs.rs/htmd/) for Markdown serialization after explicit cleaning.
Email uses [mail-parser](https://docs.rs/mail-parser/) to handle MIME and transfer
encodings. Cargo.lock fixes the resolved versions. Changes to dependency versions
must run the corresponding output contracts.

The reference project previously evaluated Calamine and a PDF layout alternative
without adopting them. Their availability alone is insufficient justification
for replacing the committed behavior; compare cell formatting, layout and assets
before selecting a different backend.
