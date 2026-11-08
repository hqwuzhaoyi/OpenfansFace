# 批次看板接口 v1

状态：已实现并通过隔离接口验证，尚未正式部署。统一发布线程负责最终 `/operations` 接线、Web + worker 切换。GPU / VM 契约见 `../processing-gpu-worker.md`；真实 640 性能及统一发布仍未验收。

## 接口

成功和错误响应均含 `schema_version: 1`，时间均为 Unix 秒。GET 使用 `Cache-Control: no-store`。客户端不得以失败读取替代为零。

- `GET /api/processing/dashboard`：批次累计、当前任务、执行历史、设置及资源状态。
- `POST /api/processing/dashboard`：`{"action":"retry","batch":"批次ID"}`；省略 batch 重试全部当前异常。返回 `{"schema_version":1,"retried":2}`。仅选中 `stage=error` 且未人工接管的任务，不把成功记录重跑。
- `GET /api/processing/settings`：返回 `schema_version, requested, maximum, effective, supported, online, last_seen`。
- `PUT /api/processing/settings`：`{"concurrency":1}`；返回新设置。仅接收整数 1..maximum，布尔值、字符串、小数拒绝。调低只限制新领取，在途执行不取消。
- 原 `POST /api/processing` 的 pause/resume、任务单项操作保持现有地址与行为。

示例设置响应：

```json
{"schema_version":1,"requested":2,"maximum":2,"effective":2,"supported":true,"online":true,"last_seen":1794078000}
```

`effective` 是当下允许领取的并发上限，不是运行中任务数。缩小后运行数可暂时大于它；实际运行看 jobs。maximum 来自 worker 心跳能力，后台默认上限 1；只有显式配置并完成资源验收后才可提高至 2。未来 GPU 不能仅修改前端上限就宣称已支持。

## 数据结构和身份

完整类型见 `lib/processing-dashboard.ts`、`lib/processing-dashboard-model.ts`、`lib/processing-progress.ts`。运行实例样例保存于本机 `.local/experiences/processing-batch-live-20261108/dashboard-response.json`，仅含标记隔离夹具。

- `batch.id` 是发现批次 ID，不是人工确认分组。
- `vid` 沿用现有任务 ID，文件身份额外校验 path、size、mtime_ns。
- `jobs[].id` / `history[].id` 是每次阶段尝试的新 UUID（run_id），不能拿 vid 代替。旧事件降级历史使用 `event-<id>`，history_source 明确区分。
- `batches[]`：id、started_at、total、completed、running、waiting、error、excluded、text_done、face_required、face_done、face_skipped、legacy、refreshing、refresh_waiting、refresh_error。
- `totals`：上述所有计数字段求和，不含 id、时间、legacy。
- `current`：当前分析任务 running、waiting、error、review；review 使用统一 `isReviewAvailable` 判断，不能简单计 stage=review。
- `jobs[]`：HomeJob。kind 为 text/face；status 为 running/waiting/paused/complete/error/offline；done/total 可为 null。进度仅对应当前阶段真实采样帧，不是整个视频处理百分比。
- `history[]`：id、label、status、stage、at、finished_at、error、batch。新运行状态包括 running/done/retry/error/waiting/interrupted/cancelled/superseded；重试历史保留。
- `online, paused, settings, inference, media_issue, discovery, references, events` 保留运行诊断。inference 为当前心跳报告的配置及可用状态，缺省为未知；每次运行的真实来源由 history.execution 提供。

## 统计与未知状态

每次登记以文件指纹保存固定批次成员。`total = completed + running + waiting + error + excluded`。已完成的历史成员优先归 completed；归档或人工确认不能让累计倒退。未完成但已移出/人工接管的成员计 excluded，不算成功。

`completed` 是首次有效分析 ready，不代表人工确认或归档。参考更新产生的复核在 refreshing/refresh_waiting/refresh_error 另列，不重复增加分母，不抹去先前成功。当前异常可含复核错误，因此与累计 error 不必相等。

人脸分母仅含已确认需要人脸阶段的成员；文字完成前需求未知，不提前当作人脸待办。已明确账号可 face_skipped。旧账本 backfill 标记 legacy，无法恢复的首次完成时间保留 NULL，不伪造速度/预计完成时间。

账本迁移完成标记为 `settings.batch_ledger_version=1`。未完成时 ledger_ready=false，前端显示未知，不画 0% 或清空任务。心跳超过 90 秒或时间超前超过 30 秒视为离线；保留最后进度，不继续推算运行耗时。API 异常返回 error，前端保留已加载快照并显示读取失败。

错误格式 `{"schema_version":1,"error":"可操作的错误信息"}`。400：无效 JSON、请求对象/操作/并发/批次格式；503：后台离线、旧心跳协议或未初始化等不可用状态；500：意外写入失败。未知响应字段可忽略；不支持的 schema_version 应拒绝执行写操作。

## 持久化和写入边界

中央 `FACES_BASE/processing/processing.db` 使用 SQLite WAL。worker 初始化表和迁移，领取使用 BEGIN IMMEDIATE；Web PUT 写 settings.concurrency，并在同一事务写 events。设置默认 1，重启保留。worker 心跳协议 3 发布 max_concurrency/concurrency/active。部署须保证一份中央队列，仅一个监督 worker；不能把数据库 WAL 放 SSHFS 由 WSL 写入。

新增表：processing_runs（尝试历史）、batch_items（累计成员）；tasks 仅新增 run_id，旧 data.ready、证据与文件指纹兼容。每个阶段进度写 `jobs/stages/sha256(run_id).json`，防止同类并发互相覆盖。

涉及共享记录：自动处理不再由 Python 写 faces.db 的 videos 或 watermarks/results.json。结果、OCR 合并、人物相似组与首次批次完成均提交原生 processing-result 操作。尝试级临时素材只在原生接受回执后可见。完整契约、恢复顺序与未完成验收见 [GPU / VM 工作流](../processing-gpu-worker.md)。
