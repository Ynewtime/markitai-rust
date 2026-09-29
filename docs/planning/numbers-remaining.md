# Numbers 剩余能力：先闭环现代目录包

状态：**现代目录包已通过 R27 源码、发布 CLI 与三绑定安装验收**，见[记录](../validation/numbers-packages-round27.md)。旧 XML 和完整画布仍未关闭。后续接入补充：CLI 单包历史与交互向导、旧恢复状态包内任务冲突检测也必须完整处理，见 CONTROL。

以下为实施前只读核对：2026-09-29 在 R25 源码冻结期间只读核对；基线为 CONTROL 记录的 R24 production `2d502b9ad4d46edf21fc854997543a79ab74c532`，当前 R25 工作不涉及 Numbers。原项目参考提交为 `ba374322f884b0e720b45466cc1196f4574a3da5`。本次未运行转换、构建、安装或原项目，未读取真实用户配置。

下一批建议只实现：**现代 `.numbers` 目录包 → 与当前单文件 ZIP 相同的有界 Rust 表格抽取 → CLI 单文档及目录批处理完整输出**。复用已锁定的 `iwork = 0.2.1`，不新增依赖、公开 flags 或外部运行时。旧 XML 格式与完整画布是不同问题，不附带承诺。

## 三类能力不能混为一谈

| 输入或目标 | 当前证据 | 本批处理 |
| --- | --- | --- |
| 现代单文件 ZIP，直接包含 `Index/*.iwa` | 已有 Rust reader、独立公开夹具和实际转换验收 | 保持正文、顺序、警告与资源限额 |
| 现代目录包，直接包含同类 IWA 文件 | Apple 官方说明现代 Numbers 可切换 package / single file；当前 native 的目录分派挡在 reader 前 | 实现容器适配及分派 |
| 旧 Numbers XML / `index.xml.gz`，或仅有嵌套 `Index.zip` 的其他容器 | 当前语义 decoder 没有这条解析路径；未取得有效导入的本机证据 | 作为单文档明确拒绝，不递归输出内部文件 |
| 所有表格、图表、图片、文本框及画布几何的视觉输出 | 现有 reader 仅表格；可用 LO 对现代夹具实际导入失败 | 保留明确 Unsupported，不拿预览图或重建表格 PDF 冒充 |

目录包不是“旧格式”的同义词。[Apple 官方说明](https://support.apple.com/en-lamr/119883)明确允许 Pages、Numbers、Keynote 文档使用 package 或 single file，并通过 Change File Type 切换。旧 XML 是存储代际，目录/ZIP 是外层容器，两者应分别判断。

## 原版实际支持边界

原项目 `packages/markitai/src/markitai/converter/markitdown_ext.py:314` 注册 `NumbersConverter`，但实现只是调用同文件 `:81` 的 `_convert`，再交给 `MarkItDown().convert(path, keep_data_uris=True)`。该路径没有 Numbers 专用 IWA 解码器；`converter/_patches.py` 只处理通用 Office 库补丁。

本机参考环境的 `markitdown-0.1.7.dist-info/METADATA` 标明 0.1.7；其 `_markitdown.py:182–205` 的 converter 注册没有 Numbers，`:325–362` 的 `convert_local` 使用 `open(path, "rb")`，不能直接读取目录包。通用 ZIP converter 递归转换可识别的内部文件，不构成表格或画布恢复的证据。参考 CLI `packages/markitai/src/markitai/cli/main.py:1240` 也先把目录交给 batch；没有 `.numbers` 包例外。已有 Numbers 测试主要验证注册/远端接收，不是本地内容 golden。

因此这一批是补齐可用的 native 输入路径，不能宣称已经复刻原版某个经验证的完整 Numbers renderer。[MarkItDown 上游 README](https://github.com/microsoft/markitdown)的支持列表也不提供这种完整画布承诺。

## 本机后端核查

- PATH 中有 `soffice`，解析到 Codex runtime 的 override；没有 `numbers-parser` 命令。`osascript` 存在只说明系统具备脚本执行器。
- `/Applications/Numbers.app`、`~/Applications/Numbers.app`、`/System/Applications/Numbers.app` 三个常规位置均未发现 Numbers。本次没有遍历用户文档或启动 GUI；这不是对其他自定义安装位置的穷举证明。
- R24 已保存真实、隔离 profile 的 `test-1.numbers → ods:calc8` 导入记录：退出 1，`Error: source file could not be loaded`，未生成输出。本次只读取该既有记录，没有重跑。

证据路径：`.local/workbook-round24/imports-r1/record.json`，SHA-256 `79bde35225a1c9f59dc78ca76e33a174c309214759142b5b1e5414e929b3b9fe`；输入为 [test-1.numbers](../../crates/markitai-core/src/formats/numbers/fixtures/test-1.numbers)，SHA-256 `b9e9772b2d2866c26d773fe46a173c373c7dc1dd3df6cefc7f0253b6ab50d4c3`。独立夹具许可证与来源见 [provenance.json](../../crates/markitai-core/src/formats/numbers/fixtures/provenance.json)。这是一个现代文件的真实失败，不推导所有历史 Numbers 格式均失败。

[LibreOffice 官方文件格式说明](https://books.libreoffice.org/en/GS75/GS7510-FileFormatsSecurityExporting.html)列的是 Apple Numbers 2；[官方 NumbersImportFilter 源码](https://docs.libreoffice.org/writerperfect/html/NumbersImportFilter_8cxx_source.html)使用 libetonyek。这不能证明本机能导入当前 IWA 版本，更不能证明完整画布。当前 [office_render.rs](../../crates/markitai-core/src/office_render.rs) 的 Numbers 视觉拒绝应保持。

## 具体接缝与实现合同

现有 [numbers/container.rs](../../crates/markitai-core/src/formats/numbers/container.rs) 的 `open(bytes)` 在 ZIP 校验后构造 `iwork::Package { entries, form: SingleFile }`，再调用 `Document::from_package`。`iwork 0.2.1` 的 `package.rs` 已提供 `Form::Directory`；其 `Document::from_package` 解码直接的 `.iwa` entries，与外层形式分离。

不能直接调用上游 `Package::read_directory`：其 `collect` 虽有 8 层深度限制，却对文件使用无界 `fs::read`、对文件名使用有损 UTF-8 转换，并静默跳过 symlink/特殊文件。native 必须保留自身的明确拒绝与分配预算。

建议内部 API（实施时由协调器确认命名）：

```rust
// formats 的内部入口；不增加用户 API 参数。
fn is_numbers_package_path(path: &Path) -> bool;
fn extract_directory(path: &Path) -> Result<Document>;
// container 内部共用入口，接收已检查的 entries，而不是重新压缩 ZIP。
fn decode_entries(entries: Vec<(String, Vec<u8>)>, form: iwork::package::Form)
    -> Result<(iwork::Document, usize)>;
```

1. **容器读取**：将现有 IWA、对象、表格引用及 decoded-cell 预算提为共用流程。目录遍历使用稳定相对路径排序；每个节点计入 4,096 上限，深度最多 8；每文件 32 MiB、全部文件 128 MiB。保留现有 IWA 总量 64 MiB、Snappy 块 64 KiB、对象/单元格/正文等限额。目录顶层 metadata 的 size 不代表文档大小，必须按实际读取计费。
2. **明确失败**：拒绝内部 symlink、FIFO/设备等非普通文件、非法 UTF-8 或路径名、加密标记和损坏 IWA；不能以跳过某个必需文件换取“成功”。文件打开前后核验普通文件及有界实际字节；发现读取期间变化时失败。不得宣称由此实现恶意并发修改下的原子快照。`.DS_Store` 可作为明确列出的无语义文件忽略，但遍历节点仍计数。
3. **一个包是一个输入**：对目录且扩展名为 `.numbers`（不区分 ASCII 大小写）的路径，先进入包验证，再决定支持/报错；不能因缺 marker 而退回普通目录 batch。`core/lib.rs` 的目录拒绝和 `formats/mod.rs` 的单文件打开需接这条路径。输出名称仍为 `name.numbers.md`，source 为包路径，正文及现有 frontmatter 形状不变。
4. **目录批处理**：`cli/app.rs` 的单输入目录判定和 `discover` 遍历必须识别原子包。发现包时产出一个 task 并停止遍历其内部；不能只用 `filter_entry(false)` 而把包本身一起漏掉。glob、递归深度和相对来源 key 均按包路径计算。单包输入不能创建一份“包内文件 batch”状态。
5. **恢复与其他调用者**：batch 的文件类 entry 可以以目录包路径作为来源，输出仍是普通 Markdown 文件；目前 codec 的 `metadata.is_file()` 检查的是输出 destination，不应无故放宽。验证 resume/report/history 仍只有一个项目。MCP 本地输入最终走 core，无需增加 tool；其 `read_existing` 普通文件检查针对 Markdown 输出，同样保留。Node/Python/Go 路径接口无需新参数。REST multipart 仍只上传单文件，目录上传不是本批 API。
6. **不偷偷扩大视觉能力**：目录包与 ZIP 包同样只返回现有表格语义及遗漏警告。Numbers screenshot/OCR 请求维持明确 Unsupported。默认单文件转换结果须保持；不能通过将表格重写成 XLSX 再导出 PDF 来声称恢复了原画布。

实现可分两条并行 lane：reader worker 持有 `formats/numbers.rs`、`numbers/**` 与格式测试；协调器持有 `lib.rs`、`formats/mod.rs`、CLI 原子包分派及过程测试注册。无需 vendor、manifest 或新增工具安装。若媒体路由在读取前阻断 Numbers 包，协调器只调整分类顺序，保留 Numbers 视觉拒绝。

## 最小有意义验收

全部夹具在测试临时目录或 `.local/`；使用独立 MARKITAI_HOME，HOME 不变。不得读真实 Numbers 文档、真实配置或自动启动 Apple 应用。

- 将两个已保留、独立 MIT 上游的 ZIP 夹具按原始 entry bytes 展开为目录，逐一核对与 ZIP 的正文、sheet/table 顺序、元数据、警告完全相同；source 路径差异单独处理。此方法验证外层容器适配，**不冒称取得了独立 Apple Numbers 新导出的目录 golden**。补作者 Unicode 包名、多个同名表及目录项乱序案例。
- 真实 CLI：一个 `.numbers` 包作为单输入；普通父目录中混合包与文本；结果各一份、包内部没有独立任务/输出；同一 batch resume 不重做 completed 包，report/history 指向实际结果。格式错误和旧 XML 包应产生一次清晰失败，无部分输出。
- 读取边界：过深目录、节点超限、超大 sparse 文件、聚合超限、内部 symlink/FIFO、坏 Snappy/缺失模型；故障应在高层解码之前或既有共用预算内失败。测试 FIFO 不得采用会永久阻塞的读取方法。
- core 与三种绑定的原有单文件 Numbers 测试保持；新增目录路径的实际绑定调用可在本轮安装包验收复用同一目录夹具，不需要新增 wrapper 接口。
- 无参数截图调用与显式截图/OCR拒绝分别检查，确保“正文支持目录包”不会让媒体路径默默回退为表格截图。

结束条件是以上现代目录包工作流实际通过并记录产物身份。完整画布的下一前提仍是**获得可验证的 renderer**：证明同一输入的每个 sheet、空/隐藏 sheet、表外文本、图表/图片与宽画布内容均保留；缺页必须失败，不能拿单张 preview 或表格数量代替。当前没有足够后端证据，故不把这项夹带进短批实施。
