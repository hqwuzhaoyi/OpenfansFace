# OpenfansFace · 视频人物识别与归档

面向本地视频库的人物核对与批量归档工作台。使用 Next.js、React、TypeScript、SQLite 和可选的 Python 校准服务。

## 功能

- **关联账号资料**：人物详情和“看对照”中可查询公开 X 账号目录，展示主页、简介及匹配依据；同昵称或未知平台保留待核对状态。详见 [关联规则](docs/external-profiles.md)。
- **全部待分类**：按实际文件路径去重，浏览此前未进入识别列表的视频。
- **提取水印**：后台抽帧识别文字，输出账号分组列表；筛选有水印、未检出、失败或来源记录，核对截图后继续归名。
- **相似视频组与人物核对**：按文件夹展开、预览视频、显示路径和文件名，批量选择人物。
- **识别待确认**：审核推荐人物，支持人工改名和新建人物。
- **分波归档**：归名后进入待归档，支持撤销归名；执行归档时移动文件并同步 Peach 账本。
- **同名文件处理**：改名保留两份，或完整校验内容后把重复源文件移入暂存。
- **定期视频体检**：检查路径和视频结构，多次确认异常后收起记录，保留文件；播放失败优先复查。
- **手机交互**：文件夹 Accordion、全屏人物选择、固定操作按钮及键盘视口适配。

## 运行条件

这是已有媒体与人脸识别管线的管理界面，仓库不包含私人视频、缩略图、数据库、人脸特征或训练数据。

需要 Node.js 22.18+、pnpm 10.10.0，以及数据目录中的 `faces.db`。界面还读取管线生成的 `report.json`、`clusters.json`、`claims-detail.json`；人工操作状态由应用保存在同一目录中。数据结构可参考 `tests/seed-fixture.py`，该脚本仅用于独立测试环境。

实际归档依赖已运行的 Peach 容器及其 `/data/database/ledger.db` 账本。校准功能依赖可选的 Python sidecar。此项目没有内置登录鉴权，应部署在受控网络或带身份验证的反向代理后；挂载 Docker socket 会赋予应用控制 Docker 的权限。

## 本地启动

```sh
corepack enable
pnpm install --frozen-lockfile
cp .env.example .env.local
# 修改 .env.local，指向自己的数据和媒体目录
pnpm dev
```

浏览器访问 `http://localhost:3000/unclassified`。当前默认媒体目录结构为 `MEDIA2_ROOT/pikpak/creators/<人物名>/`，Windows 账本路径映射使用 `R:\media\pikpak\...`；其他布局需调整 `lib/paths.ts` 和 `lib/source-resolution.ts`。

| 配置 | 用途 |
| --- | --- |
| `FACES_BASE` | SQLite、识别报告、人工归名与归档记录目录 |
| `MEDIA2_ROOT` | 实际媒体挂载根目录 |
| `DOCKER_SOCK` | Docker socket 路径，供 Peach 账本操作使用 |
| `PEACH_CONTAINER` | Peach 容器名称，默认 `peach` |
| `CALIBRATOR_URL` | 可选校准服务地址 |
| `MEDIA_NFS_SOURCE` | 定时体检预期的 NFS 来源；必须与 `findmnt` 输出一致 |

## Docker

```sh
docker build -t face-admin-next .
```

运行镜像时挂载自己的数据目录、媒体目录及需要的 Docker socket，配置上述环境变量，并将容器端口 3000 映射到所需端口。旧记录中的绝对缩略图路径需要在容器内同样可访问。

可选校准镜像使用 `docker build -t face-calibrator sidecar/` 构建。Python 侧依赖 NumPy；通过 `FACES_BASE` 指定管线目录，并在该目录提供 `identify_faces.py`、`tplgroups.py` 和 `sidecar/sidecar-suggest.py` 等辅助脚本及识别数据。不同部署须核对容器内路径。

## 自动分析与人工确认

当前主流程为：**自动发现 → 文字线索 → 按需人脸分析 → 人工确认 → 待归档**。

- `/operations`：独立处理操作台，显示阶段状态、批次、异常和运行记录，支持全局暂停、重试、优先处理及异常接管。
- `/review`：统一人工确认，按候选人物组织视频，支持冲突、无候选、暂缓筛选；桌面三栏核对，手机全屏预览。默认不勾选，批量只作用于当前页。
- `/` 保留首页总览；旧待分类、识别待确认和相似组地址跳转到人工确认。人物库、待归档、回收站继续沿用。

协调器每 5 分钟从 Peach 的一致账本快照发现视频，补登记但不覆盖已有视频记录。新视频优先，存量后台处理；每次只有一个视频进入模型阶段。确认账号映射且没有冲突时可跳过人脸，仅目录或文件名命中人物仍补做人脸。技术失败自动重试两次后进入异常，正常未检出进入人工核对。

部署与数据边界见 [自动分析运行说明](docs/automatic-processing.md)。人工归名和实际文件移动保持分离，其他出镜人物通过备注保留。

## 水印识别

抽样扫描、账号校正、批量归名或复查未检出结果时，从 [项目经验入口](docs/experiences/README.md) 查找操作流程、项目技能及真实运行资料。[水印识别与归名操作记录](docs/watermark-workflow.md) 记录了首轮样本结果、证据要求、归名验收和复扫方法。

水印提取已作为自动分析的文字阶段运行，原 `/watermarks` 和 `/unclassified/watermarks` 地址兼容到处理操作台。结果和证据在人工确认中查看。下面保留独立水印 worker 的离线实验方式；生产环境使用协调器时，不同时启动此独立 worker。

人工确认默认按人物候选组织证据；来源水印可以排除出人物建议。没有人脸记录的视频也能进入文字阶段，未检出和异常分别处理。

文字识别使用独立 Python 进程，不阻塞 Next.js 请求。运行条件是 Python 3.11、FFmpeg，以及 `maintenance/watermark-requirements.txt` 中固定版本的 PaddleOCR / PaddlePaddle。首次识别会下载 PP-OCRv5 mobile 模型；视频帧在本机处理。

```sh
python3.11 -m venv /tmp/face-watermark-env
/tmp/face-watermark-env/bin/pip install -r maintenance/watermark-requirements.txt
# 替换成应用使用的数据目录和媒体路径；容器内外媒体绝对路径须一致。
FACES_BASE=/data/base MEDIA2_ROOT=/mnt/media2 \
  /tmp/face-watermark-env/bin/python maintenance/watermark_worker.py
```

长期运行可使用独立 Docker 镜像。以下挂载中的宿主机路径需替换为实际路径；与 Web 应用共享同一个数据目录，并给媒体目录只读权限：

```sh
docker build -f maintenance/Dockerfile.watermarks -t face-watermarks .
docker run -d --name face-watermarks --restart unless-stopped \
  -e FACES_BASE=/data/base -e MEDIA2_ROOT=/mnt/media2 \
  -e WATERMARK_CPU_THREADS=2 \
  -v /YOUR/face-data:/data/base \
  -v /YOUR/media:/mnt/media2:ro \
  -v face-watermark-models:/root/.paddlex \
  face-watermarks
```

默认每段视频抽取 8 个时间点，识别全画面及放大的角落区域。同一文字至少出现在两个不同时间点才进入分组；同一帧的多个裁剪不重复计票。截图旁的分数仅表示文字识别分数，不代表人物归属概率。保留账号标点，按平台区分账号，不用模糊相似度自动合并。

核对后可把水印映射到人物，也可标为“转载 / 来源”。映射只生成建议，不会自动归名或移动媒体。映射到多个人物，或与现有人脸归属建议不一致时，页面显示冲突；批量归名仍需人工选择并确认。归名和撤销复用现有待归档接口；撤销归名保留水印规则，规则可单独取消。

状态保存于 `FACES_BASE/watermarks/`：`job.json` 为最新任务，`progress.json` 为进度，`worker.json` 为后台心跳，`results.json` 为识别记录，`mappings.json` 为人工映射，`evidence/` 为证据截图。结果绑定路径、文件大小、纳秒修改时间和识别管线版本；文件改变后旧结果失效。重启后台会续跑未完成任务。模型初始化失败会显示整批错误，不会把全部视频误记成“未找到水印”；单个视频失败不阻断其余视频。离线超过 90 秒时页面停止接受新任务。

当前版本只处理可见文字水印；纯图形标志、严重模糊或只短暂出现的水印可能漏检，字幕也可能进入候选组。实际库的准确率需用代表性视频核验后再评估自动化策略。

## 定期体检

`maintenance/video_health.py` 在宿主机运行，需要 Python 3.11+、`ffprobe` 和 `findmnt`。它只读取媒体，结果保存到 `FACES_BASE/video-health.json`。连续三次、跨足够时间的异常确认才会触发列表收起；网络超时与兼容性问题不会直接触发收起。

`maintenance/face-video-health.service` 和 `.timer` 是部署模板。安装前创建自己的服务账号，修改脚本路径、数据路径和媒体挂载路径，并在 `/etc/face-admin/health.env` 中设置 `MEDIA_NFS_SOURCE`。定时器默认每天北京时间 03:30 运行，每轮最多检查 300 个视频结构，优先检查播放失败和待分类的视频。通过挂载来源核对和只读媒体权限避免对错误挂载执行整理。

## 验证

```sh
npx tsc --noEmit
node --test tests/*.test.mjs
python3 -m unittest discover -s tests -p 'test_*.py'
pnpm build
```

`tests/http-*.py` 只允许连接隔离测试端口 8922。测试夹具会改写 `/tmp/face-admin-v10-fixture`，请按脚本要求准备独立媒体与账本；不要让这些测试连接生产数据。

`tests/http-watermarks.py` 在夹具中验证后台离线、任务互斥、无脸视频、人物和来源规则、人脸冲突、证据读取、归名撤销及文件替换失效。测试会写入模拟水印记录；运行时不要同时启动真实水印 worker。

## 目录

- `app/`：工作台页面和 API
- `components/review/`：视频预览、人物选择、批量核对和体检界面
- `lib/`：当前路径解析、归档计划、队列、账本和体检规则
- `sidecar/`：可选人物校准与推荐服务
- `maintenance/`：只读定时体检与 systemd 模板
- `tests/`：核心规则、文件流和隔离 HTTP 回归测试
