# Rust CLI 开发版

`markitai` 与 `mkai` 使用同一个 Rust 核心。当前这是迁移中的开发版本，完整参考契约见 [compatibility.md](compatibility.md)，尚未实现的请求必须返回明确错误。命令帮助通过 `markitai --help` 查看。

```sh
markitai note.txt
markitai note.txt --pure
markitai document.docx -o output/
markitai note.txt -o chosen.md --json
markitai ./documents -o output/ -j 4 --glob '**/*.txt'
markitai urls.urls -o output/ --url-concurrency 3
markitai https://example.com/article -s static -o output/
markitai -c isolated.json --config-json '{"output":{"on_conflict":"skip"}}' note.txt -o output/
```

## 已实现的命令行为

- 单文件和 URL 未给 `-o` 时输出 Markdown 到 stdout；`--pure` 去除 frontmatter；提供 `-o chosen.md` 可选择准确文件名。
- `--json -o` 输出 version 1.0 envelope，字段名与参考接口一致。运行失败仍有机器可读条目；参数错误只输出 stderr 并退出 2。
- 目录递归转换保留相对路径，支持重复 `--glob`、`!` 排除和 `--max-depth`，使用受限线程并发；目录中的 `.urls` 也会发现。同批任务预留独立名称，即使冲突策略为 overwrite/skip，也不会让两个新结果互相覆盖；大小写匹配按输出文件系统探测。URL 列表支持文本和 JSON 两种格式。
- 目录/URL 列表任意项失败退出 10；单项失败退出 1，成功退出 0。`--quiet` 仍显示错误。
- 配置优先级由核心解析；根级 `-c` 和 `--config-json` 对子命令同样生效。布尔参数支持显式否定；重复正反开关以最后一个为准，preset 应用后显式参数覆盖。
- `config list/get/path/validate/set` 可用；list 支持 JSON/YAML/table；默认隐藏凭据。set 原子更新配置，不写入临时 `--config-json` 内容。
- `init --yes [--local|-o path]` 创建最小配置；已有文件不覆盖。
- `doctor [--json]` 报告当前开发版原生能力；诊断 JSON 暂为 Rust 新 schema，尚未与旧 doctor 逐字段对齐。
- `--dry-run` 仅枚举输入和目标，不调用转换器、不创建输出目录。
- `--compress/--no-compress` 映射共享图片处理配置；未启用 LLM/OCR 的独立图片返回 `image_only` 跳过状态，不写空文档。

## 明确的迁移缺口

持久缓存、历史、断点恢复、Batch API、交互配置、订阅登录、serve、MCP、文件日志尚未在 CLI 实现。对应开关/命令请求会失败并说明原因。`--no-cache` 可用，因为当前每次请求均重新执行，不存在缓存命中。本地 OCR、截图和 alt/desc 图片分析仍未完成；这些选项只在遇到相关格式或图片时拒绝，不应阻断纯文本转换。独立栅格图片可经 LLM 视觉模型读取。rich/standard preset 仍不是对所有格式可用的完整模式。

目录批量当前有 stdout JSON 汇总，没有旧格式的持久 report/state 文件；这属于尚未完成的兼容项。同一目录内混合文件/URL 当前共享批量并发上限，尚未分别应用两类限制。URL 列表的自定义文件名只允许一个安全 basename；旧实现的名称清理细节尚待配对验收。CLI 帮助布局、移除选项迁移提示、进程中断清理及全部非 ASCII 终端行为仍需专门测试。

具体文件格式支持取决于核心当前实现，注册参考扩展名不意味着全部可用。性能与质量对比未完成前，不承诺生产替代、完整旧版兼容或具体加速比。

## 开发验证

```sh
cargo test -p markitai-cli
cargo run -p markitai-cli --bin markitai -- --help
cargo build --release -p markitai-cli --bin markitai
```

CLI 测试使用临时工作目录与 `MARKITAI_HOME`，不修改用户 `~/.markitai`。手动测试也应将 `MARKITAI_HOME` 指向项目的忽略目录，并按需只读传入测试凭据。
