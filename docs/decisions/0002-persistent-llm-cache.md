# Persistent document LLM cache

Status: planned, not implemented. Contract reviewed at native source `21bf8f5`
against reference `ba374322f884b0e720b45466cc1196f4574a3da5`.

Repeated document enhancement should reuse a successful result across processes
before spending a model request. The next implementation starts with non-pure
local documents; pure enhancement explicitly bypasses caching. Image, URL,
process-memory and fetch caches require their own contracts.

## Compatibility requirements

- The public cache default is enabled, with a 512 MiB maximum and a `cache.db`
  database in the configured global directory. Resolve the default through the
  native state-path function so an isolated `MARKITAI_HOME` remains effective.
- Persistent LLM entries have no TTL; the existing fetch TTL is unrelated.
- `no_cache` and matching `no_cache_patterns` bypass reads while still refreshing
  successful results. `--cache` clears `no_cache`; it does not force `enabled`.
- The key covers complete document content, resolved prompt templates and rules,
  and a deduplicated, sorted set of positive-weight model names. Credentials,
  endpoints, deployment order and ordinary local filenames do not change that
  model scope. Runtime timestamps must not invalidate otherwise identical work.
- Query configured-model cache entries before creating an HTTP client or requiring
  credentials. A hit consumes no request budget and returns zero new model usage.
- Never cache failures, empty answers or truncated responses. Cache damage or
  unavailability must not discard an otherwise successful conversion.

Use a native cache-version namespace to avoid reading incompatible legacy
structured responses as Markdown. Bundled SQLite is the proposed storage engine:
WAL and transactional admission/eviction provide a bounded shared cache while
keeping the CLI independent of a system SQLite installation. Measure and record
its binary-size cost before accepting the dependency. A single oversized result
must be rejected without evicting existing useful entries.

The reference processor currently omits some public enabled/size wiring, and its
memory lookup can precede the persistent bypass check. These are observed
implementation defects, not intended behavior to reproduce. Respect the public
configuration and document these corrections.

## Integration and validation

An internal enhancement outcome should carry cache-hit state into the existing
CLI `cache_hit` and `llm_cache_hit` fields without adding fields to the bindings'
JSON contract. CLI bypass controls, statistics and clearing need real cache
behavior before their unsupported guards are removed.

Local mock-server checks must demonstrate cross-process reuse with zero second
request; no new usage on a hit; bypass-refresh-reuse; no database when disabled;
content/prompt/model invalidation and filename-independent hits; concurrent
writes; capacity eviction; and successful conversion with a damaged or unwritable
cache. All state stays in test-owned directories.

Cost budgets follow separately. Accurate provider or versioned-price accounting,
including usage from attempts preceding terminal failure, is needed before the
current cost-budget rejection can be removed. Caching does not establish pricing
or authentication compatibility.
