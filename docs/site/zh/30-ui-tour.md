# 面板导览

`maestro ui`（或 `maestro open`）在 `http://127.0.0.1:7777` 启动一个内嵌在二进制里的 axum 服务器，对外是一个 React 应用。顶部五个 tab，外加一个 Settings 齿轮和连接指示灯。

## 顶栏

| 区域 | 内容 |
|---|---|
| Logo + 名字 | 品牌标 |
| Tab 导航 | 对话 · 任务 · 上下文 · 架构 · 文档 |
| 中央 | 当前运行的 `spec`（产出此计划的对话需求），或 `(无运行中任务)` |
| 右侧 | 运行汇总徽章（运行中 / 已完成 / 失败）、连接指示灯、设置齿轮 |

连接点 SSE 在线时是**绿色**，重连中变**琥珀色**。

## 对话 tab

- 左侧栏：session 列表，可按标签过滤；**+ 新建** 按钮；底部有一个只读的"外部历史"组，列出本机 Cursor IDE / Claude Code / Codex 的 session
- 中央：消息流，markdown + 语法高亮
- 输入框下：模型选择器、附加项目上下文、发送/停止

侧边栏自动滚到最近活跃 session。点任意 session 打开它；URL hash 同步更新为 `#chat`。

## 任务 tab

- 左侧栏：历次 run，每条带状态徽章，点击切换
- 中央：上半部 DAG 可视化，下半部任务列表
- DAG 节点可拖拽、可滚动/捏合缩放、双击聚焦

跑中的任务节点会脉动；已完成 → pending 的边会亮成 emerald 表示"primed，下一步随时可触发"。

## 上下文 tab

可浏览和编辑：

- **L1 事实** 按主题分组 —— CodeMirror 编辑器，markdown 高亮
- **Skills** 按 scope 分组 —— 同一个编辑器

在浏览器里保存就是写盘。下次任务派发就能看到。

## 架构 tab

- 顶栏：模块数、契约边数、紧凑图例、**+ 添加项目**
- 主体：ReactFlow 图，dagre 排版
  - 产出方节点有绿色 ▲ "provides"
  - 消费方节点有蓝色 ▼ "consumes"
  - 边是带契约文件名的贝塞尔曲线，带流动动效

节点 hover 会出现 **垃圾桶** 图标，一键移除（只改 `projects.yaml`，不动磁盘）。

## 文档 tab

就是你现在看到的页面。侧边栏按分组列出所有文档；点任意一页都会改 URL hash，可以收藏。

## 设置弹窗

齿轮 → `设置 · projects.yaml 默认值`。可编辑：

- **default agent**（codex / cursor / shell / mock）
- **agent_model** —— 全量可搜索下拉，覆盖你账户里所有 100+ 模型
- **tagger_model** —— 同样的下拉，建议挑便宜的
- **branch_prefix**
- **max_parallel**
- 有 **刷新模型列表** 按钮，会尽量刷新 provider 支持的模型 catalog

Save 是原子写入 `.maestro/projects.yaml`。
