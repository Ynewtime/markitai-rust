# 下一轮服务实施计划：内嵌 Web UI 与 LLM 设置

2026-09-29，只读契约审计，尚未实现本计划。参考仓库 HEAD 为
`ba374322f884b0e720b45466cc1196f4574a3da5`；Rust HEAD 为
`e04eaf2cd22559737d2a3cba97a037ab7a695e68`，读取的是 round20 冻结中的工作树，
并非声称它与 HEAD 字节一致。未启动原版、转换器、模型、浏览器或构建；没有读取真实用户配置、认证文件或凭据值。

## 完整工作包与并行所有权

下一轮可完成的闭环：`markitai serve` 打开内嵌页面 → 查看/添加连接 → 发现或手填模型 → 瞬时连接测试 → 原子保存 → 创建文件/URL任务 → SSE进度 → 正文/源文预览与下载 → 增强/重试 → 历史/删除 → 服务重启后配置和历史仍可用。OAuth登录、第三方本地CLI供应商、复杂编辑器集成不混入本轮。

| lane | 独占文件建议 | 可独立交付内容 |
|---|---|---|
| 设置与持久化 | 新 `server/settings/{mod,types,store,identity,handlers}.rs`、`tests/serve_settings.rs`、`docs/service-settings.md` | 全部配置CRUD、字段presence、revision/ID、凭据生命周期、配置快照；临时目录路由/磁盘测试 |
| provider 网络 | 新 `server/providers/{mod,discovery,probe}.rs`、`tests/serve_providers.rs`；协调后单独核心probe子模块 | 支持现有原生供应商的发现/测试、超时/缓存/脱敏；全部loopback mock |
| Web UI | 新 `server/web/{index.html,app.js,api.js,settings.js,style.css}`、新 `server/web.rs`、`tests/serve_web.rs`、`docs/web-ui.md` | 从零写静态UI及嵌入路由，不复制原React源码；离线单binary消费 |
| coordinator | `app.rs`、`server/{mod,http,rerun,types,security}.rs`、核心LLM公开入口/manifest、`docs/serve.md` | 配置来源和快照接缝、路由权限、动态capabilities、启动浏览器、最终门禁/真实浏览器验收 |

UI只使用现有JSON API与下列冻结schema，provider只接收设置模块返回的连接快照，三者不共同编辑大文件。建议先冻结接口，在同一次最终服务进程验收里验证闭环，避免分别重复完整转换/发布基准。

## 当前Rust缺口与最小共享接缝

- `server/mod.rs:37–46` 的 `run(cfg: Value, ServeOptions)` 丢失路径来源；`app.rs:1363` 的 Serve 分支已调用 `config::load`。新增 `SettingsSource { path: PathBuf, origin: ConfigOrigin, overrides: Option<Value> }`，由coordinator在load时一起构造。路径按 `config::selected_path`：显式 `-c` → `MARKITAI_CONFIG` → 当前目录markitai.json → 私有home/config.json；完全没有文件时选择私有home/config.json并标default。启动时固定绝对目标，不能请求时依cwd另选。
- `--config-json` 是会话覆盖。不能把整个normalized cfg保存成配置，或把会话凭据自动落盘。建议非LLM覆盖保持会话有效；若覆盖含llm.model_list/providers，明确禁止这些设置写入并提示用配置文件（本轮可记录为原生边界），不要返回保存成功但仍被覆盖值遮蔽。
- `State.cfg: Value` 替换为 `settings::Store` 持有的 `Arc<Value>` 快照；`http.rs:39,41,191` 与 `rerun.rs:101` 三组读点改取快照。启动的file/url semaphore不因LLM设置改变。每个新任务/重试只取一次快照；已在跑的任务不换模型/密钥；不得持全局锁等待网络、转换或SSE。
- 建议接口：`Store::new(base: Value, source: SettingsSource) -> ApiResult<Self>`；`snapshot(&self) -> Arc<Value>`；`view(&self) -> SettingsPayload`；`mutate(&self, command: Mutation) -> ApiResult<SettingsPayload>`；`connection(&self, selection: ConnectionSelection) -> ApiResult<ConnectionDraft>`。`ConnectionDraft`只在进程内携带原始值，不derive Debug/公共Serialize；HTTP凭据显式读取另走专用投影。
- `server/types.rs:167` 的ApiError.detail目前只能String；新增结构化detail构造，不改变现有错误输出。设置409需 `{detail:{code:"stale_revision",current_revision:"..."},code:"conflict"}`，原UI优先读取detail.code。
- `server/security.rs:guard` 已提供 `Trusted(loopback || token)`。所有 `/api/settings/llm...` 再检查Trusted，并让成功/错误均 `Cache-Control: no-store`；远端无token401优先，`--no-auth`远端设置403。原版实际同样允许有效token远端，不能被“loopback-only”旧注释误导（reference app.py:490,1821）。
- 静态资源保持Host/Origin防护；未知 `/api/*` 必须JSON404。`/`改实际HTML，资源用固定内嵌表，不能开放任意工作目录。新增浏览器启动仅启动系统URL处理程序，不shell拼接；`--no-open`完全不启动；URL用 `#token=`，不把token拼进访问日志。

## 设置HTTP合同（保留原路径与方法）

出处 `packages/markitai/src/markitai/serve/app.py:1876–2690`；请求类型见 `serve/schemas.py:77–249`。

| 方法与 `/api/settings/llm` 后缀 | 请求/行为 |
|---|---|
| GET 空后缀 | SettingsPayload，模型列表始终无原始key/path/query |
| GET `/detected` | 旧快速添加列表：provider/model/label/requires_api_key |
| GET `/providers?refresh=false` | `{providers:[cards]}`，不同kind的可选字段保持缺省，不补满null |
| GET `/providers/{id}/credentials` | 明确编辑动作才返回raw保存的api_key、api_base与独立api_base_placeholder；`env:VAR`仍返回引用，不返回展开值；missing404 |
| POST `/model-discovery` | provider必需；可选provider_id/deployment_id/key/base/refresh；草稿不写盘；引用missing404 |
| POST `/models` | 旧追加；model_name/model必需；允许相同routing group；expected_revision可省略 |
| PUT `/models/{model_name}` | 旧唯一routing group更新；missing404、多匹配409 `ambiguous_legacy_model_name` |
| DELETE `/models/{model_name}` | 同唯一匹配规则；删除最后deployment仍保留其连接凭据 |
| POST `/deployments/batch` | expected_revision必需，1..50项，一次原子成功或全不变 |
| PATCH `/deployments/{deployment_id}` | expected_revision必需；按稳定ID更新；missing404 |
| DELETE `/deployments/{deployment_id}?expected_revision=...` | CAS必需；删除deployment但保留连接 |
| PATCH `/providers/{provider_id}` | expected_revision必需；必须显式传key或base；传播至同连接所有模型 |
| DELETE `/providers/{provider_id}?expected_revision=...` | 删除连接及链接/旧tuple匹配的deployments；不是仅清card |
| POST `/test` | 存储deployment_id、旧model_name、或ad-hoc model三选一；引用不能与临时key/base/model混用；真实短请求，不写盘 |
| POST `/config/open` | 原版打开server主机上的配置文件，missing404、成功204；可本轮显式501并隐藏UI按钮，不能用假成功冒充 |

SettingsPayload精确字段：`configured,routable,source,config_path,config_origin,revision,deployments,detected`。
source为config/detected/none；origin为explicit/environment/project/user/default。
Deployment字段：`deployment_id,routing_group,model,weight,api_key_configured,api_base_configured,api_base,persisted`。
集合中的api_base仅合法HTTP(S) origin（scheme+host+port），移除userinfo/path/query/fragment。

关键presence/身份规则（不要用默认反序列化Option吞掉missing/null）：

1. Create weight为整数≥0，默认1；routing group不是唯一键。model/model_name不能为空；mask字符`…`禁止作为新凭据/模型值提交；请求unknown字段422。
2. Patch omitted字段保留；api_key/base显式null删除字段；model/model_name显式null非法；weight:null保持。Provider patch中空字符串非法，至少包含一个key/base字段。
3. 每个持久deployment `model_info.id` 用UUID。旧条目无ID时视图采用 `legacy-` + SHA256前20位；输入是compact、sorted、ASCII JSON的routing_group/model/api_base/weight(默认1)/index，**不含key**。v2第一次修改给全部旧项补ID，同时把所请求旧ID映射到新ID。
4. Revision使用原始models/providers数组，sorted-key compact JSON，ensure_ascii=False，然后SHA256；数组顺序保留，缺省/显式字段不能先normalize丢掉。源码 `app.py:828–903`。Rust serializer与Python的浮点表示需限定实际结构/补fixture验证，勿无证据称任意unknown numeric字段hash全部兼容。
5. 读取raw目标JSON，保留全部非LLM字段、unknown字段和每项未知键；只改所需models/providers。缺数组时沿用启动配置fallback（`app.py:1002–1062`）。原子private0600临时写+sync+replace+parent sync；写失败内存快照与原文件均不换。拒绝symlink/FIFO/超限文件是应记录的原生安全边界。
6. 每次修改持settings写锁重新读文件，再检查revision；两个tab同revision只能一个成功。外部普通编辑器不遵守本服务锁的最终TOCTOU不应被宣称消除；可检查原始bytes再提交并记录边界。不要为本轮引入跨项目通用事务框架。
7. 已配置优先，session detected随后；按(model,api_base,model_name)去重。仅启动时没有配置模型才做原版自动探测；保存一项不应把其他检测模型持久化或从本次会话有效池移除（app.py:943,1643–1695）。保存不擅自改llm.enabled。现有core能解析model_info.provider_id到llm.providers；设置层给出的effective list可复用该能力。
8. provider删除/修改需支持 `legacy:<deployment_id>`；旧inline连接按(provider,api_base,api_key)分组。修改连接应更新所有关联模型，避免UI显示新key而实际模型仍用旧inline key。删除最后模型应把可复用连接保留到providers；删除provider则连同关联模型删除。

## Provider发现与测试：真实可用范围

`providers/discovery.py:27–32,470–710` 提供现有网络合同。建议本轮覆盖已实现原生HTTP供应商openai/anthropic/gemini/deepseek/openrouter/azure/ollama和custom(OpenAI兼容)。claude-agent/copilot/chatgpt OAuth与SDK仍明确unavailable；不读浏览器cookie、CLI认证文件，不展示成可运行。

- 发现请求：OpenAI/DeepSeek/OpenRouter/custom `base/models` + Bearer；Anthropic `base/models?limit=1000` + x-api-key、anthropic-version；Gemini `base/models?pageSize=1000&key=...`；Ollama `base/api/tags`；Azure `/v1` base走models，否则openai/models + api-version=2024-10-21、api-key。**发现默认base不完全等于推理base**：Gemini v1beta与v1beta/openai、Ollama根与/v1尤其不能共用错路径。
- 不跟随HTTP重定向，防credential被转发；10秒请求/3秒连接，20秒整体上限。Rust额外响应字节/模型数上限要明确测试，不借助Python/外部CLI。
- 返回 `{provider,status,source,authoritative,cached,stale,models:[{model,label,supports_vision}],detail?}`。一般成功source=live_api、status=ok；Azure regional list不能当deployment权威名单：partial、authoritative=false并说明需deployment name。Custom结果model前缀openai。
- 300秒内存缓存，key包含provider/base/credential identity；同key single-flight；refresh并发只更新一次；失败可回退24小时内旧结果，status=partial/cached=true/stale=true；无旧结果unavailable。保存连接后key改变自然失效。列表缓存不落盘，不把key原文作为诊断/cache展示。
- `/test` 请求精确最小提示 `Reply with exactly OK.`、16 tokens、15秒请求/30秒backstop；无文件、缓存或增强prompt。成功 `{ok:true,detail:"<model> responded"}`，模型/网络错误HTTP200 `{ok:false,detail:脱敏且≤300字符}`，坏body/不存在或歧义引用仍422/409。成功表示完成调用，不强行要求响应字节恰为OK（原版没有这个断言）。
- 建议核心提供窄 `probe_llm(ProbeRequest)->Result<()>` 复用真正部署解析/Chat/Anthropic/Azure协议，settings worker只解析引用。不能用convert占位文件模拟连接测试，不能让snapshot更改影响已开始probe。已有runtime permit可单独实例限制探测并发；网络不持设置写锁。reqwest已在core，CLI未直接依赖；coordinator决定新增CLI async reqwest还是共享窄core发现接口，避免一套协议重写三遍。

## UI最小完整交互

原版行为参考 `webapp/src/api/{client,token,types}.ts`、`hooks/useJobs.ts`、`components/{SettingsModal,MarkdownPreview}.tsx`，新实现从Rust服务角度重新编写。

- 无CDN/运行时Node依赖；include_bytes嵌入新的小HTML/JS/CSS。index no-cache、内容哈希资源immutable；若资源路径不含hash则不要宣称永久缓存。UI不自行猜capabilities/limits/default presets。
- 文件多选/拖入及逐行URL输入，选preset后显式布尔覆写、profile/no-cache等当前JobOptions；上传进度与SSE转换进度区分。retry省略body继承旧options；enhance使用同retry路由operation=enhance；显式options为完整替换，不能只发送一个字段却期待服务合并。
- SSE snapshot恢复、终态取消订阅、刷新重挂job、401提示重新输入token；history分页/选择/ZIP/删除使用现有接口。不要重新实现第二个job状态机或给未完成项目假终态。
- 结果读取现有result API，base/enhanced切换、源码可复制、Markdown可下载、资产自身路由可预览。浏览器不能innerHTML任意Markdown/HTML/SVG；采用受控Markdown renderer/sanitizer或先提供明确源码视图，不宣称未实现的富文本预览。若选纯JS依赖必须本地随binary打包并记录license，禁止为preview请求第三方CDN。
- Token从fragment优先、兼容query获取，sessionStorage及memory fallback，马上replaceState移除URL中的token。fetch使用Bearer；EventSource/下载/图片仅为本服务相对URL追加token；绝不能对外站链接追加token。不要持久化provider keys到localStorage或页面日志。
- 设置面板：列表无secret；显式编辑连接才请求credentials，密码控件；key/base分别有“不更改/清空/替换”的presence含义。发现失败仍可手填模型，批量添加用单revision；409保留用户草稿并重新获取视图，不能悄悄覆写新revision重试。保存成功更新capabilities及未来任务选项。
- 未实现OAuth登录、print-to-PDF、复杂Markdown编辑、原UI全部视觉细节应保留明确边界。设置文件编辑器open是可选差异，不应阻塞完整保存→运行闭环。

## 最小验收矩阵与执行顺序

先各lane纯fixture/unit → 一组实际服务进程/loopback模型 → 一个真实浏览器流程 → 完整门禁 → 干净release复用同流程；不附带性能提升结论。

1. 来源：显式/环境/project/user/default五种选择仅在私有目录；保存改选定文件且其它文件不动；MARKITAI_HOME隔离，HOME不变；unknown嵌套字段保留，权限0600，malformed/FIFO/symlink/写失败不换runtime。
2. 协议：两重复group不同ID，旧ID补齐，legacy歧义409；omitted/null/empty/weight=0；batch第2项非法全回滚；两tab CAS、一项外部修改CAS；Unicode revision与raw-default差异用原创fixture对照reference纯函数，不启动原服务/读取配置。
3. 凭据：集合仅布尔/origin，显式credentials返回env引用而非展开值；provider改key/base联动所有模型；删最后模型仍可复用provider；删provider清对应模型但不误删另一连接；key不出现在错误/log/UI/sessionStorage。
4. 模型发现：loopback验证各协议的实际URL/header及返回前缀；重定向目标零请求、超时/超大响应、缓存命中/refresh单飞/旧结果回退/credential隔离；Azure非权威；unsupported local/OAuth不假报成功。
5. Probe：exact请求提示/token上限、stored reference/临时key/模型引用歧义；真实失败200 ok=false且脱敏；不修改任何配置/cache/jobs；测试时无非loopback连接。
6. 热更新：一个模型A请求被mock gate持住，保存B后新任务/重试使用B，A仍完整完成；重启后B有效；持久化失败时后续仍用A。无需重跑round19全部事务杀进程案例，只保持现有套件过门禁。
7. 权限：loopback、远端valid token、无token、no-auth远端；Host/Origin，所有settings错误含no-store；unknown API不回SPA；资源404/缓存/HEAD/MIME。用路由测试构造ConnectInfo，不伪造真实远端证据。
8. 浏览器：独立私有服务+mock，访问新UI，设置连接/发现2模型/保存/测试，文件+URL任务→进度→安全预览/源码→下载→增强→删除共享item不丢另一项→历史→刷新/重启；截图与实际DOM/请求/物理输出证据绑定release。用含脚本Markdown、带#?空格文件名测试渲染安全和URL编码。

## 精确参考与审计快照

参考app.py：490权限；828revision；843legacy ID；943effective；982payload；1002raw原子修改；1159连接保留；1249/1299probe引用；1335probe；1493origin；1530SPA；1876以后HTTP routes；2140热更新。
参考settings测试：`test_serve_settings.py` TestGetLLMSettings:99、TestAddLLMModel:201、TestUpdateLLMModel:391、TestProviderCredentialLifecycle:645、TestLLMSettingsProbe:1018、TestDeploymentIdentityAndRevision:1328、TestSettingsOriginAndSecurity:1476、TestDetectedRuntimeSeparation:1537、TestConfiguredConnectionReuse:1623、TestOpenConfigFile:1667。

源文件SHA256（本审计读取的字节，非全仓clean证明）：

| 文件（各自仓库根） | SHA256 |
|---|---|
| reference `packages/markitai/src/markitai/serve/app.py` | a62b28b4fc70983ba3171bd1c82408ef3cbd81234b3d7ddcdbb35b6672cfce17 |
| reference `packages/markitai/src/markitai/serve/schemas.py` | cc3f0407ba91a1b8bef35cf9b4d3a2c581def48f1a5faf5b052b1a17379fa93e |
| reference `packages/markitai/src/markitai/providers/discovery.py` | dc2725882e81a81799912ca7a7aafd507a9099cef9737889212225f65a409e2a |
| reference `packages/markitai/tests/unit/serve/test_serve_settings.py` | 6493fc95b36ffc36c02f368f70c5395a3e424c068e14a40aec2eef2c89cfd8d8 |
| reference `webapp/src/api/client.ts` | db44be74fb8f96bb6e6cb28fec6c933bfd2a3e0f4e195771d19fc84dd129c477 |
| reference `webapp/src/api/token.ts` | 19d55603fe13208a6c74bc2c12986f9d6f713138057bc4ac5fb6050f75e131ef |
| native `crates/markitai-cli/src/server/mod.rs` | 27076c3881d8514e5fb8f325838de993f15aeea95ee32ea35210c0abdbc47ca8 |
| native `crates/markitai-cli/src/server/http.rs` | 8a280c5d2f879eceb5ba101638a36cec794168bea0341fcfc084002c2dc38dfd |
| native `crates/markitai-cli/src/server/rerun.rs` | 11478c3bfcd31422d054e21a9838ef48ff5b76eb7dd4f17c2c7511596b576232 |
| native `crates/markitai-cli/src/server/security.rs` | cb64a201576d50ebbc2a970ec169c47ef4991f342617d757032a8778f44d3026 |
| native `crates/markitai-cli/src/server/types.rs` | 0fded8d7a137a3707ed306d4f752a88bcd7ede5121b43cc8c3f1ab9fe2bddf51 |
| native `crates/markitai-core/src/config.rs` | 651d1143ea5f64f86787ebb13386af45f4e7795ead494f730cf006f8bbc1032a |
| native `crates/markitai-core/src/llm.rs` | 36f697f20494a554146d43d09c499628edf6dd55e72120c3c2cdd3ce9624feda |
