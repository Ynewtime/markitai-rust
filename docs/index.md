# Markitai Rust documentation

- [Control center](CONTROL.md): scope, ownership, acceptance, checkpoints.
- [Architecture](architecture.md): runtime and dependency boundaries.
- [Compatibility](compatibility.md): contracts against the reference release.
- [Development](development.md): build, test isolation, and recovery.
- [Formats](formats.md): native readers and fidelity gaps.
- [Images](images.md): raster inputs, shared compression and resource boundaries.
- [HTML](html.md) and [MSG](msg.md): extraction semantics and known gaps.
- [LLM](llm.md): model routing, prompts, retries and provider requests.
- [Persistent cache](cache.md): document reuse, bypass semantics, storage and CLI management.
- [URL fetching](fetch.md): static page caching, validators and response boundaries.
- [Markup](markup.md): RST, Org and TeX reader behavior and limits.
- [Configuration](configuration.md): normalization, file selection, and isolated state.
- [Output](output.md): ordinary/pure content and metadata assembly.
- [Reports](reports.md): four CLI report formats, publication and validation limits.
- [CLI](cli.md) and [bindings](bindings.md): current user interfaces.
- [Validation](validation/README.md): differential checks and measured evidence.
- [Performance plan](performance-plan.md): profile tradeoffs, binding costs and long-lived memory checks.
- Decisions: [native core](decisions/0001-native-core.md),
  [document LLM cache](decisions/0002-persistent-llm-cache.md),
  [static-page fetch cache](decisions/0003-persistent-fetch-cache.md), and
  [run reports and planned recovery/history](decisions/0004-run-persistence.md).

Documentation describes verified behavior separately from planned behavior.
