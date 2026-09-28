# Markitai Rust engineering protocol

Read `docs/CONTROL.md` before changing this repository. It is the single work
queue and handoff record. The Python repository is a read-only behavioral
reference; implementation, comments, and documentation here are authored anew.

## Coordination and recovery

- The coordinator owns Git mutations, workspace manifests, integration builds,
  and `docs/CONTROL.md`. Workers edit only their assigned paths.
- Never use destructive reset, clean, stash, force-push, or branch switching to
  resolve concurrent edits. Inspect and preserve unexpected changes.
- Commit explicit paths after verification. Record verified commit checkpoints
  and remaining failures in the control document. Never claim feature parity
  on the strength of compilation or a smoke test.
- Never print, commit, or replace user credentials. Test state lives in ignored
  `.local/`; `MARKITAI_HOME` must isolate configuration, caches, and history.
- Core code stays runtime-independent of Python/Node/Go. Bindings call Rust in
  process. Unsupported capabilities return explicit errors until implemented.
- Document public interface differences and measured performance with commands,
  fixture provenance, build profile, platform, and limitations.

## Versioning

The reference release is 1.2.0. The rewrite targets 1.3.0; development builds use
1.3.0-dev. Changelogs use the next numbered release, without a date until
release, and English/Chinese sections must stay synchronized.
