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
to the image conversion stage. This path does not implement OCR engines,
multi-page vision planning, caption/description pipelines or screenshot capture.

The client applies configured request timeouts and a connect timeout capped at
15 seconds. Redirects are rejected so credentials cannot be forwarded to an
unexpected endpoint. Provider error bodies are inspected only for retry
classification and are never included in public errors. Public request errors
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
The current text or standalone-image enhancement performs one logical operation
per document. A future multi-stage OCR/metadata pipeline must share this budget
across its operations rather than resetting it per call. Batch concurrency is
currently managed by the caller; there is no process-wide `llm.concurrency`
scheduler shared between independent conversions.

Public usage retains the existing four fields per model: `requests`,
`input_tokens`, `output_tokens` and `cost_usd`. `usage.requests` counts parsed
successful HTTP responses, including empty paid responses preceding a later
successful retry; this differs intentionally from the attempt budget. Actual
response model identifiers are used when present. Anthropic cache-read and
cache-creation input tokens are included in input totals without adding public
fields. Costs remain zero because the native build has no pricing catalog.
Configured cost limits therefore remain unsupported.

A final error cannot return accumulated usage through the existing success-only
`enhance` result shape. If an empty/truncated paid response is followed by a
terminal failure, the outer conversion currently loses that accumulated usage.
Malformed JSON cannot supply trustworthy token counts. These are known
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
`{metadata_section}`. The metadata-section placeholder is currently empty.
Document content is inserted last so literal braces inside a document are not
interpreted as template placeholders. The default user template is exactly
`{content}`: pure mode sends the supplied Markdown unchanged. Its fresh system
instructions request byte-preserving frontmatter behavior. Non-pure formatting
and profile application remain the responsibility of the output pipeline.

Non-pure local document enhancement has a [persistent cache](cache.md). Cache
identity uses complete Markdown, resolved prompt templates/rules and the enabled
model pool, with a native version namespace. A hit precedes credential resolution
and returns zero new usage. `cache.no_cache` and matching source patterns bypass
reads while still refreshing a successful answer; disabled caching does neither.
Pure, image and URL enhancement bypass this cache. Database errors do not discard
a successful model answer and surface as sanitized conversion warnings.

The old structured-response schema, chunking, placeholder-repair staircase and
refusal/degeneration detectors are not yet implemented. The native built-ins
return Markdown; they do not promise the old structured metadata-generation
output.

## Verification

Unit tests inside `crates/markitai-core/src/llm.rs` cover weighted boundaries,
automatic pooling and explicit-model precedence, disabled/missing-environment
filtering, saved-provider credentials, Azure request parameters, deployment
rotation, fallbacks and cycles, quota/authentication short-circuiting, request
budgets, empty-response usage, Anthropic vision/cached-token accounting, prompt
precedence, literal document braces and token-limit parameter mapping. Cache
checks additionally cover credential-independent hits, zero new usage, bypass
refresh, prompt/content/model invalidation, disabled/pure/URL exclusions, corrupt
or unwritable state, and rejecting token-truncated or blank cache candidates.

HTTP tests bind loopback listeners, capture request headers and JSON, return
scripted responses, and use bounded socket timeouts. Retry sleeps are injected
as a recorder; tests never wait for real backoff and never contact a live model
provider. The project operations record contains the actual executed build and
test results.
