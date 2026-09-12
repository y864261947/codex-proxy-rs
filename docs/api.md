# Codex Proxy RS 接口

本文列出 v3 源码中的公开 HTTP 接口，路由以
`backend/crates/gateway-api/src` 中的 router 为准。配置 Codex 请先看 [客户端配置](../deploy/README.md#客户端配置)；
运行实例是否包含这些功能，应结合其版本和 revision 确认。

## 1. 鉴权与公共约定

### 客户端接口

所有 `/v1/*` 请求都使用管理端创建的 Client Key：

```http
Authorization: Bearer sk_...
```

Codex 原生生图配置还会携带 `X-OpenAI-Actor-Authorization: proxy-managed`。
它仅用于客户端识别服务端托管认证，不能代替 Client Key。网关和 OpenAI Provider 都会过滤该请求头，
上游账号身份只由服务端选中的账号提供；不要把真实账号 token 放进该标记。

Client Key 绑定接入分组时，以该组显式授权的模型、号池和渠道为准，空集合不会获得全部权限。
未绑定接入分组的旧 Key 保留账号分组范围：无账号分组关联时可使用全部账号，有关联时只能使用已启用
分组成员的并集。实际调用继续检查来源启停、健康与容量；旧 Key 的无来源后备只包含未分池账号。
账号分组可混合 `openai` 与 `xai`；同一请求只会在能力匹配且满足重放安全边界时切换来源。

Key、所属客户或接入分组停用后拒绝新请求。全站、客户、接入分组、Key 的并发/RPM 分别检查，来源和
共享配额按实际尝试另行准入。来源容量不足返回 HTTP `503`，OpenAI 风格错误码为
`source_capacity_unavailable`；没有可用候选为 `no_available_provider`，不向下游泄露来源凭据。

运行设置可以分别配置 `minCodexDesktopVersion` 与 `minCodexCliVersion`。两者只接受 SemVer，`null`
表示不限制。API 在 Client Key 鉴权成功后识别官方 Desktop/CLI 请求头；已识别客户端没有合法版本，或版本
低于对应门槛时，所有 `/v1/*` HTTP 请求和新 WebSocket 握手在访问上游前返回 `426 Upgrade Required`。
未知客户端保持兼容，不应用版本门禁。

低版本响应使用 OpenAI 风格错误格式：

```json
{
  "error": {
    "message": "Codex CLI 0.151.0 is below the minimum required version 0.152.0. Upgrade Codex CLI and retry.",
    "type": "invalid_request_error",
    "code": "client_version_too_old",
    "client": "codex_cli",
    "current_version": "0.151.0",
    "min_version": "0.152.0"
  }
}
```

已识别但缺失或携带非法版本时，`code` 为 `client_version_unavailable`，`current_version` 为 `null`。

### 管理接口

除登录、会话状态和登出外，所有 `/api/admin/*` 请求都需要以下任一鉴权方式：

- 浏览器登录后得到的 `cpr_admin_session` Cookie；
- `x-api-key: <admin-api-key>`。

请求无需自带 `x-request-id`；缺失时服务端自动生成 UUID 并在响应头回传同一 request ID。
`api.request_id_header` 可改变注入与回传的 header 名，管理端鉴权不依赖该名字。
管理端响应统一带 `Cache-Control: no-store`。

配置了 CORS 白名单 origin 时，跨域请求以凭据模式放行，仅允许 `GET`/`POST` 方法和
`authorization`、`content-type`、`x-api-key` 与 request ID 四个请求头，不使用通配符。

普通成功响应使用以下信封：

```json
{
  "code": 200,
  "message": "OK",
  "data": {}
}
```

所有 `/api/admin/*` 错误（包括 JSON/Query rejection、未知路由和错误 HTTP method）统一返回
`application/json`：

```json
{
  "code": 40001,
  "message": "请求参数不合法",
  "data": null
}
```

管理端本地产生的 `message` 是可安全展示的中文文案；Store、Serde、Provider 内部 `Display` 和原始上游
body 不进入这个通用信封。稳定业务码如下：

| HTTP | `code` | 含义 |
| ---: | ---: | --- |
| 400 | `40000` | 请求体不是合法 JSON |
| 400 / 405 / 415 / 422 | `40001` | 通用请求、方法、Content-Type 或字段错误；HTTP 状态保留具体语义 |
| 400 | `40002` | 时间范围不合法 |
| 401 | `40101` / `40102` / `40103` | 缺少管理员会话 / 登录凭据错误 / 管理 API Key 错误 |
| 404 | `40401` | 资源或管理接口不存在 |
| 409 | `40901` | 资源状态冲突 |
| 429 | `42901` | 登录尝试过多 |
| 500 | `50001` | 服务内部错误 |
| 502 | `50201` | 上游服务请求失败 |
| 502 | `50202` | 不可逆上游操作的执行结果未知；刷新状态后再决定是否重试 |
| 503 | `50301` | 依赖服务暂不可用 |

未知 `/api/admin/*` 路径使用 `40401`，不会落入 SPA；已存在路径使用错误 method 时返回 `405`、
`40001`，并保留标准 `Allow` header。request ID 继续通过配置的响应 header 返回。

### 管理写入一致性

管理写入不要求客户端提供全局配置版本。会改变路由快照或安全配置的写入由后端在事务内推进
内部 `config_revision`，并用于快照发布与审计。账号更新和分组查询/写入的部分响应会返回
`configRevision` 作为已提交事实，但它不是客户端 mutation 的前置条件。

## 2. 健康检查

| 方法 | 路由 | 鉴权 | 说明 |
| --- | --- | --- | --- |
| `GET` | `/healthz` | 无 | Core、Store 和后台任务健康时返回 `204`，否则返回 `503` |

## 3. OpenAI 数据面与模型目录

Responses、Images 和 standalone Search HTTP body、WebSocket message 和 frame 不设置网关私有长度上限；
协议可接受性由上游决定。

| 方法 | 路由 | 说明 |
| --- | --- | --- |
| `POST` | `/v1/responses` | OpenAI Responses JSON；`stream=true` 返回 SSE，否则返回完整 JSON |
| `GET` | `/v1/responses` | 通过 HTTP Upgrade 建立 Responses WebSocket |
| `POST` | `/v1/alpha/search` | Codex standalone web search；JSON 请求与响应正文原样转发 |
| `POST` | `/v1/images/generations` | 通过 OpenAI Provider 发起图像生成；JSON 请求与响应正文原样转发 |
| `POST` | `/v1/images/edits` | 通过 OpenAI Provider 发起图像编辑；JSON 请求与响应正文原样转发 |
| `GET` | `/v1/models` | 返回当前 Client Key 账号范围内各 Provider 的可用公开模型并集；有两种响应形态，见下 |
| `GET` | `/v1/models/{model_id}` | 返回 OpenAI 兼容的单模型详情 |

Codex 的 review 等子代理请求仍使用 `/v1/responses`，并通过 `x-openai-subagent` 请求头携带子代理类型；
网关不提供独立的子代理请求路径。

Responses WebSocket 仅接受文本 `response.create`，同一连接串行执行。当前响应期间收到的后续业务帧
留在有界接收队列中，待当前响应完成终结和写出后再逐条校验、准入与执行，不因请求提前到达而断开。
这对齐 Codex 客户端 `stream_request` 持锁至本轮结束的串行行为，不表示支持额外控制消息类型。
接收队列容量为 32 个事件，超载仍关闭连接；Ping/Pong、客户端关闭和服务关闭不等待队列中的请求执行。

客户端使用 HTTP/SSE 时，OpenAI Provider 仍可能选择上游 WebSocket。
客户端配置的 `supports_websockets` 只控制第一段连接，不是服务端传输策略开关。
上游在响应终态前发送 Close 1000 仍属于失败，不能按“正常关闭”计为成功。

`GET /v1/models` 默认返回 OpenAI 兼容列表 `{"object": "list", "data": [...]}`；请求携带非空
`client_version` query 参数（Codex 客户端）时改为返回 Codex 专用目录合同 `{"models": [...]}`。

OpenAI 路径保留客户端 Responses wire 语义：请求 body 的未知字段和字段顺序保持不变（受控模型
映射除外），HTTP SSE 与 WebSocket 的上游业务事件字节原样转发，response ID 按 opaque 值处理而不
假设 UUID 或固定长度；OpenAI 上游错误 envelope 和允许下发的 opaque header 值也不由 canonical
观测结果重写。Images 请求不读取或重建 JSON，也不要求或映射模型字段；它固定使用 OpenAI Provider，
只在原始字节之外完成账号选择、鉴权头替换和端点路由，成功与失败响应正文同样保持原始字节。
`/v1/alpha/search` 使用相同的 OpenAI Provider 原生端点边界：body（包括 `model`）不解析、不映射，
`x-codex-turn-metadata` 在移除客户端账号身份并按当前 lease 重写 installation ID 后转发；上游账号
Authorization、Cookie、account ID、originator 和 User-Agent 均由代理安全重建。xAI 是 Grok wire 与
Responses wire 之间的协议转换层，转换只在 xAI Provider 内完成。
上游结构化错误的 message/code/type 会透传给客户端，其中内嵌的账号指纹 UUID 已脱敏。模型映射是
全局精确映射，未命中时模型名原样交给候选 Provider；分组只限定账号集合，不参与模型改名。

## 4. 管理员认证

| 方法 | 路由 | 请求 | 说明 |
| --- | --- | --- | --- |
| `POST` | `/api/admin/auth/login` | `{ username?, password }` | 创建管理员会话并设置 Cookie |
| `GET` | `/api/admin/auth/status` | 无 | 返回当前 Cookie 是否已认证 |
| `POST` | `/api/admin/auth/logout` | 无 | 删除当前会话并清除 Cookie |

## 5. 账号

账号 API 使用统一路由，不存在 Provider Instance 或 Provider 专属账号路由。需要 Provider 的请求只接受
`provider: "openai" | "xai"`。

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/accounts` | `page`、`pageSize`、`provider`、`groupId`、`search`、`status`、排序字段 | 分页查询账号与汇总 |
| `GET` | `/api/admin/accounts/detail` | `accountId` | 查询账号详情、额度和本地用量 |
| `GET` | `/api/admin/accounts/export` | `accountIds`、`confirm=export_sensitive_accounts` | 显式导出最多 200 个账号的敏感 Provider 文档 |
| `POST` | `/api/admin/accounts/import` | `{ provider, data }` | 导入或按上游身份更新账号；新账号保持未分组，已有账号保留所属分组 |
| `POST` | `/api/admin/accounts/refresh` | `{ accountId }` | 手工刷新 OAuth credential（`idToken` / `accessToken` / `refreshToken`），不刷新额度 |
| `POST` | `/api/admin/accounts/recover` | `{ accountId }` | 管理员显式清除该账号的本地错误/额度/cooldown 事实并重新启用，不访问上游 |
| `POST` | `/api/admin/accounts/rotate` | OpenAI rotation 字段 | 手工替换 OpenAI OAuth token |
| `POST` | `/api/admin/accounts/update` | `{ accountId, enabled, concurrencyLimit, weight, groupIds }` | 一次更新账号调度状态、并发上限（`null` 表示继承运行参数）、权重（1–100）与所属分组 |
| `POST` | `/api/admin/accounts/batch-update` | `{ accountIds, enabled, concurrencyLimit, weight, groupIds }` | 一次事务统一更新所选账号的全部调度字段与完整分组集合 |
| `POST` | `/api/admin/accounts/delete` | `{ provider, accountIds }` | 批量删除 1–200 个账号 |
| `GET` | `/api/admin/accounts/quota` | `accountId` | 读取当前额度，不强制访问上游 |
| `POST` | `/api/admin/accounts/quota/refresh` | `{ accountId }` | 访问 Provider 并刷新额度，同时同步额度所属状态 |
| `GET` | `/api/admin/accounts/profile-statistics` | `accountId` | 实时查询 OpenAI/Codex 官方个人资料中的累计活动与使用洞察 |
| `GET` | `/api/admin/accounts/reset-credits` | `accountId` | 查询 OpenAI 上游主动额度重置卡，不读取本地库存 |
| `POST` | `/api/admin/accounts/reset-credits` | `{ accountId, creditId?, redeemRequestId }` | 使用 UUIDv4 幂等键消费一张 OpenAI 上游重置卡 |
| `GET` | `/api/admin/accounts/models` | `accountId` | 优先读取该 Provider + 套餐的模型 cache，缺失时有限实时拉取 |
| `POST` | `/api/admin/accounts/models/refresh` | `{ accountId }` | 强制拉取最新模型并覆盖 cache |
| `GET` | `/api/admin/accounts/connection-test` | `accountId`、`modelId` | 通过 SSE 返回实时连接测试事件，不作为业务 Responses 用量记录 |
| `POST` | `/api/admin/accounts/oauth/start` | `{ provider, name, accountId? }` | 创建 OpenAI 或 xAI OAuth flow；`accountId` 表示重新授权 |
| `POST` | `/api/admin/accounts/oauth/complete` | `{ provider, flowId, callbackUrl }` | 消费 OAuth callback；新账号保持未分组，重新授权保留所属分组 |

账号列表支持以下稳定值：

- `provider`: `all`、`openai`、`xai`；
- `groupId`: 分组 ID、`ungrouped`，或省略以不过滤；
- `status`: `normal`、`quota_exhausted`、`rate_limited`、`disabled`、`error`；
- `sortBy`: `email`、`status`、`planType`、`usage`、`lastUsedAt`、`expiresAt`；
- `sortDirection`: `asc`、`desc`。

### 账号连接测试 SSE

`GET /api/admin/accounts/connection-test` 固定探测请求指定的账号，不参与普通账号轮换。成功流沿用
`test_start`、`request`、`content`、`test_complete` 事件；失败事件为：

```json
{
  "type": "error",
  "source": "upstream",
  "gatewayErrorCode": "rate_limited",
  "sendState": "sent",
  "error": "upstream unavailable",
  "providerErrorCode": "usage_exhausted",
  "providerErrorType": "invalid_request_error",
  "upstreamStatus": 429,
  "upstreamContentType": "application/json",
  "upstreamBody": "{\"error\":{...}}"
}
```

- `source` 为 `gateway`、`provider` 或 `upstream`：分别表示尚未进入 Provider、Provider 本地且未发送、
  已发送/可能已发送或已经捕获到上游事实。
- `gatewayErrorCode` 是 `GatewayErrorKind` 的稳定机器值，管理端据此生成中文摘要。
- `sendState` 为 `not_sent`、`sent`、`ambiguous`，非 Provider 错误为 `null`。
- `error`、`providerErrorCode`、`providerErrorType`、`upstreamStatus`、`upstreamContentType` 和
  `upstreamBody` 是实际捕获的原始诊断字段；缺失时为 `null`，不会由本地猜测或翻译。

导入的 `data` 必须是 JSON object，Admin API 请求上限为 64 MiB；Provider 可以收紧限制，
当前 xAI 导入上限为 16 MiB。内部 schema 由目标 Provider 独占解释：

- OpenAI 接受单账号 OAuth 文档、`accounts` 数组（最多 200 项）和 CPR 账号 bundle；
- OpenAI OAuth token 字段接受 `accessToken`、`refreshToken`、`idToken`，以及官方
  `auth.json` 中的 `access_token`、`refresh_token`、`id_token`，可以嵌套在 `tokens` 等账号 object 内；
  每项至少包含 AT 或 RT。仅含 `OPENAI_API_KEY` 的客户端代理配置不是 OAuth 账号导入材料；
  RT-only 会在导入时换取 AT，AT-only 不具备自动续期能力；
- xAI 从单账号 object 或 `accounts` 数组中提取 OAuth token；包装中的代理、并发、优先级等字段不参与认证；
- xAI 批量导入逐条独立校验：失败条目跳过并记录日志，不中断其余条目，仅当没有任何条目成功时整个导入才报错；
- xAI API Key 不是受支持的账号 credential；
- 导入不会只凭文件外形写入账号；目标 Provider 使用认证材料完成必要的 token exchange 或已认证账号资料补全。

管理端的 OpenAI `AT` / `RT` 标签是同一导入 API 的输入便利层：每行一个 token，最多 200 行，提交前
转换为对应的 `accounts` JSON。Admin API 本身不接收纯文本 token 列表。例如：

```json
{
  "provider": "openai",
  "data": {
    "accounts": [
      { "accessToken": "eyJ..." },
      { "accessToken": "eyJ...", "refreshToken": "rt_...", "idToken": "eyJ..." }
    ]
  }
}
```

RT-only 使用同一形状，只提交 `refreshToken`。不得把真实 token 写入日志、issue、fixture 或文档。

账号导入与 OAuth complete 不接收 `groupIds`。首次创建的账号保持未分组；按既有上游身份重新导入、
重新授权以及普通 credential refresh/rotation 均保留已有分组。分组关系只通过账号编辑维护。
账号列表的每个 item 返回轻量 `groups: [{ id, name, enabled }]`。

OpenAI 的 CPR 导出保持 OAuth 账号的既有 token 与过期时间字段。

OpenAI rotation 请求字段为：

```json
{
  "provider": "openai",
  "accountId": "acct_...",
  "idToken": "...",
  "accessToken": "...",
  "refreshToken": "..."
}
```

OAuth start 使用：

```json
{
  "provider": "openai",
  "name": "account name",
  "accountId": null
}
```

重新授权已有账号时，start 请求仍携带 `provider` 和展示用 `name`，只额外提供目标 `accountId`；
客户端不得提交 `credentialRevision`、旧 token 身份或其他并发控制字段。complete 请求也不重复提交
`accountId`，后端通过 `flowId` 中保存的目标绑定完成授权。

### OpenAI 身份、额度与状态

- OAuth 文件导入接受 camelCase 与 snake_case 的三个 token 字段，内部统一保存为
  `accessToken`、`refreshToken`、`idToken`，不接受含义模糊的 `token`。
  仅有 refresh token 时先换取 access token。
- 身份补全复用官方 `token_data.rs::parse_chatgpt_jwt_claims`：优先解析 `idToken`，缺失字段再由
  `accessToken` 补齐；`email` 优先 JWT 顶层值、其次 `https://api.openai.com/profile.email`，用户 ID
  优先 `chatgpt_user_id`、其次 `user_id`。该路径不调用 `whoami`，也不信任导入文档顶层的
  `userId/accountId`。
- 首次 OAuth 保留回调 `state`、PKCE 与官方 token exchange，并持久化 `idToken`、`accessToken`、
  `refreshToken`。刷新响应中的三个 token 字段均按官方语义独立轮换：返回新值时替换，省略时分别保留
  现值。重新授权也保留这些回调保护，但只轮换目标账号的 token。回调地址只承载 `code`/`state`，
  不以 host/path 形式作为拒绝条件。
- 账号文件导入和首次 OAuth 创建在 credential 提交后立即尝试一次额度观测。观测失败只记录告警，
  不回滚已提交的账号；重新授权和手工或后台 RT 刷新只更新 token，不隐式等同于手工额度刷新，也不更新
  既有账号资料或 OAuth principal。
- OAuth pending flow 先取得带过期时间的独占 claim，只有账号事务提交成功后才消费。失败会释放 claim，
  但上游 authorization code 本身通常只能交换一次；已完成过 token exchange 时应重新创建 OAuth flow。
- `GET /accounts/quota` 只读取最后一次落库快照；`POST /accounts/quota/refresh` 才访问上游。access token
  已过期时，额度刷新要求先走 credential 刷新或重新授权，不会拿过期 token 探测额度。
- OpenAI 已耗尽账号每 30 分钟主动复核一次，也会在最早未恢复窗口的 `resetAt + 2 分钟` 到期后
  提前复核。后台每 30 秒检查触发条件；同一重置边界复核后仍未恢复时回到 30 分钟重试，
  避免旧 reset 持续触发请求。各窗口独立确认恢复，时间到期本身不会直接解除账号耗尽。
- `POST /accounts/recover` 是管理员对本地事实的强制恢复：它清除 Redis cooldown 和已保存的额度/错误，
  把账号重新启用并恢复为可调度 credential；它不验证上游账号是否已经恢复，下一次真实请求仍可重新写入
  失败事实。
- 成功额度观测会 revision-fenced 写入 quota；明确 `Allowed` 投影为 `normal`，明确耗尽投影为
  `quota_exhausted`。额度观测不会清除凭据过期、无效或封禁事实；这些事实统一投影为 `error`，并由
  `errorReason` 区分。额度接口的 401/403 也不足以判定 refresh token 永久失效，credential 终态只由
  OAuth refresh 的明确永久错误写入。
- 正常 Responses 请求会解析上游响应的 rate-limit headers，合并进同一 quota 快照并同步状态。Free、
  K12 等套餐共用该状态机；套餐只参与账号展示和按套餐隔离的模型目录 cache，不存在 K12 专属额度路径。
- 账号展开区的 Token 结构和模型排行使用代表性账号级额度窗口聚合，查询边界严格为
  `[resetAt - windowSeconds, resetAt)`；额度刷新若返回了更早的重置时间，会按新边界重新聚合。无法取得
  完整窗口边界或只有模型专属额度时显示无数据，不回退成历史累计。金额原值保持完整精度，USD 展示值
  小于 1 美元时最多保留四位小数，其余保留两位。
- 账号页没有定时静默轮询。手工额度刷新只替换响应中的账号行并同步状态汇总，不触发整页 loading；若
  新状态不符合当前筛选，该行从当前页移除。请求驱动或后台任务产生的状态变化，需要下一次显式查询账号
  列表后才会显示。

### OpenAI 官方个人资料统计

`GET /api/admin/accounts/profile-statistics?accountId=...` 仅支持 OpenAI/Codex OAuth 账号。每次查询直接
访问官方个人资料端点，不读取本地 usage/billing 记录，也不缓存或估算统计结果。响应 `data` 包含：

- `displayName`、`username`、`imageUrl`：官方账号资料；
- `summary`：累计文本 Token、单日峰值 Token、最长任务时长、当前连续天数和最长连续天数；
- `dailyUsage`：按日期返回的 Token 活动；
- `activityInsights`：快速模式占比、上游原样返回的推理强度及占比、Skill 探索/使用数、聊天总数，
  以及插件与 Skill 调用排行。

官方未返回的字段保持 `null`，不使用本地数据补齐；`hasStatsError: true` 表示账号资料可用，但官方统计
部分不可用。access token 已过期或官方返回 401 时，接口要求先刷新 credential 或重新授权。原账号级
`GET /api/admin/accounts/usage-statistics` usage/billing 报表接口及其查询链路已移除。

### OpenAI 主动额度重置卡

`GET /api/admin/accounts/reset-credits?accountId=...` 每次都查询 OpenAI 上游；后端不把卡片列表写入
PostgreSQL 或 Redis。管理端只在用户打开弹窗或点击刷新时调用，并在当前浏览器会话内缓存最近一次成功
结果，用于账号行上的 `xN` 提示。

查询响应：

```json
{
  "availableCount": 1,
  "credits": [{
    "id": "credit_...",
    "status": "available",
    "title": "...",
    "expiresAt": "2026-08-31T12:00:00Z",
    "resetType": "..."
  }]
}
```

消费请求的 `redeemRequestId` 必须是小写、带连字符的 canonical UUIDv4；`creditId` 可省略，由上游选择
可用卡。一次请求发出后若传输结果不明确，重试必须复用完全相同的 `redeemRequestId`、`creditId` 和
账号。服务在单副本进程内按账号串行消费，并在 credential 需要刷新时以同一命令重试一次；它不会对不明
结果自动创建新消费。

若服务无法确认不可逆消费是否完成，返回 HTTP `502` / 业务码 `50202`；客户端应先刷新卡片与额度状态，
并在确需重试时复用原 `redeemRequestId`。明确的上游 HTTP 拒绝仍使用 `50201`，不会误标为结果未知。

```json
{
  "accountId": "acct_...",
  "creditId": "credit_...",
  "redeemRequestId": "8fbf302d-11df-4bd5-82e4-08e4b3df7874"
}
```

消费响应只返回上游结果 `code` 和可选 `credit`。消费端确认成功后应重新 GET 卡片列表，并显式调用
`POST /api/admin/accounts/quota/refresh` 回读官方额度；不得直接改写本地 `resetAt`。xAI 不支持该能力。

## 6. 账号分组

分组是 Provider-neutral 的账号集合；一个组可包含任意 Provider 账号，一个账号也可属于多个组。

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/account-groups` | `page`、`pageSize`、`search`、`enabled` | 分页查询分组；返回账号可用性、并发槽位（Redis 不可用时 `usedSlots=null`）及成功请求 USD 用量 |
| `POST` | `/api/admin/account-groups/create` | `{ name, description, color }` | 创建空分组；`color` 严格为 `#RRGGBBAA`，返回时统一大写 |
| `POST` | `/api/admin/account-groups/update` | `{ id, name, description, color }` | 更新名称、描述和颜色 |
| `POST` | `/api/admin/account-groups/enable` | `{ id }` | 启用 |
| `POST` | `/api/admin/account-groups/disable` | `{ id }` | 禁用；已绑定 Key 保持受限，不回退到全部账号 |
| `POST` | `/api/admin/account-groups/delete` | `{ id }` | 删除未被 Client Key 引用的组 |

列表数据为 `{ items, page, configRevision }`，其中 item 返回 `memberCount`、按 Provider 聚合的
`providerCounts` 和 `clientKeyCount`。查询分组成员使用账号列表的 `groupId` 筛选，
不提供独立的分组成员路由；账号的 Provider 不代表整个分组的 Provider。

## 7. Client Key

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/client-keys` | `cursor`、`limit`、`search`、`sortBy`、`sortDirection` | 游标分页查询 |
| `POST` | `/api/admin/client-keys/create` | 创建字段 | 创建带账号范围的 Client Key |
| `GET` | `/api/admin/client-keys/reveal` | `id` | 显式读取完整明文 Key |
| `POST` | `/api/admin/client-keys/update` | 更新字段 | 原子更新名称、分组范围和限额 |
| `POST` | `/api/admin/client-keys/enable` | `{ id }` | 启用 |
| `POST` | `/api/admin/client-keys/disable` | `{ id }` | 禁用 |
| `POST` | `/api/admin/client-keys/delete` | `{ id }` | 删除 |

创建字段为 `name`、可选 `label`、`groupIds`、`maxConcurrency`、`requestsPerMinute`，更新请求再增加
`id`。`groupIds` 必须显式提交：空数组派生 `routingScope: "all"`，非空数组派生
`routingScope: "groups"`。响应同时返回分组引用 `groups`，以及从当前有效账号池派生、仅供展示的
`providerKinds`；Client Key 不再保存 `providerKind`。创建和 reveal 响应会返回完整明文 Key，调用方
必须立即安全保存。

## 8. 运行设置

| 方法 | 路由 | 说明 |
| --- | --- | --- |
| `GET` | `/api/admin/settings` | 读取运行设置 |
| `POST` | `/api/admin/settings/update` | 原子替换全部运行设置 |
| `GET` | `/api/admin/settings/client-downloads/codex-desktop/windows` | 提取 Codex Desktop Windows 离线安装直链；`refresh=true` 强制刷新进程内短缓存 |
| `GET` | `/api/admin/settings/admin-api-key` | 只返回管理 API Key 是否存在 |
| `POST` | `/api/admin/settings/admin-api-key/delete` | 删除管理 API Key |
| `POST` | `/api/admin/settings/admin-api-key/regenerate` | 重新生成并一次性返回完整管理 API Key |

设置更新字段包括：

```text
modelMappings
refreshMarginSeconds
refreshConcurrency
maxConcurrentPerAccount
requestIntervalMs
rotationStrategy
minCodexDesktopVersion
minCodexCliVersion
usageRetentionDays
opsEventRetentionDays
auditRetentionDays
```

`rotationStrategy` 可取 `smart`、`quota_reset_priority`、`round_robin`、`sticky`。
两个 `minCodex*Version` 字段为 `string | null`，只设置最低版本，不存在最大版本字段。

Windows 离线包接口固定解析 Microsoft Store Product ID `9PLM9XGG6VKS` 的 Retail 包，不接受调用方提供
产品 ID、上游地址、ring 或文件名。后端只返回通过包名、架构、Microsoft CDN host/path、scheme 和失效
时间校验的 `x64` / `arm64` MSIX 直链，不代理安装包字节。Store 内容通道返回 HTTP/80 临时地址时保留
原始 scheme，不强制改写为该 host 不保证支持的 HTTPS。动态链接不足 10 分钟即失效时不会下发；某个架构
解析失败时只将该架构降级到 OpenAI 官方 HTTPS 稳定 MSIX，并通过 `warning` 说明。响应形状为：

```json
{
  "resolvedAt": "2026-09-01T06:30:00Z",
  "cached": false,
  "warning": null,
  "packages": [
    {
      "architecture": "x64",
      "source": "microsoft_store",
      "version": "26.825.6671.0",
      "fileName": "OpenAI.Codex_26.825.6671.0_x64__2p2nqsd0c76g0.msix",
      "sizeBytes": 744250000,
      "downloadUrl": "http://dl.delivery.mp.microsoft.com/filestreamingservice/files/...",
      "expiresAt": "2026-09-01T07:30:00Z"
    }
  ]
}
```

`source` 为 `microsoft_store` 或 `official_openai`。Store 的四段 package version 只用于下载展示，不参与
Desktop 三段 SemVer 门禁，也不会自动回写最低版本设置。门禁规则见
[鉴权与公共约定](#1-鉴权与公共约定)，解析器职责见 [架构文档](architecture.md#11-生命周期安全与恢复)。

## 9. 备份

全部备份端点位于 `/api/admin/settings/backups/*`，内部由独立 BackupService 承担，不并入设置用例。响应继续使用 `AdminEnvelope`，wire 字段 camelCase，`Cache-Control: no-store`。

| 方法 | 路由 | 请求 | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/settings/backups` | 无 | 读取存储配置（含明文 Secret）、验证状态与调度配置 |
| `POST` | `/api/admin/settings/backups/storage/update` | S3 配置 | 更新存储配置；`secretAccessKey` 为空字符串会校验失败 |
| `POST` | `/api/admin/settings/backups/storage/test` | 无 | 测试已保存的存储配置（Put/Head/Get/Delete 探针） |
| `POST` | `/api/admin/settings/backups/schedule/update` | 调度配置 | 更新 Cron、时区与保留策略 |
| `GET` | `/api/admin/settings/backups/records` | 查询参数 | 分页查询备份记录 |
| `POST` | `/api/admin/settings/backups/create` | `{ expiresInDays? }` | 创建手动备份，返回 `202 Accepted`；`expiresInDays` 为过期天数（0 或缺省表示不过期） |
| `POST` | `/api/admin/settings/backups/download-url` | `{ backupId }` | 创建 5 分钟有效预签名下载地址（仅 completed） |
| `POST` | `/api/admin/settings/backups/delete` | `{ backupId }` | 请求删除（进入 `deleting`，由 Worker 收敛硬删除） |

读取设置响应（Secret 以明文返回，由前端掩码显示）：

```text
storageRevision, endpoint, region, bucket, accessKeyId, secretAccessKey, prefix,
forcePathStyle, verified, scheduleEnabled, cronExpression, scheduleTimezone,
retentionDays, retentionCount, nextRunAt, lastVerifiedAt, updatedAt
```

更新存储请求字段：

```text
endpoint, region, bucket, accessKeyId, secretAccessKey, prefix, forcePathStyle
```

`secretAccessKey` 为空字符串会校验失败；由于 GET 会回传已保存的明文 Secret，保存时始终整体提交当前值。已有备份记录时，endpoint/region/bucket/forcePathStyle 不允许变化（存储身份锁定，`409`）；只允许轮换凭据与修改 prefix。

保存相同配置保留验证状态、定时计划及配置版本。存储配置实际变化时，会同时使验证失效、暂停定时计划并清空下次运行时间；连接测试通过后需重新启用计划。

更新调度请求字段：

```text
scheduleEnabled, cronExpression, scheduleTimezone, retentionDays, retentionCount
```

`cronExpression` 为 5 段格式；`retentionDays`/`retentionCount` 为 0 表示禁用对应清理。启用计划前必须已保存完整存储配置且通过连接测试。

记录列表查询参数：

```text
page, pageSize, status, trigger
```

`status` 可取 `queued/dumping/uploading/completed/failed/deleting`；`trigger` 可取 `manual/scheduled`。记录响应字段：

```text
id, triggerKind, status, scheduledAt, objectKey, sizeBytes, sha256, attemptCount,
errorCode, errorMessage, startedAt, completedAt, expiresAt, createdAt, updatedAt
```

`expiresAt` 在创建时确定：手动备份来自 `expiresInDays`，计划备份来自当时的
`retentionDays`；到期后由 Worker 进入删除流程。

连接测试响应：

```text
{ ok, stage, code, message }
```

`stage` 为 `putObject/headObject/getObject/deleteObject`。探测成功后以 `storageRevision` CAS 写入 `lastVerifiedAt`；测试期间配置变化则丢弃结果。

备份错误映射（`AdminErrorCode` 既有体系）：

| HTTP | 场景 |
| --- | --- |
| `400` | 配置、Cron、时区或状态参数无效 |
| `404` | 备份记录不存在 |
| `409` | 已有活跃任务、状态冲突或存储身份锁定 |
| `502` | S3 兼容服务返回无效或失败响应 |
| `503` | PostgreSQL、`pg_dump` 或对象存储暂不可用 |

审计动作：`backup.s3_config_updated`、`backup.s3_connection_tested`、`backup.schedule_updated`、`backup.created`、`backup.download_url_created`、`backup.delete_requested`。审计详情与记录表均不保存 Secret、数据库连接串或预签名 URL query。

## 10. Dashboard、用量与错误

| 方法 | 路由 | 说明 |
| --- | --- | --- |
| `GET` | `/api/admin/dashboard/summary` | Dashboard 汇总；支持 `kind`、`startTime`、`endTime` |
| `GET` | `/api/admin/dashboard/realtime` | 当前进程实时并发与近 60 秒入口 RPM；[指标口径](realtime-traffic.md) |
| `GET` | `/api/admin/dashboard/trend` | Dashboard 趋势；`kind=usage|latency|errors` |
| `GET` | `/api/admin/usage/records` | 请求记录分页列表 |
| `GET` | `/api/admin/usage/records/detail` | 按 `id` 查询请求详情 |
| `GET` | `/api/admin/usage/records/summary` | 当前筛选条件的请求汇总 |
| `GET` | `/api/admin/usage/insights/overview` | 用量、成本与成功率洞察 |
| `GET` | `/api/admin/usage/insights/diagnostics` | 按维度聚合诊断 |
| `GET` | `/api/admin/operations/errors` | 运维错误分页列表 |

用量查询可组合页码/游标、时间范围、Provider、Client Key、账号、模型、route、transport、状态码、
request/response/upstream ID、outcome 与搜索文本。诊断 `dimension` 可取 `model`、`account`、
`apiKey`、`provider`、`transport`、`failureClass`、`status`。

汇总与洞察中的请求数与 outcome 分布覆盖筛选范围内全部请求；token、缓存、延迟与成本聚合仅统计
已完整交付客户端的成功响应。

详情接口按 `id` 可读取成功、失败或未完成请求。新增 `trace`（历史未采集记录为 `null`）和
`relatedRequests[]`（`requestId / relation / outcome / completedAt`）；`relation` 为 `recovered_by` 或
`recovers`。`trace` 是执行终态时的有界脱敏时间线，包含 request、attempt 和 exchange 关联、阶段、
事件摘要及淘汰计数；普通用量列表不携带此字段。

错误记录中的“已自动恢复”表示系统关联到了后续成功请求，不会把原来的失败记录改为成功。
`upstreamSendState = ambiguous` 表示无法确认该次上游执行结果，不代表后续恢复请求失败；
恢复关联也不等于逐字节验证过两次请求正文。

Dashboard 的 `accountUsage[]` 由后端提供 `usageWindow`、`metricLabel`、`metricValue`。
`usageWindow` 复用账号额度窗口合同，缺失额度事实时为 `null`；窗口标签、百分比、触顶状态、重置时间
和本地用量由 Provider/Admin 投影。前端不得从套餐缺失推断免费套餐，也不得从显示时舍入的百分比推断
触顶。滚动窗口使用相应时间范围的本地用量，独立于 Dashboard 的今日统计范围。

OpenAI 的 `serviceTier` 只接受上游响应生命周期事件确认的实际 `response.service_tier`；请求里的
期望档位只保留在 request summary，不能冒充响应事实。计费展示把 `priority`/`fast` 映射为 `Fast`，
`flex` 映射为 `Flex`，缺失或 `default` 映射为 `Default`；未知非空值原样展示。Fast 优先使用模型的
priority 价格，缺少专用价格时回退到标准价格的 `2.00x`；Flex 为 `0.50x`，Default 为 `1.00x`。

## 11. 版本、更新与重启

| 方法 | 路由 | 主要 query/body | 说明 |
| --- | --- | --- | --- |
| `GET` | `/api/admin/system/version` | 无 | 当前构建、部署模式和可用更新 |
| `GET` | `/api/admin/system/update/detail` | `refresh=true|false` | 读取或强制刷新 Release 详情 |
| `GET` | `/api/admin/system/update/events` | 无 | SSE 更新事件流 |
| `POST` | `/api/admin/system/update` | 可选 `{ targetVersion }` | 开始在线更新 |
| `GET` | `/api/admin/system/update/status` | 无 | 查询当前更新或回滚状态 |
| `POST` | `/api/admin/system/rollback` | 无 | 回滚到保留的上一版本 |
| `POST` | `/api/admin/system/restart` | 无 | 请求进程重启 |

在线更新仅在当前部署模式、Release 资产和进程重启能力都满足要求时可用，且只在同一 major 版本内
提供：跨大版本目标会以 `40901` 冲突拒绝，需按发布说明重新部署。
实例升级和仓库发版见 [部署文档](../deploy/README.md#镜像升级与源码构建)。

## 12. 网关管理扩展

本节对应网关改造分支的源码合同，部署可用性和各批次验证状态见 [实施记录](implementation-progress.md)。
所有接口沿用管理员鉴权、成功/错误信封与 `no-store`。下面四类列表均支持 `page`、`pageSize`、`search`，
返回 `items`、`total`、`configRevision`；配置写入同时提交版本与审计，事务失败不留下半完成配置。

| 方法 | 路由 | 主要内容 |
| --- | --- | --- |
| `GET` | `/api/admin/customers` | 轻量客户列表及关联 Key 数量 |
| `POST` | `/api/admin/customers/create`、`/update`、`/delete` | 创建、修改、删除客户；关联 Key 的客户不可删除 |
| `POST` | `/api/admin/customers/assign-key` | `{ keyId, customerId }`；`null` 解除归属 |
| `GET` | `/api/admin/access-groups` | 模型白名单、号池/渠道授权及组限额 |
| `POST` | `/api/admin/access-groups/create`、`/update`、`/delete` | 分组配置及来源关系一起提交；有关联 Key 时不可删除 |
| `POST` | `/api/admin/access-groups/assign-key` | `{ keyId, accessGroupId }`；`null` 解除归属 |
| `GET` | `/api/admin/channels` | 渠道来源、默认优先级/权重、限额、共享配额及配置版本 |
| `GET` | `/api/admin/channels/providers` | 可用的渠道适配器配置说明 |
| `GET` | `/api/admin/channels/connection?id=...` | Provider 脱敏后的编辑配置；不是明文凭据导出 |
| `POST` | `/api/admin/channels/create`、`/update`、`/delete` | 渠道 CRUD；修改和删除要求 `expectedRevision` |
| `GET` | `/api/admin/quota-scopes` | 具名共享配额及 `sourceCount` 引用数量 |
| `POST` | `/api/admin/quota-scopes/create`、`/update`、`/delete` | 共享并发/RPM；仍被渠道或号池引用时不可删除 |
| `GET`、`POST` | `/api/admin/settings/admission` | 读取、替换全站 `maxConcurrency`、`requestsPerMinute` |
| `GET` | `/api/admin/dashboard/realtime` | 当前进程实时并发和最近 60 秒请求量 |

表中缩写的 `/update`、`/delete` 均属于同一行的资源前缀，例如客户修改完整路径为
`/api/admin/customers/update`。客户和共享配额创建/修改使用 `name`、`note`、`enabled`、
`maxConcurrency`、`requestsPerMinute`；修改另带 `id`。分组在这些字段之外使用 `allowedModels`、
`poolGroupIds`、`channelIds`、`allowCapacityFallback` 和 `sourcePreferences`，创建/更新均要求显式提供。不因模型列表为空或渠道未勾选而隐式授权。

`sourcePreferences` 是覆盖数组，每项为 `{ kind: "account_pool" | "channel", sourceId, priority, weight }`。
`priority`、`weight` 可分别为 `null`（或省略）以继承来源默认值；每项至少覆盖一个字段，覆盖值须为 1–65535 的整数。
来源必须位于本次提交的授权集合，重复来源或未授权覆盖拒绝；传空数组清除全部覆盖，不改变来源限额或共享配额。
`allowCapacityFallback: false` 在来源或账号容量拒绝时允许同级来源，但禁止进入更低优先级来源；
不禁用既有故障重试，也不解除原生会话来源锁定。迁移后的既有分组默认为 `true`，无分组旧 Key 的行为不变。
有效偏好及回退开关随请求计划冻结，关系、版本和审计在同一事务提交。

渠道公开配置包括 `provider`、`priority`、`weight`、`quotaScopeId`；创建时还需要 Provider 验证的
`config`。配置版本 `connectionRevision` 和更新时的 `expectedRevision` 为字符串，避免数字精度损失。
版本冲突需重新读取；旧请求候选不能使用轮换后的配置。首个 `openai_api` 渠道仅开放已验证的 Responses
路径，不因录入模型 ID 自动获得 Chat Completions、图片或视频适配。

号池在既有账号分组接口的 `sourceControls` 中配置 `priority`、`weight`、`maxConcurrency`、
`requestsPerMinute`、`quotaScopeId`。更新省略整个 `sourceControls` 时保留已有值。优先级 1 最高，
权重只比较同级来源；限额 0 表示不限。设置 `quotaScopeId: null` 解除共享配额关联，停用共享配额会阻止
其关联来源的新调用，不能把它理解为不限额。

实时视图返回 `scope: "process"`、`observedAt`、`windowSeconds`、`uptimeSeconds`、
`ingressRequestsLastMinute`、`inFlightRequests`、`preparingRequests`、`executingRequests`。
重试不新增下游逻辑请求，固定账号诊断与 WebSocket 心跳不计入业务请求；进程重启后该实时窗口重新积累。
历史统计继续使用既有用量接口，实时 RPM 与各级准入 RPM 不是同一个计数器。

### 渠道模型发现与成功记录

`POST /api/admin/channels/discover-models` 使用管理员会话鉴权，响应禁止缓存。请求仅包含 `id` 和字符串 `expectedRevision`；不能临时传入 URL、API Key 或其他连接字段。

- 当前支持 `openai_api` 渠道。Provider 使用该渠道已保存的地址、Key、Organization 和 Project 请求相对路径 `models`，不使用 OAuth 账号或下游认证头，不执行 Responses 生成。
- 查询前检查渠道连接版本并预留发现序号，成功后在短事务内锁定渠道、复核版本并保存记录。版本不匹配、查询期间更新或删除渠道、较新序号的成功结果已保存时返回 409。初始读取时渠道不存在返回 404，Provider/Store 暂不可用为 503，上游状态或目录格式异常为 502。发现只保存观测记录，不提交配置、不增加配置版本、不写配置变更审计、不发布运行快照。
- 成功的 `data` 为 `{ id, connectionRevision, generation, fetchedAt, added, missing, unchanged }`。版本和发现序号都是十进制字符串，不得转换成 JS Number；序号由数据库在查询前分配，允许跳号，不按完成时间排序。时间为本次成功查询完成时间。三个数组按上游模型 ID 排序，分别表示相对于该版本已配置列表的新增、本次未发现和重合项；不会随之后的配置变更重算。`missing` 不表示应删除，也不是关停证据。
- 第一版只接受完整单页的 `object: "list"`、`data: [{ id }]` 合同。限制 15 秒、2 MiB 响应和 1000 项；拒绝重定向、重复或非法 ID、非 200 状态、未知顶层字段、`Link` 响应头以及正文分页续页信号，不返回部分结果。尚不支持分页渠道；未知格式失败不影响本地已配置模型。模型条目上的额外能力/价格字段不作为能力或定价证据。
- 页面入口为“上游渠道 → 编辑 → 上游模型发现”。查询显式触发；选择新增模型后只追加到编辑草稿，最后通过原 `/channels/update` 版本检查、审计事务与发布链保存。保留未发现的旧模型，不自动授权接入分组或证明模型支持 Responses。
- `GET /api/admin/channels/model-discovery?id=...` 使用同样的管理员鉴权和禁止缓存合同，只读本地最新成功记录，不读取渠道凭据或请求上游。已有渠道但从未成功发现时 `data` 为 null；有记录时返回上述对象，包括旧连接版本的记录。不存在的渠道返回 404，存储暂不可用返回 503。
- 每次成功保存追加一条不可变记录，成功且保存完成才返回 POST 成功；上游失败或保存事务回滚不改动旧记录。较早启动的查询若晚于较新成功结果保存，会被拒绝且不进入成功历史。最新记录仍为该渠道最大的发现序号。超时或响应丢失导致保存结果不确定时，可通过 GET 重新读取确认，不必立即重查上游。删除渠道同步删除全部发现记录。
- `GET /api/admin/channels/model-discoveries?id=...` 只读本地成功历史，使用同样的管理员鉴权和禁止缓存合同。可选 `pageSize` 默认 20、范围 1–50；`beforeGeneration` 为不含符号、前导零的十进制正整数字符串，最大 9223372036854775807。未知或非法参数返回 400，不存在的渠道返回 404，存储暂不可用返回 503；已有渠道的空历史返回空数组，不将存储失败伪装为空结果。
- 历史响应 `data` 为 `{ items, nextBeforeGeneration }`；每项同上述成功对象，游标为字符串或 null。按渠道隔离、发现序号降序，下一页严格小于返回的游标；并发新增记录不会导致向旧记录翻页时重复。没有总数或跨请求事务快照，回到首页可读取新成功记录。
- 打开编辑窗口自动读取最新本地记录；手动查询才访问上游。失败保留上次显示但禁止添加，成功重新读取或查询后恢复；旧连接版本记录始终只读，须查询当前版本才能添加。未保存连接更改时禁用发现。关闭窗口丢弃草稿，不删除成功记录；空发现结果与从未成功发现分别展示。
- “查看发现历史”才加载历史列表，每页 10 条，可向前/后翻页、刷新及展开记录；历史均只读，无直接采用按钮。显示当前渠道名称/Provider/ID及各条记录的连接版本、查询时间；名称不是历史名称快照。记录中的差异始终相对于查询时配置，并非两次发现之间的比较。失败保留上一页并提示可能过期；关闭后迟到响应失效。
- 迁移 `0012` 延续原表最后成功记录，不恢复升级前已被覆盖的记录。当前无自动清理策略，渠道删除时级联清理；失败尝试历史与定时同步尚未实现。

### 两次渠道发现比较

`GET /api/admin/channels/model-discoveries/compare?id=...&baseGeneration=...&targetGeneration=...` 使用管理员鉴权与 `no-store`，仅比较同一渠道的两条已保存成功记录，不查上游、不读取凭据、不写配置或审计。

- 两个序号均为规范十进制正整数字符串，范围 1–9223372036854775807，必须 `baseGeneration < targetGeneration`。同一记录、逆序、未知参数或非法值返回 400；渠道或任意选中记录不存在返回 404，存储暂不可用返回 503，不能把错误展示成“没有变化”。不按墙上时钟判断先后。
- Store 在单次查询中按渠道和两个序号读取，防止混用其他渠道的记录；比较期间后续成功发现不会改变这组不可变记录。
- `data` 为 `{ id, base, target, sameConnectionRevision, appeared, disappeared, unchanged }`；`base`、`target` 均包含字符串 `generation`、`connectionRevision` 和 `fetchedAt`。三个模型数组排序并去重，分别表示仅目标发现、仅基准发现、两次均发现。
- 比较集合由每条记录的 `added ∪ unchanged` 还原；记录中的 `missing` 是查询时配置而非上游已发现集合，不参与还原。只改变本地配置分区而未改变发现集合时，比较结果没有新增或消失。
- 允许跨连接版本比较，但 `sameConnectionRevision` 为 false，页面明确提示地址、凭据或配置变化也可能造成差异，不直接解释为上游新增/下架。未再发现不自动删除，也不推断能力、价格或可用性。
- 历史中可跨页设置基准和目标，点击“比较所选记录”才请求比较。结果只读，没有直接采用按钮；采用模型仍走当前渠道发现和草稿保存。改变选择清空旧结果，清空/收起/关闭使迟到响应失效；同一组重读失败保留旧比较并明确本次读取未确认。

### 管理运行模型目录

`GET /api/admin/model-catalog` 使用管理员会话鉴权并返回 `Cache-Control: no-store`，不使用 Client Key。只读取当前运行快照，不触发上游发现或测试。

- 查询：`page` 默认 1 且必须大于 0；`pageSize` 默认 20、范围 1–200；可选 `search`（最多 256 字节）、`provider`、`sourceKind`、`configurationReady`（布尔）。未知字段、非法枚举或分页参数返回 400，快照不可用返回 503 而非空目录。
- `data` 包含 `items`、筛选后 `total`、`configRevision`、`providerGenerations` 和全目录 `providers`。配置版本、连接版本和目录代数均为十进制字符串，客户端不得转换成 JS Number。
- 每项包含 `identityKey`、`provider`、`upstreamModel`、`publicNames`、展示名称/描述、`source`、`configurationReady`、操作/能力和上下文/输出上限。`identityKey` 为不透明稳定行键，同名模型跨来源分别保留；公开名称解析映射链，被映射覆盖的原模型名不自动视为其公开名称。
- `source.kind` 为 `channel`、`account_pool`、`unpooled` 或 `provider_catalog`；最后一种表示适配器有模型目录但没有账号来源，不代表可调用。来源包含可空的 `id`、`name`、`connectionRevision`、默认优先级/权重、并发/RPM 和共享配额 ID，不含连接配置或凭据。来源容量 0 表示不限，未知值为 null。
- `features` 固定列出 `tools`、`vision`、`reasoning`、`json_schema`、`native_continuation`，值为 `native`、`emulated`、`unsupported` 或 `unknown`；能力缺失保持未知，即使 `upstreamValidatesFeatures` 为 true 也不能推断为支持。能力来自适配器目录，不是每账号实测证据。
- 配置就绪仅反映来源启停/共享配额配置及账号来源存在性，不验证健康、凭据、余额、客户授权或生成成功。停用渠道若不在运行快照中不会列出；此接口不是所有持久化配置的完整目录，也不返回价格、上游同步时间或虚构测试结果。
