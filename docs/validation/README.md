# Validation records

An implementation checkpoint is reproducible only when it records the command,
source revision, test inputs, build profile, and platform. Passing new unit
tests establishes specific contracts; it does not imply reference parity.

## Local checks

Run `scripts/check.sh` for Rust formatting, workspace tests, lint, and audit
runner tests. Run each native binding's integration tests against built
artifacts as described in [bindings](../bindings.md). These use local fixtures
and HTTP mock servers; real-provider tests are separate and opt-in.

## Differential audit

```sh
python3 scripts/audit_formats.py \
  --reference /path/to/reference-markitai \
  --library target/release/libmarkitai_ffi.dylib \
  --output .local/audits/first-run
```

On Linux select `.so`; on Windows select `.dll`. The reference must have its
own installed `.venv`, or pass `--reference-python`. The runner snapshots the
native library and compares Markdown, non-clock metadata, and extracted asset
hashes. It includes unimplemented source formats in the denominator. Token
recall is diagnostic, never a substitute for the exact-content checks.

## Recorded format gates

- [Initial baseline](formats-baseline.md): 0/24 exact passes.
- [Intermediate recovery](formats-recovery.md): 13/24 exact passes.
- [Second recovery](formats-recovery-r4.md): 15/24 exact passes, with all remaining differences listed.
- [MSG/HTML recovery](formats-recovery-r5.md): 17/24 exact passes; four differences and three expected image-only rejections.
- [Final round-three format check](formats-recovery-r6.md): the same 24 case outcomes after the CSS visibility fix.
- [Round-four format check](formats-recovery-r7.md): 17/24 exact passes; PPTX text recovered, image differences retained.
- [First round-five format check](formats-recovery-r8.md): historical 979d205 snapshot, unchanged 24 outcomes.
- [Final round-five format check](formats-recovery-r9.md): 17/24 exact passes; all r8 outcomes retained.
- [Full HTML corpus](html-corpus.md): 43/209 exact local API passes, 166 differences and zero conversion errors.

Each record identifies a frozen artifact; later fixes do not retroactively change
its measurements. Full JSON reports retain the complete fixture denominator.

## Initial CLI benchmark

```sh
python3 scripts/benchmark_cli.py \
  --reference /path/to/reference-markitai \
  --binary target/release/markitai \
  --output .local/benchmarks/cli.json
```

Each sample starts a fresh process with explicit isolated configuration.
The filesystem cache is warm. Output differences are recorded; faster timing
with different output cannot establish equivalent-quality performance. Broad
format throughput, cold caches, peak RSS, repeated host calls, and platform
distribution matrices require additional measured records.

The updated CLI harness disables the reference's dotenv autoload, alternates
engine order, checks every stdout against its warmup and withholds time ratios
when conversion output differs. It preserves earlier measurements in separate
files. This isolates the exercised configuration paths; it is not an OS sandbox.

## Round-three evidence

- [Artifact identities](artifacts-round3.json) for the CLI, C library, installed
  Python wheel and npm archive at source `21bf8f5`.
- [CLI measurements](performance-round3.md), including exact-output gates and
  current isolation limitations.
- [Repeated API measurement method](api-benchmark-method.md) and
  [results](performance-api-round3.md). The native C ABI uses a pre-encoded
  request; these are not measurements of the new Python binding's full call cost.

## Round-four evidence

- [Source, rebuilt artifacts and performance](performance-round4.md): 189 Rust
  test executions, installed host packages, exact-output CLI/C-ABI measurements
  and explicit boundaries; source `5280fd3`.
- [Artifact identities](artifacts-round4.json), [CLI samples](cli-round4.json)
  and [C-ABI samples](api-round4.json) are unchanged copies of the local records.
- Persistent document caching is tested with local request counters. At this
  historical checkpoint, page caching was still planned.

## Round-five evidence

- [Source, artifacts and performance](performance-round5.md): 240 Rust executions,
  installed bindings, synthetic CLI/C-ABI measurements and explicit limitations;
  frozen source `0022e09`.
- [Artifact identities](artifacts-round5-final.json),
  [CLI samples](cli-round5-final.json) and [C-ABI samples](api-round5-final.json)
  are unchanged copies of the original records.
- [Static-page fetching](../fetch.md) documents conditional requests, TTL,
  bypass-refresh behavior and admission boundaries. Cache request avoidance is
  established with loopback counters, not a provider latency/cost benchmark.
- [Independent HTML review](html-review-round5-final.json) passes 11 cases;
  [full-corpus evidence](html-corpus.md) retains the shared-reference whitespace
  defect as an intentional exact-output difference after the native fix.
- The [first round-five checkpoint](round5-first.md), artifacts and measurements
  remain historical evidence. Its extra parity pass was not evidence of better
  extraction quality; later review found and corrected two structural defects.
- The [performance plan](../performance-plan.md) covers profile selection,
  complete binding costs, retained binary code/data and sustained memory checks.

## Round-six build-profile evidence

- [Profile tradeoff and reproduction](profile-round6.md): the existing dist CLI
  is 39.19% smaller, but measured PDF/PPTX C-ABI conversions take 6.58×/8.22× as
  long. Release remains the default. Builds use the same clean `e0cf110` source.
- [Format equivalence](profile-formats-round6.md) and
  [HTML/full-envelope equivalence](profile-html-round6.md) preserve the existing
  reference gaps while verifying 233 complete native response pairs.
- [Raw measurements](profile-measurements-round6.json) retain all CLI samples,
  worker medians, RSS peaks, input identities and isolation observations.
  Whole host-wrapper costs and long-lived memory behavior remain open.
- Eighteen Python harness tests pass, including 14 new regression tests against
  damaged or incomplete comparison evidence. These are harness checks, separate
  from the frozen artifact's Rust tests and native conversion gates.
- [Full-LTO speed-optimized follow-up](profile-fat-speed.md): dist-opt3 at clean
  `ab6b141` is a 6.14%-smaller CLI candidate with broadly similar measured timings
  on six inputs. New paired measurements and all equivalence gates are recorded;
  candidate package validation remains open and the default is unchanged.
