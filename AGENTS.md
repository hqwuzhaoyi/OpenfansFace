<!-- BEGIN:nextjs-agent-rules -->

## This is NOT the Next.js you know

This version has breaking changes — APIs, conventions, and file structure may all differ from your training data. Read the relevant guide in `node_modules/next/dist/docs/` (resolved from this file's directory; in monorepos the `next` package may not be visible from the repo root) before writing any code. Heed deprecation notices.

This block is written and re-added by `next dev` — verify at `node_modules/next/dist/server/lib/generate-agent-files.js`. Removing it from a diff only re-creates the uncommitted change; committing it with your work keeps the tree clean.

<!-- END:nextjs-agent-rules -->

## 项目经验

处理视频水印扫描、账号归名、OCR 校正或未检出复扫时，先读 `docs/experiences/README.md`，再按需读取 `docs/watermark-workflow.md` 和项目技能 `.agents/skills/video-watermark-review/SKILL.md`。

本项目运行证据保存在 `.local/experiences/`；该目录已被 Git 忽略。历史记录用于追溯，新的执行范围与队列状态应从实时数据核对。

已确认的页面分工：`/operations` 是独立处理操作台，`/review` 是统一人工确认入口；`/` 保留库存与全局任务总览，通过品牌入口返回。操作台使用 `components/processing/operations.tsx`，不要用首页总览组件替代。流程导航统一读取 `lib/workbench-navigation.ts`。

## 界面与组件约定

**Avoid unnecessary secondary labels and descriptions. Let the layout do the explaining.**

- 用布局、图标和数量表达结构与状态，省去重复标题、二级标签和教学式说明。保留真实数据、待核对依据、可操作的错误信息及必要的演示标记；图标按钮保留完整的无障碍名称和悬停提示。
- 组件优先使用 BEUI 和 ReUI：先检查项目已有组件，再查两家的官方组件目录；仓库未接入不等于组件库没有。两家均无合适组件时，继续查找其他成熟组件库或现成方案。只有检索、复用与组合仍无法满足需求时，才自行实现，并简要说明现成方案的缺口；避免直接新建组件。
- 接入组件先核对官方源码和实际 API，保留来源与许可证；适配优先采用现有组件组合或薄封装，不重写已有交互。遵循 Emil 的克制交互原则，不能只改样式就声称已接入组件库。人物搜索已选定 ReUI Input Group：搜索、按需出现的清空按钮、排序入口共用一个边框。
- 主题与语义色统一使用 `app/globals.css` 和 `components/theme-provider.tsx`；页面适配层只处理局部布局与外观，不另建浅深色配色。导航与页面外壳复用现有共享结构。
- 动效用于操作反馈和状态变化：动态状态与退出动画使用现有 Motion，简单反馈使用 CSS / WAAPI。沿用 `EASE_OUT` / `--ease-out-ui`，通常 160–180ms，低于 300ms，使用 transform / opacity。搜索、排序、列表查看保持即时响应；键盘与 reduced-motion 模式即时更新。
- 弹窗入场仅在打开时触发，切换鼠标与键盘不能重播。关闭后恢复触发点焦点；退出动画中的内容立即移出交互与焦点范围。

## 人物工作台业务约定

修改人物列表、账号关联、文件操作或合并流程前，先读 `docs/persons-workbench.md`；接口与一致性细节以该文档及当前实现为准。

- 保持左侧人物列表、中间实际目录视频、右侧推荐的结构。目录、视频数量、体积、推荐依据和完整路径来自真实接口；读取失败显示未知或错误，不当作零条，也不补造业务文件夹。
- 勾选与查看独立；搜索、排序、切换人物保持勾选。全选只作用于当前搜索结果。批量删除与合并放在人物列表顶部；合并至少选两人，保留人物必须从已选人物中指定。
- 合并确认前展示各人物的真实视频预览，支持直接播放和查看路径。同名文件保留两份。进度与合入动画跟随实际回执，成功后保留结果入口；部分失败明确区分已完成与未完成，不能把批量操作描述成跨人物的单一事务。
- 删除移入全局暂存区并保留文件内容。重命名、移动、删除和合并沿用预览、版本校验、操作回执与失败恢复流程，保持文件、本地记录和 Peach 账本一致。
- 列表账号状态只统计明确保存的手动关联记录：绿色链条加数量、灰色断链表示未关联，读取失败单独表示未知。名称匹配和文字线索不计入关联数量；关联或解除后立即刷新列表。账号关联不等于视频归属确认。
- 有账号资料时显示默认折叠的“账号资料 + 数量”入口，以 BEUI 卡片呈现；无结果不占位。保留待核对状态和依据，管理关联不切换当前查看人物。

## 验证与预览经验

- UI 改动验证桌面与窄屏、键盘、焦点恢复和 reduced motion；涉及视频时实际验证播放，涉及批量操作时验证同名保留、失败恢复和状态刷新。文案与低影响样式修改不添加镜像实现的测试。
- 写入测试使用标记过的隔离夹具，正式媒体库不作为测试对象。交付时区分代码已实现、测试预览可用与正式部署；部署范围按用户本次指令执行。
- 隔离预览同步页面依赖的共享组件、主题、全局样式与外壳，避免只复制页面而漏掉依赖。构建与正在服务的输出隔离，完成后再切换预览进程；验证页面和静态资源实际可访问后再交付链接。
