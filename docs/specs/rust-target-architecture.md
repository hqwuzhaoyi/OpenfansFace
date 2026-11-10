# Rust 统一分析调度与状态权威、Python 纯计算常驻化及 Peach 扫描协调规格（#30）

## 问题与目标

当前过渡版本已将文件操作、关联元数据及分析结果落库（`processing-result`）交由 Rust 权威执行，但分析队列发现与领取仍由 Python 管理、部分控制接口与统计查询仍由 Node.js 直接读写 `processing.db`、部分规则匹配仍留在 Python 脚本中，且 Peach 作为外部写入者在归档过程中扫描插入目标记录仍会导致归档冲突。

本规格将系统收敛为明确的**三层终态架构**，并建立 **Rust 归档与 Peach 外部扫描写入的可执行协调机制**：

1. **Next.js（页面与交互）**：仅负责界面呈现与同源 API 薄转发，不再直接读写 `processing.db`、业务 JSON 或 Peach 账本。
2. **Rust（`services/file-operations`：业务 API、调度、校验、持久化与协调权威）**：
   - 统一管理任务发现、领取、租约、心跳、超时回收、重试预算与并发控制；
   - 统一提供暂停/恢复、优先级、人工重试、异常接管、暂缓/恢复核对、备注与并发设置 API；
   - 统一提供批次统计、实时进度、运行历史、任务队列与回执查询；
   - 统一执行规则匹配、是否跳过人脸阶段、候选聚合与状态转换决策；
   - 统一管理结果缓存有效性、证据发布、参考特征快照触发与旧临时产物淘汰；
   - 与 Peach 扫描共享可执行的归档协调机制。
3. **Python（常驻纯计算算子）**：
   - 仅保留 OCR、人脸检测、特征提取及 NumPy 向量相似度等纯计算；
   - 模型进程常驻运行，避免每任务重复加载；
   - 接收 Rust 下发的明确输入、参数与版本，返回计算结果与待验收临时产物；
   - 不直接领取业务任务、不改人物归属、不写业务数据库或正式账本，也不自行决定重试。

## 工单与任务图（Task Graph）

- **父工单**：[#30](https://github.com/hqwuzhaoyi/OpenfansFace/issues/30) `[终态架构] Rust 统一分析调度与状态权威、Python 纯计算常驻化及 Peach 扫描协调`
- **子工单与依赖**：
  - [#31](https://github.com/hqwuzhaoyi/OpenfansFace/issues/31) `[终态架构 01/04] 分析调度与控制接口迁入 Rust（发现、租约、重试与操作台控制）`（初始 Frontier）
  - [#32](https://github.com/hqwuzhaoyi/OpenfansFace/issues/32) `[终态架构 02/04] 业务规则决策与批次统计、进度、历史查询收拢至 Rust`（依赖 `#31`）
  - [#33](https://github.com/hqwuzhaoyi/OpenfansFace/issues/33) `[终态架构 03/04] Python 收敛为常驻纯计算服务，Rust 接管缓存校验、证据发布与淘汰`（依赖 `#32`）
  - [#34](https://github.com/hqwuzhaoyi/OpenfansFace/issues/34) `[终态架构 04/04] 建立 Rust 归档与 Peach 外部扫描写入的可执行协调机制`（初始 Frontier，可与 `#31`–`#33` 并行）
