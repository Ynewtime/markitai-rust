# Persistent LLM cache

The native runtime can reuse a successful non-pure local document enhancement
across invocations and processes. A hit returns the saved Markdown without an
HTTP request, retry delay, request-budget charge or new token/cost usage. The
normal output pipeline still applies current metadata, profiles and output paths.
Pure enhancement, standalone image vision, URL enhancement and fetched pages do
not use this cache. There is no process-memory cache in this implementation.

## Configuration and CLI behavior

| Setting | Default | Effect |
|---|---|---|
| `cache.enabled` | `true` | Enables reads and writes for eligible document enhancement |
| `cache.no_cache` | `false` | Bypasses reads but saves a successful fresh answer |
| `cache.no_cache_patterns` | `[]` | Matching source contexts bypass reads but still refresh answers |
| `cache.global_dir` | `~/.markitai` | Directory containing `cache.db` |
| `cache.max_size_bytes` | `536870912` | Maximum sum of serialized cached-answer bytes |

The default state path honors `MARKITAI_HOME`. An explicitly configured custom
path keeps its ordinary meaning. Development and tests must select a private
state directory; they do not need to read an existing user's cache or credentials.
Disabled caching never opens or creates the database during conversion. Cache
inspection remains available when caching is disabled.

`--no-cache` sets `cache.no_cache`; `--cache` clears it without overriding
`cache.enabled`. The final occurrence wins. `--no-cache-for` supplies the existing
comma-separated pattern list. Patterns match the full source context or its
basename; Windows separators are normalized and `**/` can match zero directories.
The original context is used only for matching and is not persisted. These flags
currently affect eligible LLM document calls; URL fetching has no native cache.

Persistent LLM entries have no TTL. `cache.fetch_ttl_seconds` is a separate URL
fetch setting and does not expire document answers. The reference processor did
not forward every public enabled/capacity setting, and its memory lookup could
precede bypass controls. Native conversion honors these controls consistently.

## Cache identity

A versioned native namespace prevents old structured-response entries from being
interpreted as Markdown. The key includes:

- A SHA-256 digest of the complete input Markdown, including edits in its middle.
- Resolved system/user prompt templates, prompt category and applicable mode rules.
- The deduplicated, sorted set of model identifiers whose configured weight is
  positive. Automatic model selection uses the same model names as routing.

Prompt templates are digested before substituting timestamps or source labels.
An ordinary file rename, pool reordering, duplicate deployment, API-key rotation
or endpoint change does not invalidate an otherwise matching entry. Changing the
content, resolved prompt text or eligible model set does. Disabled models do not
participate. This follows the reference's model-pool scope rather than the model
chosen by one weighted request; an answer from any member can satisfy the pool.
Provider credentials and endpoints are not stored in cache identity or metadata.

A configured-model lookup inside the enhancement function runs before that
function resolves credentials, reads dotenv or constructs an HTTP client, so an existing answer remains usable when that
configuration's API key is temporarily unavailable. Automatic model selection
still needs the environment to identify its pool. Prompt files must remain
readable because their actual text determines whether an answer is reusable.
The normal CLI may already have read dotenv during configuration loading; this
lookup ordering does not promise that the complete CLI invocation avoids it.

## Storage and failure behavior

SQLite is bundled into the native artifact. It uses WAL, normal synchronization
and a 30-second busy timeout. The table retains the reference fields `key`,
`value`, `model`, `created_at`, `accessed_at` and `size_bytes`; `value` is a JSON
string containing Markdown and `model` records the pool fingerprint. The database
is created only when there is a successful answer to save.

Writes admit the replacement and evict the least recently accessed entries in
one immediate transaction. Ties use row insertion order. A result larger than the
configured capacity removes a stale entry for its own key but does not evict
other answers. An insert failure rolls back both replacement and eviction.
Empty, whitespace-only, failed and token-truncated answers are not stored.
Stored answers also have a 100 MiB entry bound. Separate processes can safely
write the same database; simultaneous cache misses can still issue duplicate
provider requests because there is no request coalescing.

Capacity counts serialized answer bytes, matching the reference accounting; it
is not a hard limit on SQLite metadata, free pages or WAL file size. Reducing the
capacity takes effect on subsequent admitted writes. Clearing entries does not
promise to shrink the physical database file immediately.

Unavailable, malformed or unwritable caches produce a fixed warning and leave a
successful enhancement intact. Errors do not expose the database path, SQL,
credentials or cached content. Reads update `accessed_at`; if the database cannot
accept that update, the call degrades to a cache miss and may contact the model.
Blank or structurally invalid stored answers are not replayed as successful
results. Diagnostics describe cache unavailability rather than implying a free
provider request.

## Inspection

`cache stats --json` retains the existing `cache`, `enabled` and `fetch_cache`
envelope. An absent LLM database yields `cache: null` without creating a database.
An unreadable one yields a sanitized `cache.error` and the CLI reports failure.
Normal details include entry count, logical bytes/MiB, configured capacity/MiB
and the database path. Verbose details add model-group statistics and recent
entries, including timestamps and short content previews. The recent-entry limit
is capped at 1,000; counts and model groups use one consistent read snapshot.
Previews are intentional user-requested content display, not error diagnostics.

The storage clear operation deletes LLM table entries transactionally and returns
the deleted count; an absent database returns zero without creating it. The CLI
checks unsupported fetch-cache/SPA operations before clearing anything. An
existing `fetch_cache.db` is reported as unsupported by statistics instead of
being presented as an empty cache. The native implementation never silently
clears only one part of a requested combined cache operation.

A cache hit reaches the existing CLI `cache_hit` and `llm_cache_hit` fields through
internal conversion state. It does not add fields to the JSON conversion protocol
used by Node.js, Python or Go bindings.

## Verification

Storage tests use temporary directories and cover missing/disabled databases,
key invalidation, bypass patterns, refresh/reopen, verbose statistics, clearing,
LRU eviction, oversized replacement, transactional rollback, concurrent writers
and malformed cache handling. Local HTTP mocks cover lookup before credential
resolution, filename-independent reuse, zero new usage, bypass-refresh-reuse,
content/prompt/model changes, disabled/pure/URL bypass, damaged and blocked cache
paths, and both OpenAI and Anthropic truncation signals. CLI integration tests
exercise independent child processes to verify persistence and report fields.
No provider account or live network call is required. Executed checks and artifact
size changes belong in the project's validation and operations records.
