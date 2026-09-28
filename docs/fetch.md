# URL fetching and extracted-page caching

The native `auto` and `static` strategies fetch HTTP(S) directly. They extract
HTML, plain text and Markdown, or dispatch supported downloaded documents to
the local format readers. Requests negotiate
`Accept: text/markdown, text/html;q=0.9, */*;q=0.5`, follow at most ten redirects
and bound response bodies to 100 MiB. HTML links resolve against the final
response URL. Both initial and conditional responses use the same charset and
format decoder. Text decoding follows a BOM or HTTP charset, otherwise UTF-8;
malformed sequences use replacement characters. HTML meta-charset prescanning
and heuristic encoding detection are not implemented. Explicit plain-text and
Markdown MIME types keep literal HTML examples as text rather than triggering
HTML challenge detection.

`auto` starts with the static path when screenshots are not requested, with
local browser fallback for recognized JavaScript/challenge or empty-HTML quality
failures. Screenshot requests render directly. The `playwright` strategy uses
[native Chromium CDP](browser.md) for JavaScript and captures; the complete
reference fallback policy and SPA-domain learning remain unfinished. `jina` and
`defuddle` keep explicit remote-consent and target checks and do not use this
cache. Unsupported strategies fail before cache lookup, so an old page cannot
make an unsupported strategy appear to work.

## Stored results and identity

The cache stores extracted Markdown and page metadata before output names,
profiles, frontmatter generation and optional LLM processing. A cache hit goes
through those later stages normally. It never represents an LLM cache hit or
removes the cost of a subsequent model request. The internal fetch-hit flag
feeds the CLI's existing `fetch_cache_hit` field without adding a field to the
bindings conversion JSON.

Only successful HTML and `text/*` responses other than XML are eligible.
Admission also requires nonblank Markdown, no owned asset bytes and no
extraction warnings. The warning restriction avoids replaying content without
its original warning because warnings are not stored in the reference table
shape. Downloaded PDF, Office, email and other local-reader documents bypass
this page cache, including when they happen to contain no images. Their assets
remain available to normal output processing. A successful refresh that changes
into an ineligible representation removes its old page-cache row, preventing a
later 304 or still-fresh TTL from replaying the previous representation. Failed
or rejected refreshes retain the old row instead.

The key hashes `native-fetch-v1`, a NUL separator and the exact original URL.
Query strings and fragments therefore remain part of identity. An explicitly
selected non-`auto` strategy adds a NUL-separated strategy suffix; a strategy
inherited from configuration does not. Default/configured `auto` and `static`
share a key, explicit `-s static` has its own key, and explicit `-s auto` stays
unscoped. Redirect URLs do not replace the original identity. The native
namespace prevents reuse of reference-Python extracted results.

## Reuse and revalidation

| Saved state | Behavior on the next invocation |
| --- | --- |
| No HTTP validator, younger than TTL | Return the saved page without an HTTP request. |
| No validator, expired or zero TTL | Fetch normally and save an admissible result. |
| ETag or Last-Modified | Send every available conditional header on every invocation, regardless of TTL. |
| Conditional 304 | Return the saved page as a fetch hit; update access time only. |
| Conditional usable 200 | Decode and replace the page; this is a fetch miss. |
| Conditional failure or unusable response | Retry without conditional headers; do not silently serve the old page. |

The TTL defaults to 86,400 seconds and measures age from creation, with expiry
at `age >= TTL`. Reads update access time, not creation time; a 304 does not
replace saved validators or restart the TTL. An unsolicited 304 is an error.
Expired rows remain visible in cache statistics until replaced or evicted.

This application cache does not apply HTTP `Cache-Control`, `Expires` or `Vary`
directives, including `no-store`. Set `cache.enabled=false` when fetched content
must not be persisted; `--no-cache` still saves admissible fresh results.

Empty content, failed extraction and recognized HTML challenges cannot replace
a good row. Challenge checks examine HTML structure and visible non-code text;
literal vendor names in plain text, HTML code examples or an ordinary CAPTCHA
widget are insufficient. Obvious short JavaScript shells fail static extraction;
`auto` may then select the local browser.
These checks do not implement complete quality classification. If a conditional
request and its unconditional retry both fail, the operation fails while the
old row remains intact. The reference's narrower stale-page fallback after a
failed full strategy chain is not implemented.

## Controls, capacity and privacy

| Setting | Effect |
| --- | --- |
| `cache.enabled=false` | No conversion-time fetch-cache reads, writes or creation. |
| `cache.no_cache=true` | Bypass reads and validators; still refresh from admissible fresh content. |
| `cache.no_cache_patterns` | Matching URLs use the same bypass-and-refresh behavior. |
| `cache.fetch_ttl_seconds` | Reuse window for entries without validators. |
| `cache.global_dir` | Parent of `fetch_cache.db`; default state paths honor `MARKITAI_HOME`. |
| `cache.max_size_bytes` | Logical Markdown capacity of this database, default 512 MiB. |

URL patterns match the full URL, scheme-free URL, authority/hostname and final
path segment. Scheme and hostname comparisons ignore case; path and filename
case is retained. Patterns are trimmed and support an optional leading `**/`.
URL colons are not treated as document-stage suffixes.

The SQLite store uses WAL, normal synchronization and a 30-second busy timeout.
Replacement and LRU eviction share an immediate transaction. Logical capacity
counts UTF-8 Markdown bytes, excluding metadata, SQLite files and WAL overhead.
Fetch and LLM stores each have their own capacity; this is not a combined disk
quota. The store bounds row allocation and lazily creates the database only
when saving an admissible page. Cache errors add a fixed warning to successful
content instead of exposing paths, SQL, headers, URLs or page data.
Conversion reads require writable access to update `accessed_at`. If opening the
store or updating that timestamp fails, even an otherwise reusable page becomes
a cache miss and fetching continues over the network.

The database is plaintext. It stores original/final URLs and metadata as well
as Markdown; URL credentials and query tokens can therefore be stored. A
hashed key does not make its contents secret-free. Direct local fetching keeps
its existing trusted-session behavior; this cache is not suitable for reuse
across anonymous service authority boundaries without additional policy.

## Verification boundary

The loopback tests in `fetch.rs` check actual request counts and headers, TTL and
zero TTL, 304 and replacement behavior, redirects and character encoding,
bypass refresh, strategy provenance, disabled and broken storage, malformed
status/empty content, challenge preservation, legitimate CAPTCHA mentions and
asset-bearing downloaded documents. They use temporary configured state paths;
the older direct-fetch test explicitly disables caching. Storage and CLI/API
integration tests exercise their respective boundaries separately. No tests
need provider credentials or the user's real state directory.

[Decision 0003](decisions/0003-persistent-fetch-cache.md) records the detailed
reference comparison, storage contract and deferred browser, remote-cache,
owned-asset, stale-fallback and request-coalescing work.
