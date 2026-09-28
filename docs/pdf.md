# Native PDF text and layout

The PDF reader uses `pdf-inspector` for decoding and page-level reliability
decisions, with `lopdf` for bounded content inspection and embedded images.
There is no Python runtime, external converter or implicit OCR fallback.

Layout refinement runs only after the existing page checks. A page requiring
OCR, containing suspicious hidden text, or having incomplete content inspection
does not enter refinement. The existing narrowly guarded font-decoded recovery
for a false scan verdict remains in place. A missing or unreadable page keeps
its page marker and an explicit warning.

This preserves the visibility gate; it does not repair every visibility issue
in the underlying reader. An authored page containing visible prose and a
separate `Tr 3` invisible run currently reproduces that reader's inclusion of
the invisible run. Refinement leaves that page unchanged and retains the
explicit warning that complete hidden-text filtering is not established.

## Layout reconstruction

Reliable pages receive one additional positioned-text pass. Font sizes are
weighted by character count across eligible pages; larger sizes determine a
consistent heading hierarchy across the document. Physical line gaps distinguish
wrapped paragraphs from separate paragraphs. Adjacent runs retain bold, italic,
underline and strikeout styling, including a continuous style across a wrapped
line.

Ruled tables are reconstructed from complete horizontal and vertical borders.
Stroked lines and thin filled polygons are supported, including transformed
coordinates and a nonzero page origin. The complete grid determines the column
count: empty cells and empty trailing columns remain. Multiple physical text
lines in a cell use `<br>`. A row spanning a border, an incomplete grid or
overlapping table regions prevents speculative reconstruction. The first row is
represented as a Markdown table header; this representation does not establish
that an untagged PDF declared it a semantic header.

Before replacing page Markdown, decoded alphanumeric character counts must agree
with the existing reader. This is a conservative agreement check, not proof that
two decoders are independently correct. Unknown links or form-field semantics,
rotated pages or text, invalid geometry, raised/lowered runs and ambiguous
side-by-side prose retain the existing page reader's output. A page can therefore
remain unchanged even when another page gains layout fidelity.

The refinement limits are 250,000 positioned items and 16 MiB decoded text across
the selected document, 20,000 items per page, 200,000 operations and 8,192 rule
edges per geometry pass, 64 graphics-state levels, 32 tables, 32 columns and
4,096 cells per table. Existing 64 MiB expanded page/Form inspection and image
budgets remain. Positioned-item limits are checked after the dependency returns;
they are not a claim of a hard allocation limit inside that dependency.

## Images and remaining work

Executed embedded raster images retain their existing extraction path and are
appended to the corresponding page. Exact placement and reference JPEG encoding
remain different. There is no screenshot or vector-figure renderer in this
reader. `resvg` renders SVG but does not interpret PDF graphics state: a faithful
PDF figure also needs font programs, Form matrices, clipping, shadings, blend
modes and masks. Drawing table borders from a bounded subset of PDF operators
does not establish those rendering semantics.

Table geometry is deliberately disabled when curves, resource-dependent colours,
transparency, Form/image invocations, shading or nonrectangular clipping make its
verdict uncertain. Text extraction can still proceed. Borderless or merged-cell
tables, complex columns, mathematical layout, local OCR and vector charts remain
open. The historical five-page sample's chart must not be presented as recovered
merely because its textual labels are extractable.

The additional positioned pass currently reparses the PDF. Sharing one decoded
document with the dependency is a future performance optimization; no speedup
or corpus-parity claim follows from this implementation. Focused tests author
their own PDF streams for heading consistency, paragraphs, continuous emphasis,
complete tables, hidden text and rotated/invalid geometry. Validation results
are recorded by the coordinator after the source is frozen.
