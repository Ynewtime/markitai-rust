# Complete Office page rendering

Office text extraction remains native Rust. Page screenshots optionally use an
installed LibreOffice to export PDF, followed by the native macOS PDF renderer.
LibreOffice is not bundled; its installation and fonts are additional runtime
requirements. The CLI itself remains one executable and invokes no Python
conversion code. macOS is the first rendering platform covered by this adapter;
having LibreOffice installed does not make unsupported PDF platforms available.

The adapter accepts existing presentation aliases `ppt`, `pps`, `pot`, `pptx`,
`pptm`, `ppsx`, `ppsm`, `odp`, and word-processing aliases `doc`, `docx`, `docm`,
`odt`, `rtf`. Spreadsheet screenshots return an explicit unsupported error.
Calc print areas do not establish a complete-workbook rendering contract.

For presentations, the ordered source slide list includes hidden and blank
slides. OOXML relationships and ODP presentation elements provide the count.
Legacy binary PowerPoint is first imported into a private PPTX copy so the
imported slide model can be counted; this is a LibreOffice import consistency
check, not proof of perfect Microsoft PowerPoint import fidelity. The PDF must
contain exactly that many pages. Hidden-slide export is enabled and notes pages
are disabled. Word exports all pages with blank-page omission disabled; it has no
reliable source paragraph-to-page mapping. These export options are described by
[LibreOffice's PDF parameter documentation](https://help.libreoffice.org/latest/en-US/text/shared/guide/pdf_params.html).

The existing document body is preserved. Ordered Slide/Page screenshot comments
are appended in an explicit appendix, rather than inventing a correspondence
between native paragraphs and rendered pages. Root conversion integration applies
the existing screenshot, local OCR, pure, VLM opt-out and model-page budgets.
Local OCR supplements native text; it does not replace it with PDF extraction.
See [PDF rendering](pdf-rendering.md) for page pixel and encoding limits.

## Execution and limits

Discovery searches absolute PATH entries and standard LibreOffice locations;
relative/empty PATH entries do not select a document-local executable. The
executable receives argument-array parameters, never a shell command. Each export
owns a private temporary directory containing an input copy, fresh LibreOffice
profile, and output directory. Macros and automatic link updates are disabled in
the private profile. Model credentials and other process environment values are
not inherited; PATH/platform loader state is retained, temporary/cache locations
are private, and HOME is never reassigned.

At most two exports run concurrently per process. A shared 120-second deadline
covers admission and both legacy normalization/PDF export subprocesses. A timeout
kills and waits for the child; Unix uses a dedicated process group to include
launcher descendants. Windows process-tree termination is implemented but has not
been validated on a Windows host. Output growth is polled every 25 ms. Inputs are
limited to 100 MiB; normalized PPTX plus exported PDF share a 100 MiB budget.
Presentations and resulting PDFs have a 1,000-page limit. XML parts used for source
counting retain existing bounded readers.

A successful exit alone is insufficient: exactly one expected regular output
file must exist, the PDF signature and document structure must parse, encryption
is rejected, and nonempty page count is checked. Empty, partial, malformed,
symlinked, excessive or unexpected outputs fail explicitly. Temporary output is
removed when its owner is dropped. Subprocess timing, output checks and private
profiles are not an OS memory/network sandbox; LibreOffice layout and native PDF
parsing can consume CPU/memory internally between checks. External document
resources and fonts remain subject to installed LibreOffice behavior.

## Fixtures and validation

Original fixture generators and hashes are under `office_render/fixtures`:
three presentation slides (visible, hidden, blank) with independent corner
colors; and Word pages with first/third-page text and a deliberately blank middle
page. SDK pixel checks establish full-frame rendering and slide order, separately
from text checks. Fault tests cover missing, malformed, short, unexpected and
symlinked PDFs, input preservation, timeout descendant cleanup and byte limits.
Optional installed-LibreOffice tests are explicitly ignored in the default gate;
they must be run separately and cannot count as success when the backend is absent.
Round 22's r4 validation passed the full gate and explicitly ran all six installed
backend tests: two core tests and four public API tests on macOS. These verify
hidden/blank slide order and full-frame pixels, the blank middle Word page,
all-page publication with native text retained, rejection before model requests
when the page budget is exceeded, pure/screenshot-only routing, and local OCR
supplements without replacing native text.

Initial direct backend probe used LibreOfficeDev `26.8.0.0.alpha0`, build
`2c87e51eeaa2b413ff4ae097b2705eea1995d8e5`. Both original fixtures produced PDFs;
that initial probe alone did not establish renderer/API acceptance. Its raw
commands, output hashes and logs remain in
`.local/media-llm-round22/office-probe/record.json`, separate from the later passing
tests above. Round 22 release CLI and installed-binding validation are still
pending. The six tests do not establish legacy binary input import fidelity,
cross-platform rendering, performance or Microsoft Office layout parity.

The initial Word fixture put two adjacent page breaks in one paragraph. This
LibreOffice version imported it as two pages even with blank-page omission
disabled. Separate paragraphs produce the intended three-page document; an
independent export checked first/third-page text and an entirely white middle
page. The authored generator and fixture now use those explicit paragraph
boundaries. The production PDF filter and three-page assertions were retained;
the original failed fixture and six comparative exports remain in the round's
investigation evidence.

The retained round-22 release CLI also passes full-page PPTX, legacy PPT and Word
acceptance, including hidden/blank pages and full-frame pixel checks. Refreshed
installed Node/Python/Go packages pass their binding tests against this core. See
[round-22 evidence](validation/media-llm-round22.md); these authored samples do not
establish Microsoft Office layout equivalence or other-host support.
