# Next implementation: complete vision batches and structured transport

Prepared 2026-09-29 from read-only inspection during the round-twenty-two gate.
This is an implementation contract, not validation evidence. No provider request,
reference execution or build was performed for this note. Reference anchors are
relative to `/Users/example-user/work/markitai/packages/markitai`; native paths are relative
to `/Users/example-user/work/markitai-rust`.

## Deliver two small, independently testable changes

First implement typed non-pure visual document processing and complete paged
batches using the existing JSON-text transport. Then add capability-selected
provider tools/schema requests beneath the same typed call abstraction. Neither
change needs a new public conversion option, response field or binding signature.
Keep pure text, pure image analysis, OCR routing, screenshot publication and
service connection probes on their existing paths.

Round twenty-two already provides `Enhancement.metadata`, `DocumentMetadata`,
validated description/tags, collision-safe source literals, typed cache rows,
complete text chunks and explicitly shared `DocumentScope` accounting. Reuse
these pieces. Do not rebuild the document pipeline or introduce another usage
aggregate. The ongoing gate, rather than this plan, determines their verified
status.

## Reference behavior to preserve

| Trigger | Required behavior | Source anchor |
|---|---|---|
| Non-pure document with no page images | Existing structured text processor | `src/markitai/llm/document.py:1283` |
| 1–10 document pages | One combined structured response: body plus description/tags | `document.py:1306`, `:1474–1637` |
| More than 10 document pages | First 10 pages produce body and metadata; remaining consecutive batches clean body only, then merge in order | `document.py:1328–1468`; `constants.py:52` |
| Failed combined first call | Do not pay for a cleaner-only retry of that same batch | `document.py:1320–1332`, `:1370–1393`; `tests/unit/test_llm_degraded_paths.py:419–459` |
| First batch has fatal credentials/provider failure | Do not dispatch remaining batches | `document.py:1390–1399`; `test_llm_degraded_paths.py:460–477` |
| Later batch fails | Settle active work, fail the whole enhancement; do not label mixed original/enhanced pages successful | `document.py:1401–1468`; `test_llm_degraded_paths.py:479–542` |
| Ordinary URL screenshot enhancement fails | Fall back to the standard structured text stage, retaining usage | `src/markitai/workflow/url.py:478–525` |
| Pure text / image | Existing distinct cleaner / image-analysis behavior; local file screenshot-only is an existing exception selecting visual extraction | `src/markitai/workflow/core.py:1241–1295` |

The reference page-cap policy differs from the current native policy: above
`max_vision_pages_per_document`, reference `document.py:1290–1304` falls back to
text. Native PDF/Office/image preflight currently refuses before rendering or
uploading; that verified limit must not disappear accidentally in a batching
patch. Preserve and document it in the first change. A separate coordinated
change can implement reference-style text fallback only when reliable text is
available, including renderer preflight and scanned/empty inputs. Do not return
an empty successful document merely to imitate that fallback.

Reference page splitting first recognizes slide markers, then page markers,
then apportions paragraphs (`llm/content.py:572–651`). It can discard a preamble
before the first marker, discard text after an excess marker, or duplicate a
short body for every page. Those are observable implementation defects, not
requirements to copy. Native splitting must retain every source byte in exactly
one owned text partition; image ordering is a separate invariant.

## Minimal shared interface and coordinator glue

Add `llm/vision.rs` and return the existing `Enhancement`, including typed
metadata and full-cache status. Suggested internal entry:

```rust
pub(crate) enum VisionKind { PagedDocument, WebCapture }
pub(crate) struct VisionFrame<'a> {
    pub number: usize,             // stable one-based input order
    pub mime: &'a str,
    pub bytes: &'a [u8],
}
pub(crate) fn process_vision_with_runtime(
    markdown: &str,
    source_label: &str,
    cache_context: &str,
    kind: VisionKind,
    frames: &[VisionFrame<'_>],
    cfg: &Value,
    runtime: Option<&LlmRuntime>,
) -> Result<Enhancement>;
```

The coordinator changes only the non-pure visual branch in `lib.rs`. Source URL
identity does not determine content kind: a fetched PDF uses `PagedDocument`,
while browser screenshot tiles use `WebCapture`. Preserve current pure branch
precedence, including URL pure+only and the documented local screenshot-only
exception. Existing `enhance_images_with_source_and_runtime` remains available
for these unchanged pure cases and callers until deliberately migrated.

Root continues to own source title, final metadata, base preservation,
`on_failure`, profiles, actual screenshot filenames/comments, output claims,
atomic files and `images.json`. `process_vision` never writes output files and
never reparses typed body YAML as generated metadata. Successful typed metadata
updates only enhanced description/tags. At final accounting, any paid request
means the whole item is not a full cache hit, even if a main stage was cached.

For paged documents, extract genuine page/slide boundaries before opaque marker
protection; exclude code literals and screenshot appendix comments from boundary
recognition. Require coherent ordered boundaries before treating them as precise
page alignment. Preserve a preamble in the first partition and trailing content
in the last. For unaligned native DOCX/body text, apportion complete Markdown
blocks across image batches once, retaining all text, and report the absence of
exact page alignment; never repeat the whole source once per image. Empty text
partitions remain valid when their visual frames carry the content. If later
format work supplies typed per-page text, this entry can accept those partitions
without changing public APIs.

Browser tiles represent one web page, not independent document pages. Preserve
current ordered all-tile coverage in a single bounded web-capture call; do not
apply document page-to-text matching to them. This matches the reference's
single-screenshot URL workflow while retaining the native multi-tile extension.
Splitting very long browser tile sequences into page batches is a separate
alignment problem, not something to disguise as PDF batching.

## Batches, budgets, failure and cache

1. Validate nonempty supported frames, stable order, existing page/payload bounds
   and text partition coverage before allocating base64 or sending requests.
   Borrow encoded frame bytes; encode only active batches. Bound worker count by
   the shared runtime and do not clone a document's full image set per worker.
2. A paged batch has at most 10 frames. The first call receives the typed schema
   and `{metadata_section}`. Later calls use the same vision prompts with an
   empty metadata section and return Markdown. Preserve page/image/link/literal
   markers in both forms. For visual transcription use the existing
   deletion/refusal guard without requiring textual overlap with an empty or
   unreliable extracted source; overlap-only tests would reject useful OCR.
3. Run first-batch metadata before dispatching later uncached work. A fatal
   provider/authentication/model failure stops admission. A nonfatal first-batch
   failure may still allow later batches, as reference does; retain successful
   cache entries but return the first meaningful error after work settles. Never
   rerun the failed first batch as a cleaner merely to discard the paid answer.
4. Later batches may run concurrently. Carry the same `Arc<Mutex>` accounting
   into every worker. Add an internal fatal-stop admission signal checked after
   acquiring a permit and before sending HTTP: queued requests must not start
   after a fatal sibling failure. Already active requests drain normally. Do not
   hold a document mutex over HTTP or backoff.
5. Check minimum uncached batch requests against the remaining document budget
   before sending the first batch. All protocol rungs, transport retries,
   validation retries, URL text fallback and subsequent image analysis consume
   that same native document budget. Native intentionally retains its stronger
   whole-document bound; reference separately budgets some `:images` work.
6. Successful batches cache independently in a new `native-vision-v1` namespace.
   Include complete protected text, processing kind, typed-first/plain-later
   category, prompt/schema/protection versions, ordered page identities and
   **SHA-256 of every frame's MIME and bytes**, plus enabled vision-model pool.
   Do not key only by filenames: reference keys use text and page names
   (`document.py:164–187`, `:1119–1137`, `:1488–1505`) and can miss changed pixels.
   Keep typed first-batch values and validated plain subsequent values explicitly
   tagged, without reinterpreting old text/document rows.
7. Cache only complete schema-valid, marker-valid, nontruncated results. Repetition
   degeneration must fail or be explicitly flagged and excluded from cache;
   silently caching a truncated loop is unsafe. A failed sibling never makes a
   partial enhanced document publishable. Retrying reuses only validated hits;
   final title/source/time come from the current conversion.
8. URL visual-to-text fallback uses the existing typed text processor and same
   scope. Log one sanitized warning and retain paid vision usage. Preserve
   screenshot-only memory failure rules: no reliable source body means a failed
   visual request cannot fall back to an empty successful text document. Source
   `NoModelConfigured`/unsupported/budget errors remain typed; do not inspect
   human error strings to route fallbacks.

## Exact structured transport staircase

Reference selectors are `llm/structured.py:75–168`; execution is
`llm/engine.py:435–511`. No new mode-selection CLI option is necessary.

| Resolved capability of every routable member | Initial mode and subsequent modes |
|---|---|
| Function/tool calling | `TOOLS -> JSON_SCHEMA -> MD_JSON` |
| Response JSON schema, but at least one lacks tools | `JSON_SCHEMA -> MD_JSON` |
| Unknown capability, text-only declaration, or empty capability pool | `MD_JSON` only |

Only enabled, actually routable deployment members participate; disabled entries
must not lower capability. Calculate over all candidates that the call can reach,
including configured fallbacks, after excluding nonvision deployments for a
visual call. Capability inspection itself sends no HTTP request and spends no
budget. One known-capable deployment must not cause an unknown sibling to receive
an unsupported forced-tool request.

The reference uses a local provider declaration first, otherwise LiteLLM's static
function-calling/schema metadata. Rust has no equivalent catalog today. Implement
an explicit, versioned metadata resolver for supported native providers/models,
with provenance for the fixed data; unknown/custom model IDs remain `MD_JSON`.
Do not infer support solely from an `openai/` or `anthropic/` prefix or run a probe.
A small verified table is a safe first set, but must be documented as a partial
catalog rather than equivalent to all LiteLLM metadata. Public provider discovery
and `service_probe` must not silently change request shapes due to this resolver.

Transport implementation should add a private `StructuredMode` plus typed
response envelope, rather than stuffing protocol error signals into Markdown:

- Tools: Chat/Azure send a single named function schema and forced tool choice;
  Anthropic uses native tools/tool choice. Parse the designated tool's arguments
  or input object. A missing/wrong tool, multiple conflicting invocations or
  malformed arguments fails that rung. Never execute a returned function.
- JSON schema: supported Chat-compatible providers receive an actual
  `response_format: {type: "json_schema", json_schema: ...}`; validate its text
  result locally as well. Do not send this OpenAI field to the native Anthropic
  endpoint. A mode a provider protocol cannot represent is skipped locally,
  without charging a fake request.
- MD_JSON: preserve round-twenty-two schema instruction and plain/fenced JSON
  parsing. "Plain" here means JSON in normal response text, **not arbitrary
  Markdown accepted without metadata**.

Upper rungs get one schema-validation attempt each, while existing transport
retry/fallback policy still applies. On a failed upper rung, descend once to the
next representable mode. Final `MD_JSON` keeps up to two validation retries and
feeds a bounded validation diagnostic back to the model; neither prompts nor
public errors should expose provider credentials or dump untrusted responses.
After its final failed response, optional local JSON repair costs no new request
and must pass the exact same typed/marker validation. Restrict first repair to
lexical defects with deterministic interpretation; do not invent missing body,
description, tags or structural markers. Reference uses a broader repair utility
(`engine.py:299–345`); a conservative subset is an explicit compatibility limit.

Fatal provider errors and request-budget exhaustion skip remaining rungs.
Authentication/permission errors must also stop queued document batches. Retain
internal classification from HTTP/routing rather than turning every error into
`Conversion(String)` and later matching words. Schema rejection and unsupported
request fields are descending candidates; billing/quota/authentication failures
are not a reason to purchase another response. Each parsed paid response is
recorded once, including invalid tools/schema answers and the final repaired
answer's original usage. Repair itself adds zero. Token-limit truncation cannot
enter cache or be repaired into a claimed complete answer.

## Minimum necessary acceptance, using real loopback HTTP

Reuse isolated subprocess state and small generated PNG frames; one small PDF
or TIFF composition case proves root routing without requiring a full format
corpus. Keep pure/vision mocks distinct and inspect actual request bodies.

1. **One structured visual call:** two differently colored frames, original YAML
   and page markers, injected canonical fields, valid description/tags; assert
   both image signatures/order, body preservation, metadata ownership, base
   separation and one request. Also cover screenshot-only empty text producing
   useful typed body.
2. **21 complete pages:** first batch 10 frames, then 10 and 1; deliberately finish
   later batches out of order. Assert every frame/body marker appears once in
   final order, original preamble/tail survive, first metadata wins, and requests
   and tokens sum exactly across all three calls.
3. **One failed later batch then retry:** no partial `.llm.md`, valid base/assets
   remain; retry requests only the failed batch. Change pixels but retain names
   and text, and assert exactly the affected batch misses its image-aware cache.
4. **Fatal and budget admission:** first auth failure causes no later calls;
   a held later worker plus fatal sibling prevents queued dispatch. Separately,
   known minimum work over budget sends nothing. Billed invalid structured
   responses/fallback still count once within the shared cap.
5. **URL visual fallback and pure controls:** bad visual schema followed by valid
   typed text, usage includes both; all-empty screenshot-only cannot fake text
   success. Existing pure text remains one raw nonchunked request, pure image
   retains analysis output and URL pure+only does not unexpectedly send images.
6. **Protocol table-driven HTTP cases:** tool arguments success; tool rejection
   then schema success; both fail then JSON text success; malformed JSON repair
   after final retries; mixed/unknown pool begins at MD_JSON; disabled member
   does not lower capability; fatal/budget error does not descend. Inspect
   Chat/Azure and Anthropic payloads and ensure the connection probe stays plain.

Pure helper unit tests cover lossless page partitioning, code-literal fake page
markers, schema extraction/repair boundaries and capability-pool selection. Avoid
wall-time assertions or testing only mode enums without actual HTTP bodies.

## File ownership for the next short batch

- Vision worker: new `core/src/llm/vision.rs`, scoped `llm/document.rs` validation
  reuse, vision-focused conversion tests and `docs/llm.md`/`docs/cache.md` updates.
- Transport worker: `core/src/llm.rs`, new `llm/structured.rs` and a narrow pinned
  capability-data module; preserve `llm/service_probe.rs` behavior. Coordinate
  `Prompts`/response-envelope changes before either worker edits shared types.
- Coordinator: `lib.rs` content-kind routing, unchanged output/publication
  policy, required exports/manifests, format-aware fixtures and one integrated
  gate. Renderer/cap policy remains unchanged unless explicitly included.

Freeze and verify vision JSON-text processing first; add the transport modes
under its unchanged result type. A single final local HTTP acceptance can exercise
both. No live-provider result, pricing accuracy, OS portability or model-quality
claim follows from these loopback tests.
