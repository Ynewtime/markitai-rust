# Markitai

A Rust document-to-Markdown engine, shared by a standalone CLI and native
Node.js, Python, and Go bindings. This repository is an active rewrite; see
[the project control center](docs/CONTROL.md) for verified capabilities and
remaining compatibility work.

Documentation starts at [docs/index.md](docs/index.md).

```sh
cargo build --release -p markitai-cli
target/release/markitai document.docx -o out/
target/release/markitai https://example.com --no-remote-fetch -o out/
target/release/markitai serve
```

Use `MARKITAI_HOME` to select a private configuration/state directory. During
development, set it to `$PWD/.local/test-home`. Read the [validation records](docs/validation/README.md)
before relying on this development build for reference-compatible output.
