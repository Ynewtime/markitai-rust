# 下一段媒体实现计划（只读规划，2026-09-29）

建议并行推进两个可验收增量：**macOS 的完整 Office → PDF → 全页截图**，以及 **Numbers 原生表格读取**。前者复用可选的已安装 LibreOffice 与现有 CoreGraphics；后者可嵌入 Rust reader。当前没有执行 Office 转换、安装工具、Cargo、Git 或更改受控文件；以下是实现方案，不是支持/测试完成声明。

## 1. 已确认的边界

- 原项目 `packages/markitai/src/markitai/converter/office.py` 的 `PptxConverter` 用 Windows PowerPoint COM，或 LibreOffice 导出 PDF；macOS 无 LibreOffice 时可受 `office.macos_fallback` 控制调用已安装 PowerPoint AppleScript。随后遍历全部 PDF 页。它不是纯 Python 渲染器，Rust 复用系统 Office 后端不需要 Python 运行时。
- 原项目 DOCX 读取器没有对应的完整分页截图实现。PPT 截图失败当前可能只警告并返回空集合；新实现不能在明确请求截图时悄悄返回成功的零页。
- Rust `formats/office.rs::extract_presentation` 是 OOXML 结构读取器，具有逐 slide 标记；`anydoc 0.2.4` 也只提供读取模型，没有 Word/PPT 全页面排版渲染。已有 `resvg` 不能自动实现 Office 字体布局、母版、图表、SmartArt、分页；把文本与嵌图拼成 SVG 会造成实质内容遗漏，不是本轮可用捷径。
- `lib.rs:286` 目前只显式拒绝若干 PPT 扩展的截图；DOCX/XLSX 截图可能被忽略。应以共享 Office family 分类器覆盖现有 reader 的全部 aliases，而不是只删除这一个拒绝分支。
- 本机可通过 PATH 找到 `soffice`，它是 Codex runtime 的 wrapper，最终执行已安装 LibreOfficeDev。也存在 Microsoft Word/PowerPoint/Excel；未找到 Numbers/Pages/Keynote。这里只核对了路径，没有启动任何应用。生产发现逻辑不能硬编码 Codex cache 路径。

## 2. Office 最小完整交付路径

### 范围及依赖

第一交付覆盖已有 reader 接受的 presentation（ppt/pptx/pptm/pps/ppsx/ppsm/pot/odp）与 word-processing（doc/docx/docm/odt/rtf），保持全页，而非仅第一页/缩略图。具体 aliases 以 `anydoc::Format::from_extension` 和现有专用分派为准；不凭空增加未读过的模板扩展。macOS 使用现有 `PdfRasterSession`。Linux/Windows 因现有 PDF 光栅化后端不可用，明确拒绝该路径；仅有 soffice 不等于跨平台截图已经实现。

不用引入新的大型 Rust 排版库或捆绑 LibreOffice。CLI 仍是一个二进制，**Office 截图是需要已安装系统程序的可选能力**；无法把外部 LibreOffice 体积排除后声称整个能力只有 CLI 的体积。无后端时文字读取继续可用，显式截图返回可行动的 Unsupported。

### 新模块和内部契约

1. 新 `src/office_render.rs` + `office_render/libreoffice.rs`：
   - `kind(extension) -> Option<OfficeKind>`；`available(kind) -> BackendAvailability`。
   - `export_pdf(input: &Path, kind: OfficeKind) -> Result<OfficePdf>`，返回持有私有 `TempDir` 的 `OfficePdf`，再有界读取 PDF；原文档不交给任何写入路径。
   - 使用私有 input copy、output 与 LO profile 子目录，输入固定安全文件名；`Command` 参数数组，无 shell。每次独立 `-env:UserInstallation=<file URI>`，禁止复用真实用户 profile。限制日志、输出 PDF 字节、子进程并发；超时 kill + wait 并清理本次私有目录。外部程序超时不是内存/网络沙箱，不作沙箱承诺。
   - 不接受 LO 的 exit=0 作为充分条件：必须有唯一预期普通 PDF 文件、正确 signature、可打开的非零页数。已有输出不能充当本次成功产物。限制执行期间写入的产物大小；超预算终止，不只等进程退出才读取。
2. 从 `src/pdf_media.rs` 提取一个小的共用全页 capture helper（建议 `src/page_media.rs`），复用 `PdfRasterSession::{open,pages,dimensions,render}` 与现有截图编码器/预算，不调用 PDF 原生文本提取或 PDF OCR 路由。接口例如 `capture_pdf(bytes, prefix, PageLabel::Slide|Page, cfg) -> Result<Vec<PageCapture { number, asset }>>`。先检查全部页数/尺寸/累计像素与模型页上限，再逐页渲染、编码、释放 RGB；最大 1000 页、32MP/页、2G 累计像素、5MiB/张、100MiB 编码总量沿用当前预算，拒绝截断。
3. `lib.rs` 保留 `formats::extract` 的 Office Markdown/标题/资产；仅追加 typed captures。`output::publish_page_screenshots` 统一决定实际文件名，再绑定引用。不要把 Office 的 format/source 改成 PDF，也不要用导出 PDF 提取的文字覆盖现有 DOCX/PPT 正文。
4. PPT 用真实 slide 顺序与 `<!-- Slide number: N -->` 关联截图；从结构 reader 暴露 typed slide count/order（不要以 Markdown 正则推算）。PDF 页数须与源 slide 数一致，包括 hidden/blank，否则错误。Word 没有原生段落→页映射，采用清晰的全页截图附录与 Page N 标记，不能伪造段落所在页。
5. 统一截图模型路由：显式 screenshot、screenshot_only、pure、OCR、VLM opt-out 按现有 PDF/URL 合同分开。首步完整完成截图→磁盘/API→全部模型图片；PPT 本地 OCR 复用现有 `ocr::recognize_rgb` 或嵌图识别，但不得因 renderer 的全文 OCR 覆盖可靠原生文字。所有页数上限在模型请求之前验证。无磁盘且无模型的 screenshot-only 必须有可取回资产或明确拒绝，不能空 Markdown 成功。

### LibreOffice 导出参数

直接传单个参数（下面 JSON 是参数内容，不是 shell 引号）：

- Presentation：`pdf:impress_pdf_Export:{"ExportHiddenSlides":{"type":"boolean","value":"true"},"ExportNotesPages":{"type":"boolean","value":"false"}}`。
- Word：`pdf:writer_pdf_Export:{"IsSkipEmptyPages":{"type":"boolean","value":"false"}}`。
- 不传 PageRange 或 Selection，避免意外只导出部分页。官方说明未设置 PageRange 时导出所有页；隐藏幻灯片默认不导出，须显式开启。参见 [LibreOffice PDF CLI parameters](https://help.libreoffice.org/latest/en-US/text/shared/guide/pdf_params.html)。

Spreadsheet 截图不应被冒充完成：Calc 正常 PDF 遵循打印区域/分页，可能排除隐藏 sheet 或范围外数据；`SinglePageSheets=true` 又会产生巨大整表页并改变分页。最快可靠次序是先交付 PPT/Word，上述分类器对 spreadsheet 明确 Unsupported；另一个小增量定义“全部工作表画布”与“打印页面”的合同后再实施，超大页应明确拒绝而非丢行/缩小到不可读。

macOS PowerPoint AppleScript fallback 可在 LO 路线验收后补齐参考行为；它涉及 TCC、Office container、用户已打开文档、每 app 串行队列，不适合首轮同时实现。Windows COM 可以直接导出每个 Slide 为图片，无需 PDF backend，但需要实际 Office 安装验收；不以代码编译替代证据。QuickLook/文档内 thumbnail 不作为完整多页 backend。

## 3. 可直接执行的 Office 验收

- 只读复用 reference fixtures `sample.pptx`、`sample.docx`、`sample.odt`、`legacy/sample.ppt`、`legacy/sample.doc`；记录输入 SHA，输出各自 native-text-only 与 screenshot 模式的正文保持关系。
- 原创标准 OOXML 小包：PPT 三张以上，包括纯白页、hidden 页、向量图形/母版元素/透明图、每页唯一大字与四角不同色块；DOCX 三个显式分页（中间空白），页眉/页脚、表格和嵌图。不仅验张数，还验完整画布比例、四角像素、逐页可见标志。PNG 全页，不用资产提取伪造。
- 真实安装 LO → PDF → CoreGraphics → CLI `--screenshot`，验证全部页顺序、真实 MIME/扩展、磁盘引用、名称冲突、RAG/history 重定位。loopback 模型验证全部 N 图、限额 N-1 在模型前失败、pure/opt-out 组合不误上传。
- 子进程边界测试用私有 mock executable 覆盖零退出但无 PDF、错误/空/少页 PDF、超时、超预算、包含空格/Unicode 输入；真实 LO 测试单列并保留版本/可执行路径与 hash，缺工具不算 pass。导出字体/排版可能与 Microsoft Office 不同，实际截图质量待上述样本确认。

## 4. Numbers：有真实 Rust reader 候选，建议下轮直接验证集成

锁定缓存 `anydoc 0.2.4` 没有 Numbers。原项目 `markitdown_ext.py:313` 只注册并转交通用 MarkItDown；本机安装的 MarkItDown converter 列表没有 Numbers reader，原项目测试只确认格式注册/远程 converter 接受性，没有实际 Numbers 表格 fixture。因此不能把该 registration 作为原项目本地表格保真证据。

候选 **`iwork = "=0.2.1"`** 可纯 Rust 读取 Numbers 的 IWA/表格，不需要 Apple app、Python、protoc 或动态库。此次只下载发布 crate 到内存查看源码，未安装/构建；registry archive 755866 bytes，SHA256 `8a8862036a02bcac12eade55a57905341fbf3d8a92a7455ed5ba9a4e639c7dce`。发布 Cargo.toml 声明 MIT、Rust 1.74，依赖 snap 1.1 + zip 2（deflate，禁用默认 features）。当前 workspace 仅有 zip 8.6，因此会新增 zip 2 和 snap；源压缩大小不是 binary delta，需同 source release 实测。上游 main 已是 0.2.2，不应将其源码当作本次锁定版本。来源：[发布页](https://docs.rs/crate/iwork/latest)、[0.2.1 archive](https://static.crates.io/crates/iwork/iwork-0.2.1.crate)。

实现入口新 `formats/numbers.rs`，`extract(bytes: &[u8]) -> Result<Document>`，由 formats/mod.rs 注册。可用发布源码中的 `Package { entries, form }`、`Document::from_package`、`kind`、`sheets()`、`tables()`；先读取一次 tables，再按 sheet/order 分组，不反复全表解析。每 sheet 可有多个 table，同名表不能覆盖；渲染 sheet/table 标题、原始矩形/合并跨度、显式空值、Unicode、数字/日期/货币/百分比与缓存公式值。`value.to_text()` 不等于 displayed cell formatting，需复用解析出的 format 并明确显示语义；公式只读缓存值，不声称重新计算。`doc.text_storages()` 不是 Numbers 表格入口。此模型含多个画布对象；chart/image/textbox 不支持时须有准确警告，不能宣传完全忠实 Numbers 页面。

资源保护必须在适配器实现：不要直接 `Package::from_bytes` 的无总量 `read_to_end`。用 workspace ZIP reader 有界读每 entry（建议先沿用 Office 16MiB 文本 part、64MiB asset、256MiB aggregate，具体碰到真实 fixture 再收敛）、限制 entry/表/格数并拒绝重名非法路径。逐 IWA framing 预检全部 Snappy 声明 expanded sizes 与 aggregate；发布版本已限制单 block 64KiB，但没有总 expanded limit。构造 `Package` 后才调用 `from_package`；检查 Numbers kind、加密、损坏对象，不能成功为空。先支持单文件 ZIP .numbers，目录 package 需让 CLI 识别为文档而不是递归目录任务，可随后独立接入；不能误称两种均支持。

验收不只用 reader 自己生成的文件：需要一个独立、许可明确的真实 Numbers 保存样本，含两 sheet/三 table、同名表、合并单元格、rich text、非拉丁文本、格式化小数/日期和缓存公式；记录文件 hash 与预期单元格来源。上游 iwork 发布包排除了 tests/fixtures，README 的 AppleScript 比对是上游证据，不是我们实测。本机未装 Numbers，因此优先从上游 parser 项目取得许可确认后的真实 fixture，或用户已有样本；先核查许可再复制。另加由最小 IWA/ZIP 生成器原创的明确 goldens、截断/压缩炸弹/异常尺寸输入。read→write→read 的自闭环不能单独证明 Numbers 兼容。

## 5. 交付顺序与大小口径

1. 一个 lane 实现 LO adapter + 真实 PPT/Word 全页 fixture，另一个 lane 实现有界 Numbers reader；root 抽共用页捕获/发布并接 lib。
2. 一次 focused test 后立即运行真实 LO 和真实 Numbers 小样本；发现 importer/布局差异先记录，再决定是否调整。不要重复无关旧 corpus/benchmark。
3. 完整 gate 后从同 source 记录 CLI/FFI bytes；Office 额外系统安装体积单列，Numbers 新依赖的 binary/RSS delta 实测。没有本次运行数字，不预计“极致性能提升”百分比。

只读快照 SHA256：lib.rs `659fcafa126cd22150f216b4ae051e8c8ea62cdce507a9149ab059e033fe0ccf`；pdf_media.rs `47d03cea0446cd2b69503d651401aac72a29d0ac87c6c84a53b0c607870c217b`；formats/office.rs `7233a4ffab360e0e030c99a872494a9e0edd9dd40cf8e553db6bf66bf7351f36`；Cargo.lock `d16fc769245adb5b1198e81606f48b35646a196223ecbd5fd9d4d55e7a930112`。

## 6. 后续补记：本机 Rosetta OCR 未通过，不等于实体 Intel 不支持

根协调器实际 x86_64 HEIF decoder 六项单测与两个视觉/损坏输入 API case 已通过，但 `heic_local_ocr_and_optout_use_original_resolution_without_image_upload` 在 Vision 调用失败；原日志 `.local/media-cli-round20/x86-r1/1.log`。`ocr/vision.rs:33–34` 将 SDK 请求失败统一转为 `Local OCR: Vision text recognition failed`，没有保留 NSError domain/code。

为区分 codec 与系统框架，已按授权执行一个私有 Objective-C SDK probe（不是 Cargo、生产改动或兼容性修复），直接读取仓库原创 `ocr/fixtures/english.png`：

- macOS 27.0 / 26A428，x86_64 的 `sysctl.proc_translated=1`，Vision revision 3，支持语言列表含 en-US；默认计算策略和 `usesCPUOnly=true` 都返回 `performRequests=NO`，NSError 为 nil，无文字。
- 同一 probe 源码编译为 arm64，translated=0，同配置成功返回三行 `MARKITAI OCR`、`LOCAL TEXT ONLY`、`2026`。
- `.local/media-cli-round20/rosetta-ocr-probe/record.json` 保存精确编译/执行命令、输入和二进制 SHA、系统版本；相邻文件保存完整 stdout/stderr。探针进程全部结束。并未对 ARM 与 x86 做性能计时比较。

这足以把本次失败定位到 **本机 Rosetta 下直接 Vision 请求也失败**，而不是先归罪 HEIC 解码、Rust FFI 或图片方向。官方 [Rosetta 说明](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment) 描述整进程翻译及 `sysctl.proc_translated`，没有承诺此 OCR 请求在所有翻译环境均可用；[usesCPUOnly](https://developer.apple.com/documentation/vision/vnrequest/usescpuonly) 是计算策略开关，不是 Rosetta 兼容开关，且本机实测无效。没有找到可据以宣称“Vision 全面不支持 Rosetta”或“实体 Intel OCR 已失败”的官方结论。

建议本轮保留 arm64 已通过证据，x86 发行验收明确未完成；后续有实体 Intel runner 时复验。可在后续独立小修保留安全的 NSError domain/code（不可输出任意 userInfo/路径），并针对翻译进程的失败给出 arm64 包建议；不应静默空 OCR、强制 CPU-only 或在当前冻结版暗改计算策略。Office 路线的截图渲染和本地 OCR 也须分开验收，不能从 x86 图片解码成功推导 x86 Vision 可用。
