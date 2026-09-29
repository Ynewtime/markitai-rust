# Exact-version upstream license evidence

Packaging validates this directory offline before copying it. Package manifests,
published VCS records, original source notices and exact byte identities remain
part of the payload. This is mechanical evidence collection, not legal review.

Twelve of the fifteen supplemental crate versions now have at least one complete
license text. Six were collected directly from their exact upstream commits in
R29. R30 adds five Apache-2.0 options and selectors' MPL-2.0 terms from the official
publishers expressly linked by each pinned source notice. The helper permits only
these reviewed package/version/commit and URL/digest combinations; arbitrary URLs
and later versions cannot inherit them. Original notices remain unchanged.

Three packages remain unresolved: objc2 0.6.4, objc2-encode 4.1.0 and
objc2-foundation 0.3.2. Their original upstream MIT text, including Steven Sheldon’s
notice, is retained as supplemental/review-needed. The original 2016 file exactly
matches the file removed by the upstream 2025 licensing clarification. Target-
anchored histories and both patches are included. Current manifests remain MIT
and explicitly allow MIT for future contributions. This is a historical notice,
not a file present in the target release; it does not close those three full-text
and copyright-review flags. No author/year or license grant was fabricated.

The complete original selectors 0.38.0 source archive is included at
`source-archives/selectors-0.38.0.crate` (SHA-256
`8adfa1c298912827b8a28b223b3b874357397ae706e6190acd9bf28cee99114d`).
It is the unmodified crates.io archive, with matching VCS/manifest and all 22 files
checked against the published local registry source. The MPL license and original
notice are under `overlay/licenses/selectors-0.38.0/`. The manifest records the
canonical source download URL as well as this offline copy. See MPL sections
3.1–3.4 for source and notice terms; supplying these materials is not a legal
assessment of every dependency or distribution scenario.

The objc2-family Apple SDK discussion is preserved as written upstream. Apache
section 4 notice conditions and any other existing notices are not displaced by
adding the complete terms. The historical MIT evidence does not relicense the
MIT-only crates. Collection does not elect an alternative for other packages.

`inventory.json` covers every other file and detects unreviewed changes; the
source/build identity binds that inventory. `round30/provenance.json` and raw
response records retain successes and failed retrieval attempts. The generic MIT
publisher template was not substituted. Old R29 evidence and its recorded 404
remain intact. CLI archives, npm packages and Python wheels copy the validated
subset as well as the static-Go bundle; wheel paths are relative to its
`.dist-info/licenses/` attribution directory.
