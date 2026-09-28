# Markitai Rust documentation

- [Control center](CONTROL.md): scope, ownership, acceptance, checkpoints.
- [Architecture](architecture.md): runtime and dependency boundaries.
- [Compatibility](compatibility.md): contracts against the reference release.
- [Development](development.md): build, test isolation, and recovery.
- [Formats](formats.md): native readers and fidelity gaps.
- [Numbers](numbers.md): bounded native table decoding and saved-value limits.
- [Office page rendering](office-rendering.md): isolated optional LibreOffice export, full-page capture and OCR supplements.
- [Local image OCR](ocr.md): native macOS recognition, language selection and limits.
- [Browser fetching and screenshots](browser.md): isolated optional Chromium and CDP.
- [Images](images.md): raster/SVG inputs, shared compression and resource boundaries.
- [HTML](html.md) and [MSG](msg.md): extraction semantics and known gaps.
- [EML](eml.md): MIME body selection, scoped CID images and attachment downloads.
- [HTML code blocks](html-code.md) and [article boundaries](html-article.md): code fidelity and structural content selection.
- [Streamed HTML](html-stream.md): React transport and script-stripped article snapshots.
- [PDF layout](pdf.md): positioned text, paragraphs, headings and ruled tables.
- [PDF rendering](pdf-rendering.md) and [page OCR](pdf-ocr.md): native page pixels, recognition, screenshots and model routing.
- [LLM](llm.md): model routing, prompts, retries and provider requests.
- [Persistent cache](cache.md): document reuse, bypass semantics, storage and CLI management.
- [URL fetching](fetch.md): static page caching, validators and response boundaries.
- [Markup](markup.md): RST, Org and TeX reader behavior and limits.
- [Configuration](configuration.md): normalization, file selection, and isolated state.
- [Output](output.md): ordinary/pure content and metadata assembly.
- [Reports](reports.md): four CLI report formats, publication and validation limits.
- [Output ownership](output-ownership.md): member leases, prepared receipts and safe retries.
- [Recovery storage](state-storage.md): checkpoint codec, journal, replay and durability boundaries.
- [History](history.md): optional independent output/asset archives and metadata.
- [REST service](serve.md): native jobs, event streams, results and persistent history.
- [Embedded workspace](web-ui.md): browser conversion, preview and connection editing.
- [Service settings](service-settings.md) and [provider management](provider-management.md): saved connections, revisions, model discovery and probes.
- [MCP service](mcp.md): stdio tools, conversion results and in-memory batch jobs.
- [CLI](cli.md) and [bindings](bindings.md): current user interfaces.
- [Native CI](ci.md): per-platform builds, installed artifacts and evidence boundaries.
- [Validation](validation/README.md): differential checks and measured evidence.
- [Performance plan](performance-plan.md): profile tradeoffs, binding costs and long-lived memory checks.
- Decisions: [native core](decisions/0001-native-core.md),
  [document LLM cache](decisions/0002-persistent-llm-cache.md),
  [static-page fetch cache](decisions/0003-persistent-fetch-cache.md), and
  [run reports, recovery and history](decisions/0004-run-persistence.md).

Documentation describes verified behavior separately from planned behavior.

- [Image captions and descriptions](image-enrichment.md): resource localization, prompts, shared budgets and metadata publication.
