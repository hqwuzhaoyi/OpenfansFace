# OpenfansFace · 视频人物识别与归档

面向本地视频库的人物核对与批量归档工作台。使用 Next.js、React、TypeScript、SQLite 和可选的 Python 校准服务。

## 功能

- **全部待分类**：按实际文件路径去重，浏览此前未进入识别列表的视频。
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

## 定期体检

`maintenance/video_health.py` 在宿主机运行，需要 Python 3.11+、`ffprobe` 和 `findmnt`。它只读取媒体，结果保存到 `FACES_BASE/video-health.json`。连续三次、跨足够时间的异常确认才会触发列表收起；网络超时与兼容性问题不会直接触发收起。

`maintenance/face-video-health.service` 和 `.timer` 是部署模板。安装前创建自己的服务账号，修改脚本路径、数据路径和媒体挂载路径，并在 `/etc/face-admin/health.env` 中设置 `MEDIA_NFS_SOURCE`。定时器默认每天北京时间 03:30 运行，每轮最多检查 300 个视频结构，优先检查播放失败和待分类的视频。通过挂载来源核对和只读媒体权限避免对错误挂载执行整理。

## 验证

```sh
npx tsc --noEmit
node --test tests/*.test.mjs
python3 -m unittest discover -s tests -p test_video_health.py
pnpm build
```

`tests/http-*.py` 只允许连接隔离测试端口 8922。测试夹具会改写 `/tmp/face-admin-v10-fixture`，请按脚本要求准备独立媒体与账本；不要让这些测试连接生产数据。

## 目录

- `app/`：工作台页面和 API
- `components/review/`：视频预览、人物选择、批量核对和体检界面
- `lib/`：当前路径解析、归档计划、队列、账本和体检规则
- `sidecar/`：可选人物校准与推荐服务
- `maintenance/`：只读定时体检与 systemd 模板
- `tests/`：核心规则、文件流和隔离 HTTP 回归测试
