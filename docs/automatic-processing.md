# 自动分析运行说明

设计约定见 [设计访谈](design/processing-workbench-interview.md)、[ADR 0001](adr/0001-automatic-analysis-with-conditional-face-matching.md) 和 [ADR 0002](adr/0002-separate-processing-state.md)。

## 页面与数据流

处理操作台 `/operations` 管理自动任务；人工确认 `/review` 处理人物归属。首页 `/` 保留库存与全局任务摘要，品牌入口可以返回；五个流程入口来自 `lib/workbench-navigation.ts`。旧待分类、审批及相似组链接兼容到人工确认，并尽量保留视频或组筛选参数。

新视频以现有媒体库账本为来源，不遍历整块 NAS。`/api/processing/inventory` 在 Peach 内用 SQLite backup API 生成一致内存视图，包含已提交的 WAL 数据，再返回视频资产。处置为 trash / vanished 的条目、creators、回收站、已归名队列和已归档记录都按当前范围排除；不沿用旧扫描器的 60 秒限制。

发现线程每 5 分钟检查一次，登记与人脸分析分开。任务按人工优先级、新增视频、历史存量排序。全局暂停停止领取新的模型阶段，已开始的阶段会完成，轻量发现仍可更新队列。批次按发现轮次记录，各视频独立进入人工确认，不等待整批结束。

领取任务前先检查媒体根目录是否可读。整盘挂载异常时保留任务，记录一次挂载异常事件；恢复后自动继续。若模型阶段中途遇到同样的整盘故障，将本次尝试退回，避免全库任务逐个耗尽重试次数。挂载正常时的单文件读取或模型错误仍按首次执行加两次重试处理。该检查针对根目录不可读，不把个别文件错误当作整盘故障。

文字阶段复用有效 OCR 缓存，并匹配文件名、文件夹线索。只有命中已确认的账号映射且无已知冲突时才跳过人脸；来源水印不作为人物建议。人脸阶段使用 detection / recognition 模块提取特征，参照已有明确代表脸和已确认视频的相干参考形成候选；未匹配已知人物的相似人脸可形成未命名组，仍须逐视频人工核对。

## 部署

Web 继续使用项目 Dockerfile。协调器依赖已有的水印基础镜像，以及 InsightFace / ONNX Runtime；版本记录在 `maintenance/*-requirements.txt`。构建命令：

```sh
docker build -f maintenance/Dockerfile.watermarks -t face-watermarks:v1 .
docker build -f maintenance/Dockerfile.processing -t face-processing:local .
```

启动协调器时，挂载与 Web 相同的 `FACES_BASE`、同路径的只读媒体目录、可读取的 `buffalo_l` 模型目录及 PaddleOCR 模型缓存。实际部署将 CPU 限制为 1 核、内存上限设为 1.5 GiB；正式使用前以本机模型实测资源为准。

```sh
docker run -d --name face-processing --restart unless-stopped \
  --network host --cpus 1 --memory 1536m --pids-limit 256 \
  -e FACES_BASE=/data/base -e MEDIA2_ROOT=/mnt/media2 \
  -e FACE_ADMIN_URL=http://127.0.0.1:8919 \
  -e WATERMARK_CPU_THREADS=1 -e OMP_NUM_THREADS=1 -e OPENBLAS_NUM_THREADS=1 \
  -v /YOUR/face-data:/data/base \
  -v /YOUR/media:/mnt/media2:ro \
  -v /YOUR/insightface-cache:/root/.insightface:ro \
  -v face-watermark-models:/root/.paddlex \
  face-processing:local
```

Web 地址必须从协调器容器内可达。启动自动协调器前停止独立 `face-watermarks` worker，让水印结果只有一个写入者。OS 文件锁阻止两个协调器同时领取任务。上线预览可先把 `settings.paused` 设为 true，验证发现、模型和接口，再通过操作台恢复自动分析。

## 运行资料

| 位置（相对 FACES_BASE） | 用途 |
| --- | --- |
| `processing/processing.db` | 任务阶段、重试、优先级、暂缓、备注与事件 |
| `processing/heartbeat.json` | 协调器心跳及当前文字管线版本 |
| `processing/references.npz` | 版本化人物参考 |
| `processing/faces/` | 按视频保存、绑定路径/大小/修改时间/模型版本的特征 |
| `processing/thumbs/` | 每帧每张脸的独立截图 |
| `watermarks/results.json`、`watermarks/evidence/` | 兼容原水印证据缓存 |
| `jobs/` | 逐阶段实际进度，供任务摘要使用 |

旧人脸特征不冒充已通过新指纹校验的缓存。既有人物校准接口仍针对旧库，新流水线结果由人工确认页呈现。候选相似度不是人物正确率；文件变化、阶段异常或媒体可读数量骤降时，系统保留可追溯状态而不伪装分析成功。

## 人工处理

默认按候选人物分组，冲突和无候选独立筛选。每个文件只出现一次；同片多个候选不会变成多个可重复归档的文件。组内默认不勾选，全选只作用于当前页。无候选可以手选或新建人物，技术失败可从操作台接管；其他出镜人物通过备注记录。

归名提交会再次核对文件指纹，只改本次选中项。旧队列条目不会因新增归名而补写来源字段。可选保存共同作者账号的映射；原始截断读数不能无条件推广成全库规则。归名后进入待归档，备注随记录保留；撤销会恢复可用的人工确认状态。

## 本次验收

已验证状态机与接口回归，包括暂停、两次重试上限、文件变化、候选去重、备注与暂缓、队列保留和撤销。真实容器跑通了 OCR → 人脸采样 → 就绪；真实待处理视频试跑检出 3 张脸，因两个候选进入冲突核对，没有自动归名。

首次发现登记了 962 个此前未登记的视频，形成 4,125 个可处理任务；这是当时的快照，后续以实时页面为准。最终容器检查覆盖原生 SQLite、媒体 Range 读取、归名与撤销；页面检查还拦截了操作台被错误替换成总览的情况，回退后保留首页并恢复独立操作台。

部署回执、测试记录、构建快照和回滚信息保存在项目 `.local/experiences/processing-release/`。恢复旧协调器容器时，应同时核对其 Web 服务地址；预览地址不是正式服务地址。回滚应用版本不应把旧队列备份覆盖到用户后续操作上。
