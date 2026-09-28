# Authored PDF rendering fixtures

These six small PDFs were authored for this project. `expectations.json` retains
the original preparation manifest, including each PDF's SHA-256, expected page
geometry, corner colors and text. Its preparation status is historical; actual
rendering assertions live in `../tests.rs` and run only with the native SDK.

The original generator SHA-256 was
`68fd5cb66a22ba853c4bcc7d26bf092e396242ce7df85d5a422b1c2f14ac9ccc`.
The tracked generator differs only in locating the repository relative to itself
instead of an absolute workstation path. Run it with Python's standard library
and `--out` pointing to a new directory to regenerate the inputs. Compression
bytes can depend on the zlib version recorded in its manifest.

The scan is the existing OCR fixture `../../ocr/fixtures/english.png`, embedded
without decoding, editing or recompressing its PNG IDAT payload. That image's
SHA-256 is `a9f547051baeaaa96c9299363a3c79b1fb3842c2c7192c0bde0663f86081b94d`.
No external PDF renderer, image editor, OCR process or network is used by the
generator. The PDFs total 13,162 bytes.

The rotation fixtures counter-rotate their content, so correct displayed pixels
have red/green/blue/black top-left/top-right/bottom-left/bottom-right corners at
every quarter turn. A separate text sentinel sits outside CropBox. The Form
fixture combines nested transforms, vector fills and zero-alpha text; it does
not exercise an image mask. An independently authored three-level soft mask is
constructed by the Rust test, with white/pink/red pixel expectations.
