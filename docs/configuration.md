# Rust 配置模型

Rust 核心将配置的结构验证与运行能力分开。配置描述用户期望；转换层决定是否能兑现某项能力。浏览器、OCR、模型路由等字段在结构上有效，并不代表对应功能已经可用；尚未实现的运行路径必须给出明确诊断。

## 默认值与配置选择

`config::defaults()` 返回与参考版本 1.2.0 全量模型一致的 JSON 对象：14 个顶层配置组，27 个模型共 169 个声明字段（含嵌套模型定义）。完整默认快照作为测试夹具跟踪，核心只嵌入约 13 KiB 的类型、枚举、边界和默认值事实。数据首次使用时解析，随后复用不可变元数据；没有引入完整 JSON Schema 引擎或 Python 运行时。

文件选择顺序为显式路径、`MARKITAI_CONFIG`、当前目录 `markitai.json`、用户目录 `config.json`。仅一个文件入选；不会合并项目和用户文件。其上可以递归应用临时 override，再执行规范化。数组和标量替换原值。

配置加载器选中了不存在的显式或环境路径时会在 stderr 警告并使用默认值，不继续选择低优先级文件；CLI 在此之前单独将缺失 `-c` 判为用法错误。已存在但无法读取、不是 UTF-8 JSON 对象、内容损坏或字段无效的文件都返回错误，不降级为默认值。

`MARKITAI_HOME` 是 Rust 开发隔离入口，替换通常的用户 `.markitai` 目录。序列化默认仍保留旧路径字面值；运行时使用 `config::state_path` 将默认 `~/.markitai/...` 路径解析到隔离目录。显式自定义路径不会被重定向。不要为了测试修改进程的 HOME。

## 规范化与验证

`config::normalize(&Value)` 返回有效配置，递归填入缺失默认值，包括 `model_list`、`providers`、自定义 preset 和域名配置中的条目。返回对象与输入不共享可变状态。`config::validate(&Value)` 只报告是否可规范化，不改变原对象；调用者需要使用规范化结果才能获得类型转换和默认值。

结构规则包括：

- bool、整数、浮点数、字符串、nullable、数组和映射的元素类型；字符串字段不会把数字或布尔值转换成文本。
- 旧模型支持的常见标量转换，如 `"yes"`→true、`"1_000"`→1000、整数值浮点数→整数；字符串 `" true "` 仍不是合法 bool。
- 所有声明的 Literal 枚举及 minimum/maximum 边界。未声明的范围不会自行收紧，例如结构层不为批量并发添加旧版不存在的 1024 上限。
- 模型部署必须含 `model_name` 和 `litellm_params.model`；供应商连接必须含 `id` 与 `provider`。
- fetch strategy priority 必须非空、没有重复且只含实际策略名，不能包含 auto。local-only 模式不能为空，带 `/` 的值必须为有效 IPv4/IPv6 网络。
- 已知模型中的未知字段在加载时忽略，与原模型默认规则一致；任意键映射仍按其声明的值类型验证。

内部 `output.filename` 和 `output.reserved_stem` 用于 CLI 输出路径和同批名称预留，会在规范化时保留，但不出现在公开默认值中，也不能通过公开 `config set` 创建。

## 定点编辑

`config::key_pointer` 处理 `llm.model_list[0].litellm_params.weight` 一类键。`config::parse_cli_value` 根据字段声明保留字符串，因此数字形密钥和名为 `true` 的目录不会变成数值/bool。`config::set_value` 验证已声明键及数组索引，检查新配置后才更新原始对象。失败不修改内容；成功只写变化的路径，保留原文件里的未知扩展键，不输出全量默认配置。

未知静态叶子（例如 `image.qualty`）是错误；类型化动态映射可以增加条目，例如 `presets.custom`。数组下标不能越界，也不能隐式创建部署。`--config-json` 为临时读取 override；与 config set/edit 同用会报用法错误，避免把临时值误写入文件。

CLI 保存采用同目录临时文件和原子替换。配置路径本身是符号链接时先解析目标，更新目标文件，保留链接。输出文件的符号链接策略与配置保存独立。

`config validate <path>` 的显式位置参数必须存在，否则返回用法错误 2。配置显示在模型对象中省略 null 字段，保留任意映射中的显式 null；直接 `config get` 查询 nullable 字段仍显示 `null`。

`config list/get/set` 的默认显示通过同一脱敏函数处理：密钥/token/密码等字段隐藏，`env:VAR` 保留引用名称，API base 只显示 scheme、host 和端口，丢弃用户信息、路径、query 和 fragment。HTTP header 的名称保留、所有值隐藏，形似 `env:VAR` 的 header 也按实际内联内容隐藏。单键读取先定位值再按键路径脱敏，因此敏感容器的子键仍可查询。脱敏不会改写配置文件；`--show-secrets` 明确请求原值。

## 环境引用

`env:VARIABLE` 在结构加载时保留原样，不要求变量存在。运行到需要凭据的路径时，调用 `resolve_env_value` 或 `resolve_optional`，传入环境快照与 strict 标志：strict 缺失变量报错，非 strict 返回 None。显式引用变量存在但为空字符串时仍算已找到。

Jina/Cloudflare 一类可选凭据的 fallback 环境变量仅在配置值缺失或为空时使用。显式 `env:MISSING` 在非 strict 模式解析失败后，不再回退到另一个变量。读取 `.env` 的顺序为进程环境、当前目录文件、隔离用户目录文件；已存在值优先，不修改宿主进程环境。

## 证据与边界

`tests/derive_config_contract.py` 使用明确传入的参考 checkout、其 Python 环境和 Pydantic 模型重新计算事实。它仅提取默认值和 schema 约束，不复制旧文档、注释或实现。当前事实对照参考提交 `ba374322f884b0e720b45466cc1196f4574a3da5`。

```sh
/path/to/reference/.venv/bin/python \
  crates/markitai-core/tests/derive_config_contract.py /path/to/reference
```

测试覆盖全量默认快照、文件选择优先级、缺失/损坏文件、override、嵌套部署默认值、类型和数值边界、策略/CIDR、未知字段、稀疏编辑回滚、字符串输入、环境引用、单独模型验证、脱敏和配置符号链接保存。Python 配置适配器消费相同的嵌入式 schema，并通过 `normalize_model` 构造顶层或嵌套模型。调度中心记录实际 Cargo/差分测试结果；生成事实文件成功不能替代 Rust 编译和运行测试。

目前明确的结构边界：JSON 整数保存为 i64/u64，超出 64 位可表示范围的整数拒绝；非有限浮点数拒绝。Python 的任意精度整数以及个别非标准 Infinity 输入不能逐值等价。错误按第一个字段给出，未复制 Pydantic 的聚合错误格式。dotenv 插值在不修改宿主环境的约束下仍需跨文件差分测试。运行能力清单仍以 [CLI 状态](cli.md) 和统一调度中心为准。
