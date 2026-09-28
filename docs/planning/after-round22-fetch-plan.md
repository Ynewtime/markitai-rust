# 下一批获取功能：HTTP 认证、代理与浏览器会话

2026-09-29。只读实施准备，尚未实现或验收本计划。参考仓库固定为
`ba374322f884b0e720b45466cc1196f4574a3da5`；Rust 读取 round22 冻结工作树。
没有运行原版、浏览器、转换器或构建，没有读取用户配置或真实凭据。

建议先完成**现有浏览器 HTTP 凭据 → 受限 challenge 响应 → auto 获取/截图 →
私有输出**这一小批。认证代理复用同一 challenge 机制，但另设凭据类型；
持久域上下文随后实施。PAC 不属于原项目已实现合同，不用为它引入 JavaScript
运行时或推迟前两项交付。

## 参考合同与原生缺口

| 项目 | 参考实际行为 | 当前 Rust 与本批边界 |
|---|---|---|
| HTTP 凭据 | `fetch.playwright.http_credentials: dict[str,str] | null`，默认 null；直接传 `browser.new_context(http_credentials=...)` | `browser/options.rs` 明确拒绝非 null；先支持原有对象形状，不添加顶层 `fetch.http_credentials` |
| 凭据内容 | 项目注释约定 username/password；没有独立展开环境引用的逻辑；字符串字典也能透传 Playwright 的 origin/send | 校验字符串与有界长度；保留空字符串与缺字段的区别。不要把密码字符串自动解释成环境变量名 |
| 静态 HTTP | `fetch_http.py` 使用 HTTP 客户端；未把 Playwright 凭据传给静态请求 | 不把浏览器凭据隐式加到 reqwest、PDF 探测或远程服务；认证 PDF 下载不是第一批已关闭能力 |
| 代理发现 | HTTPS_PROXY、HTTP_PROXY、ALL_PROXY，然后小写同名；首个非空全局值，之后系统手动代理；检测结果进程内缓存 | 浏览器当前按请求 scheme 选小写/大写环境变量，拒绝代理 URL userinfo；保留并说明优先级差异，认证改动不顺便重写发现策略 |
| 认证代理 | 静态 HTTP 库接收代理 URL；Playwright launch 只传 server/bypass，没有分解成 username/password 的代码 | 浏览器认证代理是实际缺口；不能凭接受含密码 URL 就宣称端到端可用 |
| PAC/WPAD | Linux 明确忽略自动模式；macOS 只读 HTTP(S) 手动字段，Windows 只读 ProxyEnable/ProxyServer/ProxyOverride | 未找到 PAC 下载/执行合同；单列未来功能，避免称“恢复原有 PAC 支持” |
| 域会话 | `session_mode=isolated` 默认；`domain_persistent` 启用 context 缓存，TTL 默认 600 秒、范围 60–7200，最多 8 个 | 当前拒绝 persistent；需要浏览器/context 生命周期调整，不能只删 Unsupported 分支 |
| 会话作用域 | `_url_to_session_key` 是小写 netloc（含端口、不含 scheme）；新 page 每次关闭，context 保留到过期/逐出/renderer close | 参考没有把 profile 登录态跨进程落盘；原生先做进程内复用，不接用户 Chrome profile |

来源：参考 `config.py:671–701`、`fetch_support.py:45,76–144`、
`fetch_playwright.py:570–812`、`fetch_session.py:63–292,600–704`、
`fetch_http.py`。Cloudflare 的独立 `http_credentials → authenticate` 是远程
策略合同，不能混用本地浏览器密码或顺便宣称已支持。

Playwright 的 origin 限定完整 scheme/host/port；省略时可向任何 challenge
站点响应。send 只影响其 APIRequestContext，不改变浏览器请求。当前上游文档
还接受列表，但参考项目 schema 只接受字典，本批不扩展成多凭据列表。
[Playwright HTTP credentials](https://playwright.dev/python/docs/api/class-browser#browser-new-context-option-http-credentials)

## 第一批：浏览器 HTTP auth 与获取链

建议新增私有 `browser/auth.rs`，在现有 `Options` 中持有有界、不可 Debug 展开
的 `HttpCredentials`，不改公共 ConvertOptions、Document 或绑定 JSON。CDP 已有
Fetch.requestPaused 处理与同步消息泵；启用 handleAuthRequests，处理
Fetch.authRequired，使用 continueWithAuth 响应，不需要新增依赖或 Python。
协议定义区分 Server/Proxy challenge，并提供 Default/CancelAuth/ProvideCredentials。
[CDP Fetch 协议](https://chromedevtools.github.io/devtools-protocol/tot/Fetch/)

实施规则：

1. 先真实支持 Basic challenge；只有真实 Digest fixture 通过才能列入已验收。
   不以操作系统自动登录代替配置密码，NTLM/Kerberos 未验收不宣称支持。
2. 建议 origin 缺省绑定初始请求 origin，显式 origin 接受有效 HTTP(S) origin，
   不接受 userinfo/path/query/fragment。与参考“未设 origin 可发给任意服务器”
   有意不同，需在实施前由协调器定稿并记入兼容边界；不能偷偷扩大密码作用域。
   显式 origin 可用于已知跳转目标，但仍必须精确匹配，不做后缀/子串匹配。
3. 只对匹配 origin 的 Server challenge 提供服务器密码；跨端口、跨 scheme、
   跨主机跳转及第三方子资源没有隐式授权。Proxy challenge 永不复用服务器密码。
   不预置全局 Authorization；已配置的 extra_http_headers 是另一现有合同，
   不能因为此补丁就声称所有自定义敏感头也已经具备同样的隔离。
4. 每个请求/认证空间最多提供一次同一凭据；再次拒绝即 CancelAuth，并有整个
   导航的 challenge 数量上限。auth 处理用现有非递归 send 消息路径，避免在
   event 内递归 call 导致相邻协议回应丢失。所有分支共享原导航 deadline。
5. `send=always|unauthorized` 保留浏览器无影响语义，不转换成提前发送密码；
   其它值、坏字段类型与超长值在 Chromium 启动前给脱敏配置错误。
6. 错误/警告只说明认证失败及操作阶段；不返回用户名、密码、Authorization、
   Cookie、带 userinfo 的代理 URL、完整 challenge 或 CDP 消息。

获取链由协调器修改 `fetch.rs`，第一批建议如下：

- explicit playwright 直接走认证浏览器；无凭据的所有现有行为保持。
- auto 配置了 HTTP 凭据时，在读取静态缓存前选择认证浏览器，避免匿名 401
  或匿名 200 登录页先被当作最终正文；capture 同样使用该次认证页面。
  这是对参考 static-first 行为的明确调整，不声称和原版请求次数相同。
- explicit static 仍不消费浏览器凭据；不在收到 401 后隐式切换用户指定策略。
- 认证路径不尝试远程 fallback，不把密码、cookie 或私有 HTML 发送到其它服务。
  超时、响应预算、协议失败不能转换成匿名成功。不要仅在认证失败时把配置清空重试。
- 当前 Chromium 禁止下载，浏览器认证 PDF 不应伪装为成功空页面。保留未认证
  typed PDF 一次下载链；认证 PDF 是后续需要单独设计 scoped HTTP 下载的缺口。
  后续若要支持，必须明确新 HTTP 凭据作用域，而不是借用浏览器配置暗中发送。

## 第二批：认证 HTTP 代理；PAC 单列

复用 `Fetch.authRequired`，新增 `ProxyEndpoint { server, credentials, bypass }`。
从原有环境代理 URL 分离 userinfo，用户名/密码 percent-decode 一次；传给
`--proxy-server` 的值只包含端点，不能让密码进入进程参数、日志、缓存 key 或
错误。只向该已选代理的 Proxy challenge 响应，Server/Proxy 两套计数与作用域
独立。HTTP 与 HTTPS CONNECT 都需真实验收；带认证 SOCKS5 保持明确 Unsupported，
不能把 HTTP challenge 机制称为 SOCKS 认证。

静态客户端的代理、NO_PROXY 与浏览器启动配置仍须来自同一已选快照。建议先
保持当前环境变量优先级，不顺便加入系统检测。参考始终让 loopback 直连；
普通生产行为不得为了代理测试取消这条规则。测试代理请求可使用保留的
`.test` 主机名，由私有 loopback 代理直接响应，不需要外网 DNS 或修改 hosts。

PAC 若以后实施，需单独决定系统解析还是内嵌解释器、PAC 下载认证/重定向、
DNS/脚本执行时间与大小限制、每 URL 决策、DIRECT 回退和缓存；这些均不是
本批“认证代理”完成的前置条件或已实现承诺。

## 第三批：进程内 domain_persistent

将目前每次 fetch 拥有的 Chromium 进程、临时 profile、WebSocket 与 page 生命周期
拆开，先提供有界 context 租约池。建议 key 为 origin 加配置身份摘要，摘要包含
代理端点/凭据/bypass、服务器凭据、初始 cookies/headers、user agent 和影响
context 的设置；不得只按 host 复用不同账号。摘要只在内存使用，不作为日志或
持久状态公开。参考复用时不复核 ctx_options，原生不复制这一凭据串用风险。

继续接收原 session_mode/TTL；默认 isolated 完全独立。persistent 上限 8、TTL
按最后使用时间，闲置项才逐出；运行中 context 不被第九个请求或过期清理关闭。
池锁只保护 lookup/创建预留/租约计数，不跨导航。一个 context 可以先串行租用
以减少会话并发冲突，不同域保持并行；排队仍受总 deadline 限制。坏连接/崩溃
淘汰该实例，page 必须每次关闭，进程退出回收所有私有目录和子进程。

不得使用用户 profile、永久 cookies 文件或跨 CLI 运行恢复。服务保持长进程
能受益，单次 CLI 的多 URL 也能复用；跨账号配置更新产生新身份，已有在途
任务保留原快照。受限公共网络调用继续独立隔离，不复用可信本地会话。

## 重试与缓存必须先锁定

当前 `fetch_cache` 的 native-fetch-v1 key 仅 raw URL + 显式策略 scope，没有
凭据/cookie/会话身份。第一批对会影响认证结果的浏览器请求直接绕过静态
HTML 缓存读写，浏览器结果维持不缓存：账号 A/B 同 URL 绝不能命中匿名或
另一账号条目。已有公开缓存不必删除；只是私有请求不能读取或覆盖它。
不把密码哈希加入现有持久 key 后就宣称 cookies/session 变化已解决。

LLM 缓存继续按实际输入、模型与现有处理参数隔离；本批没有把 auth 字段加入
模型 prompt 的理由。文档输出和模型请求本来就可含私有正文，缓存开关仍需
遵守。需要证明的是新凭据不会被塞进 metadata、请求记录或 prompt，不能把
“凭据没落缓存”误写成“私有内容从不落盘”。

401/407 的浏览器内部 challenge 次数、外层 fetch fallback 次数、模型重试各自
分开记录。重复认证失败不得触发远程 fetch，不得以无限 reload 维持窗口。
参考 `fetch_http.py:25–105` 的受限公共 HTTP 路径在跨 origin 重定向时去掉
Authorization/Cookie/Proxy-Authorization；该窄路径不能当作整个原版 HTTP 栈的
全局保证。

## 最小真实验收与拆分所有权

| 验收 | 必须观察的结果 |
|---|---|
| Basic + 截图 | loopback 401 → 同 origin 正确凭据 → 页面正文/真实截图；首次请求无预置 Authorization |
| 拒绝与次数 | 错密码、重复 challenge、未配置凭据都在有界请求数内失败；无模型/远程服务调用 |
| origin | 两端口和 localhost/127.0.0.1 跳转、第三方图片/frame；接收方请求记录证明不收到服务器密码；显式 origin 单独验证 |
| 自动链 | 匿名缓存已有登录页/公开页时，配置认证的 auto 仍得到私有正文；非认证 auto/static/PDF 原有用例保持 |
| 账号隔离 | A/B 同 URL 不互相命中页面或上下文；修改配置后新请求用 B，在途 A 保持 A |
| 代理 | loopback 407 mock + `.test` 目标：代理收到代理认证，源站只收到源站认证；HTTP/CONNECT 分开；错误密码和 bypass 有界 |
| 会话 | 同 context 的第二页读取前页 cookie/localStorage；isolated 不复用；不同 origin/账号不共享；第九项、TTL、失败重建不杀在途任务 |
| 清理与脱敏 | 超时/取消/崩溃后无残留 child/profile；stdout/stderr/错误/状态/缓存查不到测试秘密原文及编码形式 |

全部使用原创临时页、测试密码、私有 MARKITAI_HOME/cache/profile，HOME 保持不变。
测试需真实 Chromium 导航，不用只喂 CDP JSON 的单元测试冒充端到端。计数与
header 记录保存在私有验收目录，真实敏感数据不参与。HTTPS 如需测试使用私有
测试证书与限定测试配置，不添加生产全局忽略证书开关。

建议 lane：browser worker 独占 options/CDP/auth 与浏览器单元；另一个 worker
独占实际 CLI/服务 loopback 验收；coordinator 独占 fetch/cache 策略、配置验证与
文档公共能力、统一构建和提交。完成第一批再移动到 proxy/session，避免三人
同时拆同一 Browser 结构。第一批不需要新依赖、公开 binding 参数或 REST 字段。

## 读取快照

下列 SHA256 对应本次实际读取文件；不是整个工作树 clean 的替代证明。

| 仓库 / 相对路径 | SHA256 |
|---|---|
| reference `packages/markitai/src/markitai/config.py` | `c69ec01b13b9b8f6a2df61278d904406f7f08a2073084a3c947f517b88bb06a5` |
| reference `packages/markitai/src/markitai/fetch_support.py` | `af74dcc3e5c019f8fba48918e61f4e2f9750eb8c79a011586614667d41fde779` |
| reference `packages/markitai/src/markitai/fetch_session.py` | `b2012b1c8326c9ab27081b63d6338b28f4179000a086cf07611edbb7e8c8d300` |
| reference `packages/markitai/src/markitai/fetch_playwright.py` | `7e8b11b6330dc56f6594eb71839900112870dca717241d8988a98784fd07660e` |
| reference `packages/markitai/src/markitai/fetch_http.py` | `f55247b8d4021518d1db055a27b214fbf4c513c65549b858ca335e1741c22821` |
| native `crates/markitai-core/src/browser/options.rs` | `9fc48fecbb4155a05d8f8835656a3ba5cd63ab5561fda6c98d004cf4d286ec13` |
| native `crates/markitai-core/src/browser/cdp.rs` | `f69b639779727f3f1148f7526cac57969fd76fc17adaba8ba3645bb72b7974ca` |
| native `crates/markitai-core/src/fetch.rs` | `09c2980ad65d954d48621dc6191633ae9ab7050ced122a8a218e4cfee2d58e83` |
| native `crates/markitai-core/src/fetch_cache.rs` | `af9e2cefc0b348ed7ba6e0bfa55e1ea21bcd7a3b826f60ce40550f81d1260642` |
