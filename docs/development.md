# Development and recovery

Start at `docs/CONTROL.md`; inspect `git status --short --branch` before work.
Use Rust stable, Node 24+, Go 1.22+, and Python 3.11+ for adapter testing.

Planned standard checks:

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p markitai-cli
```

Tests must set `MARKITAI_HOME` to a private directory under `.local/` or a
temporary directory. Real-provider tests are opt-in. If needed, explicitly
import environment variables from `~/.markitai/.env` into an isolated test
process; never source it as shell code, print its values, copy caches, or write
back. Offline fixtures and localhost mock servers are the default.

Before a checkpoint, inspect the diff, run relevant checks, then stage explicit
paths. A commit marks a recoverable state, not a claim of feature completeness.
Use `git log` and `git show` to recover an individual file into a new file for
comparison. Never discard working tree changes to recover an earlier version.

The original repository is not a build dependency. Any referenced differential
test runner must accept its location explicitly and record the reference SHA.
