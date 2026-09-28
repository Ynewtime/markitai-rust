# Recovery state storage

The Unix CLI connects this store to directory and URL-list dispatch. Every batch
persists merged work and flushes each admitted target before a worker can make
provider requests; `--resume` retains completed entries and retries unfinished
work. Reports remain independently selectable. History export is still pending.
The native conversion API and language bindings do not acquire recovery state.
See [output ownership](output-ownership.md) for the separate publication protocol
and [the control center](CONTROL.md) for the current validation status.

## Files and run scope

A directory or URL-list run uses stable files below its output directory:

```text
.markitai/states/markitai.<six-hex-hash>.state.json
.markitai/states/markitai.<six-hex-hash>.state.jsonl
.markitai/states/markitai.<six-hex-hash>.state.lock
```

Directory state identity uses resolved input/output directories and the selected
feature, scan-depth and glob options. URL-list state uses the list path, output
directory and five feature flags; it is distinct from the URL-list report hash.
The shared Python-compatible MD5 serializer supplies the filename. Six hex digits
are not proof of ownership: the loader validates saved scope and destinations.

The store holds an OS exclusive lock on the open lock file for its lifetime.
It never removes that inode to release the lock. This excludes cooperating
writers of one checkpoint, including processes that start a fresh run. It does
not reserve document output names across different hashes or overlapping roots.
Separate [member leases and receipts](output-ownership.md) protect document names across those run scopes.

Original output path spelling is retained privately for symlink-policy checks,
while the serialized scope contains resolved identities. Native checkpoint
publication anchors scope options, URL provenance and saved destinations to
absolute paths before durable encoding, so changing cwd cannot reinterpret them.
New journal mutations also store validated absolute destinations. The codec interprets
old relative destinations against the process cwd. It checks resolved containment,
mirrored parents and the metadata boundary independently of serialized path
spelling. It does not turn an edited path into permission to overwrite a document.
The existing core path policy is reused; hostile parent-directory replacement
races are not closed by these path checks.

## Legacy codec and ordered data

Legacy snapshots retain the `"1.0"` envelope with `options`, `documents` and
`urls`. Documents serialize by key; URLs preserve encounter order. State options
put the reference's preferred fields first and retain remaining top-level option
order. These use local ordered maps without changing global binding JSON order.

A completed entry keeps optional output; a failed entry keeps optional error;
unfinished entries keep their reserved target. URL records retain `source_file`
and named entries retain the underlying URL. Missing URL provenance and explicit
null stay distinct when serialized. Detailed legacy measurements are retained
as optional in-memory observations; missing values are not fabricated as newly
measured usage or time. New snapshots remain minimal.

Merging preserves stored entries and adds discoveries. Legacy bare-URL adoption
moves one old entry to the first eligible named entry in explicit discovery
order, only when the current list has no unnamed claimant. Raw names such as
`name` and `name.md` remain separate identities. Merged keys must be checkpointed
before the store will accept their first event.

Legacy JSONL lines update only known file/URL keys. Missing status means pending;
missing output/error/target preserves its old value, while explicit null clears
it. Malformed JSON syntax is skipped with a bounded diagnostic. Semantic errors
or invalid UTF-8 stop further replay and retain the valid prefix. Only after all
replay does an in-progress entry become failed for retry. Missing/corrupt base
state cannot be reconstructed solely from a sidecar.

## Native generation and sequence

A native snapshot adds `_markitai` metadata containing a UUID generation, applied
sequence and full run scope. Native events keep `type,key,data` and add their
generation and sequence. A fresh run gets a new generation; resumed native state
retains the loaded generation. Legacy state is upgraded before new events.

`record` validates and buffers one known-item mutation and returns its sequence.
That return value is not a durable claim. `flush` appends pending events and syncs
the journal and its directory before returning the durable sequence. The scheduler waits for this acknowledgment before starting work that relies on
recovery, including provider requests. A dropped writer does not pretend to flush.

Compaction writes the complete snapshot through sequence N to a same-directory
temporary file, syncs it, replaces the base, syncs the directory, then removes
the old journal and syncs that directory change. Replay ignores other generations
and events already included through N. Remaining events must advance in order;
unknown native keys, gaps, duplicates and reversal stop replay with a diagnostic.
A native tagged base does not replay leftover untagged legacy events.

This fence prevents a stale `in_progress` event from undoing a completed snapshot
if the process dies after base replacement but before journal removal. The Python
reader can ignore added fields but does not understand the fence; it does not
inherit the native crash guarantees. No protocol can promise exactly-once paid
requests across a process kill between a provider response and durable completion.

## Limits and failures

Default limits are 10 MiB per base, 64 MiB per journal, 1 MiB per event line and
100,000 combined document/URL entries. Files are bounded before unrestricted
reads; serialization uses a bounded writer. Journal capacity triggers compaction,
and an oversized snapshot fails instead of becoming unreadable on the next run.
Diagnostics do not dump raw state lines, provider configuration or document data.

New native base and journal files use private permissions on Unix. Before
replacing a corrupt base, the store preserves both base and sidecar in
unique same-directory quarantine files, syncs them and retains private file
permissions. Oversized quarantine copies fail while leaving originals in place.
Foreign scope is an error, not corruption to overwrite as a fresh run. I/O failure
poisons the writer; callers must reopen and recover rather than append after an
uncertain partial write. Temporary staging is cleaned up when publication fails.

Directory synchronization is implemented for Unix. Other platforms still require
explicit durability and lock validation. The tests do not substitute for those
release gates or for scheduler interruption tests.

## Validation and next stage

The [validation record](validation/state-round8.md) records source gates,
reference comparison and limits. Ownership is recorded in the
[control center](CONTROL.md). The authored legacy-state differential harness uses
an explicit ignored test-binary entrypoint; no production CLI test switch is
added. It calls the actual reference state codec and journal loader under private-state guards and
compares the native codec's values, types, ordering and task hashes. It exercises
state data, not conversion, paid requests or a working `--resume` command.

The [persistence decision](decisions/0004-run-persistence.md) separates storage,
scheduling and history. The CLI now keeps recovered entries separate from
observed invocation outcomes, applies the configured flush interval and drains
active work after a first SIGINT/SIGTERM. A second signal exits immediately.
Interrupted runs do not publish a report and return 130/143; their stdout JSON
contains observed results with an interruption error. Real-process acceptance
uses local HTTP/model gates and request counters. The storage-only round-eight
evidence above does not establish these new integration claims; current gate
results are recorded in the [round-nine validation](validation/recovery-round9.md).
