# Round 22 后的 CLI 小批实施计划

状态：只读设计，尚未实施。范围限顶层转换向导、doctor 的可操作诊断、旧状态恢复提示；不重新实现已经交付的 init、config edit 或日志。Round 22 的源码和门禁保持冻结。本次没有执行原项目、用户配置、安装器或构建。

## 先做哪一批

建议下一批并行完成 **`-I` 引导一次实际转换** 与 **配置相关的 doctor 检查**，由协调器合并 `app.rs`。旧状态先补窄诊断和真实迁移用例，不重写已经验证的 codec/receipts，也不自动放宽所有权。三者都应落到现有转换/状态入口，避免维护第二条调度路径。

| Lane | 独占文件建议 | 完整结果 |
| --- | --- | --- |
| 转换向导 | 新 `crates/markitai-cli/src/app/guided.rs`、新 `tests/guided.rs` | 选择输入类型、路径、输出、功能，预览有效配置，确认后真正转换；取消不产生转换输出 |
| Doctor | 新 `crates/markitai-cli/src/app/doctor.rs`、新 `tests/doctor.rs`；若必要，独占 core 窄诊断 helper | 检查实际配置依赖、可启动后端和缺失环境引用，稳定 JSON/退出码及人工修复指引 |
| 协调器 | `app.rs` dispatch/优先级、现有 `app/interactive.rs` 的最小可复用接口、`batch_run.rs` 窄提示、文档 | 同一份配置、一次日志生命周期、原有 JSON/报告/历史/恢复行为；不同时让多个 worker 改共享入口 |

可复用现有终端输入、密码 ECHO 恢复、原子配置写入和配置验证。需要共享 helper 时由协调器明确 visibility；不以复制整个 init/config editor 实现加速。

## 1. `-I`：本次明确缺的完整流程

参考入口在 `packages/markitai/src/markitai/cli/main.py:223–260,513–519`，会收集 session、询问是否执行，再启动原 CLI；取消、EOF、Ctrl-C 均打印取消并退出 0。`cli/interactive.py:82–235,437–540` 的实际顺序是：

1. 文件 / 目录批量 / URL。
2. 对应路径或 URL；输出目录默认 `./output`。
3. 是否启用 LLM。存在正权重配置模型时显示模型，否则使用 provider 检测结果作为默认建议；没有模型仍可选择配置或关闭 LLM。
4. LLM 开启时选择 alt、description、pure；随后独立选择 OCR、截图。
5. 显示摘要，再确认执行；返回真实转换的退出码。

Rust 目前在 `app.rs:303` 附近直接拒绝 `--interactive`，而顶层无参数已显示帮助并退出 0，不应误记成缺少自动启动向导。保留无参数帮助这一行为。

### 建议内部接口和闭环

`guided::collect(config: &Value, context: &GuidedContext) -> CliResult<GuidedAction>`，结果区分 `Cancelled` 和带有类型化 source/output/选项的 `Convert`。向导不启动第二个可执行文件，也不调用模型。协调器将选择叠加到一次普通 `execute_conversion`，复用 URL/directory 分发、缓存、并发限额、报告、历史和错误处理。

配置来源保持显式 `-c` / `--config-json`，摘要显示实际生效值。原版 eager callback 重新拼参数时没有传递这些 root 配置参数，不应照搬这项意外丢失；这是需要记录的小兼容修正。摘要及所有提示写 stderr，转换 stdout 合同不变。首批 `-I` 与 `--json` 明确在读取 stdin 前拒绝为用法错误，避免 Clap 的 `--json` 必须有 `-o` 与向导输出目录提示互相冲突；不要暗中把 JSON 变成混合终端输出。

缺少模型的闭环复用已实现的配置交互能力：提供“配置已有支持的 API 模型 / 重试检测 / 本次关闭 LLM / 取消”，配置后重新读取有效配置再回到摘要。手工密钥输入沿用 secret guard；保留 env 引用或已有 provider 连接，摘要不显示值。原版还提供 Claude/Copilot CLI 自动检测和手工 `.env` 文件加载（`interactive.py:238–414`）；本地 SDK/OAuth runtime 未实现时必须明确不可用，不能因 PATH 上有 CLI 就宣称可转换。不把这两种 provider 的实现混入向导批次。

原版 session 只发出已选的正向 alt/desc/pure/OCR/screenshot 参数；未勾选不等于 `--no-*`，配置中的 true 仍可生效。实现时必须保留这一 presence 差别，或明确改为“启用 / 关闭 / 使用配置”三态并把有效值展示给用户。推荐三态，避免摘要说关闭而实际启用。LLM 明确关闭仍发出等价 `--no-llm`。

### 快捷选项只补真实差异

原版没有另一套快捷转换菜单。`-o/-c/-p/-j/-g/-s/-b/-v/-q/-I/-V` 及 paired flags 的短选项均来自 `main.py:269–525`；Rust 除 `-I` 功能拒绝外已具备这些解析，不应重复做“shortcuts”新功能。

发现两个可随入口整理修的小差异：

- 原版 `--preset` 在查找前 `.lower()`（`main.py:814–837`）；Rust 当前原样 match，因此 `-p Minimal` 被拒绝。统一与原项目的查找规则，同时验证自定义 preset 的行为。
- 原版只有配置/转换选项但无 INPUT 时显示帮助并退出 0（`main.py:608`）；Rust 仅完全无参数这样做，`--quiet` 等无 INPUT 会返回 2。选择是否保持原接口时需要显式测试，不能误认为两边已经相同。

### 最少验收

四组真实 PTY 足够验证路径，而不是只 mock 选择函数：

- Unicode/空格文件、默认输出、关闭 LLM → 实际 Markdown 和单文件报告正确。
- 两文件目录 + 选择 OCR/截图关闭 → 实际两个输出、报告、历史采用普通批量流程。
- loopback URL + 已配置 mock 模型 → 一次 fetch、一次模型调用，选择的 alt/desc/pure 按有效值进入普通路由。
- 确认前取消和密钥输入时 Ctrl-C → 退出 0、终端 ECHO 恢复、没有转换请求/输出；配置若已经明确保存则不得谎称被回滚。

另用普通进程覆盖非 TTY 拒绝、`-I --json` 用法错误、preset 大小写/显式 paired flag 优先级。PTY 测试沿用现有平台门控，不能把 Unix PTY 通过说成 Windows 交互验证。

## 2. Doctor：从“编译有能力”改为“这份配置能用什么”

Rust `app.rs:1320–1362` 目前只验证配置并打印 bool capabilities；`browser_available()` 等主要是平台/可执行文件可发现性，不等于成功启动。`--fix`、`--suggest-extras` 均返回 Unsupported。参考 `cli/commands/doctor.py:544–897,998–1144` 则按固定顺序输出检查条目，每项有 `name/description/status/message/install_hint`，有些还有 `path/optional`。状态为 `ok/warning/missing/error`。

最重要的兼容语义不是 Python 包名，而是 **可选缺失不失败；配置明确要求却不能提供时退出 1**：

- 活跃模型按 weight > 0 计算；全为 0 是 warning。只有活跃模型的 `env:` api_key/api_base 缺失才是阻断，提示变量名而不输出值。
- 原版显式 playwright 策略或启用 screenshot 要求 browser；活跃本地 provider 要求对应 runtime 和认证。原版 RapidOCR 仍是 optional，不要擅称其缺失一定失败。
- `vision-model` / `vlm-ocr` 必须使用现有实际能力投影，识别 `MARKITAI_NO_VLM_OCR`，不要靠模型名称包含某词猜测或请求真实 provider。
- 原版 `doctor --json` 输出检查字典本身；缺失 optional 返回 0，required 失败返回 1。`--json --fix` 是用法错误 2。Rust 当前 JSON 外壳不同，需由协调器确定改成 reference shape；不同时维持互相矛盾的 “configuration:valid 等于 healthy” 判断。

建议 typed `Check { key, name, description, status, message, install_hint, required }`；required 是内部分类，公开投影按参考字段。公共 key 保留 `playwright/libreoffice/rapidocr/anydoc/serve/llm-api/vision-model/vlm-ocr` 的兼容用途，但 name/message 必须明确原生 Chromium/CDP、Vision、Rust reader，不假装安装了 Python Playwright、RapidOCR 或 anydoc Python 包。对于原生 provider 尚未支持的模式显示 unsupported 的说明和 blocking 状态，不启动 SDK。

独立检查有界并行，按固定 key 顺序投影；不持配置锁发请求。最小真实 smoke：Chromium 使用已有原生 process/CDP 管理、私有 profile、`about:blank` 后退出；LibreOffice 使用有界 `--version` 及既有 executable 检查，明确“发现并可启动”不等于完成全部格式导出。Vision 的可用性/语言查询和昂贵 OCR 模型预热应区分，不让普通 doctor 冷启动悄悄承担一次全文 OCR；平台/架构未验证必须可见。内部 helper 可放在原模块并向 CLI 暴露结构化、已脱敏结果，不能把任意 child stderr 原样送入 JSON。

### `--fix` 的边界必须真实

原版仅自动安装 **已有 Python Playwright 包对应的 Chromium**：同解释器 `-m playwright install chromium`、隔离 cwd、300s timeout，随后实际 launch 再检查。缺 Python 包只提供人工建议，不 pip install；LibreOffice、认证、模型、系统库全部不自动修（`doctor.py:70,138–204,933–996`，测试 `unit/cli/test_doctor.py:793–917`）。

原生项目没有这个可安全复用的 installer，不能为了接受参数就执行 pip/npm、curl 管道、brew/sudo 或猜测系统包管理器。下一小批先完整交付诊断与人工修复闭环：`--fix` 可以运行同一套检查，明确哪些没有 native 自动修复器；需要修复但只有人工路径时非零退出，全部健康时无需动作。文档必须说“自动浏览器安装仍未实现”，不能把打印提示称为安装成功。`--json --fix` 先按参考拒绝。

如果要消除这一最后差异，应单独授权一个原生托管 Chromium 下载/校验/原子发布工作包，定义平台版本、来源、校验、解压界限、取消与重试，再接入 `--fix`。这不是仅改一个 CLI flag 的工作。`--suggest-extras` 的参考输出是发行包元数据驱动的逗号分隔 **Python extras**，不适用于单二进制；首批保留清楚的 Unsupported，不能输出 native feature 名来误导旧安装脚本。

最少 process 验收：无可选工具仍退出 0；配置要求 browser 且可执行文件无法启动→条目 warning/error+退出1；活跃缺 env 引用→退出1且无值泄露；同模型weight0不阻断；`--json` 唯一合法JSON且键序稳定；`--json --fix` 返回2；伪安装器/PATH哨兵证明不会启动安装命令；一台已安装 Chromium 的实际私有 profile launch/close。配置显式传入临时文件，HOME保持原值、MARKITAI_HOME隔离，不读真实凭据。

## 3. 旧 state：已有支持与仍需解释的差异

已有 codec/replay/hash 和真实 CLI 恢复测试，不应重新列为“resume 未实现”。对应 `run_state/{codec,store}.rs`、`batch_run.rs:242–447`、`tests/recovery.rs:1311` 及 `docs/state-storage.md` / `docs/output-ownership.md`：

- 相同 scope/options 使用 Python-compatible 六位 task hash；读取旧 `version:"1.0"` base 和 JSONL；恢复完成状态、failed/in_progress 重试、bare URL 按发现顺序迁移为 named key。
- 原生 checkpoint 新增 generation/sequence。旧 Python 可以忽略字段读取结构，但不理解 fence 或遵守原生锁/receipt；双向轮换不等于继承 crash guarantees，不能同时由两种进程操作同一输出树。
- 旧 unfinished target 没有所有权凭据；原生会按 rename/skip/显式overwrite处理并在升级前清空未证明target。这会出现 `.v2`，是保留外部文件的必要差异，不应通过信任旧路径修掉。
- completed 保持完成，即使输出后来删除或被编辑也不会重新转换；这是现有语义，不是自动修复文件的承诺。历史测量缺失不能补成此次新测量。
- 旧相对路径按调用 cwd 解释；foreign scope、出界/错误 mirrored parent、符号链接政策不符会拒绝。不能把拒绝当作损坏状态覆盖。native新写入的路径已锚定绝对位置。
- 非 Unix 普通批处理可运行，`--resume` 仍明确拒绝 durable ownership；单文件/单URL的 `--resume` 也不支持。Windows lock/identity/fsync 是独立工作包，不宜与交互菜单合并。

### 下一小批只补三件可见改进

1. `--resume` 没找到当前 hash 的 base 时，非quiet stderr明确“未找到匹配配置的恢复状态，将开始新批次”，并显示安全的预期状态路径；不静默让用户误认为付费任务全部恢复。原版也只查精确文件，不应自动挑一个六位hash相近的checkpoint。flags/扫描深度/glob改变导致hash不同属于配置身份，不合并任务。
2. 加一次旧state接管提示：已恢复completed多少、待重试多少、旧target按何种冲突策略处理。stdout JSON继续只包含本次实际观察结果；既有报告的恢复投影仍单独使用snapshot。路径诊断不回显raw URL query、旧错误全文或模型参数。
3. foreign scope/legacy相对路径和显式overwrite在receipt前被kill的安全拒绝，给出具体恢复方法（回到原cwd/原配置，或确认输入后fresh显式overwrite），不提供自动重绑文件的捷径。

最少两个实际迁移进程用例：参考 codec在隔离目录生成base+sidecar，Rust真正执行 `--resume`，证明completed零请求、unfinished正好一次请求及named URL输出；旧failed target已有外部内容，验证rename保存原字节、skip零模型调用、overwrite只在显式策略下改变。已有 `recovery.rs` 已覆盖后一组，不必再复制大矩阵；只补前一组实际参考产物与提示断言。额外一例换cwd/foreign scope拒绝且原base/journal/output哈希不变。

文档清理可随协调器完成：`docs/state-storage.md` 开头“History export is still pending”已与现有 history 实现不符，但不能据此误判代码缺失。保留旧阶段的证据边界，把当前功能状态链接到最新CONTROL。

## 实施终点

一次冻结后运行新增 PTY/process、相关现有CLI/recovery测试及统一门禁；只对这一批的真实 workflow 做干净CLI验收。不重复格式全量corpus、旧性能计时或包矩阵。交付时分别说明：`-I` 实际转换已完成；doctor诊断已完成；自动Chromium安装、OAuth/local SDK、跨平台durable resume仍是哪些独立未完成能力。不能以一张帮助菜单代表这些运行时能力已经实现。
