# Raster images and shared assets

Raster preparation lives in the Rust core, so CLI and language adapters use the
same decoding, resize and asset rules. No Python process, image editor or remote
image fetch is involved. The enabled codecs are JPEG, PNG, GIF, BMP, TIFF and
WebP; SVG rendering and HEIF/AVIF decoding remain unfinished.

## Standalone inputs

A standalone raster file with LLM enabled is decoded within resource limits and
sent as actual image content to the configured vision-capable model pool.
Orientation is applied when encoding a new payload. The
base document retains a local image reference, while the enhanced document is
the model's Markdown. Standalone inputs bypass embedded-image size filtering.
When re-encoding GIF or animated WebP, the first image supplies the pixels.
With compression disabled their original streams are retained; provider-specific
animation handling has not been validated. Multi-page TIFF is rejected before
upload until all-page routing is implemented. The reader never presents the
first TIFF page as a complete multi-page document.

With both LLM and OCR disabled, CLI image items are skipped with
`skip_reason="image_only"`; no Markdown file is written. The shared library API
returns a conversion error explaining `llm=True` and `ocr=True`, matching the
reference Python API. Missing files still report a missing-file error.

`ocr=True` with LLM enabled selects image vision. `MARKITAI_NO_VLM_OCR` prevents
that upload; the current build reports that the local backend is unavailable.
Local OCR, PDF page OCR, SVG/HEIF/AVIF conversion, screenshots, alt-text and
image-description enrichment still have explicit unsupported paths.

## Embedded assets

After document extraction and before output profiles, the core processes raster
assets already held in memory. It applies EXIF orientation, configured minimum
width/height/area filters and raw-byte deduplication. All references to a duplicate
are rewritten to the retained asset. Filtering removes rendered references;
examples inside code spans and fenced blocks remain literal.

Each preparation stage collects original paths and applies one mapping to the
document. File publication does the same for content-addressed destinations in
both base and enhanced Markdown. A generated filename matching another original
asset name therefore cannot redirect or remove an already relocated reference.
When input assets use the same source path more than once, the first entry
determines that path's reference mapping, including retained or filtered assets.
The original asset order is preserved.

Rewriting handles Markdown links, reference definitions, wikilinks and exact
frontmatter path scalars, plus HTML `a[href]`, `img[src/srcset]`,
`source[src/srcset]`, `video[src/poster]`, `audio[src]` and `track[src]`.
All relevant attributes are visited once; the first duplicate attribute wins.
Candidate lists preserve surviving descriptors and URI suffixes. Filtering a
candidate retains alternatives; filtering media destinations removes the affected
attribute rather than its container. Audio/video sources alone do not count as
image references; image candidate lists and video posters do. CSS/style content,
custom lazy-load attributes, code and comments remain literal.

With compression enabled, maximum width/height use proportional Lanczos resize.
JPEG uses the configured quality and composites alpha onto white. PNG keeps
transparency. WebP currently uses a lossless native encoder, with a warning that
`image.quality` has no effect for WebP. Native encoded bytes are not claimed to
match Pillow's encoders; image-output parity needs pixel/quality measurements in
addition to the existing strict asset-hash audit.

With compression disabled, supported embedded image bytes remain unchanged.
The actual decoded format determines the output suffix. Standalone JPEG/PNG/
WebP/GIF assets and vision payloads also preserve original bytes in this mode;
BMP/TIFF previews become PNG without resizing. Metadata such as EXIF orientation
is retained in the unchanged payload; orientation is baked in only when encoding
new pixels. Provider size limits are not yet reproduced for every service.

Unsupported or malformed embedded images retain their original bytes with a
warning. This is an explicit fidelity choice: conversion does not quietly erase
an asset simply because its codec is unavailable. Non-image attachments bypass
raster processing entirely. Input files are never modified.

## Resource boundaries

Each raster decode allows at most 32 million pixels and a 256 MiB decoder
allocation budget. Dimensions are checked before decoded pixel allocation;
codec limits can reject earlier. The total temporary working set may be larger
while resizing, compositing or encoding because those buffers coexist. This is
not a peak-RSS guarantee. General local inputs have the reference's 500 MiB
preflight limit; individual parsers retain their tighter format-specific limits.

The implementation uses the bounded decoder interfaces in
[image](https://docs.rs/image/0.25.10/image/struct.ImageReader.html) and its
[resource limits](https://docs.rs/image/0.25.10/image/struct.Limits.html).
No codec downloads or external executable installation occur during conversion.
