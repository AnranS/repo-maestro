# DAG 可视化

两个视图，都基于 `@xyflow/react` + `dagre`：

| 在哪里 | 是什么 | 状态语义 |
|---|---|---|
| **任务 tab** | 单次 run 的任务 DAG | 颜色由任务状态驱动 |
| **架构 tab** | 项目契约图 | 颜色由节点类型驱动 |

## 任务 DAG

节点：

- ✓ **done** —— emerald 背景，无动画
- ◐ **running** —— 蓝色背景，呼吸 ring，边带流动动效
- ⏸ **awaiting_approval** —— 琥珀色，发光 + 动画边
- ✗ **failed** —— 红色，静止
- ○ **pending** —— 灰色（上游 done 时边变 emerald-tinted）
- ⊖ **skipped** / **cancelled** —— 暗灰

边：

- 贝塞尔曲线（比 smoothstep 的直角折线更柔和）
- 当源是 `done`、或目标是 `running`/`awaiting_approval`/`done` 时，动画虚线
- 颜色 = 目标状态色（让"接下来要发生什么"一眼可见）
- `running` 状态下线条略粗（2px vs 1.6px）

视觉化进度提示：

- `pending` 且上游 `done` → emerald-tinted **流动**边（primed）
- `running` → 蓝色流动边 + 节点 ring 脉冲
- `done → done` → emerald 流动边（已走过的路径仍然"活着"）
- `failed → *` → 红色边，下游不再流动

每次 run 完成后，还会在 run 目录写入一组 evidence：

- `evidence/summary.json` 记录每个任务实际使用的 workspace/worktree、开始/结束时间、观测到的最大并发数、并发重叠窗口和验收结果。
- `evidence/browser/` 会在检测到浏览器相关验收命令或 Playwright/Cypress 产物时生成，包含 check 摘要、截图/trace/report/video 的拷贝，以及 console/network 失败片段。
- `PR_BODY.md` 基于报告、验收结果、并发证据和 changed files 自动生成 PR 草稿。

需要证明多项目任务确实并行、同仓任务确实隔离时，可以跑 `maestro runs evidence [run-id]`、`maestro runs replay [run-id]`、`maestro runs pr-body [run-id]`，或者直接看 Tasks 页里的 Evidence 面板。

## 架构图

节点从 `projects.yaml` 生成，每个节点显示：

- 类型图标（backend / frontend / mobile / tool / library）
- 项目名 + 类型标签
- 前 3 个 stack 标签做成等宽 chip（更多时显示 +N）
- ▲ provides —— emerald
- ▼ consumes —— 蓝色
- hover 时显示垃圾桶图标

边从契约关系生成。每条边：

- 源 = 产出方，目标 = 消费方
- 标签 = 契约文件路径，带不透明背景 pill 防止被节点遮挡
- 带动画的贝塞尔曲线表示契约方向

布局是 LR（从左到右）+ dagre。我们在 dagre 之后做了一次 Y 对齐 pass：单一上游的节点 Y 被 snap 到上游的 Y，简单链状结构就变成完美的水平 pipeline。

## 交互

- **拖拽**平移，**滚轮**平移，**捏合**缩放
- 右下角控制面板有手动缩放 + fit
- 双击什么都不做（故意禁用，避免阅读时误触缩放）
- 架构节点**可拖动**；任务节点不可（自动排版，拖动没有意义）

## 为什么选 ReactFlow

我们对比过 Mermaid（便宜但静态）、d3-dag（强但重）、手写 SVG。ReactFlow 的胜出点：

- 节点是真实 DOM —— 每个节点可以放任意可交互内容（状态图标、菜单……）
- 自带 pan/zoom/键盘处理
- 原生支持动画边

代价是体积——`@xyflow/react` + dagre 约 150KB gzip。两个 DAG 视图都是懒加载的，对话 tab 仍然轻。
