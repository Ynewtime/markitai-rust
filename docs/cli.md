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
```

## 已实现的命令行为

- 单文件和 URL 未给 `-o` 时输出 Markdown 到 stdout；`--pure` 去除 frontmatter；提供 `-o chosen.md` 可选择准确文件名。
- `--json -o` 输出 version 1.0 envelope，字段名与参考接口一致。运行失败仍有机器可读条目；参数错误只输出 stderr 并退出 2。
- 目录递归转换保留相对路径，支持重复 `--glob`、`!` 排除和 `--max-depth`，使用受限线程并发；目录中的 `.urls` 也会发现。同批任务预留独立名称，即使冲突策略为 overwrite/skip，也不会让两个新结果互相覆盖；大小写匹配按输出文件系统探测。URL 列表支持文本和 JSON 两种格式。
- 目录/URL 列表普通转换项失败退出 10；单项失败退出 1，成功退出 0；状态存储致命错误退出 1，中断退出 130/143。`--quiet` 仍显示错误。
- 四种输入模式支持持久 JSON 报告。`output.report` 为 null 或省略时，目录/URL 列表默认启用，单文件/URL 默认关闭；true/false 显式覆盖。报告写入输出目录的 `.markitai/reports/`，各模式的字段和计数差异见 [reports.md](reports.md)。
- 报告发布失败保留已完成文件及 stdout JSON 条目，并退出非零；报告不替代 stdout envelope。stdout 转换、dry run、无可恢复状态的空目录和失败/跳过的单项不生成报告；批量部分失败仍可生成报告。报告的 skip 冲突策略保留已有报告。
- Unix 目录/URL 列表每次保存恢复状态；`--resume` 合并新发现任务、保留完成项并重试未完成项。输出归属凭证保护隐式重试，旧状态按普通冲突策略升级。首次 Ctrl-C 停止派发、同步状态并等待在途转换，退出 130；再次中断立即退出。详见 [恢复状态](state-storage.md) 与 [输出归属](output-ownership.md)。
- `--record-history` 将本次实际处理项保存到隔离 home 下的 `serve/jobs/`，包含独立的最终文档、资产和兼容元数据；归档失败只警告，stdout/dry-run/中断不归档。开关覆盖环境和配置，详见 [历史归档](history.md)。
- 配置优先级由核心解析；根级 `-c` 和 `--config-json` 对子命令同样生效。布尔参数支持显式否定；重复正反开关以最后一个为准，preset 应用后显式参数覆盖。
- `config list/get/path/validate/set` 可用；list 支持 JSON/YAML/table；默认隐藏凭据。set 原子更新配置，不写入临时 `--config-json` 内容。
- `init --yes [--local|-o path]` 创建最小配置；已有文件不覆盖。
- `doctor [--json]` 报告当前开发版原生能力；诊断 JSON 暂为 Rust 新 schema，尚未与旧 doctor 逐字段对齐。
- `--dry-run` 仅枚举输入和目标，不调用转换器、不创建输出目录。
- `--compress/--no-compress` 映射共享图片处理配置；未启用 LLM/OCR 的独立图片返回 `image_only` 跳过状态，不写空文档。
- 非 pure 本地文档的 LLM 结果可跨进程复用；`--no-cache` 跳过读取但仍写入成功结果，`--cache` 清除此绕过设置，不强制启用已禁用的缓存。`--no-cache-for` 接受逗号分隔的 glob，JSON 条目的 `cache_hit/llm_cache_hit` 反映实际命中。
- 静态 HTML/文本抓取支持独立页面缓存：无验证头时按 TTL 复用，有 ETag/Last-Modified 时发送条件请求。`fetch_cache_hit` 记录直接复用或 304 命中；`cache_hit/llm_cache_hit` 仍只表示 LLM 缓存。显式 `-s static` 与配置文件中的默认策略使用独立缓存作用域。
- `cache stats [--json] [-v] [--limit N]` 查看 LLM 与抓取缓存；不存在的数据库不会因查看而创建。LLM 详细列表最多返回 1,000 条。`cache clear [-y]` 清理两个缓存，未给 `-y` 时需要在终端确认。清理前检查两个数据库，后续部分失败会明确报告并退出非零；浏览器域名缓存仍未实现。

## 明确的迁移缺口

URL 抓取的其他策略、图片/URL 的 LLM 缓存、非 Unix 断点恢复、Batch API、交互配置、订阅登录、serve、MCP、文件日志仍有迁移缺口。pure 按参考行为绕过 LLM 缓存；图片和 URL 的 LLM 增强当前每次重新处理，静态页面缓存遵循独立规则，不能将文档缓存命中理解成所有输入已支持缓存。其余未实现的开关/命令请求会失败并说明原因。本地 OCR、截图和 alt/desc 图片分析仍未完成；这些选项只在遇到相关格式或图片时拒绝，不应阻断纯文本转换。独立栅格图片可经 LLM 视觉模型读取。rich/standard preset 仍不是对所有格式可用的完整模式。

持久报告、可选历史导出和 Unix 批量恢复已实现；单项和非 Unix 恢复仍明确拒绝。普通非 Unix 转换保留既有行为，但尚未完成实机验证。混合目录分别应用文件与 URL 并发上限。URL 列表的自定义文件名只允许一个安全 basename；旧实现的名称清理细节尚待配对验收。CLI 帮助布局、移除选项迁移提示、非 Unix 进程中断清理及全部非 ASCII 终端行为仍需专门测试。

具体文件格式支持取决于核心当前实现，注册参考扩展名不意味着全部可用。性能与质量对比未完成前，不承诺生产替代、完整旧版兼容或具体加速比。

## 开发验证

```sh
cargo test -p markitai-cli
cargo run -p markitai-cli --bin markitai -- --help
cargo build --release -p markitai-cli --bin markitai
```

CLI 测试使用临时工作目录与 `MARKITAI_HOME`，不修改用户 `~/.markitai`。手动测试也应将 `MARKITAI_HOME` 指向项目的忽略目录，并按需只读传入测试凭据。

历史 round7 报告实现已通过源码门禁和四个成功场景的 clean-source release 差分审计。该证据早于本轮恢复调度；最新门禁与剩余范围见 [调度中心](CONTROL.md) 和 [报告验证说明](reports.md#validation-scope-and-remaining-work)。
