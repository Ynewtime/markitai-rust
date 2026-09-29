# R24 后：旧增强历史的原位读取与重试

状态：**下一任务设计，尚未实施或执行验收**。2026-09-29 根据 R24 工作树与只读参考源码核对；参考提交为 CONTROL 记录的 `ba374322f884b0e720b45466cc1196f4574a3da5`。本次未运行原项目、构建或转换，未读取用户 `~/.markitai` 的文件内容，连目录盘点也没有必要执行。

建议下一批只闭环一条路径：**已有旧 CLI/serve 增强历史 → 原位展示正确的 base/llm 结果 → URL 或保留上传的项目用原接口重试 → 重启后仍正确**。不做全 home 搬迁，不新增公开 flags，不批量改写缓存键。

## 先区分读取、复用与协议升级

| 存量数据 | 当前实际能力 | 本批判断 |
| --- | --- | --- |
| `serve/jobs/<12-hex-id>/meta.json` 和 `out/` | 已扫描终态历史，读取原有版本形状，补默认 options/finished_at/CLI 文件 retryable；已有旧 `assets/` 的结果与 ZIP 测试 | 不需要搬目录。缺的是少量旧字段的语义适配，见下文。 |
| 输出目录 `.markitai/states/*.state.json` 与 `.jsonl` | 已读旧 1.0 状态、重放旧事件、恢复 completed 项、处理 bare URL → named URL；已有 codec 差分及真实 resume 用例 | 原位读取不是未实现。下一次开始 native run 会写 generation/scope/sequence，这是协议升级，不能误称完全无写入。 |
| `cache.db` 的旧 `cache` 表 | 表结构可原位统计/清理，旧行与 native 行可共存 | 旧 LLM 答案不自动成为 native 命中；typed 元数据、保护标记和图像身份不同，直接换 key 会伪造有效性。 |
| `fetch_cache.db` 的旧 `fetch_cache` 表 | 旧列可读统计；正常写入会补缺失列，保留兼容表形状 | `native-fetch-v1` 有意隔离 Python 已抽取正文，不能把旧抽取结果当本次 native 结果复用。 |

“保留旧行”不等于永不删除：共享数据库的容量淘汰和显式 clear 仍作用于表内数据；不承诺新旧程序同时写入时能够无损回退。LLM 的旧 key 是 `sha256(prompt|model|sha256(content))[:32]`，fetch 的旧 key 是 `sha256("2\0" + URL/strategy scope)[:32]`；新 namespace 分离有明确正确性原因，不应为增加命中率撤掉。

定位：native [`server/store.rs`](../../crates/markitai-cli/src/server/store.rs) 的 `rehydrate`；[`tests/serve.rs`](../../crates/markitai-cli/tests/serve.rs) 的 `legacy_visible_assets_and_multi_history_zip_names_follow_the_saved_contract`；[`run_state/store.rs`](../../crates/markitai-cli/src/run_state/store.rs) 的 `begin`；[`tests/recovery.rs`](../../crates/markitai-cli/tests/recovery.rs) 的 `legacy_failed_targets_without_receipts_use_the_selected_ordinary_conflict_policy`；[`fetch_cache.rs`](../../crates/markitai-core/src/fetch_cache.rs) 的 `key`/`legacy_rows_are_inspectable_and_write_migrates_only_missing_columns`；[`llm_cache.rs`](../../crates/markitai-core/src/llm_cache.rs) 的 `key`/`document_key`/`vision_key`。

## 确认的缺口：旧 `output_name` 指向增强文件

原版 `packages/markitai/src/markitai/serve/jobs.py:405–462` 的 `_base_output_name` 和 `_item_from_payload` 已专门处理这个历史形状：

```json
{
  "item_id": "i1",
  "name": "https://example.invalid/page",
  "kind": "url",
  "status": "done",
  "output": "page.html.llm.md",
  "output_name": "page.html.llm.md",
  "llm_enhanced": true
}
```

这里 `output` 是实际文件；旧 recorder 曾错误地把同一增强文件名也放进应表示 base 文件名的 `output_name`。原版加载后把后者规范为 `page.html.md`，不重命名物理文件。原版的 `tests/unit/serve/test_serve_history.py:641–681` 明确验证这个旧记录的普通重试写入 `page.html.md`，避免 `.llm.llm.md`。

当前 Rust 的三个接点没有一致处理：

1. `server/store.rs:166–183` 用默认 `Item` 叠加 raw JSON，原样保留旧 `output_name`。缺少 `llm_enhanced` 时也保持默认 false，而原版按实际 `output` 的 `.llm.md` 后缀补值；显式 false 则必须保留。
2. `server/files.rs:125–132` 的 result 路由单独计算 base，将 `page.html.llm.md` 当 base 文件，返回错误的 `variant:"base"`，也漏掉真实 base sibling。
3. `server/files.rs:281–315` 的 `item_base` 同样可以得到 `page.html.llm`。`rerun.rs` 随后指定 `{base}.md`，普通 retry 会把未增强正文写进旧增强文件名；若再增强则可能形成双 `.llm`。删除使用同一错误家族，也可能遗留真实 base 文件。

以上是源码推导的具体行为，**尚未运行新复现，不记为已验证修复**。现有 legacy history 测试使用的是已经正确的 `output_name:"a.txt.md"`，因此不覆盖这个缺口。

## 最小实现边界

一个 worker 持有 `server/store.rs`、`server/files.rs` 和新的 `tests/serve/legacy_history.rs`；协调器仅负责测试注册、文档和构建。通常不需修改 core、配置、数据库或依赖。若 `rerun.rs`/删除接点要改，只调整它们调用统一家族解析器的接线，不重写事务流程。

- 在旧项目加载适配中，将非空 `output_name` 末尾的一次 `.llm.md` 转为 `.md`；保留 `output`、原文件名、item ID、顺序、时间和已有用户结果。缺失 `llm_enhanced` 才按原版从 `output` 推导，不能覆盖显式 false。
- 已有 `native_bases[item_id]` 的 native 记录以其精确身份为准。合法源名本来就是 `notes.llm` 时，base 可以是 `notes.llm.md`；不能不分来源全局剥离后缀，误认成 `notes.md` 的增强结果。
- 让 result、retry 和 deletion 使用同一项内输出家族解析。原生 indexes 仍优先，旧数据只恢复已知参考语义；路径安全、兄弟项家族冲突检查、共享资产保护继续生效。互相矛盾或存在重叠归属的记录不得靠猜测获得覆盖/删除权限；保留安全文件下载与整包导出的能力。
- 本批只覆盖上述合法旧记录及 missing enhanced flag，不顺带放宽所有畸形 JSON、任意 job ID、非终态元数据或未知文件扩展名。旧 CLI 文件未保留 upload 时继续 409；URL 和确有 upload 的旧 web 项才可重试。
- 原接口不变：`serve`、history/snapshot/result/files/archive 和已有 `POST .../retry`、`DELETE .../items/...`。不增加“导入”“迁移”“修复全部历史”公开命令。

## 可回退的边界

**启动、列历史、预览和下载只做内存适配，不把规范化值自动写回 `meta.json`，也不生成 `native_bases` 文件索引。** 对没有待恢复 native 文件事务的合法旧 archive，读取前后 metadata/输出/资产的 bytes、hash 和目录成员完全不变。关掉 native server 后，原 Python 服务仍读到原始历史；这条回退不需要恢复备份。

用户明确 retry/delete 后走现有 staged publication 和持久化协议，metadata 在普通提交时自然记录正确字段。失败重试与提交前中断必须仍能恢复此前结果；成功 retry/delete 是用户要求的实际修改，不承诺自动撤销。保留旧程序能识别的字段形状与版本，native 附加字段不替代原字段。无需仅为只读适配批量复制历史，也不能把普通转换的成功写入包装成“只读迁移”。

旧 batch checkpoint 的原位升级是另一项真正需要单独定义回退快照的工作：当前只对 corrupt base 做 quarantine，合法旧 base 在 `begin` 会被 native checkpoint 替换；Python 不理解 native journal fence，也不参与其完整所有权锁。以后若实现可回退接管，需在停用另一实现的写入后成对保存原 base/journal，并绑定 scope/哈希，而非此次顺手增加“旧状态直接可并行使用”的承诺。

## 一次完整验收

全部使用作者构造的私有 `MARKITAI_HOME/serve/jobs`、独立配置和 loopback 服务；HOME 不变。无需读取真实历史、真实配置、模型账号或运行原 CLI。原版 helper 的差分如需要，只给独立临时夹具并禁止环境配置加载。

1. **原位读与回退**：至少两个旧 enhanced 项，一项包含 base+enhanced，一项只含 enhanced；使用旧 `output_name`，其中一项省略 `llm_enhanced`。启动、列 history、snapshot、result、单文件和 ZIP 下载，核对正确 variant/完整资产 bytes；停止重启后仍一致。前后完整目录 inventory 与 hashes 不变。
2. **原接口普通重试**：旧 URL 项用 loopback 正文重试 `{"options":{"llm":false}}`；输出稳定落在 `page.html.md`，不存在 `.llm.llm.md`，状态和 result 是 base，记录一次预期 HTTP 请求。重启后保持正确。旧 CLI 文件无 upload 仍 409。
3. **失败保全**：同一合法旧 URL 项在 loopback 明确返回失败时，旧 metadata 与实际 base/enhanced/共享资产均不变；生产事务测试已有的提交中断 fixture 复用到旧家族，不再写一套故障框架。
4. **身份负向与删除**：有 `native_bases` 的原生 `notes.llm` 不被错规范化；显式 false 保留；兄弟项争用家族时拒绝 retry；删除旧 enhanced 项清除正确的一对 Markdown，仅保留仍被 sibling 引用的资产；不能扩大为按文件名前缀清理。

结束条件是这条存量历史工作流的真实进程与重启验收通过，不是宣称所有旧状态与缓存都已迁移。文档同步时可顺带纠正 [`history.md`](../history.md) 中“serve 仍未实现”“未来 native server 消费”的旧句子，当前 [`serve.md`](../serve.md) 已描述实际原位读取；本设计未修改这些文档。
