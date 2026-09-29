# Native LLM execution

The Rust core sends text and image requests directly over HTTP. It does not
start Python, load LiteLLM, or require an installed provider CLI. Request
construction, model selection and retry accounting share one implementation.
Configuration validation remains separate from runtime capability: an accepted
configuration can still request a provider or routing strategy that this build
explicitly rejects.

## Model selection

A nonempty `llm.model_list` owns the model pool. Entries with `weight: 0` are
excluded. Missing explicit `env:NAME` credentials or endpoint references make
that deployment unavailable; usable siblings can still serve the request. An
entirely disabled or unavailable configured pool does not fall back to ambient
credentials.

With an empty model list, a nonempty `MODEL` selects one model. Otherwise each
available API credential joins the pool: Anthropic, OpenAI, Gemini, DeepSeek
and OpenRouter. The model aliases are derived from the reference checkout's
provider-default table, not an independent assertion about current provider
availability. Pin `MODEL` or a configured deployment to control which provider
receives documents. Subscription providers and local CLI authentication are
not auto-detected by the native implementation.

`simple-shuffle` chooses deployments in proportion to positive integer weights.
A randomized process seed and an atomic sequence provide independent selection
without a runtime random-number dependency. The selection calculation uses a
128-bit weight sum. A failed deployment is avoided during the current request
while another eligible deployment remains; after every candidate has failed,
retries may revisit the pool.

Without configured fallbacks, all model names share the `default` pool. With
fallbacks, configured group names are preserved and requests enter `default`.
For example:

```json
{
  "llm": {
    "model_list": [
      {"model_name":"default","litellm_params":{"model":"openai/my-primary","api_key":"env:PRIMARY_KEY"}},
      {"model_name":"backup","litellm_params":{"model":"openai/my-backup","api_key":"env:BACKUP_KEY"}}
    ],
    "router_settings":{"fallbacks":[{"default":["backup"]}],"num_retries":2}
  }
}
```

Fallbacks are traversed in declared order, including nested fallback groups.
Each reachable group is attempted once; cycles, malformed target lists and
references to unavailable groups fail before requests begin. Each group gets
its configured transport retry allowance. `least-busy`, `usage-based-routing`
and `latency-based-routing` return an explicit unsupported error: the native
core does not yet maintain the persistent measurements those strategies need.
There is no cross-document deployment cooldown or health-history database.

## Providers and request parameters

| Model prefix | Native request protocol |
|---|---|
| `openai/`, or no prefix | OpenAI Chat Completions |
| `anthropic/` | Anthropic Messages |
| `gemini/`, `deepseek/`, `openrouter/` | Provider's OpenAI-compatible endpoint |
| `azure/` | Azure deployment Chat Completions, with `api-version` |
| `ollama/`, `ollama_chat/` | Ollama's OpenAI-compatible `/v1` endpoint |

Other prefixes return an unsupported error when no usable deployment remains.
This table describes implemented request shapes, not live compatibility tests
against every vendor. No provider network call is required by the test suite.

Deployment `api_key` and `api_base` take precedence. A saved provider selected
by `model_info.provider_id` can supply missing deployment credentials and base
URL. Provider-specific environment variables then supply missing values; OpenAI
also accepts `OPENAI_BASE_URL`. Explicit missing environment references do not
fall through to other credentials. Azure requires `api_version` or
`AZURE_API_VERSION`. Endpoint construction preserves query parameters and
encodes the Azure deployment name as one URL segment.

`litellm_params.max_tokens`, followed by explicit `model_info.max_tokens`, sets
the output cap. OpenAI/Azure GPT-5 and o1/o3/o4 names use
`max_completion_tokens`; other chat requests use `max_tokens`. Anthropic
requires a cap and defaults to 8192 when none is supplied. Other providers use
their own default when no cap is configured. Nonpositive explicit deployment
caps fail before a request. This implementation does not ship LiteLLM's model
catalog or tokenizer and cannot reproduce its dynamic context-window sizing.

Image requests reuse routing, authentication, budgets and retry behavior.
OpenAI-compatible requests carry a data URL; Anthropic receives native base64
image blocks. JPEG, PNG, WebP and GIF MIME types are accepted. Deployments
explicitly marked `supports_vision: false` are excluded from image requests.
The caller supplies encoded image bytes; compression and image decoding belong
to the image conversion stage. The orchestrator supplies complete PDF/TIFF page
sets and screenshots; [image enrichment](image-enrichment.md) adds structured
caption/description analysis through this same transport.

The client applies configured request timeouts and a connect timeout capped at
15 seconds. Redirects are rejected so credentials cannot be forwarded to an
unexpected endpoint. Provider error bodies are inspected for retry classification
and structured token usage; they are never included in public errors. Public request errors
omit URLs, authorization headers, document text and response payloads.

## Retries, budgets and usage

`router_settings.num_retries` means additional attempts after the first attempt
in a group. Connection failures, timeouts, interrupted response reads, temporary
HTTP failures, rate limits, recognized unavailable-model responses and empty
text responses may retry. Authentication failures do not retry the same group;
a configured fallback group can still run. Billing/payment/insufficient-quota
failures stop the entire operation. Truncated output is rejected instead of
being accepted as a complete document.

Backoff starts at one second and doubles. A numeric `Retry-After` value can
replace that delay. Individual delays and total backoff sleep across all groups
are capped at 60 seconds; exhausting the sleep allowance returns the last
failure. HTTP request timeouts are separate from this sleep allowance. The
request budget is checked before requests and before scheduling another sleep,
so an exhausted budget does not wait needlessly.

`llm.max_requests_per_document` counts every HTTP attempt, including failed
requests, transport retries and fallback groups. Zero disables this budget.
One conversion-scoped counter is shared by document enhancement, image analysis
and structured-answer fallback calls. Parallel document chunks explicitly share an
`Arc<Mutex>` accounting context; creating a worker does not reset its allowance.
Nested conversions restore the caller's
counter on return or unwinding; each independent conversion starts a new budget.

`LlmRuntime` supplies the request capacity for one caller-controlled run. A CLI
run shares one instance across its file and URL workers, using `llm.concurrency`
(default 10); `--llm-concurrency` changes this cap independently of batch worker
concurrency. Text and image requests use the same capacity. Native callers can
pass a borrowed runtime through `ConvertContext.llm_runtime`; cloning a runtime
shares its capacity. A supplied runtime's constructor limit is authoritative,
even if individual conversions have different configuration values. Separately
constructed runtimes are independent. Calls without a supplied runtime create
a local instance for their enhancement; independent binding calls therefore do
not acquire a process-wide or cross-process limit automatically.

Waiting requests enter in arrival order. A permit covers the HTTP attempt,
including reading and interpreting its full response, and is released on success
or error. Retries reacquire capacity; backoff sleep holds no permit. Persistent
cache hits return without acquiring one. Queue waiting is separate from the
configured HTTP timeout, which applies after admission to a request. This
blocking limiter introduces no async runtime dependency and does not cancel
already queued conversions. Hosts with an async event loop should dispatch the
blocking conversion outside that loop. CLI interruption continues to use the
batch scheduler's drain behavior.

The reference also scopes its shared runtime to a batch or service job; its
independent processors may own separate semaphores. This implementation does
not add provider-specific concurrency, rate-per-minute accounting, adaptive
routing metrics or a budget shared across unrelated documents.

Public usage retains the existing four fields per model: `requests`,
`input_tokens`, `output_tokens` and `cost_usd`. `usage.requests` counts parsed
successful HTTP responses and HTTP error responses containing structured usage,
including paid responses preceding successful retries or image-analysis fallback.
This differs intentionally from the attempt budget. Actual
response model identifiers are used when present. Anthropic cache-read and
cache-creation input tokens are included in input totals without adding public
fields. Costs remain zero because the native build has no pricing catalog.
Configured cost limits therefore remain unsupported.

A final error cannot return accumulated usage through the existing success-only
conversion result shape. If conversion itself returns a terminal error, no usage
object can accompany it. A successful fallback result now retains usage from
failed main/image stages through the shared scope. Malformed response JSON
cannot supply trustworthy token counts. These are known
accounting limits, not evidence that the provider charged nothing.

## Prompt selection

The caller supplies a source label separately from the document. Text requests
choose `cleaner_*` in pure mode, `url_enhance_*` for HTTP(S) sources, and
`document_process_*` for files. Image requests choose `document_vision_*`.
The root conversion layer can redact source labels before passing them here.

For each system/user template, precedence is its explicit `prompts.<name>` path,
`prompts.dir/<name>.md`, then freshly authored built-in text. A missing explicit
file falls through to the next source. A present unreadable or non-UTF-8 file
fails with a configuration diagnostic. Default `~/.markitai/...` paths honor
`MARKITAI_HOME` isolation.

Templates support `{source}`, `{timestamp}`, `{mode_rules}`, `{content}` and
`{metadata_section}`. For non-pure text processing the metadata placeholder carries
the typed JSON contract. The same contract is appended to custom system templates;
custom prompts do not bypass response validation. The first non-pure visual batch
also supplies its typed contract through this placeholder. Later visual cleaner
batches and pure requests leave it empty; pure behavior is unchanged.
Document content is inserted last so literal braces inside a document are not
interpreted as template placeholders. The default user template is exactly
`{content}`: pure mode sends the supplied Markdown unchanged. Its fresh system
instructions request byte-preserving frontmatter behavior. Non-pure formatting
and profile application remain the responsibility of the output pipeline.

## Structured documents and complete long text

Non-pure text from files and URLs uses a typed document response:

```json
{"cleaned_markdown":"Faithful Markdown", "frontmatter":{"description":"A concise description", "tags":["topic"]}}
```

The description and each tag must be nonblank strings. Description whitespace is
collapsed and values over 150 Unicode characters are shortened to 147 plus `...`.
Tags remove quotation marks, convert whitespace and colons to hyphens and are
limited to 30 Unicode characters each. An empty normalized tag collection fails.
Other model fields are ignored: the application owns title, source and processing
time. Only enhanced output receives generated description/tags; base metadata is
retained separately. A source identified as a social post requests metadata while
preserving its body verbatim.

Typed document requests use the capability ladder below. The final JSON-text
mode accepts a bare JSON object or a complete Markdown JSON fence. Arbitrary
plain Markdown is not promoted into successful metadata, including with custom
document prompts. Every decoded mode passes the same metadata and source-content
checks. Pure text retains its separate plain contract; image analysis uses its
caption/description/text schema through the same ladder.

Before a request, fenced and indented code, inline code, math, links/images,
reference definitions and HTML/comment markup are replaced by collision-safe
source-owned tokens. Their values never become system instructions. The answer
must retain the tokens exactly once in source order; original bytes are restored
only after successful validation. An oversized protected code block remains one
opaque token, so it cannot be truncated or have its internal blank lines changed
by chunking. This conservative scanner does not claim full CommonMark parsing.

Remaining text is packed at blank-line or line boundaries into at most 32,000
Unicode scalar values per chunk; an oversized prose line splits at scalar
boundaries. No tail is dropped. Bounded worker threads process chunks concurrently
through the same caller runtime and shared document accounting. Results merge in
source order and only the first chunk supplies metadata. For multi-chunk plans,
uncached calls plus `ceil(uncached / 5)` retry headroom must fit the remaining
request allowance before any model request. Zero disables the allowance.

Successful chunks are cached independently. All admitted workers settle before
an error is returned; no partial enhanced document is published. A retry can reuse
previously successful chunks. Empty boilerplate chunks are permitted, but the
merged document must remain nonempty and pass complete-document plausibility:
for sources of at least 200 non-whitespace characters, keep at least 20% of their
length and 30% of distinct character bigrams. Chunk checks are more permissive
when the response's character four-grams substantially come from that chunk.
These are deletion safeguards, not proof of semantic accuracy.

Both local and ordinary URL text use the [typed persistent cache](cache.md).
Cache hits require no model request and have zero new usage; bypass controls still
refresh successful answers. Pure text and standalone caption/description analysis
bypass this document cache. Non-pure page/browser vision uses its own image-aware
batch namespace, described below. The cache stores validated semantic data, never
a provider tool-call envelope. Round twenty-four changes the prompt-contract
fingerprint so older protocol rows remain on disk without being admitted as new
results. The randomly selected model deployment and successful protocol rung do
not enter this semantic fingerprint.

## Structured provider protocols

Document chunks, the first non-pure visual batch and image caption/description
analysis select a common wire mode from the actual reachable model pool. Positive
weight, resolved deployment credentials, configured fallback groups and explicit
vision exclusions retain their existing routing behavior. A missing referenced
environment variable removes that deployment; a credential-free configured
endpoint remains a candidate because it may be a local server. Capability
selection itself sends no requests, and a valid cache hit does not need it.

The initial exact capability table is deliberately small:

| Provider and exact model IDs | Available modes before JSON text |
|---|---|
| OpenAI `gpt-4.1`, `gpt-4.1-2025-04-14` | Named tools, JSON schema |
| Anthropic `claude-haiku-4-5`, `claude-haiku-4-5-20251001` | Named tools, native JSON schema |
| Anthropic `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-fable-5-1`, `claude-mythos-5-1` | Native JSON schema; these models restrict forced named tools |
| Gemini `gemini-3.8-flash` through its OpenAI-compatible endpoint | JSON schema |
| Other or unknown IDs, including Azure deployment aliases | JSON text |

The [official capability evidence](planning/after-round23-structured-transport.md)
records model/protocol sources and limits. Neither a model-name prefix nor
`supports_vision` establishes structured support. Tool and schema support are
independent bits: intersect them across all reachable candidates, then visit
supported modes in the order tools → schema → text. A pool containing an unknown
model therefore starts at text. No new public capability options are introduced.
A custom endpoint may reject the optional fields even for a documented model.

Chat tools use a forced, named function and decode exactly one expected
`tool_calls[].function.arguments`. Anthropic uses `input_schema`, native tool
choice and exactly one matching `tool_use.input`. Tool data is never executed.
Missing, wrong-name or multiple results fail validation. Chat schema mode uses
`response_format.json_schema`; Anthropic uses `output_config.format`. Both then
validate the decoded object with the same application-owned contract. Tool
responses may have no text body; this is valid when their typed data is valid.

Each non-final mode gets one schema-validation attempt. Rejection of tools/schema
parameters with HTTP 400/422 descends without resending the same shape. Unrelated
invalid-input errors, explicit refusal, token-limit truncation, quota and budget
failures stop the ladder; authentication failures retain configured routing
fallbacks before stopping. Transport retries stay inside the router. Exhausted network/HTTP transport errors
stop the ladder in every mode and do not trigger image caption/description fallback.
The final JSON-text mode gets three validation attempts total. Response-size
limits are terminal resource errors, not invalid JSON to retry or downgrade. All actual HTTP attempts
use the same document budget, runtime permit and paid-usage accounting. Structured
fatal responses publish cancellation before releasing their permit.

After the final invalid JSON-text candidate, a local repair may remove trailing
commas immediately before existing object/array closing tokens, outside strings.
It runs only for responses up to 1 MiB and must pass all schema and content guards.
It adds no HTTP request. It never invents quotes, missing closers, fields, metadata
or document content; truncated/refused responses never enter repair. More general
Python-style JSON, free-text extraction and incomplete-object repair remain
unsupported. Image analysis retains its existing plain caption/description
fallback after nonfatal ladder exhaustion, within the same accounting scope.

## Structured visual documents

Non-pure PDF/Office page images and standalone image/TIFF transcription use
`MARKITAI_VISION_JSON_V1` for the first batch. The response has the same
`cleaned_markdown` plus `frontmatter.description/tags` shape as typed text.
Canonical title, source and time remain application-owned. The original base
body does not receive model metadata. Standalone alt/description analysis keeps
its separate existing image-analysis schema.

Paged documents use at most ten images per request. The first batch completes
before later batches begin; later requests use `MARKITAI_VISION_CLEAN_V1` and
return plain Markdown without metadata. Workers share the caller's runtime and
conversion accounting context. They may complete out of order, but their bodies
are assembled in frame order and only the first batch supplies metadata.
Browser screenshot tiles describe a single web page and remain one bounded
visual request; they are not assigned fictitious document page boundaries.
Pure requests retain their existing path.

The caller supplies positive increasing frame numbers, actual image MIME and
bytes. Existing page-count limits apply before requests, and the complete image
set must be nonempty and at most 100 MiB. Complete ordered `Page number:` or
`Slide number:` comments align text to pages. Preamble and trailing material are
retained, and code examples containing those comments do not create pages.
If a complete map is absent, all source text is retained once, split at safe
line/literal boundaries across batches; a warning explicitly declines precise
text-to-page alignment. No frame is dropped in that fallback.

Source literals use the same protected markers as text processing, separately
for each visual batch. Each marker must survive exactly once and in order.
Scanning an image with no extracted text is valid: the output need not overlap
empty text. For substantial extracted prose, the existing deletion safeguard
also checks the visible text after protected literals are removed. Short explicit
refusals, repetitive degraded output, blank results and token-limit truncation
are rejected. These are conservative safeguards, not proof of correct OCR or
semantic fidelity; model accuracy still needs independent evaluation.

The first typed visual batch follows the provider ladder above. Later plain
cleaning batches retain at most three validation attempts for malformed output
or violated content guards, within the shared request budget. The known minimum number
of uncached batches must fit the remaining budget before the first request.
Transport retries, visual validation, document fallback and image analysis use
that same budget; zero still means unlimited. Paid error and invalid responses
retain their parsed usage exactly once.

A fatal first-batch failure prevents later dispatch. Fatal authentication after
available routing fallbacks, quota/billing failure and exhausted budgets stop
queued batches before HTTP admission. Already active calls finish. A nonfatal
batch failure may leave valid sibling batches cached, but it never returns a
partial enhanced document. Ordinary rendered web pages may use the remaining
budget for typed text fallback; visual-only/empty pages, PDF/Office documents
and fatal failures do not use that fallback. Existing output failure policies
govern retained base files and screenshots.

The native vision-model diagnostic uses the actual deployment and credential
resolver, reachable groups, positive weights and explicit `supports_vision`
exclusions. It reports eligible identities without a provider probe or guessing
capability from model names. An unspecified capability is eligible, not evidence
that the provider can process images.

## Verification

Unit tests inside `crates/markitai-core/src/llm.rs` cover weighted boundaries,
automatic pooling and explicit-model precedence, disabled/missing-environment
filtering, saved-provider credentials, Azure request parameters, deployment
rotation, fallbacks and cycles, quota/authentication short-circuiting, request
budgets, empty-response usage, Anthropic vision/cached-token accounting, prompt
precedence, literal document braces and token-limit parameter mapping. Cache
checks additionally cover credential-independent hits, zero new usage, bypass
refresh, prompt/content/model invalidation, disabled/pure exclusions, corrupt
or unwritable state, and rejecting token-truncated or blank cache candidates.
Shared-runtime tests hold partial HTTP response bodies to check that concurrent
text and image requests stay within the same cap. They also exercise requests
during another request's backoff, terminal and budget failures, and a cache hit
while every permit is occupied. `llm_runtime.rs` tests cloning, independent runs,
waiting callers, invalid zero capacity and permit release during unwinding.

`tests/conversion/document_processing.rs` adds isolated public-API scenarios for
typed fields and base separation, Unicode parallel chunk order, literal fidelity,
URL cache hits without credentials, partial-failure retry, preflight admission,
shared retry budgets, custom prompts and unchanged pure behavior. Chunk/cache
unit checks cover typed namespaces, Unicode tails and malformed metadata.

`tests/conversion/vision_processing.rs` contains loopback HTTP
cases for 21-frame bounded concurrency/order, eleven-frame cache reuse and pixel
changes, paid authentication failure, later failure with partial-cache retry,
zero-request budget rejection, and queued-batch cancellation. Module tests cover
page/slide boundaries, code-literal examples, cache content guards and actual
vision-model eligibility. Executed round-twenty-three results are recorded in
[its validation report](validation/vision-auth-cli-round23.md).

Round twenty-four adds `tests/conversion/structured_transport.rs`: actual loopback
HTTP exercises named tools, native Anthropic shapes, schema/text descent with
paid errors, conservative pool capabilities, cache reuse without credentials,
protected literals, bounded repair, fatal errors, a shared request budget,
image-analysis sidecars and first-visual-versus-later-cleaner behavior. These new
cases pass the coordinator's source-frozen R24 gate; live-provider compatibility
and installed-release evidence are separate checks.

HTTP tests bind loopback listeners, capture request headers and JSON, return
scripted responses, and use bounded socket timeouts. Low-level retry tests inject sleeps as a recorder; integration fixtures avoid
retryable transport failures or use bounded local behavior. They never contact a
live model provider. The project operations record contains the actual executed build and
test results.
