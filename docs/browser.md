# Native browser fetching

The `playwright` fetch strategy retains its public name and uses Chromium's
DevTools Protocol directly from Rust. It does not import Playwright or run
Python, Node.js, or a browser automation CLI. Chrome/Chromium remains an optional
installed executable; the standalone Markitai binary does not bundle it.
The integrated workspace gate passes. The frozen diagnostic executable also
passes real loopback Chrome fetching, capture, timeout and cleanup checks;
release-artifact evidence is recorded separately in the control center.

Each fetch launches a new headless process with a temporary user-data directory,
a separate disk-cache directory, a loopback debugging endpoint on a dynamically
selected port, and no reused browser profile. Extensions, synchronization,
background update services and password/keychain integration are disabled.
The browser inherits only a small platform environment allowlist; provider keys
are not forwarded. The implementation keeps the browser sandbox enabled.

`MARKITAI_BROWSER_EXECUTABLE` selects an exact executable. An invalid override
fails discovery instead of falling back to another installation. Otherwise the
reader checks executable names in `PATH`, common macOS/Windows application
locations, then known Playwright Chromium cache layouts. Cache discovery honors
`PLAYWRIGHT_BROWSERS_PATH`; only executable paths are inspected. It never loads
stored browser state, installs a browser or opens the user's normal browser.
Availability checks do not launch a process.

## Fetch and screenshot behavior

An explicit `playwright` request renders the page. An `auto` request without
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

- `session_mode=isolated` is implemented. `domain_persistent` and
  `http_credentials` return an unsupported error before browser launch.
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
  normal `about`, `data` and `blob` resources. Downloads are disabled. This does
  not make browser execution an operating-system sandbox for untrusted sites.
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
[Fetch](https://chromedevtools.github.io/devtools-protocol/tot/Fetch/) domains.
Unit coverage targets option validation, cookie field conversion, glob boundaries,
proxy credential rejection and filename identity. Real loopback browser evidence
must additionally verify delayed JavaScript, headers/cookies, redirects, capture
bytes and tiling, canvas-only results, timeouts and child cleanup against the
actual built binary.
