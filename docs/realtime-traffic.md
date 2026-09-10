# 实时负载

管理端 `GET /api/admin/dashboard/realtime` 返回标准 `{code, message, data}` 信封，要求现有管理员会话或部署级管理 Key，响应禁止缓存。

| data 字段 | 口径 |
| --- | --- |
| observedAt | 服务端 UTC 采样时间 |
| scope | 固定 `process`；当前单进程，不是跨副本汇总 |
| windowSeconds | 60 |
| uptimeSeconds | 计数器单调时钟运行秒数 |
| ingressRequestsLastMinute | 当前秒及之前 59 秒的业务入口次数，包含接入失败 |
| inFlightRequests | preparingRequests + executingRequests |
| preparingRequests | 已进入 Core，尚在路由、准入或首个执行会话准备中的逻辑请求 |
| executingRequests | 已创建执行会话，覆盖上游执行、重试、流式交付和取消收敛 |

入口次数在 HTTP 业务 POST、WebSocket 文本创建尝试处记录；执行生命周期在 Core 持有同一个 RAII lease。一次重试不能变成第二次下游请求。准备失败、future 被丢弃、会话终结和取消路径均需释放。

计数器采用固定 60 桶，空间不随 RPM、模型、Key 或账号数量增长。读写锁内不执行网络、磁盘或异步操作，不影响原有 Redis 准入限额；模型请求日志仍是完成用量事实的唯一持久化记录。

前端每 5 秒读取一次，同一组件禁止请求重叠；页面隐藏时暂停发起请求，卸载取消未完成请求。采样失败保留上次读数并标明失效，不以零值替代未知。号池槽位和历史健康图来自原有概览查询，保持 30 秒刷新。

当前没有异步视频任务或排队系统；待这些能力实现后，为它们定义任务生命周期与独立容量指标，不能使用 HTTP 提交请求数代表后台生成任务数。供应商、渠道、号池、模型及客户维度将在对应实体接入后添加。
