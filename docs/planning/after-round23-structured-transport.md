# Structured transport after visual batching

This is an implementation contract, not evidence that these protocols already
work. Round twenty-three's JSON-text processing and pure requests remain intact
while the transport is extended. No new public configuration is needed.

## Existing configuration and reference behavior

Neither reference `config.py:216–284` nor Rust `config_contract.json` defines a
structured-output capability override. `ModelInfo` has `id`, `provider_id`,
`supports_vision`, `max_tokens` and `max_input_tokens`; `LiteLLMParams` contains
model, credentials, base URL, weight, API version and token cap. Adding hidden
`supports_tools`, `supports_json_schema` or `structured_mode` keys would change
this contract; unknown fields are not a usable configuration channel.

Reference `llm/structured.py:79–150` resolves capability from a local provider's
`STRUCTURED_OUTPUT_MODE`, otherwise LiteLLM metadata through
`supports_function_calling(model_id)` then `supports_response_schema(model_id)`.
Unknown models use Markdown JSON. Positive-weight routable pool members determine
the weakest common starting rung: tools → JSON schema → JSON text. Capability
selection sends no requests. Rust must additionally respect its established
credential resolution, reachable fallback groups and vision eligibility when
identifying a request's actual pool; `supports_vision` does not imply tools.

Reference `llm/engine.py:431–518` gives non-final rungs one validation attempt;
configured transport retries remain separate. Any nonfatal exhausted rung
continues downward. The final JSON-text rung gets two additional validation
attempts (`constants.py:35`) and only then may repair its last response locally.
Non-retryable provider errors and budget refusal stop descent. Authentication
and permission failures also stop document batches after routing fallbacks are
exhausted (`engine.py:388–412`). Paid responses retain usage regardless of schema
success; a repair is not another billable request. Token-limit truncation is
rejected before parsing or repair.

## Small implementation boundary

1. Add `llm/structured.rs` with a private response contract (schema name, schema,
   validation) and mode ladder. Document processing and the first visual batch
   use the document contract; later visual cleaners and pure/probe calls stay
   plain. The same adapter should support the existing image-analysis contract
   without changing its public caption/description/text shape or adding a second
   accounting layer. Migrate that caller only with its existing failure tests.
2. Extend private request construction and response decoding in `llm.rs`.
   Chat/Azure tools carry a named function schema and forced tool choice; validate
   exactly the expected function's arguments. Native Anthropic uses tools with
   `input_schema`, its native choice shape and `tool_use.input` objects. Neither
   response is document text to execute. Missing, wrong-name, conflicting or
   multiple structured results fail validation. JSON-schema mode must be shaped
   for that protocol, never by blindly sending a Chat field to Anthropic.
3. Decode a bounded provider envelope once, record its paid usage once, then
   dispatch by expected response mode. A tools response with null text must not
   hit today's “no text” retry path. Keep refusal/truncation and HTTP failure
   classification explicit through the existing `VisionFailure`/shared scope.
   Apply existing metadata, protected-marker and content guards after decoding.
4. Preserve `process_document_with_runtime` and `VisionRequest` interfaces.
   Cache only the validated semantic answer, not tool-call wrappers. Bump prompt/
   contract fingerprint versions if semantics change; retain existing rows.
   Every fallback HTTP attempt shares the document cap and runtime permit.
   No per-mode budget reset or second usage aggregation is permitted.
5. Local JSON repair, if included, runs only on the final JSON-text candidate
   after validation retries, with no new HTTP call. Revalidate the complete
   schema and source guards. Never repair token-truncated responses into an
   allegedly complete document. If bounded safe repair is deferred, document
   that remaining difference instead of claiming the full reference ladder.

## Official evidence checked on 2026-09-29

These pages were searched and opened; no model request was made. They establish
wire contracts and candidate capability rows, not account availability or actual
endpoint conformance. Do not use a model-name prefix as a capability rule.

| Candidate | Evidence and initial decision |
|---|---|
| OpenAI `gpt-4.1`, `gpt-4.1-2025-04-14` | Its [model page](https://developers.openai.com/api/docs/models/gpt-4.1) names both IDs and explicitly lists function calling, structured outputs and Chat Completions. Seed exact IDs with both tools and schema capability. This is not a wildcard for all GPT or reasoning models. |
| OpenAI Chat wire | The [Chat API reference](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create) uses nested `function` objects for named tool choice, unlike Responses examples. Schema mode uses `response_format` with `json_schema`. Decode `tool_calls[].function.arguments`; never execute them. |
| OpenAI schema/refusal | The [structured-output guide](https://developers.openai.com/api/docs/guides/structured-outputs) requires a restricted schema, including required object fields and `additionalProperties: false`. A `refusal` field and `finish_reason: length` can prevent a schema result. Keep those separate from malformed JSON; no repair or cache admission for truncated output. |
| Anthropic Haiku 4.5 | [Model IDs](https://platform.claude.com/docs/en/models/overview) identify `claude-haiku-4-5` and `claude-haiku-4-5-20251001`; [structured-output compatibility](https://platform.claude.com/docs/en/build-with-claude/structured-outputs) includes Haiku 4.5. With the native Messages protocol and no enabled thinking override, these exact rows can seed both tools and schema. |
| Anthropic native schema | [Structured outputs](https://platform.claude.com/docs/en/build-with-claude/structured-outputs) uses `output_config.format`, with JSON returned as text content. The old `output_format` field is transitional. Strings/arrays still need application-side validation; do not send unsupported string length or numeric constraints. |
| Anthropic forced tools | [Define tools](https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools) explicitly excludes forced `any`/named `tool` for Opus 5.5, Sonnet 5.5, Fable 5.1 and Mythos 5.1, even though they offer other tool behavior. Those documented exact IDs should start at native schema, not forced tools. Manual extended thinking also restricts forced choice. Native tool definitions use `input_schema`; replies use `tool_use.input`. |
| Azure | [Microsoft's structured-output guide](https://learn.microsoft.com/en-us/azure/foundry/openai/how-to/structured-outputs) ties support to underlying model/version and supported API versions (first `2024-08-01-preview`). Current native configuration names a deployment, not necessarily that model. Keep arbitrary deployment aliases at JSON text; an OpenAI-looking alias alone proves nothing. Add table rows only where underlying identity is established by existing metadata without a paid probe. |
| Gemini | [Google's OpenAI compatibility page](https://ai.google.dev/gemini-api/docs/openai) demonstrates Chat structured parsing for `gemini-3.8-flash` and tool examples. It supports a conservative exact schema-capable seed; the page inspected here did not establish our required forced-name tool behavior for every Gemini model. Keep other IDs unknown until their actual protocol/model evidence is checked. |

DeepSeek search results described endpoint-dependent beta strict tools, but the
pages failed to open during this review. No DeepSeek capability row is justified
by that search snippet alone. OpenRouter, Ollama and custom gateway IDs likewise
remain JSON text for this first small table. A caller's custom endpoint may
reject a documented model's optional wire fields; that is handled by bounded
mode rejection, not a hidden endpoint rewrite or capability probe.

## Concrete private APIs and capability rules

Use two independent capabilities, not a single ordinal: support for forced
schema tools does not prove native JSON-schema output, and the converse is false.
A new `llm/structured_capabilities.rs` can contain exact entries with provider,
model ID, `forced_tools`, `json_schema`, protocol restrictions and source URL/date.
Unknown means both false. Do not introduce configuration keys or mutate saved
model information.

Resolve deployments with existing credentials, positive weights, reachable
fallback groups and request-specific vision exclusions. Intersect capability
bits across that actual pool, then visit shared supported modes in the fixed
order Tools → JsonSchema → JsonText. Skip a mode with no supported native wire
shape. This is a deliberate conservative refinement over treating reference
LiteLLM's strongest tier as proof of every intermediate tier. A pool containing
an unknown member starts at text; a disabled or unreachable member does not
lower it. Native Anthropic forced-choice exclusions therefore remain effective
inside mixed-provider pools.

Suggested internal surface (no public core/binding API changes):

```rust
enum WireMode { Tools, JsonSchema, JsonText }
enum SchemaKind { Document, ImageAnalysis }
struct StructuredRequest<'a> {
    prompts: &'a Prompts,
    schema: SchemaKind,
    stop: Option<&'a AtomicBool>,
}
fn run_structured<T>(request: StructuredRequest<'_>, cfg: &Value,
    env: &HashMap<String, String>, runtime: Option<&LlmRuntime>,
    validate: impl Fn(&Value) -> Result<T>)
    -> std::result::Result<(T, ConversionUsage), CallFailure>;
```

`CallFailure` remains private and carries typed disposition/status, not raw
provider text. Split today's request-body decoder into bounded envelope read →
usage record → expected-mode extraction. Keep a plain wrapper for pure, cleaner
and connection-probe calls. `document::run_chunk`, `vision::run_batch` first mode
and image analysis supply their existing validators. The ladder owns validation
retries: remove outer retry loops for migrated structured calls, or the current
three attempts would multiply across modes. Cache writes remain outside it.

Recognized tools/schema parameter rejection (400/422) moves to the next eligible
mode without resending that rejected shape. Inspect bounded structured error
codes/parameter names and retain only a sanitized classification. Unrelated
invalid-image/input errors do not become capability evidence. Configuration,
authentication after routing fallback, quota and budget failure do not descend.
Temporary transport failures retain current bounded retry/fallback behavior;
validation failure can descend after the single non-final attempt. Final JSON
text gets the existing total of three validation attempts. Preserve cancellation
publication while still holding the HTTP permit, including fatal structured
responses, so queued visual siblings cannot bypass the stop flag.

Keep semantic cache identity independent of the randomly selected deployment or
successful rung. Include schema-contract/validator version and prompt changes;
retain full visual bytes. A mode rejection is never cached. Successful validation
can reuse the semantic answer later without credentials or a protocol probe.
Mode discovery itself consumes zero requests, and parsed paid failures still
contribute once to the authoritative document usage.

## Minimum loopback acceptance

- Table-driven actual HTTP payloads: Chat/Azure named-tool success, native
  Anthropic tool success, wrong tool/multiple results rejected, and no secret or
  tool arguments included in errors. Assert existing plain probe/pure payloads.
- Tools rejection → schema success, then tools/schema rejection → JSON-text
  success. Assert separate original request bodies (no cumulative prompt mutation),
  exact request count and paid usage; metadata/body remain identical across modes.
- Mixed/unknown pool starts at JSON text; disabled or unusable members do not
  lower the actual pool. Capability selection itself makes zero requests.
- Auth/quota and exhausted document budget do not descend. Cover a concurrent
  later visual batch so structured fallback cannot bypass fatal cancellation.
- Final malformed JSON either passes bounded repair plus all guards, or fails
  without a cache entry. Truncated output, changed protected markers and empty
  required metadata remain failures. A second conversion proves validated cache
  reuse, and a failed later batch still publishes no partial enhanced body.

These fixtures prove native wire shapes, accounting and failure behavior. They
do not establish live-vendor availability, model accuracy or pricing accuracy.
