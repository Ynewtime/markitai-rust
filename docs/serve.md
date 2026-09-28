# Native REST service

`markitai serve --host 127.0.0.1 --port 3600 --no-open` runs an HTTP API inside
the Rust binary. Conversion calls the same Rust core as the native bindings;
it does not launch the CLI, Python, or a conversion subprocess. This delivery
covers the job and history workflow. An interactive web UI, settings/provider
administration and workspace management are not implemented by this service.

## Job workflow

1. `GET /api/capabilities` returns version, effective LLM configuration, presets,
   SVG/browser availability, and the 1,000-item limit.
2. `POST /api/jobs` accepts form fields `urls` (a JSON array) and `options` (a JSON
   object), plus repeated multipart `files`. URL-only requests also accept
   `application/x-www-form-urlencoded`. A successful submission returns
   HTTP 201 with `job_id` and ordered `{item_id,name,kind}` entries. File entries
   precede URL entries. Upload names are sanitized and made unique, including
   case-folded collisions. Base and enhanced Markdown names are reserved as a
   pair, so an upload named `notes.llm` cannot overwrite another item's enhanced
   result.
3. `GET /api/jobs/{job_id}` returns the current snapshot. Items move from
   `queued` to `running` to `done` or `error`; failed conversions remain in the
   job with their error and nullable output. A job containing failed items
   still completes with `status: "done"` and separate success/failure counts.
4. `GET /api/jobs/{job_id}/events` streams a first `snapshot`, subsequent `item`
   and `job` events, and idle keepalives. A terminal job event closes the stream.
   Slow subscribers receive a fresh snapshot after a bounded event-buffer
   overrun.
5. `GET /api/jobs/{job_id}/items/{item_id}/result` returns `name`, selected
   `variant`, `markdown`, and downloadable `{relpath,size}` artifacts. Fetch an
   artifact through `GET /api/jobs/{job_id}/files/{relpath}`. Download all output
   files through `GET /api/jobs/{job_id}/archive` after completion.

The `options` object supports the existing tri-state preset, LLM, OCR, profile,
image description/alt text, screenshot, pure, cache, compression, strategy, and
backend fields. Unsupported conversion features remain explicit core errors.
Requested LLM processing uses core environment/model resolution, including
`MODEL`; the server does not silently disable LLM because `model_list` is empty.
Unlike the reference service's no-model fallback, an unavailable requested model
follows the core's configured failure policy. Remote consent cannot request
interactive terminal input. When LLM is enabled, base output is retained.

## Retry, enhancement and item deletion

`POST /api/jobs/{job_id}/items/{item_id}/retry` returns HTTP 202 with the original
job ID and the one queued `{item_id,name,kind}` entry. Its optional JSON body is:

```json
{"operation":"retry","options":{"llm":false}}
```

`operation` accepts `retry` (default) or `enhance`. An empty body, JSON null, or
omitted/null `options` inherits the item's last conversion options, falling back
to the job's options for older histories. A supplied options object replaces those
options; omitted fields then use the server configuration rather than the item's
previous overrides. Unknown fields and invalid options return 422.

Both operations reconvert the original retained upload or refetch the original
URL. Enhancement is an explicit one-off attempt: effective options must enable a
routable LLM, and success must contain an actual enhanced Markdown output. It does
not silently enable LLM, and a fallback base conversion is not reported as a
successful enhancement. Enhancement does not replace the saved options for future
ordinary retries. Ordinary retry updates both per-item options and the job's latest
options; sibling items retain their own options.

Only `done` and `error` items can be queued. A duplicate pending retry returns 409;
a missing original upload returns 404. CLI-recorded file entries without retained
uploads remain nonretryable (409). Original URLs pass the same trusted-peer and
HTTP/HTTPS checks used for new jobs. Retrying a terminal item while an initial
sibling still runs is supported. Retries run in admission order within their job,
share the job's LLM runtime, and use the service's file/URL concurrency limits.
Only the final active conversion or retry can publish terminal job completion.

A retry writes to a private temporary output directory. Publication validates the
item's reserved Markdown pair and preserves any asset another item claims. A
failed rerun of a previous successful, non-skipped item restores its previous
public result, including operation, warnings, cost and timing. Its actual Markdown
and assets remain unchanged. Success reuses the same output names; a successful
plain retry removes a stale enhanced variant. Old unreferenced extracted assets
may remain in the job archive until item/job cleanup; they are not fabricated into
the new result's artifact list.

`DELETE /api/jobs/{job_id}/items/{item_id}` returns 204 for a terminal job. It removes
that ledger row, its retained upload, Markdown pair and owned assets/screenshots,
while keeping files claimed by another item. Image metadata rows for assets actually
removed are pruned in the same recoverable transaction; shared rows remain. The last item's deletion removes the
whole job. Deleting while any job work runs returns 409; repeated deletion returns
404. Native ownership indexes are preferred; legacy histories use exact Markdown
references and bounded converter filename suffixes. Arbitrary filename substrings
do not grant ownership.

## History and persistence

Jobs live below `MARKITAI_HOME/serve/jobs/<12-hex-id>/`, containing private
`uploads/`, `out/`, and an atomically replaced `meta.json`. Job, upload and output root directories use mode
0700 and generated metadata/uploads use mode 0600 on Unix. Current server
metadata uses the existing version-2 shape, with additive native output-base and
asset indexes, per-item options, and internal file-transaction commit identifiers. Existing CLI and reference terminal histories are readable. Legacy artifact lists
recover assets referenced in Markdown, including visible `assets/` profiles;
assets without references and without a native item index are still downloadable
by path and included in the ZIP, but are not guessed into an item artifact list.
`GET /api/history` refreshes jobs added by other local processes and returns the
existing summaries in reverse creation order. `GET /api/history/archive` streams
a ZIP of completed histories. `DELETE /api/history/{job_id}` removes a terminal
job and its files; running jobs return 409. This implementation does not apply
an automatic age-based deletion policy.

Metadata is flushed and atomically published before reporting durable terminal
completion. If inventory or final metadata publication fails, the live snapshot
and terminal event use `status: "error"` with `persistence_error`. This is an
explicit extension to the ordinary reference job schema. Already completed
item outputs remain downloadable during that process, the job is excluded from
saved-history listings, and shutdown returns failure. Incomplete jobs are not
automatically rerun after restart.

Rerun and item-deletion publication has a durable undo journal beneath the private
job directory. Previous regular files are streamed into bounded backups; a prepared
journal is synced before replacements or removals. The corresponding transaction
identifier in atomically published metadata commits those bytes. On restart,
uncommitted transactions restore the previous files in reverse publication order;
committed transactions keep the new files and discard their backup. This protects
the last persisted result across publication-before-metadata interruption. Journals
reject traversal/symlinks and bound backups at 5 GiB, 100,000 members and 16 MiB of
metadata. A failed rollback is reported as a persistence error; new retry admission
is rejected until restart recovery, rather than claiming the old result is intact.
These are local private-state integrity measures, not authentication
against a local actor able to forge both metadata and recovery files. A full process
or machine failure during filesystem recovery can require another recovery attempt.

SIGINT and SIGTERM stop admission and queued conversion dispatch, close event
streams, and drain active blocking conversions before writing final metadata and
exiting. Active native work is not forcibly interrupted. Pending items record a
shutdown cancellation error. Queued reruns instead restore a prior successful result
when one exists; active retries drain before the service exits. A second forceful process termination can still
leave an incomplete job.

## Network and file boundaries

Loopback peers are trusted according to their actual socket address. Other API
clients require the startup token through `Authorization: Bearer …` or `?token=`;
`MARKITAI_SERVE_TOKEN` can supply that token. Forwarding headers do not grant
trust. `--no-auth` disables the token requirement, but unauthenticated nonloopback
URL conversion is explicitly rejected: a full DNS/redirect-aware public-network
fetch policy has not been implemented. Such clients may still upload files and
access/delete history, so expose this mode only as intended by its operator.

Host validation accepts localhost, IP literals, and explicit `--allowed-host`
entries. State-changing requests with an Origin must satisfy the origin policy.
File paths reject traversal, absolute paths, and symlink components. Downloads
use attachment responses with `nosniff`. HTML and SVG artifacts remain untrusted
content; downloading them does not execute them on the server.

Uploads are bounded at 100 MiB per file, 1,000 items, and 5 GiB plus 64 MiB per
request. Text form fields are limited to 1 MiB. JSON Markdown results are limited
to 64 MiB; larger output remains available through file downloads. Files and ZIPs
stream in bounded chunks. Every archive request builds its own private temporary
ZIP, retained until the response completes or disconnects. It never rewrites a
shared job ZIP, so concurrent downloads do not invalidate each other. Internal
`.images.lock` files are excluded from downloads and ZIPs. Rerun publication rebases
image-description metadata paths from its staging directory to the final job output
and merges sibling entries while holding the same stable lock as the core writer.

The native build currently provides an API landing response instead of opening
an unimplemented UI. `--no-open` remains accepted; without it the startup message
explains that an interactive UI is not bundled.

## Validation scope

The independent Unix CLI process suite in
[`tests/serve.rs`](../crates/markitai-cli/tests/serve.rs) uses private configuration
and `MARKITAI_HOME`, authored text/EML fixtures, and loopback HTTP gates. It covers
submission and name collisions, public response types, SSE, downloads, concurrent
ZIPs, source-upload removal, restart and late CLI history import, malformed form
rollback, host/origin/path protection, shutdown queue cancellation, and explicit
metadata-publication failure. Additional cases in
[`tests/serve/rerun.rs`](../crates/markitai-cli/tests/serve/rerun.rs) cover per-item option
inheritance/replacement, enhancement and failure preservation, sibling overlap,
queued cancellation, shared-asset deletion, and retry metadata failure followed by
restart. Module tests exercise committed/uncommitted file recovery. Separate synthetic-peer router tests exercise
remote token and trust decisions without relying on a host network interface.
These scoped checks do not establish complete REST/UI compatibility, production
load limits, remote-provider behavior, or cross-platform acceptance.
