# EML messages and Content-ID images

The native EML reader uses the MIME parser to decode transfer encodings, character
sets and structured headers, then selects a body from the MIME tree. It performs
no network requests and opens no attachment paths. Its output is an owned Markdown
document and byte buffers for attachments; normal asset processing, model analysis
and publication happen afterward.

## Body selection

HTML takes precedence over plain text among eligible message-body parts.
`Content-Disposition: attachment` parts and attached `message/rfc822` messages
cannot become the outer message's body. Within `multipart/related`, its `start`
parameter selects the root by Content-ID; without a usable unique root, the first
member is selected and an invalid explicit `start` produces a warning. Other
related members are resources, not competing body candidates. Multipart ordering
is preserved while searching alternative/mixed body branches.

Actual HTML runs through the existing fragment sanitizer and Markdown renderer.
Plain text stays plain body content. HTML failures do not cause raw, unsanitized
HTML to be emitted as a fallback. Unknown charsets or malformed transfer encoding
are reported when the MIME parser supplies recovered content. MIME parsing remains
tolerant of recoverable mail transport damage; this is not a strict validator for
every RFC grammar rule.

The existing native `# Email Message`, header order, `## Content`, date formatting,
and ordinary `[Attachment N](...)` layout remain. Header angle brackets are escaped
and controls are collapsed. Ordinary attachments keep download links, including
unreferenced image attachments. A filename alone never promotes an attachment
into an inline image.

## Content-ID binding

Only real HTML image targets are inspected before sanitization. Images in script,
style, comments, code/preformatted examples, ordinary text, or unrelated attributes
are not rewritten. Real `img` sources and supported image candidate attributes use
the common image-reference parser. HTML text containing literal Markdown image
syntax is not treated as an HTML image element.

A reference binds to a MIME part only when:

- its `cid:` scheme is recognized, its percent encoding decodes once, and its ID
  has no whitespace, controls or malformed angle brackets;
- the part belongs to the same nearest `multipart/related` scope as the selected
  body, or both belong to the outer unscoped message;
- the ID identifies one part, that part declares an `image/*` MIME type, and its
  transfer-decoded binary payload is nonempty without a parser encoding error.

Header names and the URI scheme are case-insensitive. Exact Content-ID spelling
wins. For mail-client compatibility, an ASCII case-insensitive match is accepted
only when it identifies one part; case-folded ambiguity never chooses an arbitrary
image. Duplicate exact IDs, missing IDs, non-image parts, and resources in another
related scope remain unresolved and receive warnings. An ordinary `.png` attachment
with an `application/*` MIME type does not satisfy these rules.

Resolved targets point to owned assets before HTML sanitization. Unresolved targets
survive the sanitizer through per-message collision-checked placeholders and are
restored as CID URIs afterward; their labels and surrounding body remain. The
restoration preserves URI percent escapes rather than interpreting them as raw
filesystem filenames. Unresolved CID schemes must never fall through to local
file lookup during image enrichment.

Attachment filenames are decoded by the MIME parser, then reduced to bounded safe
labels prefixed with their ordinal. Slashes, controls and non-ASCII characters
cannot become path traversal. They are not filesystem inputs. Transfer-decoded
bytes remain associated with the part identity, so filename collisions or misleading
extensions cannot redirect CID lookup.

## Publication and model analysis

CID images use the existing embedded-image pipeline. Filtering and compression
apply to owned images, and final content-addressed paths replace their references.
Repeated references to one image share one analysis and asset. With LLM image
caption/description options enabled, successful analysis changes enhanced captions
and contributes `ConversionOutput.images` and the published `images.json` entry.
Base author captions and attachment download links remain available. Profile
changes, such as the visible `assets/` directory in RAG, apply to both references
and metadata paths.

Unresolved images do not create successful analysis records. The reader itself does
not submit images or trust instructions embedded in mail; downstream model calls
remain opt-in and use the shared document budget. See
[image enrichment](image-enrichment.md) for the full failure and accounting policy.

## Boundaries and reference behavior

Input is rejected above 100 MiB before MIME parsing. Parsed messages are limited to
4,096 total parts, 64 levels of multipart/nested-message structure and 128 MiB of
decoded MIME content. These checks bound admitted documents; they do not claim a
streaming parser or a measured peak-memory ceiling. Image decoding has its own
pixel and payload limits downstream. Nested messages remain downloadable `.eml`
attachments; this implementation does not expand them recursively into the body.

The reference Python reader also prefers HTML bodies and uses the email library's
related-root selection. It renders image attachments as data-URI images, ordinary
attachments as a name/size list, and one level of attached messages as quotations.
It does not supply the scoped CID-to-owned-asset association implemented here.
The Rust change intentionally preserves its established attachment download-link
interface instead of adopting those unrelated output differences. Unreferenced
image-attachment promotion and nested-message quotation remain separate parity
items; CID support does not claim that all EML output matches the reference.

Focused tests cover unchanged no-CID output, body versus attachment selection,
related roots and scope isolation, exact/case-folded IDs, percent and entity
boundaries, duplicate/missing/non-image IDs, malformed encodings, safe filenames,
charsets, literal HTML boundaries and depth rejection. Public conversion tests use
real multipart EML, generated PNG, isolated state and a loopback model to verify
captions, unchanged base text, published bytes, absolute metadata paths and normal/
RAG profiles. Execution results are recorded by the coordinator after integration;
no live-provider or cross-platform claim follows from fixture authoring.
