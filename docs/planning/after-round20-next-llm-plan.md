# Next LLM implementation contract: document metadata and long text

Prepared 2026-09-29 by read-only source inspection during round twenty. This is
an implementation proposal, not execution evidence. Neither reference execution,
Cargo, Git, model calls nor native conversions were used for this note.
Reference paths below are relative to `/Users/example-user/work/markitai`; native paths are
relative to `/Users/example-user/work/markitai-rust`.

## Recommendation

Implement one typed document-processing path for non-pure text documents and
URLs: validated Markdown + generated description/tags, structural protection,
32,000-character chunking, independently cached chunks, complete ordered merge,
and existing failure/publication policy. Reuse existing native transport,
provider routing, budgets, usage and publication. Do not rebuild image analysis,
OCR, service schedulers or the cache command surface.

The smallest coherent first delivery covers non-pure text and all inputs routed
to that path, plus a typed metadata result usable by existing vision callers.
Vision batches of ten images, provider tool/schema capability tiers and the
reference vision-to-text fallback need explicit separate decisions below;
text chunking alone cannot honestly close the entire LLM parity theme.

## Current native behavior already implemented

- `src/llm.rs` supports provider HTTP protocols, deployment routing and retries,
  typed fatal/unsupported errors, timeout/response limits, truncation rejection,
  shared `LlmRuntime`, request and cost budgets, and paid response usage including
  error responses. Existing tests already exercise these; preserve them.
- `DocumentScope` establishes authoritative per-conversion usage and request
  accounting, also used by image analysis. It restores the outer scope on drop.
  **It is currently thread-local `Rc<RefCell<...>>`**: spawning chunk threads
  without explicitly carrying a shared accounting context bypasses that budget.
- Local non-pure text answers are persisted in SQLite by `src/llm_cache.rs`.
  Keys include complete content digest, pre-substitution prompt digest and sorted
  enabled model identities; renaming/key rotation preserves hits. Bypass globs,
  size limits, management commands, error warnings and zero-cost hits exist.
  Its value is currently a JSON-serialized Markdown string, not metadata.
- `lib.rs` routes local non-pure text through that cache; URL and pure text bypass
  persistent cache; page/image vision also has no persistent document cache.
- Current document prompts ask for Markdown, not the reference typed result;
  `{metadata_section}` is empty. Custom prompt files and source/mode substitution
  exist. Image analysis already has its own typed caption/description/text flow.
- `lib.rs` opportunistically splits any model YAML and extends enhanced
  frontmatter; it currently allows model title/extra fields except source and
  timestamp. This is not the reference structured metadata contract.
- `output.rs` owns separate base metadata, final profile application, atomic
  publication and pure literal behavior. Do not let enhanced metadata mutate the
  base frontmatter or route schema JSON directly into `.llm.md`.

Source anchors: `src/llm.rs` DocumentScope / enhance_cached / prompts / request;
`src/llm_cache.rs` key / Cache::get / Cache::set; `src/lib.rs` main enhancement
branch; `docs/llm.md` current explicit gaps.

## Reference trigger and result contract

There is **no metadata-enable flag or configurable text chunk threshold**.
`llm.enabled=true` with `llm.pure=false` selects document processing for normal
text. Both local workflow and standard URL workflow use `process_document`.
The existing `llm.on_failure`, `keep_base`, concurrency, request/cost caps and
cache controls continue to govern it.

The model result is exactly this semantic shape:

```json
{
  "cleaned_markdown": "# Existing title\n\nFaithful cleaned body.",
  "frontmatter": {
    "description": "The core conclusion, in the source language.",
    "tags": ["software-engineering", "Rust"]
  }
}
```

`title`, `source`, `markitai_processed` and fetch strategy are **program-owned**.
Reference `Frontmatter` validates a nonblank string description and a nonempty
list of nonblank strings; null/wrong shapes fail. Prompt guidance of 100
characters and 3–5 tags is not a strict cardinality constraint in that model.
The final frontmatter builder collapses description whitespace and clips to
150 characters (`147 + ...`), folds tag whitespace to hyphens, removes quote
characters, replaces `:` with `-`, clips each tag to 30 characters, and omits
empty normalized tags. It preserves resolved original title, caps title at 200
characters, creates a fresh processing timestamp and excludes conflicting
canonical fields/language from external metadata.

No model-generated title should replace an existing source title. Preserve the
public `ConversionOutput` schema: `llm_markdown` is body-only, `frontmatter` is the
enhanced mapping, saved `.llm.md` has canonical order, base `.md` retains its own
metadata. `rag`, `obsidian` and `okf` run after the final successful merge as now.

Reference evidence:
- `packages/markitai/src/markitai/config.py:325–364` (`LLMConfig`).
- `.../llm/types.py:143–204` (`Frontmatter`, `DocumentProcessResult`).
- `.../llm/document.py:1734–2020` (plan, finalize and typed metadata).
- `.../utils/frontmatter.py:370–449` (normalization and field ownership).
- `.../workflow/url.py:450–474` (standard URL processing).

## Pure, image and vision distinctions that must survive

- Text `pure` calls `clean_document_pure`: raw Markdown, no placeholders,
  no metadata generation, no text chunking/truncation, no persistent cache,
  model text returned as-is subject to existing failure validation. Existing
  YAML remains governed by pure behavior, not the new metadata builder.
- Standalone image pure uses image analysis, not document metadata. Local
  non-image pure skips image enrichment as already implemented. URL pure may
  still enrich downloaded image references through its established path.
- Normal PDF/PPTX page vision uses `enhance_document_complete`: <=10 images gets
  one combined structured call; >10 images gets a structured first batch and
  text-only cleaning responses for later batches, merged in page order. First
  batch metadata is retained. Reference page cap overrun falls back to text
  processing; current native cap may intentionally reject before rendering.
  Do not quietly change the verified PDF/screenshot contract during text work.
- URL screenshot vision has its own structured metadata prompt and image-position
  protection; failed URL vision explicitly falls back to standard processing.
  Current Rust does not implement that entire reference fallback/cache chain.
- `{metadata_section}` is meaningful on combined document vision; leaving it
  empty after claiming vision metadata parity would remain a gap.

Reference evidence: `.../llm/document.py:2022–2070`, `:930–1084`,
`:1230–1450`; `.../workflow/url.py:478–526`; `.../workflow/core.py:781–870`.

## Chunking, content fidelity and admission

1. Reference threshold is **32,000 Unicode characters**, after image and
   protected-content substitution (`constants.py:39`, `document.py:1880`).
   Do not accidentally count UTF-8 bytes or split inside a scalar value.
2. Pack whole Markdown blocks; blank lines inside fenced code remain in their
   block. If one block exceeds the limit, split on lines, then long lines on
   character boundaries as last resort. No tail may be truncated. Rust should
   preserve literal fence content and collision-safe placeholders rather than
   reproduce known weaker regex parsing. Any intentional oversized-fence policy
   must be documented and tested, not silently drop or rewrite literal text.
3. Protect image positions and page/slide structural markers before requests;
   missing required markers is a failed enhancement. Keep code/math/links and
   user identifiers distinct from generated markers. A prefix collision must
   not make user text restorable as an internal token.
4. Before any network call, count uncached chunks. Required admission is
   `uncached + ceil(uncached * 0.2)` versus remaining request budget; zero disables
   the cap. If all calls fit without cache reads, expensive inspection can be
   avoided. Refuse before paying for a doomed partial document.
5. Each chunk validates independently. Reference permits a boilerplate-only
   chunk to clean to empty; do not apply whole-document nonempty rejection to a
   valid typed result with empty `cleaned_markdown` inside a multi-chunk plan.
   Nonblank JSON response and nonblank cleaned body are different checks.
6. Merge results in input order regardless of completion order, with blank-line
   separators; first chunk supplies description/tags. Whole-document validation
   still rejects a refusal, severe deletion or unrelated replacement.
7. Any failed chunk fails the whole enhancement, even if other chunks succeeded.
   Settle/drain already admitted calls, preserve successful chunk cache entries
   for retry, and publish base/fallback per existing `on_failure`. Never save a
   partial `.llm.md` while reporting success.
8. Social `content_profile=social_post` is metadata-only: preserve body verbatim
   even if the structured model rewrites it. Parent extra metadata must reach
   the document plan; reading source filename alone cannot detect this case.

Reference cleanup checks use source length >=200 non-whitespace chars, minimum
20% output length and 30% distinct character-bigram recall. Per-chunk validation
relaxes that when at least half the answer 4-grams come from its chunk, including
empty boilerplate answers; final whole-document check still applies. These are
observable reference heuristics, not proof of linguistic fidelity.

Reference evidence: `.../llm/content.py:680–855`,
`.../llm/document.py:1795–1835`, `:1859–1906`, `:2125–2229`;
`packages/markitai/tests/unit/test_llm_content.py:386–436` and
`test_llm_degraded_paths.py:138–260`.

## Cache, protocol and accounting integration

- Cache validated typed body + description + tags, not final YAML timestamp or
  source label. A new schema/version namespace must prevent old native string
  rows from being misinterpreted as typed metadata. Reuse the SQLite file/table,
  capacity, diagnostics and command tools without deleting old rows.
- Scope per chunk by content, effective prompt including schema/mode instructions,
  enabled model pool and processing kind. Changing chunk size/protection version
  needs key versioning. The first successful chunks survive a later failure;
  retry only misses. Rebuild timestamp/title/source on every hit.
- Reference standard URLs use the same document cache; extending the present
  native local-only policy is needed to claim full standard-URL parity. URL
  vision has a separate image-aware key; do not accidentally cache it by text
  alone or claim that extension from text-only evidence.
- An item is a full LLM cache hit only when at least one hit, zero misses and
  zero paid requests across that item. A cached main response followed by new
  image analysis is not a zero-cost hit. Cover this composition explicitly.
- Preserve one authoritative aggregate. Every paid valid/invalid structured
  answer, retry and transport response with usage contributes exactly once.
  Cache hits contribute zero requests/tokens. Every actual HTTP attempt still
  spends shared request admission; denied attempts spend none.
- Reference image-analysis stages may use a separate `:images` request context;
  current native round19 intentionally shares the whole-document budget. Keep
  the native documented stronger bound unless root deliberately changes it.
- Parallel chunk work must share `DocumentScope` accounting and cache tally;
  existing thread-local Rc cannot cross scoped worker threads. A private
  Arc<Mutex<Accounting>> handle with explicit propagation + RAII restoration is
  a small safe foundation. `LlmRuntime` remains the global HTTP-attempt capacity;
  do not create one fresh independent semaphore per chunk. Sequential execution
  is a possible first correctness stage only if its concurrency gap is stated.
- Reference protocol ladder is pool-capability driven: TOOLS → JSON_SCHEMA →
  MD_JSON, starting at the weakest routable model's known capability. Unknown
  models use MD_JSON, no probing. Upper rungs have one schema attempt; final
  MD_JSON has bounded validation retries and conservative local JSON repair.
  Fatal provider/budget failures must not trigger the next expensive tier.
  Existing Rust request() only reads message/content text, so tool-call arguments
  need actual response parsing before claiming TOOLS support.
- Smallest provider-neutral first implementation may validate plain/fenced JSON
  across native Chat/Anthropic protocols; if schema/tool tiers are deferred,
  label them as deferred. Do not advertise the full staircase from MD_JSON-only
  tests or silently treat arbitrary Markdown as valid structured metadata.

Reference evidence: `.../llm/engine.py:113–150`, `:163–273`, `:706–853`,
`:435–511`; `.../llm/structured.py`; `.../llm/document.py:190–218`.

## Proposed disjoint file ownership

| Lane | Files | Responsibility |
| --- | --- | --- |
| Document planner | NEW `src/llm/document.rs`, NEW `src/llm/chunks.rs`, scoped `docs/llm-document.md` | Typed fields/validation, protection, chunk plan/merge, admission arithmetic, model metadata normalization; no output writes. |
| Transport/accounting | existing `src/llm.rs`, optional NEW `src/llm/structured.rs` | Shared accounting handle, runtime/call execution, provider response parsing, JSON/schema modes and paid usage; exports exact typed call API to planner. |
| Cache | existing `src/llm_cache.rs`, cache tests | Versioned typed values/per-chunk hits, preserving old rows and CLI management; coordinate value API before planner edits. |
| Coordinator | `lib.rs`, `output.rs`, module wiring, manifests, final docs | Carry source title/fetch metadata into plan, apply enhanced metadata only, preserve pure/failure/output/profile semantics. |
| Independent acceptance | NEW `tests/conversion/document_llm.rs`, NEW CLI process tests / small `.local` release driver | Loopback public outputs/cache/failure/concurrency proof; no provider credentials. |

No files have been assigned or modified by this proposal. Avoid two lanes editing
`llm.rs`; agree transport API first. Suggested typed result contains body,
normalized description/tags, usage/cache tally/warnings; final YAML belongs to
output integration. Plan construction should be deterministic and callable
without network, so admission and chunk-cache tests do not need fake public flags.

## Minimal meaningful acceptance set

1. Short local Markdown + local structured mock: exact body-only API fields;
   saved enhanced title/source retained, normalized description/tags in canonical
   order, base frontmatter unchanged, usage exactly one response. Return a bogus
   model title/source as extra keys to ensure they cannot override program data.
2. Standard loopback URL with HTML title and extra metadata: same typed contract,
   real fetch strategy retained; pure counterpart gets unchanged raw text and
   no generated metadata, with one non-chunked request even above32k.
3. 32,000 versus32,001 Unicode characters, and >64k CJK/emoji document with unique
   start/middle/tail sentinels, fenced blank lines/trailing spaces, table, page
   markers, image references and placeholder-looking user identifiers. Assert
   complete ordered preservation and per-request bounded character count.
4. Force responses to complete in reverse order: merged order remains source
   order; metadata comes from first input chunk, not fastest response; observed
   loopback concurrency <=shared limit across two simultaneous documents.
5. Chunk admission: six uncached chunks with insufficient budget produce zero
   HTTP calls; repeat with four valid cached chunks and enough remaining budget
   sends exactly the two misses. Include 20% reserve and zero-unlimited cases.
6. Fail a middle chunk after another succeeds: `fallback` retains full base;
   `fail` reports failure but preserves previous base/output files. No partial
   enhanced file or poisoned cache. Rerun pays only failed/missing chunks; exact
   paid usage includes invalid/truncated/error responses already received.
7. Typed invalid answers: null/empty description, invalid/empty tags, wrong field
   types, truncated finish, missing image/page token; no cache admission. Bounded
   repair/fallback never invents metadata or silently labels original body as a
   successfully enhanced result. Unknown model must not make a probe request.
8. Legitimate boilerplate-only chunk can return empty cleaned body with valid
   metadata while merged real content succeeds; refusal chunk and whole-document
   severe deletion fail. Social metadata-only input retains exact original body.
9. Fresh-process cache hit after rename and removed credentials: zero calls/usage,
   fresh timestamp and current source title, typed metadata recovered. Prompt/model
   changes miss. Existing old native cache string row remains readable by tools
   but cannot satisfy the new structured namespace.
10. Composition: cached main document + new alt/desc call shares remaining budget
    and correct global usage/hit flag; chunking does not analyze one duplicated
    asset repeatedly. Vision metadata/batch support, if included, needs an
    eleven-page local fixture proving first-batch metadata and final-page content.

All public tests use private cwd/config/MARKITAI_HOME and loopback models, inherit
HOME rather than repurpose it, and record actual request bodies and output bytes.
Use deterministic body extraction/JSON mapping assertions instead of asking a real
model for text. Separate exact reference parity from deliberate native fidelity
improvements and unresolved protocol/vision gaps.
