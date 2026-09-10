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

管理接口为 `/api/admin/access-groups` 及其 `create/update/delete/assign-key` 子路径，结构与客户一致。创建和更新还要求显式提供 `allowedModels` 与 `poolGroupIds` 数组；赋组要求 `keyId` 与显式的 `accessGroupId`（组 ID 或 `null`）。Key 投影的 `accessGroup` 包含 `id/name/enabled`。

组授权在模型别名映射之前检查，模型列表使用相同白名单。分组 Key 调用原生端点也必须提供 Provider 能识别的已授权模型。模型权限和号池范围均不能被直接填写上游模型名绕过。

分配接入分组后使用组内号池权限，保留 Key 原有账号分组绑定；显式解除接入分组会恢复这些旧权限，页面显示恢复范围。请求历史保存准入时的组引用，修改归属不会改变正在执行请求的释放与重启恢复依据。

上述实现覆盖第二批的客户、接入分组与 Key 管理。外部渠道、来源调度及其他层级限额继续按实施记录推进。
