# Output assembly

The Rust core separates extraction from output assembly. Readers return Markdown,
source metadata and byte assets. The output layer resolves the public title,
normalizes ordinary Markdown, reserves paired filenames and writes files atomically.

## Ordinary and pure output

Ordinary local output has generated `title`, `source` and `markitai_processed`
fields. Reader metadata does not automatically become local frontmatter. URL
output can retain trusted fetch metadata; canonical fields and unreliable language
metadata are excluded from that merge.

Explicit reader titles take precedence. Otherwise the basic workflow uses its
first heading or the source stem. CSV, TSV and XML fall back to the complete source
filename because a data row or element name is not a reliable document title.

Ordinary Markdown receives heading spacing, trailing whitespace and blank-line
cleanup, residual placeholder removal, broken-link repair and repeated page-footer
cleanup. It ends with one newline. Existing input YAML remains part of the body
beneath the generated frontmatter, matching the reference workflow.

Pure output bypasses this normalization. Existing YAML is parsed for API metadata,
but its original bytes and the original body are retained for file output. Pure
input without YAML has an empty frontmatter map. The OKF profile still adds its
required document type and generator identity; generated timestamps use UTC `Z`.

Pure model requests receive the complete original reader Markdown, including any
input YAML. API metadata is parsed from the output actually retained. Normal LLM
output is normalized separately; a retained base file keeps its own metadata.
Profiles run after enhancement on both outputs so model-produced page markers and
image references receive the same transformations as reader-produced content.

## Files and assets

Output names preserve the source extension: `report.pdf.md` and
`report.pdf.llm.md`. Both share a conflict namespace; renamed results begin with
`report.pdf.v2`. Explicit CLI filenames and batch-reserved stems reach this layer
as private configuration fields. Assets are addressed by SHA-256 content prefixes.
The public result contains only durable asset paths when an output directory was
provided. Asset remapping recognizes inline images/links, wiki references,
reference definitions, and HTML `img`/`a` destinations, including multiline tags.
It preserves titles and other attributes while leaving code and literal examples
untouched. Filtered reference images are removed together with their definitions;
download-link labels remain readable.

The current implementation serializes file reservations and writing within one
process. Cross-process reservations and throughput on asset-heavy batches still
need dedicated validation. Malformed input frontmatter remains content.

## Verification

Unit checks cover pure byte preservation, existing YAML, structured-data titles,
local metadata exclusion, nested fences, repeated footers and OKF timestamps.
The format audit additionally compares complete API Markdown and metadata, allowing
only the generated clock field to vary. Historical baselines remain unchanged.
