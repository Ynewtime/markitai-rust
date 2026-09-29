# OpenAI Codex model data attribution

The embedded restricted model catalog derives from OpenAI Codex rust-v0.159.0,
licensed under Apache License 2.0. LICENSE contains the complete original terms
and copyright notice. NOTICE retains the original upstream notice verbatim,
including its project-level Ratatui attribution; no Ratatui code is copied by this
catalog-only adaptation. models.json is the exact modified data compiled into the
adapter and is provided for offline source identification.

Changes: retain only the gpt-5.5 model entry and set its apply_patch_tool_type
field to null. Other fields of that selected entry are unchanged. No official
executable or authentication implementation is modified or redistributed here.
The installed official runtime is a separate user-provided dependency.

provenance.json identifies the original catalog, upstream archive, selected
model, deliberate capability change and exact packaged/compiled data hashes.
This notice does not establish model entitlement, a live subscription test or a
complete legal review of the surrounding dependency bundle.
