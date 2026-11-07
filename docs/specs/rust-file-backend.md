## Problem Statement

用户需要可靠地归档视频、合并人物、改名、移动文件、将文件移入暂存区并恢复，且文件位置、人物归属、人工确认记录与 Peach 账本始终可核对。当前文件业务分散在 TypeScript/Node.js 与调用 Peach 的 Python 逻辑中，一次操作可能涉及文件系统、本地 SQLite、JSON 元数据及 Peach SQLite。

最近已修复的生产故障暴露了必须带入迁移验收的约束：NFS 服务身份映射与目录 ACL 不一致；人物名大小写不同导致重复创建账本实体；无关后台任务的进度变化使人物预览反复过期；长操作缺少连续反馈。它们是权限、业务规则、并发和测试覆盖的问题，不能仅凭更换语言认定解决。

用户已明确希望把现有 Node.js 文件业务和 Python Peach 账本同步一起迁至 Rust。目标是统一执行与恢复模型，降低跨语言调用和分散写入规则带来的维护风险，同时保留当前已确认的业务行为。

## Solution

建立独立部署的 Rust + Tokio 文件操作后端，统一承接文件业务 API、预览校验、受控执行、进度查询、本地记录同步、Peach 账本适配与失败恢复。Next.js 负责界面及必要的薄转发，不再执行文件业务变更或通过 Python 更新账本。

确认后的操作先持久化，再进入执行队列。用户关闭页面、刷新、丢失响应或重试时，可以按同一操作编号查询结果；服务重启后能核对已完成步骤和未完成步骤。任务不能脱离可查询的日志执行，也不能把超时当作“没有执行”。

成功意味着该操作承诺的文件和关联记录均已确认提交。部分成功、已回退、结果待核对和需要人工恢复必须分别呈现。文件删除默认仍是移入全局暂存区，保留内容；不借迁移改变删除语义。

## User Stories

1. As an operator, I want file operations to use one Rust backend, so that the same business rules apply across archive and person management.
2. As an operator, I want a preview of affected files, destinations, counts and sizes, so that I can confirm the exact scope before execution.
3. As an operator, I want unreadable or unwritable paths identified before execution, so that I can resolve permission problems without partially moving files.
4. As an operator, I want archive to remain separate from confirming a person, so that accepting an identity does not immediately move media.
5. As an operator, I want previews to stay valid during unrelated background analysis, so that normal processing does not prevent me from confirming an operation.
6. As an operator, I want relevant file, identity and ledger changes to invalidate a preview, so that stale confirmation cannot affect changed data.
7. As an operator, I want case variants of an existing creator to reuse its ledger identity, so that archive does not fail with a duplicate entity constraint.
8. As an operator, I want ambiguous identities to stop the operation with an explanation, so that the backend does not guess which person to use.
9. As an operator, I want same-name files preserved according to the confirmed plan, so that merging people does not overwrite either copy.
10. As an operator, I want suspected duplicates compared by full content before staging, so that equal file size is not mistaken for identical content.
11. As an operator, I want original content retained when a duplicate is staged, so that the operation remains recoverable.
12. As an operator, I want file renaming to preserve required extensions and validate names, so that the resulting files remain usable and within managed directories.
13. As an operator, I want moving a file to update its known references, so that existing video links continue to resolve.
14. As an operator, I want whole-person merges to include nested files, attachments and empty directories under the existing rules, so that content is not silently omitted.
15. As an operator, I want to choose the retained person from my selection, so that merge direction is explicit.
16. As an operator, I want aliases, account links, templates, rejections and pending work reconciled during a merge, so that the person remains consistent across the workbench.
17. As an operator, I want manual account associations to remain distinct from inferred clues, so that migration does not turn suggestions into confirmed ownership.
18. As an operator, I want delete actions to move content to staging by default, so that a mistaken deletion does not destroy the original bytes.
19. As an operator, I want supported restore operations to preserve conflicts and update references, so that restoring does not overwrite newer files or leave stale paths.
20. As an operator, I want clear distinctions between completed and unfinished people in a batch, so that I can safely continue after partial failure.
21. As an operator, I want completed batch items excluded from repeated execution, so that retries cannot apply the same merge or deletion twice.
22. As an operator, I want immediate feedback after confirming an operation, so that I know the request was received.
23. As an operator, I want real stages, current files and measured counts, so that I can understand long-running work without a fabricated percentage.
24. As an operator, I want a persistent result and links to final locations, so that I can inspect the outcome after the operation completes.
25. As an operator, I want failures associated with the relevant files and paths, so that I can act on the cause rather than interpret a generic error.
26. As an operator, I want a lost response resolved through an operation receipt, so that I do not accidentally submit the same change again.
27. As an operator, I want a confirmed operation to survive a page refresh or navigation, so that its execution is not tied to an open browser tab.
28. As an operator, I want interrupted work reconciled after a service restart, so that I can distinguish completed steps from unfinished or uncertain ones.
29. As an operator, I want cancellation or timeout to report what actually happened, so that an in-flight filesystem call is not falsely reported as stopped.
30. As an operator, I want affected resources blocked when recovery is uncertain, so that a later operation cannot make recovery harder.
31. As an operator, I want search, selection and preview interactions to remain usable with keyboard and narrow screens, so that the backend migration does not degrade the workbench.
32. As an operator, I want reduced-motion settings and focus restoration preserved, so that confirmation and progress remain accessible.
33. As an operator, I want videos to remain playable through stable identifiers after operations, so that reorganizing media does not break playback.
34. As an administrator, I want bounded concurrency and consistent NAS identities, so that the service does not overload storage or rely on excessive privileges.
35. As an administrator, I want immutable operation intent and auditable step receipts, so that I can trace the actual state of an interrupted operation.
36. As an administrator, I want a controlled cutover with one authoritative writer, so that the old and new backends cannot process the same operation concurrently.
37. As an administrator, I want a verified rollback path for the service and data contract, so that deployment failure does not strand media or metadata.
38. As a maintainer, I want the existing HTTP acceptance scenarios reusable against Rust, so that the migration is judged by behavior rather than implementation language.
39. As a maintainer, I want failures injected at filesystem and transaction boundaries, so that restart, compensation and uncertain outcomes are tested before production use.
40. As an operator, I want the migration to preserve existing queued work and historical receipts, so that I do not have to reclassify or requeue my library.

## Implementation Decisions

以下区分本轮已确定方向与据此制定的实施约束；具体 Rust Web 框架和 SQLite 客户端库未选定，不将建议冒充已验证实现。

1. **技术与职责边界已确定。** 文件业务及其 Peach 账本同步统一使用 Rust，Tokio 用于 API、异步调度、通信和任务协作。Next.js 保留界面及必要转发。迁移完成后，该业务链不再由 Node.js 执行文件变更，也不再启动 Python 来修改 Peach 账本。
2. **能力范围。** 接管归档、改名、移动、人物合并、删除至暂存区、已验收的恢复流程，以及这些流程所需的目录查询、权限检查、文件指纹、路径解析、关联元数据和结果回执。相关媒体访问契约应保持稳定，包括现有播放器使用的路径解析及 Range 行为；实现可通过同源转发接入 Rust。
3. **保持当前人工确认边界。** 延续已接受的自动分析与状态分离 ADR：模型提供候选，归属由人工确认；确认进入待归档，归档再移动文件。操作执行状态与人脸特征、分析队列及人工决定分离，不通过全库重建或重新识别来完成迁移。
4. **一个执行权威。** Rust 统一执行操作引起的媒体、本地记录与账本变更。所有共享写入口须列入迁移清单，包括会改写同一 JSON 或人物记录的界面操作；不能留下一条旧入口绕过新锁。分析 worker 可继续维护其负责的独立状态，但必须与 Rust 的相关更新遵守事务及版本协议。
5. **模块划分。** 以操作计划、执行队列、文件操作、元数据适配、Peach 适配和恢复协调为职责边界；优先作为同一服务中的内部模块，避免为了语言迁移新增多个难以协调的网络事务。
6. **兼容公开契约。** 现有界面的预览、确认、进度和结果查询请求保持兼容，或提供明确的一次性适配。Rust 内部统一操作编号、请求指纹、预览版本、执行阶段和逐项结果。提交必须在意图持久化后才宣告已受理；受理不等于执行成功。
7. **持久化操作记录。** 在服务可可靠访问的本机 SQLite 中保存意图、预览范围、指纹、请求去重信息、逐步回执、失败原因与恢复状态。确认一经受理便可脱离 HTTP 连接执行。表结构与升级策略需可版本化；保留已有队列、稳定视频 ID 和历史回执的可查询性。
8. **明确状态与完成定义。** 区分已受理、检查、文件处理、账本同步、完成、部分完成、回退中、已回退及需要恢复。文件移动结束不得提前显示整次成功；最终成功还要求相关元数据提交得到确认。无需引入没有真实测量依据的总百分比。
9. **幂等和迟到请求。** 相同操作编号及相同请求返回同一操作或最终回执；同一编号不能改换范围。结果未知时查询持久化回执。未开始请求如被取消，须留下可拒绝迟到提交的记录，而不是仅凭“当前查不到”允许再次执行。
10. **正确的版本范围。** 文件内容／身份指纹、目标占用、相关人物记录、关联视频索引、会被改写的任务与账本必须校验；无关后台进度不参与失效判定。不能通过取消版本校验或忽略真实冲突来解决误报。
11. **统一身份规则。** Peach 适配遵循现有规范化名称及唯一约束，优先复用已有人物。多条候选或不同身份规则冲突时明确拒绝；不引入模糊归名，不把账号资料匹配视为视频身份确认。
12. **文件安全语义。** 不覆盖既有目标。保留两份的名称在预览和回执中明确；重复件须完整校验。遵守管理根目录、路径穿越及符号链接约束；身份检查不能只在预览时做一次。文件变化、源消失或目标晚到时停止对应变更并保留实际状态。
13. **权限是部署契约。** 容器 UID/GID 与 NFS 映射、目录访问及继承 ACL 一致。预览与执行前检查所需读写能力，错误给出实际路径与步骤。不得以放开整个库权限或关闭 NFS 保护替代正确配置；迁移不批量改动既有文件内容或属主。
14. **一致性与恢复。** 文件系统、本地数据库、JSON 和 Peach 不构成单个原子事务。每个不可逆边界前保存足够的意图，边界后保存可核对回执；重启时检查真实文件及事务结果，不盲目重放。仅在确认条件成立时补偿；状态未知、目标被外部更改或补偿失败时保留恢复状态与必要写锁。
15. **账本接入。** 将现有 Python 事务和回执语义移入 Rust 适配器，沿用 Peach 服务、数据和约束。实现前核实实际部署的数据库位置、schema 与并发写入者；适配器靠近数据库，通过可靠的本地数据库访问或已核实的事务接口操作。不得假设尚未确认存在的 Peach API，也不得把 SQLite 放到 NFS 来简化连接。
16. **本地记录同步。** 保留人物别名、人工关联账号、待归档归名、归档历史、模板、拒绝记录、稳定 ID、视频路径及相关分析候选的现有一致性规则。保留原始 OCR 和历史审计，不为改名重写证据。批量合并继续保留目标已有资料的既定优先级。
17. **并发与取消。** 首版以受控、保守的写入并发实现，冲突资源不得并行变更；读取与校验单独限流。Tokio 的 Future 取消或请求超时不能视作底层文件 I/O 已取消。执行队列应在安全边界停止领取或暂停步骤，并记录实际结果。
18. **批量语义。** 一个人物可作为其操作的恢复单位；跨人物批次不是一个原子事务。记录并展示已完成、当前、失败和未开始项目，按现有停止策略处理失败，不能重做已成功的前项。后续项在同一写入协议内重新核对受前项影响的冲突。
19. **交互兼容。** 确认后立即显示提交状态，固定操作区接续真实进度，最终结果保留位置入口和失败依据。复用现有组件、语义主题、短时 transform/opacity 反馈；键盘和 reduced-motion 即时更新，关闭恢复焦点。不得因后端迁移新增重复说明或要求用户理解内部实现。
20. **切换与回滚。** 先跑同一套隔离行为契约，再进行真实数据的只读计划对照。切换前停止接收新写入并核对在途任务，备份必要数据与版本，确保只有一个执行器持有写入权。发布验证代码、运行契约和静态资源，保留可执行的服务回滚步骤。存在未完成新格式操作时，必须先核对兼容性，不能直接启动旧写入器。
21. **生产验收边界。** 所有破坏性和故障注入测试使用带标记的隔离夹具。真实库只做读取、预览对照与状态核对；发布不自动执行用户待归档或待合并任务。

## Testing Decisions

**建议采用一个主要测试边界：工作台面对的 HTTP 文件操作契约。** 通过预览、确认、进度和回执查询驱动 Rust 后端，使用真实隔离文件、本地 SQLite、JSON 与独立 Peach 账本，检查外部结果。浏览器作为少量交互验收补充，不把每个内部模块都建立为独立 mock 边界。该测试边界已随 14 张工单拆分获得用户确认。

1. 好测试断言最终文件字节、源／目标存在性、目录结构、稳定 ID、账本关系、元数据、操作回执和可见反馈；不依赖内部函数调用次数、线程实现或 Rust 类型布局。
2. 复用现有归档、人物文件操作、整个人物合并、人物批量操作和预览版本回归的 API 场景。新增 Rust 适配应让同一行为夹具能验证迁移前后语义；现有 bug 不能作为应当保持的差分基线。
3. 档案与身份：大小写复用、唯一约束、身份歧义、缺失或重复资产记录、目标已有账本记录、重复关联去重；预检失败时没有意外移动。
4. 文件与目录：同名保留、晚到冲突、相同大小但内容不同、完整重复校验、嵌套目录、附件、空目录、长名、中文和特殊字符、无效路径、符号链接和根目录越界。
5. 权限：以部署服务实际 UID/GID 验证 NFS 映射；在隔离导出目录中重放创建目录、硬链接、复制和删除失败，验证 ACL 继承及错误定位。不以开发机 root 的成功替代线上挂载验证。
6. 并发版本：让无关分析任务持续改变阶段、重试次数和更新时间，操作仍可确认；同时分别更改相关索引、候选、人物资料、源文件或账本，旧确认必须被拒绝。
7. 幂等：快速双击、相同编号重复提交、不同范围复用编号、响应丢失、请求未到达、迟到提交、浏览器刷新和重复读取回执；每个真实变更只发生一次。
8. 事务边界：在文件处理前后、账本提交前后、本地记录提交前后和最终响应前注入故障，检查已确认提交与未提交的区别；结果未知时不得错误补偿或继续写入。
9. 重启恢复：强制终止服务后重启，验证步骤核对、任务认领、遗留写锁、过期执行者隔离以及不会重复提交已完成事务。中止等待必须与底层 I/O 实际完成状态分别验证。
10. 多人物批次：前项成功、后项失败，结果与真实范围一致；重复执行不重做成功项；重试未完成范围时仍保留同名文件和目标资料。
11. 删除与恢复：删除保留原始字节；已支持的恢复不会覆盖晚到文件；账本和稳定播放链接跟随实际位置。若基线含永久删除能力，其既有显式确认和不可恢复结果必须单独验证。
12. 交互：慢操作、快速失败、断线、部分结果和需要恢复；桌面与窄屏、键盘、焦点恢复、reduced-motion、真实隔离视频播放。成功回执前不展示最终成功，失败项保留可操作依据。
13. 部署：同源访问与 API 兼容、健康检查、单一写入权、数据备份／迁移、旧回执读取、切换失败回退和 worker 持续运行。生产检查不得写入正式媒体。
14. 可允许少量内部测试覆盖难以从 HTTP 稳定注入的纯规则或系统适配边界，但它们补充而不替代上述行为验收。首版不预设吞吐提升比例或虚构性能指标，以真实 NFS 测量决定进一步并发调优。

## Out of Scope

- 重写 Peach 应用本身、替换其媒体库或重新导入全库数据。
- 将人脸模型、OCR、视频分析算法及其 Python 生态整体迁至 Rust。
- 自动确认人物身份、根据资料匹配批量改归属、重新扫描既有视频或重新执行当前用户队列。
- 重做页面分工、视觉设计、组件库、账号资料功能或播放器产品体验；仅做迁移必需的契约与状态适配。
- 新增永久删除能力、自动清空暂存区，或放宽当前删除确认。若最新已验收基线已有这些能力，则按同一安全契约迁移。
- 承诺文件系统与所有数据库成为一个原子事务，或声称更换语言必然提升吞吐和消除业务错误。
- 将任务数据库迁到 NFS、取消目录权限保护、停用所有预览版本检查，或通过 Node/Rust 双执行来做生产差分测试。

## Further Notes

- 该规格的产物是可执行的迁移工作定义，不表示 Rust 服务已经实现或获得本轮以外的真实文件批处理授权。
- 遵守已接受的“自动启动分析，按需补充人脸证据”和“分离任务状态、人脸特征与人工归名”两项 ADR。
- 当前工作区另有并行的回收站实现，包含暂存恢复及其他操作的扩展。它不是本轮已经验证的生产基线；实施开始时必须清点最新已验收契约和全部写入入口，完成协调后再切换，不覆盖其他任务改动。
- 现有修复提供了迁移所需的行为证据：规范化身份复用、NFS ACL、账本回执、迟到请求防护、归档进度及相关记录版本范围。迁移应保留这些语义并增强进程级恢复，不能只翻译现有代码。
- Tokio 的普通文件操作通常仍调用底层阻塞 I/O；取消等待并不保证取消实际操作。设计取消、超时与恢复时应遵循官方说明：[Tokio filesystem cancellation](https://docs.rs/tokio/latest/tokio/fs/index.html#cancellation)、[spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)。
- 技术库、部署连接细节及 schema 演进需要实施时核实；在未核实前保留为工程选择，不假造已有接口、吞吐指标或已通过的生产结果。
