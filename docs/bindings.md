# Native language bindings

All bindings execute the Rust core in the host process. They share a UTF-8 JSON
request and response contract. Neither an installed CLI nor a Python worker is
used for conversion. Feature availability is therefore the same as the core;
an installed binding does not add missing format, OCR, or browser capabilities.

The latest [installed-package validation](validation/runtime-history-round25.md)
targets source `ccf51ba8563a9066b14cf1fdee0aef6b1b7e05b3` on macOS arm64.
Python 3.13, Node 24 and Go 1.27.1 checks pass: 18 installed Python tests,
five installed Node tests and Go source-package race tests. The earlier
[round-seventeen PDF comparison](validation/pdf-native-round17.md) remains its
own recorded evidence. Native media backends are platform-specific; separate
Rosetta testing retains an OCR failure and does not validate physical Intel,
Linux or Windows hosts.

## Shared contract

```json
{"source":"report.md","options":{"llm":false,"config":{}}}
```

`source` is a local path or HTTP(S) URL. Options are `output_dir`, `config`,
`llm`, `ocr`, `screenshot`, `alt`, `desc`, and `profile`. Omitted booleans inherit
configuration; `false` explicitly disables the feature. An explicit empty
`config` uses built-in defaults and bypasses configuration files. An omitted
configuration follows the core's configuration loading rules.

A modern `.numbers` directory package is a single local document, just like its
ZIP form; the extension is case-insensitive. It uses the same bounded IWA table
reader and keeps the package path as `source`. Supplying `output_dir` writes one
`<package-name>.md` file. Ordinary directories still return `is_directory`
(`IsADirectoryError` in Python); the bindings do not recursively convert them.
Legacy XML packages, Numbers OCR and complete worksheet screenshots remain
explicitly unsupported. See [Numbers reader boundaries](numbers.md).

The binding tests unfold two pinned MIT ZIP fixtures into directory packages
and compare their bodies, warnings and output files through Python/Node sync
and async calls and Go typed/JSON calls. `MARKITAI_TEST_NUMBERS_FIXTURES` selects
the fixture directory for installed-package tests; repository tests also have a
local fallback. These are container-equivalence checks, not independently
exported Apple directory-package goldens or canvas-fidelity tests.

Success is `{"ok":true,"result":{...}}`; failure is
`{"ok":false,"error":{"code":"...","message":"..."}}`, with optional
`error.usage` when model responses have already been recorded. The typed adapters
unwrap this envelope into a result or host-language error. Result fields are
`source`, `markdown`, `llm_markdown`, `frontmatter`, `output_path`,
`llm_output_path`, `assets`, `screenshots`, `images`, `usage`, `skip_reason`,
`duration`, and `warnings`. Durations are seconds. In-memory conversions have
null output paths and empty asset/screenshot path lists.

Terminal usage uses the same `cost_usd`, `requests`, `input_tokens`,
`output_tokens` and `by_model` fields as successful conversions. Python errors
raised by the adapter expose `.usage` (`ConversionUsage` or `None`), Node
`ConversionError.usage` is optional, and Go `ConversionError.Usage` is a nullable
pointer. Existing constructors, exception categories, code and message stay
compatible. An absent value differs from a recorded response with zero tokens;
neither establishes zero provider cost. Pricing remains unimplemented.

Rust callers can use `convert_detailed` and the detailed context/publication
entrypoints to receive `ConversionFailure { error, usage }`, including final
publication errors. Existing `convert` variants still return the original
`Error` variants. Scope accounting remains document-local even with a shared
runtime. Native panics retain their existing generic boundary handling and do
not promise detailed usage. CLI/report, REST and MCP terminal diagnostics need
separate propagation and are not covered by this binding contract.

JSON serialization adds copies at the boundary. Benchmark total host-call time
separately from native extraction when evaluating this cost. The protocol is
deliberately shared so compatibility can be checked across every host before
introducing format-specific zero-copy interfaces.

## Python

Python 3.10+ loads the PyO3 extension `markitai._native`. Its ABI3 build can be
packaged for multiple supported CPython versions on the same OS/architecture.
The extension releases the GIL during conversion. `aconvert` dispatches to
Python's thread pool; cancellation stops waiting, but an already running
conversion can finish and write its requested output.

```python
import asyncio
from pathlib import Path
import markitai

out = markitai.convert(Path("report.md"), config={}, llm=False)
print(out.markdown)

async def main():
    out = await markitai.aconvert("report.md", output_dir="out", config={})
    assert isinstance(out.output_path, Path)

asyncio.run(main())
```

`convert` and `aconvert` retain the existing keyword arguments. Results remain
dataclasses, and filesystem result fields are `pathlib.Path`. Calling
`convert` from a running event loop raises `RuntimeError` as in the reference.
`enable_worker_processes` remains an importable compatibility hook; Rust does
not require Python worker processes.

`MarkitaiConfig` and the 26 nested configuration model classes are available
from `markitai.config`. They use Rust's shared schema, defaults, coercions and
construction validation. Model lists contain typed objects; preset and domain
maps remain ordinary dictionaries with typed values:

```python
cfg = markitai.MarkitaiConfig(output={"on_conflict": "overwrite"})
cfg.llm.enabled = False
out = markitai.convert("report.md", config=cfg)
```

Declared field assignment remains unvalidated, as in the original models;
assigning an unknown model attribute raises `ValueError`. Configuration is
validated again on conversion. `model_copy` retains shallow/deep behavior,
and `model_validate` accepts an existing instance without replacing it.
`model_dump` supports JSON/Python modes and common include/exclude,
exclude-unset/defaults/none filters; `model_dump_json` and JSON validation
round-trip these values. `model_json_schema` exports structural types, required
fields, bounds and defaults without the reference project's prose. Environment
resolver methods preserve explicit-reference and fallback behavior.

These classes are not Pydantic `BaseModel` subclasses. Validation raises
`ValueError`, without Pydantic's aggregated `ValidationError` details. Custom
validators, custom schema generators, `extra="allow"`, serializer warning
behavior and Pydantic internals are not reproduced. Advanced combinations of
serialization selectors remain a compatibility testing target. JSON
dictionaries and external objects exposing
`model_dump(mode="json")` also work. The wrapper never mutates supplied config.
Native `fetch_error` maps to `FetchError`; input/configuration errors map to
`ValueError`; conversion/unsupported errors map to `ConversionError` with a
`code` attribute. Specific filesystem and missing-model codes map to the
existing named exception types when supplied by the core.

Build from the repository root in a private environment:

```sh
python3 -m venv .local/python-env
.local/python-env/bin/python -m pip install 'maturin>=1.9,<2'
source .local/python-env/bin/activate
cd bindings/python
maturin develop --release
MARKITAI_HOME=../../.local/test-home python -m unittest discover -s tests -v
maturin build --release --out ../../dist/python
```

The wheel contains the extension and small typed Python wrapper. Building
requires Rust and a compatible Python interpreter; using a built wheel does
not require Rust. Wheel release automation and additional OS/architecture
validation remain release work.

## Node.js

The addon targets Node-API 8 and Node.js 18+. `convert` uses a native async
worker, leaving the JavaScript event loop available. `convertSync` blocks the
calling thread. Concurrency uses the host's libuv worker pool; hosts may set
`UV_THREADPOOL_SIZE` before startup after measuring their workload.

```javascript
const { convert, convertSync, ConversionError } = require('./bindings/node');

const out = await convert('report.md', { config: {}, llm: false });
console.log(out.markdown);
const sync = convertSync('report.md', { config: {}, llm: false });
```

Both functions expose the same snake_case result and option fields as the
shared protocol. Native conversion failures reject/throw `ConversionError`
with a stable `code`. TypeScript declarations ship with the package.

```sh
npm --prefix bindings/node run build
MARKITAI_HOME="$PWD/.local/test-home" npm --prefix bindings/node test
cd bindings/node
npm pack
```

`MARKITAI_BUILD_PROFILE=debug` or `dist` selects another Cargo profile. The
build script copies the compiled dynamic library to `markitai.node`; it is a
build-time tool, never a runtime fallback. A packed package contains a native
addon for the build machine's OS and architecture. Publish platform-specific
artifacts before promising a universal npm install. Node-API compatibility
does not remove operating-system or architecture requirements.

## Go and C ABI

The Go package uses cgo and the C header in `bindings/c/markitai.h`. Build the
native library before compiling Go:

```sh
cargo build --release -p markitai-ffi
cd bindings/go
MARKITAI_HOME="$PWD/../../.local/test-home" go test -race ./...
```

The development module name is `markitai.local/go`; use a local `replace`
directive while the release repository/module path is being decided. The
default cgo linker searches `target/release` and embeds its path as an rpath on
macOS/Linux. Deployment must package `libmarkitai_ffi` and configure the loader
path for the destination. `CGO_LDFLAGS` can supply additional library paths;
`DYLD_LIBRARY_PATH` on macOS or `LD_LIBRARY_PATH` on Linux can select a test
build. A portable static-link recipe and Windows cgo distribution are not yet
validated. Go consumers need cgo enabled and a C linker at build time.

```go
out, err := markitai.Convert("report.md", &markitai.Options{
    Config: map[string]any{},
    LLM: markitai.Bool(false),
})
if err != nil { return err }
fmt.Println(out.Markdown)
```

Go result optional text/path fields are pointers so null and empty string stay
distinct. `ConvertJSON` returns a raw envelope for callers that need the
language-neutral protocol. Go calls can run concurrently. Cancellation is not
currently propagated into native work.

The C ABI is version 1:

1. Call `markitai_convert_json(request, len)` with valid readable bytes that
   remain alive through the call. The input is borrowed, never freed by Rust.
   Requests larger than 64 MiB, malformed JSON, or invalid UTF-8 receive error
   envelopes. Null with zero length is accepted as an empty, invalid request.
2. The returned `MarkitaiBuffer` owns exactly `len` UTF-8 bytes; there is no NUL
   terminator. Copy or consume them before releasing the buffer.
3. Pass the address of that same buffer to `markitai_buffer_free`. It clears
   its pointer and length, making a repeated free on that struct harmless.
   Never copy the ownership handle, change its fields, free it with C `free`,
   or retain a data pointer after release.

Go passes only byte storage, which contains no Go pointers, during the cgo
call. Rust never retains it. Go copies the response into Go-managed memory
and defers native freeing on every return path. Native panics during
conversion are caught at each language boundary. Invalid foreign pointers,
double frees of copied handles, allocation failure, and builds configured to
abort on panic remain outside this recoverable contract.

## Verification and maintenance

`cargo test -p markitai-ffi` exercises null pointers and oversized lengths,
invalid UTF-8, repeated Unicode conversions, and explicit release. Python,
Node, and Go integration suites load compiled native artifacts and cover
Unicode, concurrent repeated calls, output files, and structured failures.
Local HTTP-server tests also verify that Python's GIL and Node's event loop
remain available while native work runs.
Conversion tests use explicit empty config or test-owned config objects.
Terminal-usage tests use loopback HTTP fixtures with fake credentials, including
concurrent paid failures and zero-token responses. They do not contact real model
providers or read user configuration. Old-producer envelopes are tested separately
from actual compiled-native error paths. Run each suite against newly rebuilt
artifacts after core or ABI changes.

Implementation references: [PyO3 function and module interface](https://pyo3.rs/v0.27.2/module.html),
[PyO3 GIL release](https://pyo3.rs/v0.27.2/parallelism.html), and
[NAPI-RS native async tasks](https://napi.rs/docs/concepts/async-task).

## Initial verified checkpoint: 2026-09-28

Platform: macOS arm64. Toolchain: Rust 1.98.1, Python 3.13.15, maturin 1.15.0,
Node.js 24.21.0, Go 1.27.1. Native libraries use the Cargo `release` profile;
the Python wheel additionally targets macOS 11.0 through maturin.

| Adapter | Verification | Result |
|---|---|---|
| Python | Build wheel, install into `.local/bindings-venv`, run `unittest discover -s bindings/python/tests -v` from repository root | 8 passed against the installed wheel |
| Node | `node --test bindings/node/test.cjs` using the rebuilt addon | 3 passed |
| Node package | `npm pack`, local archive install under `.local/node-installed`, Unicode conversion | Passed from installed package |
| Go | `go test -race -count=1 ./...` in `bindings/go` | Passed |
| C library | `otool -L target/release/libmarkitai_ffi.dylib` | Relocatable `@rpath/libmarkitai_ffi.dylib`; only macOS system-library dependencies |

The test wheel is
`.local/bindings-wheels/markitai-1.3.0.dev0-cp310-abi3-macosx_11_0_arm64.whl`:
6,575,897 compressed bytes and 13,433,091 unpacked bytes, including maturin's
SBOM. The test npm archive is
`.local/bindings-packages/markitai-1.3.0-dev.0.tgz`: 6,493,005 compressed bytes
and 13,064,120 unpacked bytes. These are local development artifacts, not
published releases or cross-platform size guarantees. Artifacts and test
environments stay ignored; source and reproducible build commands are tracked.


## Recovery checkpoint: 2026-09-28

The configuration adapter now materializes all 27 native configuration models,
including nested deployments, maps and lists. The newly built wheel was installed
into the same private test environment and passed all 16 tests (eight native API
and eight configuration contracts). Tests imported the installed wheel from the
repository root, not the wrapper source directory.

The rebuilt Node addon passed all three tests. Its new npm archive was installed
under `.local/node-installed-round2`; synchronous and asynchronous Unicode
conversion passed through the installed package. Go passed `go test -race
-count=1 ./...` against the rebuilt native C library.

The source, native schema helper and shipped `_native.pyi` agree on the
configuration protocol. Core and wrapper tests still do not establish full
Pydantic extension-protocol compatibility or cross-platform support.

Recovery artifacts live under `.local/bindings-wheels/round2` and
`.local/bindings-packages/round2`, preserving the initial artifacts. Their sizes
and identities are recorded in `validation/artifacts-round2.json`.

## MSG, raster vision and HTML checkpoint: 2026-09-28

Source `21bf8f5` was rebuilt after the CSS visibility fix. The newly installed
wheel passed 16 tests, the addon passed three, and an independently installed npm
archive passed synchronous/asynchronous Unicode conversion. Go's race tests
passed against the rebuilt C library on macOS 27; its host test did not force an
older deployment target. These checks use the same core as the CLI, including
the new MSG, image and routing paths; they do not individually exercise every
format through every host adapter.

The final wheel and npm archive are preserved in
`.local/bindings-wheels/round3-final` and
`.local/bindings-packages/round3-final`. Their compressed/unpacked sizes are
7,391,308/15,142,519 and 7,292,463/14,735,272 bytes respectively.
[Artifact identities and validation logs](validation/artifacts-round3.json)
record the exact source and hashes. The earlier round-three packages remain
available in their own ignored directories; subsequent builds do not replace
this evidence. Cross-platform release validation remains unfinished.
