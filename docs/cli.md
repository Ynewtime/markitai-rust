# Rust CLI 开发版

`markitai` 与 `mkai` 使用同一个 Rust 核心。当前这是迁移中的开发版本，完整参考契约见 [compatibility.md](compatibility.md)，尚未实现的请求必须返回明确错误。命令帮助通过 `markitai --help` 查看。

```sh
markitai note.txt
markitai note.txt --pure
markitai document.docx -o output/
markitai note.txt -o chosen.md --json
markitai ./documents -o output/ -j 4 --glob '**/*.txt'
markitai urls.urls -o output/ --url-concurrency 3
markitai urls.urls -o output/ --resume
markitai https://example.com/article -s static -o output/
markitai -c isolated.json --config-json '{"output":{"on_conflict":"skip"}}' note.txt -o output/
markitai note.txt -o output/ --config-json '{"output":{"report":true}}'
markitai serve --host 127.0.0.1 --port 3600 --no-open
markitai mcp
markitai note.txt -o output/ --log-level DEBUG --config-json '{"log":{"dir":"./logs","format":"json"}}'
markitai config edit
markitai init --local
```

## 已实现的命令行为

- 单文件和 URL 未给 `-o` 时输出 Markdown 到 stdout；`--pure` 去除 frontmatter；提供 `-o chosen.md` 可选择准确文件名。
- `--json -o` 输出 version 1.0 envelope，字段名与参考接口一致。运行失败仍有机器可读条目；参数错误只输出 stderr 并退出 2。
- 目录递归转换保留相对路径，支持重复 `--glob`、`!` 排除和 `--max-depth`，使用受限线程并发；目录中的 `.urls` 也会发现。同批任务预留独立名称，即使冲突策略为 overwrite/skip，也不会让两个新结果互相覆盖；大小写匹配按输出文件系统探测。URL 列表支持文本和 JSON 两种格式。
- `--llm-concurrency` 限制整个运行中的在途模型请求，目录中的文件与 URL 共用该上限；重试等待和缓存命中不占请求槽。文件与 URL 的转换并发仍分别由 `-j` 和 `--url-concurrency` 控制。
- 目录/URL 列表普通转换项失败退出 10；单项失败退出 1，成功退出 0；状态存储致命错误退出 1，中断退出 130/143。`--quiet` 仍显示错误。
- 四种输入模式支持持久 JSON 报告。`output.report` 为 null 或省略时，目录/URL 列表默认启用，单文件/URL 默认关闭；true/false 显式覆盖。报告写入输出目录的 `.markitai/reports/`，各模式的字段和计数差异见 [reports.md](reports.md)。
- 报告发布失败保留已完成文件及 stdout JSON 条目，并退出非零；报告不替代 stdout envelope。stdout 转换、dry run、无可恢复状态的空目录和失败/跳过的单项不生成报告；批量部分失败仍可生成报告。报告的 skip 冲突策略保留已有报告。
- Unix 目录/URL 列表每次保存恢复状态；`--resume` 合并新发现任务、保留完成项并重试未完成项。输出归属凭证保护隐式重试，旧状态按普通冲突策略升级。首次 Ctrl-C 停止派发、同步状态并等待在途转换，退出 130；再次中断立即退出。详见 [恢复状态](state-storage.md) 与 [输出归属](output-ownership.md)。
- `--record-history` 将本次实际处理项保存到隔离 home 下的 `serve/jobs/`，包含独立的最终文档、资产和兼容元数据；归档失败只警告，stdout/dry-run/中断不归档。开关覆盖环境和配置，详见 [历史归档](history.md)。
- 配置优先级由核心解析；根级 `-c` 和 `--config-json` 对子命令同样生效。布尔参数支持显式否定；重复正反开关以最后一个为准，preset 应用后显式参数覆盖。
- `config list/get/path/validate/set/edit` 可用；list 支持 JSON/YAML/table；默认隐藏凭据。set/edit 原子更新配置，保留未知字段，不写入临时 `--config-json` 内容。
- `config edit` 是终端中的导航编辑器：`/关键字` 模糊搜索、数字选择当前页设置、`n/p` 翻页，也可输入完整配置 key。输入值后按同一配置 schema 验证并立即保存；无效值不落盘。空输入保留当前值，`:empty` 写空字符串，`:cancel` 返回，`q` 退出。列表隐藏凭据，外部修改配置后拒绝覆盖，要求重新打开。
- 编辑器遍历标量设置和嵌套配置，跳过数组、字典、prompts、presets、domain_profiles；复杂值仍用 `config set`。界面使用按行导航，不复刻参考版全屏方向键界面；顶层转换选择器 `-I/--interactive` 仍未实现。stdin/stderr 非终端时明确拒绝，不等待脚本输入。Unix 敏感值输入暂时关闭终端回显；此时 Ctrl-C/SIGTERM 恢复回显后退出 130/143，不保存未确认值。其他平台敏感输入明确拒绝，可使用 `config set` 保存 `env:VARIABLE` 引用。
- `init [--local|-o path]` 提供位置选择与已有文件 update/overwrite/keep；默认 keep。`init --yes` 无提示创建配置，已有配置只追加新发现的模型、保留已有模型和其他字段，重复运行无变化。`-o` 指向已有目录时写入该目录的 `markitai.json`。坏配置仅允许交互明确覆盖，自动更新报错并保留原字节。
- 初始化只检测当前核心支持的 API 环境配置和 `MODEL`，不发模型请求，不保存凭据明文，生成配置默认关闭 LLM。不检测订阅登录、不安装运行时、不创建参考版 `.env` 模板；可用环境变量或现有隔离 home 下的 `.env` 配置凭据。交互编辑/初始化与其他新功能的验证状态以调度中心为准。
- `doctor [--json]` 报告当前开发版原生能力；诊断 JSON 暂为 Rust 新 schema，尚未与旧 doctor 逐字段对齐。
- `serve` 启动原生 REST 服务，支持提交文件/URL、任务快照与 SSE、结果/资产/ZIP 下载及持久历史。沿用 host、port、no-open、no-auth、allowed-host 参数；完整工作区界面和其他未迁移接口见 [REST 服务](serve.md)。
- `mcp` 通过标准输入/输出提供 `convert_document`、`convert_url`、`batch_convert`、`job_status` 四个工具。配置与文件输出沿用同一核心，批处理任务保存在当前 MCP 进程内；协议和结果边界见 [MCP 服务](mcp.md)。
- `--dry-run` 仅枚举输入和目标，不调用转换器、不创建输出目录。
- `--compress/--no-compress` 映射共享图片处理配置；未启用 LLM/OCR 的独立图片返回 `image_only` 跳过状态，不写空文档。
- 非 pure 本地文档及文本 URL 的 LLM 结果可逐块跨进程复用；`--no-cache` 跳过读取但仍写入成功结果，`--cache` 清除此绕过设置，不强制启用已禁用的缓存。`--no-cache-for` 接受逗号分隔的 glob，JSON 条目的 `cache_hit/llm_cache_hit` 反映实际命中。
- 静态 HTML/文本抓取支持独立页面缓存：无验证头时按 TTL 复用，有 ETag/Last-Modified 时发送条件请求。`fetch_cache_hit` 记录直接复用或 304 命中；`cache_hit/llm_cache_hit` 仍只表示 LLM 缓存。显式 `-s static` 与配置文件中的默认策略使用独立缓存作用域。
- `cache stats [--json] [-v] [--limit N]` 查看 LLM 与抓取缓存；不存在的数据库不会因查看而创建。LLM 详细列表最多返回 1,000 条。`cache clear [-y]` 清理两个缓存，未给 `-y` 时需要在终端确认。清理前检查两个数据库，后续部分失败会明确报告并退出非零；浏览器域名缓存仍未实现。

- `--ocr` 在 macOS 使用原生 Vision 识别独立图片，读取原始像素；`MARKITAI_NO_VLM_OCR` 可选择本地识别后仅发送文字增强。语言和平台限制见 [本地 OCR](ocr.md)。
- 本地 PDF 和静态/自动下载的 PDF 支持 macOS 逐页 OCR 与截图。下载字节直接复用，支持重定向和无后缀 URL；PDF 仅截图模式仍保留 Markdown，URL `pure` 优先级不变。详见 [PDF 媒体](pdf-ocr.md) 与 [抓取](fetch.md)。
- `-s playwright` 直接控制本机 Chromium；`auto` 可在静态质量失败时回退浏览器。URL 截图支持完整长页分块，`--screenshot-only` 不隐含 LLM，历史归档保留所有分块。未给 `-o` 的无 LLM 仅截图模式使用配置输出目录或当前目录。依赖和边界见 [浏览器](browser.md)。

## 明确的迁移缺口

URL 抓取的其他策略、图片/视觉 URL 的 LLM 缓存、非 Unix 断点恢复、Batch API、顶层交互转换选择器、订阅登录、serve 完整工作区仍有迁移缺口。pure 按参考行为绕过 LLM 缓存；图片和视觉 URL 的 LLM 增强当前每次重新处理，静态页面缓存遵循独立规则，不能将文档缓存命中理解成所有输入已支持缓存。其余未实现的开关/命令请求会失败并说明原因。Office 演示与文字文档可通过可选的独立 LibreOffice 安装获得全页截图和 OCR 补充，详见 [Office 渲染](office-rendering.md)；表格截图和其他平台本地 OCR/PDF 渲染仍未完成；未实现的选项只在遇到相关格式或图片时拒绝，不应阻断纯文本转换。独立栅格图片、完整多页 TIFF 和 SVG 可经 LLM 视觉模型读取。alt/desc 已接入真实图片引用、结构化分析及 images.json 合并，详见[图片分析](image-enrichment.md)；需要启用 LLM。rich/standard preset 仍不是对所有格式可用的完整模式。

持久报告、可选历史导出和 Unix 批量恢复已实现；单项和非 Unix 恢复仍明确拒绝。普通非 Unix 转换保留既有行为，但尚未完成实机验证。混合目录分别应用文件与 URL 并发上限。URL 列表的自定义文件名只允许一个安全 basename；旧实现的名称清理细节尚待配对验收。CLI 帮助布局、移除选项迁移提示、非 Unix 进程中断清理及全部非 ASCII 终端行为仍需专门测试。

具体文件格式支持取决于核心当前实现，注册参考扩展名不意味着全部可用。性能与质量对比未完成前，不承诺生产替代、完整旧版兼容或具体加速比。

## 文件日志

转换的文件日志默认关闭（`log.dir=null`）。设置目录后，默认文件级别为 INFO；
`--log-level DEBUG|INFO|WARNING|ERROR|CRITICAL` 覆盖 `log.level`，仅影响文件。
`--quiet`/`--verbose` 沿用终端策略，日志不进入 stdout Markdown 或 `--json` envelope。
配置、缓存、服务等子命令保留自己的输出，不因根级 `--log-level` 启用转换日志。

`MARKITAI_LOG_DIR` 的非空值覆盖目录，合法的 `MARKITAI_LOG_FORMAT=text|json` 覆盖格式。
路径支持 `~`；显式隔离 `MARKITAI_HOME` 时，`~/.markitai/...` 路径跟随隔离 home。
每次运行独立建立 `markitai_<日期>_<时间>_<微秒>_<进程号>.log`，Unix 新文件权限为 0600。
持久报告的 `log_file` 指向本次首个日志文件；轮转文件使用同一前缀和序号。

文本日志逐行写入，换行转义；JSON 日志每行含 `ts`、`lvl`、`src`、`msg`，
其中 `src="cli"`。记录配置加载、实际文档开始/完成/跳过/失败、转换 warnings、
CLI 诊断和退出状态；不记录文档正文、模型请求响应或全配置，也不截获所有核心库和第三方内部日志。
线程共享同一个受锁保护的 sink，整行写入，退出时 flush/sync。
URL 的用户信息、全部 query/fragment、可疑路径段和已知配置/环境凭据在日志及 CLI 诊断前脱敏；
这是诊断脱敏，不改变输出文档、报告或 stdout JSON 的既有数据合同。

`log.rotation` 支持正数 B/KB/MB/GB/KiB/MiB/GiB，默认 `10 MB`；
`log.retention` 支持秒、分钟、小时、天、周，默认 `7 days`。
超阈值在下一整条记录之前切换文件，单条记录不会拆开。
启动时只清理符合 Markitai 日志命名的过期普通文件，跳过符号链接及被其他原生运行锁定的文件。
参考 Loguru 的时间点轮转、组合时长等其他表达式会明确报错，不会静默忽略。
日志初始化失败使转换报错；运行中写入或最终 flush 失败会在 stderr 报错并退出非零，
保留已完成的文档和已发出的唯一 stdout JSON，不追加第二个 envelope。

## 开发验证

```sh
cargo test -p markitai-cli
cargo run -p markitai-cli --bin markitai -- --help
cargo build --release -p markitai-cli --bin markitai
```

CLI 测试使用临时工作目录与 `MARKITAI_HOME`，不修改用户 `~/.markitai`。手动测试也应将 `MARKITAI_HOME` 指向项目的忽略目录，并按需只读传入测试凭据。

历史 round7 报告实现已通过源码门禁和四个成功场景的 clean-source release 差分审计。该证据早于本轮恢复调度；最新门禁与剩余范围见 [调度中心](CONTROL.md) 和 [报告验证说明](reports.md#validation-scope-and-remaining-work)。
