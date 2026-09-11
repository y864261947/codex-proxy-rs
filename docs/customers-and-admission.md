# 客户与下游限额

客户是多个下游 Key 的共享限额与归属单位，不是登录用户。无需创建客户即可继续使用原有 Key；关联客户后，一个请求必须同时满足 Key 和客户的并发、RPM 上限。0 表示该层不限制，不覆盖其他层限制。账号分组权限不随客户归属变化。

管理入口位于「下游接入 → 客户」，点击密钥名称下的客户归属可关联或解除客户。客户停用后其 Key 不接受新请求，列表状态显示“客户停用”；已经执行的请求保留原策略直到结束。仍有关联 Key 的客户不能删除。

## 管理接口

所有接口使用既有管理员鉴权，响应禁止缓存，并使用统一 `code/message/data` envelope。

| 方法与路径 | 请求 | data |
| --- | --- | --- |
| `GET /api/admin/customers` | `page`（默认 1）、`pageSize`（默认 50，最大 200）、可选 `search` | `items`、`total`、`configRevision` |
| `POST /api/admin/customers/create` | `name`、可选 `note`、`enabled`、`maxConcurrency`、`requestsPerMinute` | `id`、`configRevision` |
| `POST /api/admin/customers/update` | 上述字段及必填 `id` | `id`、`configRevision` |
| `POST /api/admin/customers/delete` | `id` | `id`、`configRevision` |
| `POST /api/admin/customers/assign-key` | `keyId`、必填 `customerId`（客户 ID 或显式 `null`） | Key 的 `id`、`configRevision` |

客户列表包含名称、备注、启用状态、独立限额、关联 Key 数量和创建/更新时间。Key 列表新增 `customer`：未关联时为 `null`，关联时包含客户 `id/name/enabled`。Key 创建支持可选 `customerId` 和 `accessGroupId`，与 Key 本身一次提交；省略时保留旧客户端的创建行为。已有 Key 通过归属接口修改关联，普通编辑不改变关联。

## 请求与恢复

Core 在准入时冻结全部限额范围；Redis 单个 Lua 操作先检查每层，再一次性占用。并发释放不删除 RPM 事实，重复请求 ID 不重复计数或刷新 RPM 时间。结束、失败、取消沿既有释放通道释放冻结范围。

迁移 `0002_customers.sql` 新增客户、Key 当前归属和请求历史客户引用。PostgreSQL 请求记录保存准入时的客户引用；恢复 Redis 时按该引用聚合，不读取 Key 当前所属客户。因此调整归属、删除 Key 或删除已无 Key 的客户，都不会改变原请求的恢复依据。原有请求客户引用为空，按 Key 恢复。

Redis 多范围键使用同一 hash tag，单次准入/释放可以原子执行；它仍是可重建的协调状态。控制面修改客户或归属时，在同一 PostgreSQL 事务更新配置版本、记录审计，然后发布新运行快照。

## 接入分组

「下游接入 → 接入分组」定义精确对外模型白名单、显式允许的号池和共享并发/RPM。一个 Key 可同时关联一个客户和一个接入分组，各层独立限制。空模型或空号池表示尚未授权，停用组后拒绝新请求；仍关联 Key 的组不能删除。

管理接口为 `/api/admin/access-groups` 及其 `create/update/delete/assign-key` 子路径，结构与客户一致。创建和更新还要求显式提供 `allowedModels`、`poolGroupIds`、`channelIds`、`sourcePreferences` 数组和 `allowCapacityFallback` 布尔值；赋组要求 `keyId` 与显式的 `accessGroupId`（组 ID 或 `null`）。Key 投影的 `accessGroup` 包含 `id/name/enabled`。

组授权在模型别名映射之前检查，模型列表使用相同白名单。分组 Key 调用原生端点也必须提供 Provider 能识别的已授权模型。模型权限和号池范围均不能被直接填写上游模型名绕过。

分配接入分组后使用组内号池权限，保留 Key 原有账号分组绑定；显式解除接入分组会恢复这些旧权限，页面显示恢复范围。请求历史保存准入时的组引用，修改归属不会改变正在执行请求的释放与重启恢复依据。

分组可对已授权号池或渠道分别覆盖优先级和权重，留空继承来源默认值；覆盖不改变容量与共享配额。关闭满载回退后，来源/账号容量拒绝只允许同级来源，不会降级到低优先级；故障重试和原生会话锁定规则不变。旧分组迁移后默认允许满载回退。页面保存失败保留输入，取消授权的来源不再提交覆盖。

上述功能的开发、验证和发布状态以 `implementation-progress.md` 为准；不能把已实现视为已上线。

## 全站请求限额

系统设置中的「全站请求限额」配置所有下游 Key 共享的最大并发和最近 60 秒准入 RPM，0 表示本层不限。`GET/POST /api/admin/settings/admission` 读取或更新 `maxConcurrency`、`requestsPerMinute`，返回两字段及 `configRevision`。与配置版本、安全审计原子提交；运行设置的其他保存动作不修改这两个值。

全站范围与 Key、客户、组在同一 Redis 操作中检查和占用。全站限额耗尽通过服务容量错误返回，客户/Key/组限流保留下游限流错误；不对外回显内部归属。即使当前不限也保存全局占用，降低上限不会终止已有执行，只限制之后的准入。概览的入站 RPM 包含未通过准入的请求，因此与准入 RPM 口径不同。

重启恢复从历史业务请求恢复全站范围，排除内部探针；请求结束释放并发但保留窗口内 RPM。WebSocket 每次新生成读取当前权限和限额，已经执行的生成仍使用本次冻结的范围。
