# Native browser fetching

The `playwright` fetch strategy retains its public name and uses Chromium's
DevTools Protocol directly from Rust. It does not import Playwright or run
Python, Node.js, or a browser automation CLI. Chrome/Chromium remains an optional
installed executable; the standalone Markitai binary does not bundle it.
The integrated workspace gate and frozen release pass real loopback Chrome
fetching, capture, timeout, history and cleanup checks. Source and artifact
identities are recorded in the [validation report](validation/native-backends-round16.md).

Each fetch launches a new headless process with a temporary user-data directory,
a separate disk-cache directory, a loopback debugging endpoint on a dynamically
selected port, and no reused browser profile. Extensions, synchronization,
background update services and password/keychain integration are disabled.
The browser inherits only a small platform environment allowlist; provider keys
are not forwarded. The implementation keeps the browser sandbox enabled.

`MARKITAI_BROWSER_EXECUTABLE` selects an exact executable. An invalid override
fails discovery instead of falling back to another installation. Otherwise the
reader checks a completed private [managed installation](browser-installation.md),
then executable names in `PATH`, common macOS/Windows application
locations, then known Playwright Chromium cache layouts. Cache discovery honors
`PLAYWRIGHT_BROWSERS_PATH`; only executable paths are inspected. It never loads
stored browser state, installs a browser or opens the user's normal browser.
Availability checks do not launch a process.

## HTTP authentication and diagnostics

`fetch.playwright.http_credentials` accepts an object containing `username` and
`password`, with optional `origin` and `send`. Credentials are supplied only after
a Chromium **Server Basic** challenge. They are not placed in browser arguments,
URLs or a global Authorization header. The username and password are each bounded
to 4,096 UTF-8 bytes; control characters and a colon in the Basic username are
rejected. Empty strings are valid configured credentials. Unknown fields and
invalid types fail before a browser is discovered or launched.

Without an explicit origin, the credentials are restricted to the initial URL's
scheme, hostname and effective port. An explicit HTTP(S) origin is matched exactly
and can authorize a known redirect destination. A trailing root slash is accepted;
userinfo, non-root paths, query strings and fragments are rejected. This default
is deliberately narrower than the reference Playwright behavior, which permits
credentials at any challenged origin when no origin is specified. Redirects and
third-party subresources do not gain credential authority from the referring page.
Proxy challenges never receive the server password. `send=always` and
`send=unauthorized` retain Playwright's browser semantics: neither makes these
browser requests send a preemptive Authorization header.

Each intercepted request receives the configured credentials at most once;
a repeated challenge is cancelled. A navigation session accepts at most 128
challenge events, including rejected events. Authentication shares the existing
navigation deadline. Errors do not include challenge payloads or credentials.
Digest, NTLM/Kerberos and authenticated proxies are not supported by this first
implementation. Extra HTTP headers retain their separate context-wide contract
below; this credential policy does not silently narrow explicitly supplied headers.

The internal presence helper allows the fetch coordinator to route authenticated
`auto` requests before anonymous cache access or PDF probes. Nonempty browser
cookies or extra HTTP headers also select this private route, including without
screenshot capture. Empty collections retain ordinary static-first behavior. Explicit `static`
does not acquire browser credentials. Initial top-level PDF responses now return
the authenticated response bytes to the native PDF pipeline, described below.
Rendered private pages and authenticated PDF bytes are not added to the static
fetch cache.

`browser_diagnostic()` discovers an installed executable, launches a fresh private
profile and verifies a CDP `about:blank` page, then closes and reaps the process.
It returns no executable when none is installed, or a fixed sanitized error when
initialization fails. It does not load conversion settings, proxies, cookies or
credentials. Startup has a 15-second bound and protocol setup/evaluation a shared
five-second deadline, followed by process cleanup. Discovery alone remains a
separate non-launching availability check.

Optional tests explicitly launch installed Chromium against authored loopback
servers for successful Basic authentication and actual screenshot pixels, bounded
wrong-password failure, cross-origin redirect isolation with an explicit-origin
positive case, third-party resource isolation, and diagnostic/profile cleanup.
These are ignored in the ordinary workspace invocation and must be selected
explicitly by the coordinator; implementation does not itself establish a passing
release acceptance result.

## Authenticated PDF responses

The browser intercepts response headers for the initial main-frame navigation,
including HTTP redirects. `application/pdf` and `application/x-pdf` select the PDF
pipeline even when a subsequent PDF parser will reject malformed bytes. Generic
binary responses and final URL paths ending in `.pdf` require a supported `%PDF-`
header within the first 1,024 bytes. Explicit HTML and `text/*` representations
remain rendered content; a textual PDF example is not a document download.
A content-disposition attachment does not prevent a recognized PDF from being read.

The existing Basic challenge policy, supplied cookies and request headers remain
inside the same private Chromium process. `Fetch.takeResponseBodyAsStream` and
sequential `IO.read` calls consume the paused response in 64 KiB chunks. There is
no second HTTP request by Rust or an anonymous download client. The original
navigation is cancelled after the bytes have been obtained, before a browser PDF
viewer can replace them with its interface HTML. The typed result carries owned
bytes and the real final URL; the coordinator applies native extraction, local
OCR, screenshots and model routing according to the existing PDF options.
The browser itself does not create a fake page screenshot or publish a download.

A response exceeding 100 MiB, a partial HTTP 206 response, a truncated declared
uncompressed body, an invalid stream or the navigation deadline causes an error.
Header declarations provide an early size check; streamed bytes independently
obey the same cumulative limit. The deadline is not restarted between chunks.
A generic response that is not PDF is supplied back to Chromium with its original
status and representation headers and the same decoded body. Wire compression and
length headers are adjusted for that body. This resumes the intercepted response
without repeating its request. Non-PDF attachments remain subject to Chromium's
existing disabled-download policy.

This stage handles the initial HTTP navigation and redirects, not later
JavaScript-triggered downloads, viewer interactions, arbitrary attachment types
or persisted browser sessions. Optional authored loopback tests cover authenticated
inline PDFs, extensionless binary attachments, redirects, origin isolation,
explicit text priority, advertised size rejection, truncated bodies and timeouts.
These tests require the coordinator's explicit installed-browser run; adding the
implementation or tests alone is not release acceptance evidence.

## Fetch and screenshot behavior

An explicit `playwright` request renders the page. An `auto` request without credentials or
screenshots retains the existing static/cache path and can fall back to the local
browser for recognized JavaScript/challenge or empty-HTML extraction failures.
It does not introduce remote-provider fallback. An `auto` screenshot request
renders directly, so a cached text-only result cannot masquerade as a capture.

Explicit `static`, `defuddle` and `jina` requests retain their selected text
representation and capture separately when screenshots are requested. Remote
consent and private/credentialed-URL policy checks still run before an explicit
remote request. Capture failure preserves successfully extracted text with a
warning; screenshot-only operation requires a successful capture. An empty canvas
page is accepted when screenshot-only is effective and tiles are available.
Pure LLM mode retains its existing DOM-text requirement instead of becoming
visual-only.

The browser applies cookies, additional HTTP headers, custom user agent,
`load`/`domcontentloaded`/`networkidle`, a visible CSS selector wait, extra wait,
and bounded automatic scrolling. Additional HTTP headers apply to page-context
requests, including subresources, matching the reference context scope; they are
not restricted to the initial navigation origin. Final source metadata redacts
sensitive query values while link resolution uses the actual final URL.
Exact domain profile overrides support wait
state, selector, extra wait, scroll suppression and resource rejection patterns.
Built-in GitHub and X/Twitter waiting hints are available; no third-party social
content enrichment is added. Selector timeout warns and uses available content,
as in the reference browser path. Network idle requires no tracked requests for
500 ms.

Screenshots are captured **before** shadow-root flattening. The configured
viewport and JPEG quality apply. Long pages are captured as full-width vertical
tiles named `host_path.full.jpg`, `host_path.full--1.jpg`, and so forth. Query
strings and hash routes affect an eight-character filename hash; ordinary
heading anchors do not. `tile_height=0` produces a single image, reducing its
height proportionally when it exceeds `max_height`. Payloads return to the core
publication layer; the browser module does not publish output files itself.
Screenshot and subsequent extraction have separate time budgets. Extraction
serializes the rendered DOM and flattens accessible open shadow roots after
capture; closed roots and browser-internal content remain opaque.

The core publishes JPEGs under `.markitai/screenshots` and returns every tile in
the result. Identical captures reuse existing files; changed captures receive a
versioned filename so older results keep their referenced bytes. Screenshot-only
CLI history retains all tiles as binary assets in its self-contained archive.
Screenshot-only conversion without an LLM publishes captures without an empty
Markdown file.
For this CLI mode, an omitted output directory defaults to the configured output
directory or current directory. The in-memory API requires an output directory
unless the LLM produces Markdown: if enhancement fails and neither text nor
published captures can be returned, conversion reports an error. With a directory,
the configured LLM fallback policy can retain captures and report a warning.

Non-pure enhancement sends all captured tiles together, subject to the configured
image-page and payload bounds. Pure LLM mode continues to use the text path.

## Explicit limits

- `session_mode=isolated` is implemented. `domain_persistent` returns an
  unsupported error before browser launch. Scoped Server Basic authentication is
  implemented as described above; browser contexts are not persisted.
- Proxy configuration comes from scheme-appropriate `HTTP_PROXY`, `HTTPS_PROXY`
  or `ALL_PROXY` environment variables and their lowercase forms. `NO_PROXY`
  rules and loopback bypasses are passed to Chromium. Only unauthenticated
  HTTP(S)/SOCKS5 proxies are supported. Operating-system proxy discovery,
  automatic/PAC configuration and proxy-port probing are not implemented.
- Resource rejection accepts literal URL characters, `*` within a path component
  and `**` across components. Brace expansion, bracket groups and backslash
  patterns fail explicitly. Filtering is implemented through the attached page's
  request events; it is not a public-network-only SSRF boundary. Existing service
  restrictions on remote unauthenticated URL conversion remain necessary.
- Top-level input and final documents must use HTTP(S). Embedded URL credentials
  are rejected. Chromium request interception rejects other fetch schemes except
  normal `about`, `data` and `blob` resources. Filesystem downloads remain disabled;
  supported main-response PDF bytes are intercepted in memory. This does not make
  browser execution an operating-system sandbox for untrusted sites.
- Navigation and each capture/extraction stage use a finite configured timeout
  from 1 to 120,000 ms; browser startup has a separate 15-second limit. Extra
  waits are at most 30 seconds, scrolling at most eight steps. Zero/unbounded
  timeout is rejected instead of hanging indefinitely.
- DOM extraction is bounded to 200,000 inspected elements and 100 MiB of UTF-8
  HTML. Protocol messages are bounded to 140 MiB. Screenshot dimensions are at
  most 8,192 pixels wide, 100,000 pixels high and 50 million pixels total; at
  most 128 tiles and 100 MiB of compressed payload are accepted. Exceeding a
  bound reports a browser/capture failure rather than silently truncating a page.
- Rendered results are not written to the persistent fetch cache in this stage.
  Existing static cache behavior remains intact; personalized browser state is
  never reused across conversions.
- Unix cleanup terminates the browser's dedicated process group and waits for
  its direct child. Other platforms use direct child termination; descendant
  cleanup and executable discovery need platform-specific validation. No
  cross-platform readiness or speedup follows from the implementation alone.

The protocol operations use Chromium's published
[Page](https://chromedevtools.github.io/devtools-protocol/tot/Page/),
[Network](https://chromedevtools.github.io/devtools-protocol/tot/Network/) and
[Fetch](https://chromedevtools.github.io/devtools-protocol/tot/Fetch/) and
[IO](https://chromedevtools.github.io/devtools-protocol/tot/IO/) domains.
Unit coverage targets option validation, cookie field conversion, glob boundaries,
proxy credential rejection and filename identity. The actual release additionally
passes delayed JavaScript, headers/cookies, redirects, tiling, canvas-only results,
failure policies, timeout and cleanup cases. Real proxy routes, every wait state,
shadow-root content, explicit static/remote text plus capture and non-Unix process
cleanup remain outside this local acceptance corpus.
