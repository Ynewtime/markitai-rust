# Restricted Codex model catalog

`models.json` contains the `gpt-5.5` entry from the official OpenAI Codex
`rust-v0.159.0` catalog. The only selected-entry change is
`apply_patch_tool_type: null`, which disables the patch tool. Other entries are
omitted to reject unvalidated models. The source is Apache-2.0; its license is
in `LICENSE.codex`. Distribution copies the full license and original upstream
NOTICE, modification notice and exact catalog into `licenses/codex/`. This is data, not a fork or altered official executable.

Source: https://github.com/openai/codex/blob/rust-v0.159.0/codex-rs/models-manager/models.json
Original catalog SHA256: a5e25107506f0934cb62144c093c72bf8c7fa5de1436f4aa02fe54769d789618

An actual official Linux amd64 exec probe with the full original catalog and
this one field changed produced an empty `tools` array and no `additional_tools`.
A second actual official exec probe used this selected-entry-only packaged
file, a custom model_instructions_file and two authored PNGs. It observed the
exact system text, original image bytes/order, empty tools and no additional_tools.
Both probes used an isolated guest user and local fake Responses only; neither
proves live ChatGPT authentication, model availability or subscription inference.
