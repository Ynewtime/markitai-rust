# Native PDF page rendering

The macOS backend draws complete PDF pages with CoreGraphics in process. It
holds one `CGPDFDocument` and an owned immutable copy of its input for the session.
Page drawing includes native text, vector paths, image masks and Form XObjects;
it does not approximate a page by collecting embedded images. No Python, browser
or command-line renderer is involved. Other platforms return an explicit
unsupported capability error.

`PdfRasterSession::dimensions(page, dpi)` validates a one-based page number and
the complete drawing frame before allocating pixels. `render(page, dpi)` uses
the same checks and returns upright RGB8 pixels composited onto white. The frame
is CropBox intersected with MediaBox, with the page's quarter-turn rotation.
CoreGraphics supplies the point-space crop/rotation transform; the adapter then
applies DPI scaling explicitly, since Quartz's fit transform does not upscale
to a larger bitmap. Aspect ratio is preserved within ceil-rounded dimensions.
The media orchestration layer chooses 150 DPI
and manages document-wide page, pixel and screenshot budgets.

Each raster is limited to 32 million pixels. Coordinates and DPI must be finite,
dimensions must be positive, and all row/buffer products are checked. Input is
limited to 500 MiB. Invalid/locked documents and invalid frames fail explicitly;
the backend does not attempt passwords. `dimensions` allows the caller to reject
a cumulative budget excess before creating that page's bitmap. A session is
confined to its caller's thread and writes no files.

At the maximum size, the explicit RGBX drawing buffer occupies 128 MB and the
RGB return buffer occupies 96 MB during conversion. CoreGraphics may allocate
additional internal memory and spend unbounded time on document parsing/drawing;
these application limits are not a renderer sandbox or a hard SDK timeout.
Rasterization is separate from native text reliability, OCR routing and screenshot
publication, which must report their own page outcomes rather than infer success
from a nonempty pixel buffer.

The native tests exercise authored native/scanned/blank pages, all four rotations
with independent corner colors, nonzero and intersected crop origins, nested
Forms, vector fills, zero alpha, a real image soft mask, owned input lifetime,
locked/malformed documents and oversized geometry. The fixture manifest records
input hashes and independent expectations. Compilation and execution results
belong to the coordinator's validation record; fixture preparation alone is not
rendering evidence.

The backend follows Apple's documented
[PDF page drawing and transform APIs](https://developer.apple.com/library/archive/documentation/GraphicsImaging/Conceptual/drawingwithquartz2d/dq_pdf/dq_pdf.html).
