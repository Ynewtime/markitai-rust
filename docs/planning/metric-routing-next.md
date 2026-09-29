# Next routing slice: observed usage and latency

Planning evidence collected on 2026-09-29. Neither strategy is implemented by
this document. R28 implements least-busy separately; its runtime ownership,
request budget, retries and paid-usage contracts remain the starting point.

## Reference contract and evidence

The read-only reference checkout pins LiteLLM **1.100.1** in `uv.lock`; the local
installed distribution reports the same version. Evidence below names paths
relative to that checkout or to its installed `litellm` package, without
depending on a workstation home directory.

- `packages/markitai/src/markitai/config.py`, `RouterSettings`: the public names
  are `usage-based-routing` and `latency-based-routing`. There is no public
  `routing_strategy_args`, `tpm`, `rpm`, Redis or metric-window option.
- `llm/processor.py::_create_router` and `_build_router_entries`: standard model
  settings are forwarded; the explicit parameter whitelist does not forward
  TPM/RPM limits. Weight zero removes a deployment. Local subscription providers
  continue to use the Markitai wrapper's weighted selection.
- `llm/router.py::_standard_acompletion` delegates standard calls to LiteLLM;
  Markitai does not implement either metric algorithm. Internal LiteLLM retries
  are disabled, preserving the application's retry ownership.
- Installed `router_strategy/lowest_tpm_rpm.py` supplies the public usage v1
  algorithm. The separate `lowest_tpm_rpm_v2.py` is not the public configuration
  value and must not be substituted silently.
- Installed `router_strategy/lowest_latency.py` supplies latency selection;
  `litellm_core_utils/core_helpers.py::safe_divide_seconds` defines the zero-token
  denominator fallback. The following defaults come from those files, not from
  an unverified current website or a guessed model capability.

| Detail | Usage v1 | Latency |
|---|---|---|
| Score | Successful response `usage.total_tokens` accumulated for the current clock minute | Mean of the most recent 10 recorded latency samples |
| Normal sample | Success callback adds total tokens; RPM is also counted | Complete non-streaming response seconds divided by completion tokens; zero/missing usable denominator retains raw seconds |
| Time scope | Minute key formatted `HH-MM`; each group TPM/RPM cache write has a 60-second TTL | Group map TTL is 3,600 seconds and refreshes on a sample write |
| New deployment | Zero observed tokens | Zero score until an observation exists |
| Ties | First lowest entry in the observed dictionary's insertion order | Random among equal minimum scores; configured latency buffer defaults to zero |
| Failures | No success sample from failed requests | Async timeout callback appends a `1000.0` penalty; other failures add no sample |
| Weights | Positive weights do not divide or multiply the score | Positive weights do not divide or multiply the score |

The dependency can also enforce TPM/RPM ceilings and streaming time-to-first-token
metrics. Neither is required by the current Markitai public configuration and
native non-streaming transport. Do not add public settings or claim those
capabilities as part of this slice.

Reference metric state belongs to each LiteLLM Router's default in-memory
`DualCache`, not a persistent Markitai database. `LLMProcessor` lazily retains its
main router; its vision router is either the same router or a separately built
vision subset. The native equivalent remains caller-owned `LlmRuntime` state.
Independent runtimes and serialized binding calls remain independent.

## Minimal native implementation

Implement usage first, then latency using the same private observation seam.
Keep public configuration, `ConvertContext`, JSON output and binding signatures
unchanged. Both strategies retain existing eligible-candidate filtering,
transport retry/fallback order and first-batch cancellation rules.

1. Extend `llm/routing.rs` with private strategy parsing, retained observations
   and selection. Reuse R28 salted deployment identity: resolved credential,
   endpoint, model, protocol, group and explicit deployment ID. Keys never enter
   Debug, disk, errors or reports. Keep observation namespaces separate for
   strategy and text/vision pool; do not turn differently timed visual requests
   into unqualified text latency evidence. Document this native scope rather
   than claiming byte-identical LiteLLM callback state.
2. Add a private transport observation, conceptually
   `Success { elapsed, total_tokens: Option<u64>, output_tokens: Option<u64> }`,
   `Timeout`, or `OtherFailure`. Measure with a monotonic clock from HTTP send
   through complete bounded response decoding, excluding permit queueing and
   backoff. Do not infer a timeout by matching an error string.
3. Capture the observation at the provider-envelope boundary in
   `llm.rs::request_with_mode`, before application schema/content validation.
   A valid paid provider response followed by invalid typed document content
   is still a provider success observation, as with the reference callback.
   Invalid JSON, transport failures, cancellation and resource-limit rejection
   are not successful metric samples. Provider-reported zero tokens are known
   zero; missing usage must remain unknown.
4. Keep this separate from `record_usage`: paid error responses still belong in
   terminal accounting, even though usage routing does not score them as
   successful calls. Routing observations must never add to `ConversionUsage`
   or charge coalesced waiters. Persistent cache hits and shared-answer waiters
   produce no transport observation.
5. Selection remains after global capacity and cancellation/budget admission.
   Update observations under the runtime lock once per physical attempt; keep
   retries in the existing router rather than adding a metric-specific retry
   loop. Preserve RAII active-request release independently of whether an
   observation can be recorded.

Usage should retain only the current minute's totals, keyed by a full epoch
minute rather than repeating `HH-MM`. Wall-clock minute boundaries determine
the bucket; monotonic expiry bounds stale state after a clock jump. This is a
deliberate collision-avoidance difference. Prefer explicit provider total tokens;
derive a total only from known protocol token fields, never from estimated
source characters. The native provider adapters must document their total-token
normalization, particularly Anthropic cache-read/cache-creation input tokens.

Latency retains at most 10 finite samples per deployment. Unknown usage uses
raw elapsed seconds, not a fabricated token count. Preserve the reference
1000-second timeout penalty and random minimum-score ties. TTL belongs to the
metric group map: do not accidentally claim exact expiry parity while expiring
each deployment independently.

Unlike active counts, retained metrics need their own bound. Proposed bound:
4,096 idle deployment records per runtime, plus identities currently holding an
existing runtime permit. Remove expired records first, then least-recently-used
idle records; never evict an in-flight reservation. Eviction causes a documented
cold start, not a routing failure or an unbounded history. A group-expiry record
must disappear when it no longer owns any deployment records. This is an
internal safety limit, not a new user setting.

## Acceptance before enabling each strategy

- Unit tests with injected private clocks prove minute rollover, clock rollback,
  expiry, exact known-zero versus missing usage, bounded retention and identity
  isolation. Latency tests cover last-10 eviction, denominator zero, timeout
  penalty, finite scores and random ties without probabilistic pass criteria.
- Real loopback HTTP usage test: two deployments with different authored token
  totals are selected according to observed totals across several conversions
  using one runtime. A gated concurrent pair completes together and verifies
  both updates survive; no read-modify-write loss or double accounting.
- Real loopback latency test: hold whichever deployment receives the first
  request until its configured HTTP timeout; the eligible alternate succeeds.
  A following conversion prefers the measured alternate. Gate on observed
  requests, not sleeps or benchmark speed thresholds. Selection arithmetic and
  expiry use deterministic unit clocks rather than fragile wall-time assertions.
- Shared cache/coalescing test: only the owner contributes observations and paid
  usage; a waiter and durable cache hit preserve the result with zero new HTTP.
  A paid invalid structured response contributes provider success metrics once,
  while a paid HTTP error affects accounting without a success metric sample.
- Preserve R28 disabled/vision/failed-candidate/fallback coverage and test separate
  runtimes, same model at different credentials/endpoints and explicit IDs.

Completion evidence should state the runtime scope and implemented strategy
semantics. It must not imply a performance gain, global fairness, distributed
quotas, persistent health history or validation against live providers.
