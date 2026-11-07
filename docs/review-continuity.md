# 人工确认连续操作（Rust，#18）

`/review` 的已展示依据由 Rust 保存在本机操作日志库，Next.js 仅展示和转发。Python 仍拥有分析任务与模型结果。人脸重算可改变候选、排名、相似组和分析进度，但不改写操作员正在核对的快照。

## HTTP 契约

同现有归名入口，POST `/v1/archive`，请求为 `{command,body}`，使用既有认证、串行执行器和回执查询。

- `review-page`：body 是查询参数对象（`view,group,q,folder,page,vid,person,cluster`）。首次不传 `session` 创建快照；后续必须传返回的 `session`。返回原有页面字段，以及 `session,remaining,next_page,previous_page`；每行携带 `review_token`。
- `review-status`：`{session}` 返回 `has_updates,changed,waiting,refreshing`，不替换或扩展快照。
- `queue-confirm` 的 `source:'review'` 必须带 `review_session` 和 `review_tokens:{[vid]:token}`。令牌绑定该会话实际送达的行、其人工状态和原分组；任意 ID、别的会话令牌及尚未送达的行均被拒绝。可继续传原有 `review_group` 和 `fingerprints`。
- `review-note`、`review-defer`、`review-restore` 同样要求会话及已送达令牌，写入走原生预览、WriteGate、持久化意图、任务行 CAS、补偿和回执。只修改明确的人工作业字段，保持模型数据、阶段、尝试和租约字段；不另开 Node 写入路径。
- 带 `review_session` 的 `queue-undo` 只撤销该会话的原始归名回执，不能撤销另一标签页后来保存的决定。界面回执保留原确认会话，手动或空闲刷新后仍用原会话撤销。无会话的待归档入口保留既有契约。
- 写操作可带 `operation_id`。同编号、同请求重放原回执；不同请求冲突。界面对不确定传输保留原编号并在重试时复用。

会话有效期固定为 8 小时，服务重启后保留。未知、失效或不兼容的会话返回 409 `review_session_expired`，不会改用最新候选。送达令牌在同一会话、同一依据和人工状态下保持稳定。完成回执的幂等重放不依赖尚未过期的会话。

## 不变依据与人工回执

日志 schema 7 新增 `review_sessions` 和 `review_deliveries`。启动迁移保留已有操作、回执和锁记录；旧版本拒绝打开 schema 7，新版本也拒绝高于其支持范围的日志。整合其他工单时，中央迁移版本取包含全部处理器后的最大值。

每次请求和准入各创建独立 `MaterialReader`，复用该次读取的源索引、业务 JSON、人工字段和按视频索引的队列/历史；没有跨请求材料缓存。正常视频不重复读取整个体检报告。500 行隔离样本首次快照 427ms、状态检查 175ms，仅为本机样本，不代表正式存储吞吐。

快照保存完整任务依据、规范化人物字段、原始分组、相关人物/账号/视频的 `shared_metadata::material_state`、文字证据和当前有效体检隐藏状态。确认在共同准入锁内重新校验：

- 实际路径和文件指纹，以及任务中的路径、大小、mtime；
- 相关账号规则、别名、人工账号关联、人物资料和归属/处置；
- 原文字与目录线索，人工接管、暂缓及备注；
- 当前文件对应的有效体检隐藏状态。

单纯人脸重算和无关任务进度不参与材料失效判断。当前任务消失仍会拒绝旧决定；后台重算耗尽重试进入失败状态，本身不使已送达依据失效。刷新也保留已有 ready 依据且 retry_stage=face 的失败任务，文字失败或无 ready 依据不因此进入核对。原生人工操作的逐视频回执序列还防止“归名后撤销”“改备注后改回”等 ABA 重放。

快照本身不变。展示层只叠加**本会话已完成的原生回执**：移除已归名项、更新本次备注或暂缓状态。撤销的可选任务恢复只有存在完成的子操作回执且该行确实恢复时才进入待核对展示。别的会话和后台变化只成为更新提示。

分组次序来自原始快照。每个状态通道保留原始页槽；后续恢复到该通道的条目追加到末尾，不挤动已存在的跨页条目。失败批次保留全部未提交项。跨页备注完成后，界面仅续取受影响行的会话令牌，不导航或替换当前页面。

## 前台更新

`REVIEW_IDLE_MS=120_000` 集中定义空闲边界。点击、键盘、输入、滚动、触摸、窗口重新获焦或页面恢复可见都算活动。

轮询只更新轻量提示和真实分析数量。最近活动、任意勾选、人物/账号输入、未保存备注、正在播放的视频、详情/参考弹层、选择详情、更新确认框或提交任务都会阻止自动更新。隐藏页面也不会自动替换。

空闲至少两分钟且没有上述保护时才请求新快照；响应返回前又发生活动，或已有更新的请求，则丢弃响应。失败保留原列表和输入。明确更新遇到勾选/草稿时说明清空影响并允许取消；只有新快照成功接收后才清理已同意丢弃的输入。关闭确认框和窄屏详情恢复触发点焦点。

`/operations` 的真实阶段计数保留，`review` 可用数量包含人脸后台更新期间已有完成依据的任务；模型工作仍显示为分析中。未改写 Python 队列实现。

## 验证证据（2026-11-08）

全部写入使用 `/tmp/face-admin-person-fileops-*` 标记夹具，无正式媒体写入。

- `python3 tests/http-rust-review-continuity.py --port 19008`：送达/会话/分组范围、未查看行、未知会话、后台候选变化和新增、固定页槽、实际完成范围、重复提交、跨标签页、重启、ABA、备注/暂缓/恢复、通道追加、失败批次、文件/任务路径/账号规则变化、相关体检隐藏和过期。
- `python3 tests/http-rust-archive.py --port 19008`：原有 #5 全套归名、撤销、可选规则、归档、真实失败、回退、重启和结果未知回归。夹具只增加服务端展示会话，不绕过新校验。
- `python3 tests/http-rust-shared-metadata.py --port 19008`：#13 全套材料范围、账号、发布、并发及中断恢复回归通过。实际旧 #5 二进制拒绝 schema 7，日志版本保持 7。
- `cargo test --manifest-path services/file-operations/Cargo.toml`：64 项 Rust 测试通过。
- `node --test tests/review-continuity.test.mjs tests/review-selection.test.mjs tests/processing-model.test.mjs tests/review-list-summary.test.mjs`：10 项通过；120000ms 边界及请求活动代数有明确断言。
- `pnpm exec tsc --noEmit` 和 `pnpm build` 通过。构建在预览进程停止后执行，未覆盖正在服务的构建输出。
- 真实浏览器连接隔离 Next/Rust 服务（含生产构建 standalone）：跨页备注后确认及撤销保留原页，活动冻结（保留 24 行与 200px 滚动位置）、勾选、草稿、播放保护、空闲更新、迟到响应丢弃、手动更新取消/接受、失败保留列表、隐藏及回到前台的保护；详情/键盘、390px 无横向溢出、关闭恢复触发焦点、reduced-motion 入场 0s。50 个隔离 ffmpeg 视频，实际播放验证时长 8 秒，播放状态为 `paused:false`。

浏览器空闲边界通过页面测试时钟推进验证；轮询和新快照请求均访问真实服务。隐藏状态与网络失败使用页面测试注入，未新增产品测试后门。原始截图和会话结果位于 `.local/experiences/review-continuity-rust-20261108/`（Git 忽略）；提交的摘要见 `tests/evidence/review-continuity-20261108.json`。

本工单完成代码与隔离验证，尚未正式部署。#17 有效健康材料、#19 真实 GPU/worker 发布与中央 schema 最大版本的联合验收由统一集成继续执行。
