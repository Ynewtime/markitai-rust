# Terminal usage in CLI, REST and MCP

This is an implementation plan inspected during round 25, not evidence that these
surfaces already retain failed-call usage. It follows [runtime stage B](after-round24-llm-runtime.md).
The new core detailed entrypoints and binding error fields are a separate change.
No new flag, model request, persistent cache identity or error category is needed.

## Contract and accounting boundary

Keep every existing `error`/`detail` string and exit/status policy. Carry a private
typed failure until the final serializer instead of calling `to_string()` at the
core boundary. A small adapter type can hold `message: String` and
`usage: Option<ConversionUsage>`; core failures preserve their original category
long enough to handle `ImageOnly` and `NoModelConfigured` as today. Validation,
queue cancellation and join/panic errors construct this adapter without usage.

An observation exists when the core reports requests, tokens or model records,
using the same predicate as `convert_json`. A response with one recorded request
and zero tokens remains an observation. Missing or null usage means no accounting
was supplied, not a free operation. `cost_usd` currently lacks provider pricing;
zero dollars cannot establish zero cost. Do not infer usage from elapsed time,
HTTP attempt counts, configured token limits or a reused owner's measurements.

The detailed core wrapper owns its `DocumentScope` through final core publication,
snapshots it once after workers settle, and then restores any prior thread-local
scope. Nested conversions own separate scopes. An ordinary returned error can
retain recorded responses; hard process termination, a worker panic, or an
unreadable provider response does not become a complete billing ledger. Service
timeouts must drain already admitted blocking work before claiming final totals.

Use optional, nullable diagnostics for new data where no existing compatible slot
exists. Omit a new field for old/no-observation records; readers accept both
missing and null. Do not replace string errors with the C ABI's error object.

## Existing seams

| Surface | Current loss point | Smallest integration |
| --- | --- | --- |
| CLI conversion | `app.rs::convert_task` calls the old publication API and turns errors into strings; `recorded` leaves failure usage at its default. | Call `convert_with_publication_detailed`, retain the output claim and context, and copy the failed usage into the actual `RunItem`. Keep image-only skips unchanged. |
| CLI JSON | `outcome` already emits `llm_usage` (per-model map) and `cost_usd`; `error` is a string or null. | Populate those established fields from known failed usage and add optional `diagnostics: {usage: ...}` for the complete observation, including request/token totals. Existing no-observation envelopes remain unchanged. |
| CLI saved reports | Directory records already expose model usage. `report.rs::aggregate`, `list_entry` and `resumed_usage` intentionally exclude failed URL-list items or their measurements. Single-input report eligibility is also restricted. | Preserve established report fields and eligibility. Add an explicitly named terminal-diagnostics section keyed by the existing item identity for known failed observations. Do not silently redefine reference-shaped success totals as lifetime spend. |
| CLI resume | `batch_run.rs::terminal` currently journals only status/output/error. The codec already retains `llm_usage` and `cost_usd` observations. | Record those two supported observations for the completed attempt, validate their finite/nonnegative counters, and project recovered diagnostics without synthesizing requests or timestamps. |
| CLI history | `history/mod.rs::Item::from_record` saves cost but no token/request observation. | Add optional usage diagnostics to newly authored metadata; old history remains readable and read-only. Adapt the server history reader in the same change. |
| REST initial job | `server/jobs.rs::convert_one` calls the old context API. `server/types.rs::Item` has nullable cost, but **no nullable usage field** today. | Use the detailed context API inside the existing blocking worker. Add a serde-defaulted optional diagnostics field, persist it in metadata and emit it in the existing SSE item event. Keep error/status/output unchanged. |
| REST retry/enhance | `server/rerun.rs` loses failures at the core call and at later sidecar, shared-file and transaction checks. `failed()` may restore an entire prior successful item. | Capture the attempt observation immediately after core success or failure. Carry it through all later errors and attach it after restoring prior output metadata. See the attempt rule below. |
| MCP direct tools | `mcp/tools.rs::convert`, `State::convert` and dispatch carry `Result<Value, String>`. The handler renders only an error text block. | Carry the private typed failure through the oneshot channel. Preserve `isError` and the exact text prefix; add structured diagnostics only when an observation exists. Keep the existing no-model configuration hint. |
| MCP batch tools | `mcp/jobs.rs` stores `{source,status,error}` for failed slots and drops diagnostics. | Add optional diagnostics to that same failed slot; `job_status` already returns stored slots in input order. Queued/unstarted cancellation gets no fabricated usage. |

For direct MCP failures, the additive structured value can be
`{"error":"the existing message","diagnostics":{"usage":{...}}}` while the
existing human-readable content remains intact. Do not convert application
failures into JSON-RPC protocol errors. Review tool output schemas when adding
structured error content so success-only required fields are not incorrectly
required of an error result.

## Retries, resume and publication

Usage describes an attempt, not ownership of an output file. REST retry staging
can consume tokens successfully and then fail during shared-asset comparison or
transaction publication. The previous output must stay intact, while the failed
attempt remains visible. A minimal additive REST field is
`diagnostics.last_attempt = {operation, status, error, usage}`: `error` remains a
string/null and `usage` remains optional/null. Attach it after `failed()` restores
the previous item; never overwrite old output authority merely to report spend.
Clear stale attempt diagnostics when a new attempt starts, then publish its own
observation at termination. The old `cost_usd` field continues to describe the
retained output under the existing policy.

Native resume currently treats observed entries as replacements for saved
entries. Preserve that rule: save the latest attempt's observation, and do not
sum a saved failed attempt again when its retried item is observed. Recovered
failed entries may expose their explicit saved observation; missing legacy
measurements stay unknown. This is not a cumulative account across all retries.
A durable lifetime ledger would need separate attempt identities and is outside
this small increment.

Report or history publication may fail after all core conversions succeeded.
Retain the existing successful items and their usage in the CLI JSON envelope;
report the final publication error separately. Do not reclassify every item or
merge its usage a second time. Likewise, REST metadata/output rollback does not
roll back provider work. A process crash before metadata persistence remains an
explicit durability limit; adding this field alone does not make billing durable.

## Implementation order and ownership

1. Coordinator defines one private CLI adapter failure/observation helper and
   the optional diagnostics shape; no public Rust `Error` variant changes.
2. One CLI lane changes `app.rs`, both batch paths, report/resume projection and
   history serialization together. Keep output claims and journal fences intact.
3. A separate service lane changes initial jobs, rerun staging, `Item` metadata,
   legacy adaptation and SSE. Serialize diagnostics through normal publication;
   no provider work occurs while job/store locks are held.
4. An MCP lane updates tools, worker channel, handler and batch slots. Existing
   text, acknowledgement and job-status fields remain compatible.
5. Coordinator runs frozen full gates and retained CLI/REST/MCP acceptance. Keep
   the core/binding checkpoint independently usable if a surface needs more work.

## Required evidence

- Real loopback paid 401, quota response, invalid structured answer and successful
  document processing followed by failed image analysis: correct string error,
  one observation per actual response, no raw key or provider body leakage.
- A known zero-token response remains distinguishable from missing usage; early
  config/input rejection and unstarted cancellation make zero provider requests
  and do not manufacture an observation.
- CLI single, directory and URL-list failures preserve exit codes and item order;
  JSON, new report diagnostics and history agree. Resume reloads explicit saved
  data without adding the old and new attempt together or changing legacy bytes.
- REST first conversion and retry both retain usage. Force a post-core staging
  publication failure: old output bytes/ownership survive, latest attempt usage
  appears in SSE, GET and restarted metadata, and no stale prior usage is reused.
- MCP direct errors keep the text prefix and `isError`; batch failures keep string
  errors and input order. Disconnect/shutdown drains admitted work without
  attributing one document's usage to another or inventing usage for pending slots.
- Two conversions sharing a runtime exercise coalescing: only the HTTP owner is
  charged; a failed owner and a later successful waiter retain separate attempts.
  Transport without parseable usage and panic paths are reported as unknown.

These checks are proposed acceptance work. This planning review ran no CLI,
provider request, conversion, build or performance measurement.
