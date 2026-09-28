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
