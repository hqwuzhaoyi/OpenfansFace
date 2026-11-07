# 原生视频体检

Rust 文件服务内的单独 `video-health-lane` 负责每日北京时间 03:30 的只读检查；
现有 `/staging#video-health` 核对页继续读取结果，运行中每 15 秒刷新。
刷新不触发扫描。人物、OCR、人脸和自动归档不属于体检职责。

## 配置与切换

- 文件服务设置 `VIDEO_HEALTH_ENABLED=1` 启用调度；默认关闭，供统一切换时显式开启。
- `VIDEO_HEALTH_BUDGET` 默认 300，最多 1000 个结构探测；`VIDEO_HEALTH_SECONDS`
  默认和上限 1200 秒。路径观察有单独 10 秒上限；读取页面观察为 5 秒。
- `MEDIA_NFS_SOURCE` 必须等于 `findmnt -n -T MEDIA2_ROOT -o FSTYPE,SOURCE`
  返回的 NFS 来源；类型必须为 nfs/nfs4。两个预期媒体目录都必须可读取且非空。
- 运行镜像安装 ffprobe（ffmpeg 包）和 findmnt（util-linux 包）。每次 ffprobe
  最多 20 秒、单解码线程、5 MB 探测量／5 秒分析量、单次分配上限 64 MiB。
  Linux 子进程降低 nice 优先级。所有媒体访问只读，不修复、转码或删除。
- 切换时停用旧 `face-video-health.timer/service`。Python 入口和 `run()` 检测到
  原生状态 DB 或 Rust 配置会拒绝发布；Next 播放报告按 Rust 开关转发，发现原生
  状态 DB 后也拒绝退回旧 JSON 写入口。没有自动部署或生产修改。

`FILE_OPERATIONS_FIXTURE_ROOT` 通过公共 Config 的完整 /tmp 标记校验后，才允许
`VIDEO_HEALTH_FIXTURE_NOW`（Unix 秒）、`VIDEO_HEALTH_FIXTURE_RUN_ON_START=1`
和 `VIDEO_HEALTH_FIXTURE_PROBE`（受控探测可执行文件）。这些参数不用于生产。

## 状态与发布

`GET /v1/video-health` 返回兼容页面的摘要和异常 `rows`；不存在报告为 `never_run`，
损坏／不可读为 HTTP 503，成功为 `complete`，失败为 `skipped`，中断重启为
`interrupted`。后三者保留最近一次完整记录，失败不伪造成功时间。`running` 保留
此前记录直至整份报告验证通过。运行编号、开始／完成、最近成功、下次计划、检查／
等待数量都持久化。停机错过的已有计划会在启动后补跑一次，不重复当天完成的计划。

探测与文件观察在写锁外执行；文件观察使用本机 Rust 可执行文件的受限辅助模式，
不是 Python 扫描器。发布经 #13 `metadata-write` 同一 Engine、authority、WriteGate、
文档 CAS 和持久回执。入场再次核对当前目录挂载、稳定 ID 的权威路径、完整指纹
（路径／dev／inode／size／mtime_ns）和相关处置／恢复材料。旧结果不能覆盖移动、
恢复、替换或永久删除后的状态。公共 HTTP 拒绝构造内部 health-start/finish 命令。

检查期间的并发操作按当前记录重新合并。报告只接受 `GET /v1/video-health?vid=…`
取得的 `path`、`fingerprint`、`material`；可带 `path` 拒绝旧预览位置。
`POST /v1/video-health` 接收这些字段、`vid`、`code` 和可重放 `operation_id`。
报告计数在统一写队列合并，相同操作 ID 重放不重复计数；同一秒的新报告也会优先复查。
结构正常但近期报告播放失败保留 `playback_warning`，不隐藏文件。保留最新 1000 个报告。

## 保护规则

- 只有 missing / not_file / empty / corrupt 累计确定失败。同一指纹、路径、处置材料和
  状态至少 3 次，计数间隔至少 20 小时、首尾至少 40 小时，才标记 hidden。
- 暂时 I/O、权限、网络、超时、探测器输出问题、兼容性和播放警告都不累计损坏。
  ffprobe 同时输出结构错误与 I/O 错误时，以不确定状态为准。
- 探测前、探测后、发布时均检查确定异常是否大于 max(50, 总记录×20%)；超限
  不发布部分成功。挂载异常同样保留上次结果。
- 超过 7 天无成功检查，读取时暂停旧隐藏。文件变化立即取消旧指纹的隐藏。
- 当前路径在回收站的记录不进入正式视频队列。#16 对暂存副本的删除不改变正式
  副本材料；恢复到原路径也通过 `staging-origins[].restored_by/restored_path`
  区分处置代次，不把仅路径相同视为同一次检查。

子进程输出写入本地有上限的临时文件，避免管道阻塞。超时杀死进程组并轮询回收；
500 ms 后仍未退出则由专门回收线程保留子进程所有权，阻止新的探测并明确记录
底层 I/O 未结束，不把等待取消冒充已终止。Linux 使用 PDEATHSIG，服务被 SIGKILL
后不遗留探测器；macOS 没有该机制，隔离强杀测试需显式回收测试进程组。正常关闭
会请求停止、回收子进程并保留失败／中断依据。不要以 PID 超时清除公共写锁。

## 验证

`cargo test --manifest-path services/file-operations/Cargo.toml` 覆盖可控时钟阈值、
结构／临时错误分类、激增边界、代次／路径发布竞争、报告合并、过期、重启和进程组回收。
`python3 tests/http-rust-video-health.py --port 19007` 使用标记隔离夹具与真实 Rust HTTP、
实际 ffprobe 和受控子进程；需要 `/tmp/persons-flower.mp4` 作为可播放样片。
它验证实际三次阈值、真实 inode 替换、并发／重放报告、优先级、超时 PID 回收、探测
期间改名不堵塞文件操作、权限和挂载失败，以及真实 SIGKILL／服务重启。

生产 NFS 的实际 UID、挂载及资源限额验收由最终统一切换执行；本票本地验证不等于生产部署。
