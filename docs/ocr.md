# Local image OCR

On macOS 11 or later, local OCR uses the operating system's Vision framework
through Rust bindings. It runs in the current process without Python, Node,
Tesseract, a browser, downloaded OCR weights, provider credentials or a remote
recognition service. Other platforms currently return an explicit unsupported
error for this local path. This is an image implementation; PDF page OCR and
multi-page TIFF OCR remain unfinished.

## Selection and output

`ocr.enabled=true` (`--ocr` in the CLI) selects local OCR when LLM enhancement is
disabled. The image reference remains in the base document and recognized text
is appended below it. Metadata records `ocr_used: true` and `ocr_path: vision`.
A successfully processed image with no recognized text retains its image
reference and emits a warning. An invalid image, unsupported language or engine
failure fails the conversion instead of inserting error text into Markdown.

With LLM enabled, the existing image-to-model route remains the default.
Setting `MARKITAI_NO_VLM_OCR` to a nonempty value other than `0`, `false` or `no`
(case insensitive, ignoring surrounding whitespace), together with OCR enabled,
chooses local OCR followed by text enhancement. Image bytes
are not included in that model request. Enabling OCR does not enable LLM or
require a configured model. Disabling both still produces the existing image-only
error or CLI skip behavior.

OCR consumes the original raster bytes, independently of preview compression,
image width settings and embedded-image filters. Static SVG uses the existing
bounded in-process SVG rasterizer. JPEG, PNG, GIF, BMP, single-page TIFF and WebP
use the enabled Rust decoders; animation uses its first image. HEIF and AVIF are
not implemented. Multi-page TIFF is rejected rather than silently recognizing
only its first page.

## Language selection

`ocr.lang` defaults to `en`. Each call asks the configured accurate Vision
recognizer which languages its current OS and request revision support, then
requires a match. It never substitutes English for an unsupported language.
System revisions can change supported languages and recognition results.

| Configuration spelling | Requested Vision language |
| --- | --- |
| `en` | `en-US` |
| `zh`, `zh_cn`, `zh-cn`, `cn`, `ch` | `zh-Hans` |
| `zh_tw`, `zh-tw`, `cht`, `chinese_cht` | `zh-Hant` |
| `ja`, `jp`, `japan` | `ja-JP` |
| `ko`, `korean` | `ko-KR` |
| `ar`, `arabic` | `ar-SA`, if installed Vision supports it |
| `fr`, `french`; `de`, `german` | `fr-FR`; `de-DE` |
| `es`; `it`; `pt` | `es-ES`; `it-IT`; `pt-BR` |

Case, surrounding whitespace and underscore/hyphen aliases are normalized.
Other short ISO codes may match the system's regional spelling. An explicit
region or script must match exactly, ignoring case. The reference's RapidOCR
model-family names such as `latin` and `cyrillic` are not Vision languages and
are rejected. Structural configuration acceptance does not imply runtime
language availability. `ocr.per_page_routing` has no effect on this image path.

## Bounds and reading order

Encoded input is limited to 64 MiB. Decoded images must fit 32 million pixels,
with a 256 MiB decoder allocation limit. EXIF orientation is applied before OCR;
alpha is composited over white. A bounded PNG buffer of at most 128 MiB transfers
the normalized pixels to Vision in memory. Source paths, user configuration
files and provider keys are not passed to the engine.

The request uses accurate recognition with language correction. At most 10,000
observations and 8 MiB of recognized text are accepted. Text and line rectangles
are checked for finite, valid confidence and geometry. Internal rectangles use
top-left pixel coordinates after orientation correction, and mean confidence
uses nonempty recognized lines. These diagnostics are not new fields in the
public conversion JSON.

Lines are ordered top to bottom; observations sharing at least 60% of the shorter
vertical band form a left-to-right row. A vertical gap greater than 80% of the
taller adjacent row separates paragraphs. This handles ordinary upright text;
the reference's column, table, vertical-writing and marginal-note reconstruction
is not reproduced. A wide multi-column document can therefore have a different
reading order. Vision inference itself has no wall-clock cancellation deadline
in this slice, and recognition quality is not guaranteed for handwriting, small
text or every supported language.

## Validation fixtures

The authored [English PNG](../crates/markitai-core/src/ocr/fixtures/english.png)
contains three lines drawn from simple bitmap glyphs, without textual PNG
metadata. Its independent [expected text](../crates/markitai-core/src/ocr/fixtures/english.txt)
is `MARKITAI OCR`, `LOCAL TEXT ONLY` and `2026`. The macOS test requires every
token, valid confidence and at least three line rectangles. The same test checks
a white image, undecodable input and an unavailable language. Pure tests cover
language aliases, bounds, alpha handling, row/paragraph assembly and malformed
geometry. These are focused checks, not an OCR accuracy corpus or a claim that
results match RapidOCR on arbitrary documents.

Implementation references: Apple's [text-recognition guide](https://developer.apple.com/documentation/vision/recognizing-text-in-images),
[recognition request](https://developer.apple.com/documentation/vision/vnrecognizetextrequest)
and [in-memory request handler](https://developer.apple.com/documentation/vision/vnimagerequesthandler/init(data:options:)),
plus the maintained [objc2 Vision bindings](https://docs.rs/objc2-vision/0.3.2/objc2_vision/).
