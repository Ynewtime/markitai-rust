# Small runtime stages after structured transport

Implementation status: the contracts selected here have source validation in
[round 25](../validation/runtime-history-round25.md). The proposal below preserves
its original design reasoning; broader follow-up work is tracked separately.

This is a proposed next implementation, not a claim that requests are already
coalesced or that terminal errors expose usage. It requires no new configuration
keys. Keep the two stages independently reviewable: shared in-flight work first,
then an additive detailed-error path across core and adapters.

## What the reference actually does

Reference `llm/processor.py:181` creates one `ContentCache` per processor.
`llm/cache.py:567–631` implements a bounded, thread-safe LRU/TTL result cache;
its lock protects individual reads and writes. `llm/engine.py:757–867` checks
memory/persistent caches, releases those cache locks, performs a request and then
stores the validated answer. That sequence has no per-key pending owner/future:
two concurrent cold misses may both call the model. Provider-discovery
single-flight logic is separate and does not establish LLM-request coalescing.

Reference `api.py:409` raises `ConversionError(result.error)` on failed local
conversion. The class in `utils/errors.py:72` carries the rendered message, not a
public usage object. Successful `ConversionOutput.usage` comes from workflow
usage. Consequently terminal-error usage would be an intentional additive native
improvement, not restoration of an existing Python exception property.

Native code already shares each conversion's attempts and paid responses through
`DocumentScope`, including chunks, image analysis, retries and visual fallback.
It publishes that usage into successful fallback results. A terminal early
`Err` or late publication error drops it: `Error` has no usage field, and
`convert_json` emits only `error.code` and `error.message`. Existing Python/Node/Go
wrappers preserve those error categories but likewise carry no usage on failure.

## Stage A: merge identical active typed requests within a shared runtime

Start with typed text chunks and first visual batches, scoped to the existing
`LlmRuntime` identity. Clones share the table; independently constructed runtimes
remain independent. This gives CLI batches and each service job real reuse without
introducing a global tenant cache. Independent host-binding calls currently do
not share a runtime, so this stage must not claim cross-call merging for those
adapters. Image analysis and plain/pure request reuse can follow after the typed
path proves its accounting and failure behavior.

Use a small private `llm/flight.rs` coordinator owned by the runtime and invoked
around each worker's cache-miss operation. It returns either an owner guard or a
waiter. Owners perform the existing ladder, source validation and cache write;
only then publish validated semantic data. Waiters apply their own source/content
validation before consuming it. They never hold a request permit or document
accounting mutex while waiting. Recheck persistent cache after ownership is won,
since another already-completed operation may have written it meanwhile.

A persistent cache key alone is insufficient. Its current model scope intentionally
omits endpoints, credentials and some request settings. Build the private flight
identity from all of:

- Contract/schema and validation version, exact rendered system/user prompts,
  ordered frame number/MIME/full bytes, and semantic cache identity.
- Resolved eligible deployments, protocol, endpoint including Azure version,
  credential identity, weights, reachable fallback graph, token limits and
  transport retry/timeout policy. Resolved values matter: an unchanged `env:KEY`
  spelling with a rotated value must not join the earlier request.
- Runtime identity, cache read/bypass state, and document budget policy. Only
  requests eligible for reuse join; `cache.enabled=false`, matching bypass
  patterns or forced refresh keep their established fresh-request behavior.

Hash secret-bearing identity only in process memory, preferably using a keyed
process-random digest. Never serialize it, expose it through Debug/logs, or change
persistent keys into credential fingerprints. Use explicit framing and stable
ordering; do not rely on an ambiguous joined string or unordered object iteration.
No provider request is allowed while calculating identity. A cache hit continues
to work without credentials because this identity is constructed only on a miss.

Usage belongs to the owner that actually sent HTTP. Successful waiters receive
zero new paid usage/attempts and count as reused output; they must not merge the
owner's usage into their `DocumentScope`. Total batch accounting must equal the
actual provider calls, even if several documents display the same answer. Keep
existing conservative multi-chunk admission checks initially; do not relax a
budget merely because a future peer might eventually become an owner.

Failure and cancellation are the important boundary:

1. A waiting document's cancellation detaches that waiter and does not cancel the
   owner's already active HTTP call or another document.
2. An owner that fails, reaches its own budget, is cancelled before admission or
   unwinds wakes every waiter. Failed data is never a reusable answer. A live
   waiter rechecks its own cache/budget and may become the next owner; it must not
   inherit the first document's exhausted budget or paid usage.
3. Ownership cleanup must compare the exact generation/entry identity before
   removal. An older guard cannot remove a newer owner for the same key.
4. Success, error and panic all release/wake via RAII. Use bounded wake intervals
   to observe caller cancellation; do not hold a global lock across HTTP, parsing,
   cache I/O or a condition-variable wait.
5. Bound the table and retained response bytes separately. Overflow bypasses
   merging and uses the existing router; it never rejects an otherwise valid
   conversion. Remove completed map entries immediately; outstanding waiters own
   an `Arc` result until consumed. Do not accidentally create an unbounded result
   cache or retain image byte copies per waiter.

Suggested private surface: `FlightKey`, `FlightTable::join`, an owner guard with
`publish(Arc<Value>)`, and a waiter result distinguishing ready/retry/cancelled.
Avoid cloning `Error` or `DocumentScope` state into a foreign document. Share only
validated response data; each caller still constructs and publishes its own
output files and profiles.

Minimal loopback evidence: two conversions sharing a runtime and identical
request data produce one gated POST, two complete outputs and aggregate usage of
one call; rotated credentials, different endpoint/prompt/image bytes, independent
runtimes and forced refresh each produce distinct requests. A failed or unwound
owner releases waiters; a cancelled waiter does not interrupt a surviving owner.
Use server barriers/counters, not time-based speed assertions. Add an actual CLI
batch case with duplicated inputs/content where the rendered prompts are equal.
Record avoided requests first; any speed measurement is a separate experiment.

## Stage B: preserve terminal usage without changing existing error categories

Do not add a generic `Error::WithUsage` wrapper as the first step. Existing Rust
callers and native code match concrete variants such as `NoModelConfigured` and
`Unsupported`; wrapping them would change those matches and error policies.
Instead add an additive detailed entry point/result:

```rust
pub struct ConversionFailure {
    pub error: Error,
    pub usage: ConversionUsage,
}
pub fn convert_detailed(/* existing source/options */)
    -> std::result::Result<ConversionOutput, ConversionFailure>;
```

Keep `convert`, context/publication variants and their existing `Result<_, Error>`
signatures, delegating to the common detailed implementation and discarding only
the extra diagnostics for old Rust callers. Supply equivalent internal detailed
context/publication paths to CLI/services; do not bypass durable output claims.
Capture the conversion's scope snapshot before it drops, including errors during
final Markdown/assets/sidecar publication. Pre-model validation errors have empty
usage. Capture once at the orchestration boundary, after admitted workers settle,
not by wrapping each inner error and repeatedly merging the same tokens.

For C ABI JSON, retain `ok:false`, `error.code` and `error.message`; add optional
`error.usage` only when there is known recorded usage. Existing wrappers ignore
unknown error fields. Update them to expose that optional value while preserving
exception class, code, message, function signature and native buffer ownership:
Python exception `.usage`, Node `ConversionError.usage`, Go
`ConversionError.Usage *ConversionUsage`. Existing constructor arguments remain
valid; no usage must remain distinguishable from a known zero-token paid response.
Do not put raw model bodies, tokens, prompts or credentials into diagnostics.

CLI/REST/MCP adoption needs a separate explicit contract check: current failure
records often use a string error and null usage. Do not turn those string fields
into objects. Prefer retaining their established shape and populating an existing
nullable usage slot where available; otherwise add an optional diagnostics field.
Saved report/history schema changes must be documented and tested before claiming
these surfaces preserve terminal usage. This can follow the core/C-ABI stage;
no new command-line flag is necessary.

Minimum evidence: paid invalid responses followed by `on_failure=fail`, paid
401/quota responses, a successful main model followed by failed image analysis,
and a publication failure after successful paid processing all return accurate
single-count detailed usage. An early config/input failure has no paid usage.
Verify the old Rust error variant and each existing host exception category are
unchanged. A concurrent two-document case must prove scopes never exchange spend.
Run installed Python/Node/Go error-path smoke checks after the adapters are built.

Neither stage adds price estimates, provider rate limits, shared cross-process
budgets or a general process-global cache. These are separate contracts and must
not be inferred from reduced duplicate calls or additional failure diagnostics.
