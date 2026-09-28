# Numbers tables

The local reader accepts modern, single-file `.numbers` ZIP packages and decodes their IWA streams in Rust. It uses the pinned MIT-licensed `iwork = 0.2.1` library. Conversion does not launch Numbers, Python, a browser or a remote service.

## What is retained

Sheets follow the document's sheet-reference order. Tables follow each sheet's drawable-reference order. Object identifiers distinguish tables with the same name; names are never used to deduplicate content. A sheet becomes a level-one heading and each table becomes a level-two heading. Decoded tables lacking a sheet reference are appended with a warning.

The reader retains Unicode text, blank positions, booleans, saved numeric values, dates and durations. Formula cells use the value saved in the document; they are not recalculated. A warning identifies this limitation. Error cells become `#ERROR!`.

Tables with one header row and no merged cells use Markdown pipe tables. Other tables use HTML, preserving header cells and merged anchors through `rowspan` and `colspan`. Covered positions are not repeated. Overlapping or out-of-bounds merges are errors. Hidden and filtered rows and columns remain in stored order, with a warning; this reader does not reproduce Numbers' current filtered view.

Extractor metadata includes `sheet_count`, `table_count`, `numbers_value_mode: saved-values` and an ordered `numbers_tables` list. Each table records its sheet/name, dimensions, header/footer counts, cached-formula count and merged ranges. Merge row/column coordinates are one-based. This extractor metadata does not add fields to the existing conversion frontmatter contract.

## Values are not a screenshot of Numbers

The adapter does not treat `CellValue::to_text()` as a complete display formatter. It explicitly handles basic decimal places, percentages and currency codes, using decimal128's integer mantissa rather than passing numbers through binary floating point. Fixed precision is supported through twelve places and rounds halfway values away from zero. Percentages multiply the saved value by 100. Currency output uses an available three-letter code, such as `USD 12.50`, rather than guessing a locale-specific symbol. Very large exponents retain an exact scientific representation.

Dates use an ISO-style calendar/time representation without an invented timezone; the stored value does not identify a timezone. Fractional date seconds are not retained. Durations use seconds. Rich-text cell text is retained, but fonts, colors, bullets and inline styling are flattened. Scientific/custom/fraction formats, locale separators, accounting signs, conditional formatting and interactive controls are not reproduced exactly. A formatting warning accompanies such normalized output.

Only tables are extracted. Linked non-table canvas objects, including charts, images, shapes and text boxes, produce an omission warning. Their geometry, captions and image assets are not exported. This is not a canvas renderer. Directory packages, encrypted documents and older XML/pre-BNC cell storage are not supported by this reader; damaged or unsupported tables fail rather than silently becoming empty successful output.

## Resource bounds

Checks precede high-level table decoding:

| Resource | Limit |
| --- | --- |
| Input ZIP and expanded package entries | 128 MiB each aggregate |
| ZIP entries | 4,096 |
| Individual expanded ZIP entry / IWA stream | 32 MiB |
| Expanded IWA streams combined | 64 MiB |
| Snappy block | 64 KiB expanded |
| IWA objects | 100,000 |
| Tables / sheets | 1,024 each |
| Rows / columns per table | 100,000 / 1,024 |
| Declared table positions combined | 2,000,000 |
| Individual cell text | 1 MiB |
| Rendered table body | 32 MiB |

An additional conservative decoded-cell estimate charges every declared position for a cell structure plus the document's longest interned/rich-text value, with a 256 MiB ceiling. It can reject a large sparse table containing a long string even when many cells are blank. Referenced table side-list payloads are charged again for each model and limited to 64 MiB. These are allocation guards, not a promise that total process RSS stays below one of those numbers.

The ZIP reader limits actual bytes read and verifies entry data. It rejects duplicate names, path traversal, non-regular entries and encrypted packages. Nothing is extracted to the filesystem. IWA headers, Snappy expanded lengths, object identities and table dimensions are checked before `iwork` allocates table rows and columns. Multiple table-info objects may not point to the same table model: otherwise the decoder would multiply allocations outside the dimension budget.

Output size is checked after each heading and cell, so rejection may temporarily exceed the body limit by one escaped cell, at most about 6 MiB. The adapter avoids scanning every merge for every cell by using a bounded coverage bitmap.

## Fixtures and evidence

Two unmodified public files come from the independent MIT-licensed [`numbers-parser` repository](https://github.com/masaccio/numbers-parser/tree/726bd6cbbfe1d00ec5449865a51eb7a4a3f472aa), fixed at commit `726bd6cbbfe1d00ec5449865a51eb7a4a3f472aa`:

- `test-1.numbers`: two sheets, three tables, ordered names, dimensions and cell values independently specified in its [`test_tables.py`](https://github.com/masaccio/numbers-parser/blob/726bd6cbbfe1d00ec5449865a51eb7a4a3f472aa/tests/test_tables.py).
- `test-formats.numbers`: rich-text/bullet content independently specified in [`test_styles.py`](https://github.com/masaccio/numbers-parser/blob/726bd6cbbfe1d00ec5449865a51eb7a4a3f472aa/tests/test_styles.py). Despite its filename, this is not a golden for numeric display formatting.

Original bytes, hashes, source URLs and the upstream license are retained in [fixture provenance](../crates/markitai-core/src/formats/numbers/fixtures/provenance.json) and [LICENSE.rst](../crates/markitai-core/src/formats/numbers/fixtures/LICENSE.rst). No local Apple application was used to inspect or regenerate them.

Authored tests cover Unicode and Markdown escaping, fixed decimal/percentage/currency output, cached formulas, merged cells, duplicate table names across sheets, bounded expansion and hostile dimensions/references. The duplicate-name fixture first creates distinct tables, then changes only the second model's name through the library's public archive API because its writer rejects ambiguous names. These authored fixtures supplement the independent files; writer/reader round trips alone are not evidence of complete Numbers compatibility. Public conversion tests check in-memory/disk body agreement without changing the existing frontmatter schema. Coordinated gate and release results are recorded separately.
