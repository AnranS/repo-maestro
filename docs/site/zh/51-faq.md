# FAQ

### 跟 Codex 或 Cursor 本身有什么不同？

Codex / Cursor 是引擎，`maestro` 是围绕它们的脚手架：项目注册表、DAG 调度器、共享记忆层、Skills 镜像、面板、action 协议。每一次真正的 LLM 调用最终都会委托给当前选中的 provider / adapter。

### 能改用 Claude Code 或 Codex 吗？

Codex 已支持任务执行（`agent: codex` 或 `defaults.agent: codex`），也支持 chat（`maestro chat send --provider codex` 或 session provider pin）。Claude Code 目前支持 chat（`provider: claude`，简单非 resumable 流），但还没有任务执行 adapter。Cursor 仍是默认任务和 chat provider。

### `maestro` 会把我的代码上传到哪里吗？

`maestro` 自己完全本地：状态、记忆、计划、运行 —— 都在工作区磁盘上。LLM 调用走的是你 AI 工具自己的后端（Cursor、Codex 配置的 provider，或 Claude Code 配置的后端）。自动打标签明确只看**用户侧**消息，不会把 assistant 输出送过去。

### 为什么注册表不放到每个项目里？

因为注册表描述的是**项目之间的关系**。单一仓库里看不到全貌。工作区级别的文件天然能编码"这 N 个仓库属于同一份讨论"。

### 为什么用 YAML 而不是真正的 DSL？

- 人读得懂、模型也读得懂
- 跟 diff 工具配合良好
- 我们需要的结构它都有（列表、可选字段、多行字符串）
- 少一个编译器要维护

### Agent 能不经过我同意就跑计划吗？

不能，故意的。计划必须经由 `maestro-action run` 按钮确认。计划内部，单个任务确实可以自治执行，但**计划本身**永远是有人工 gate 的。

如果你想完全无人值守，可以从 shell 自己 `maestro run plans/foo.yaml`，面板只是入口之一。

### 我的会话不见了！

会话在 `.maestro/chat/sessions/`。如果文件还在，多半只是被标签过滤了——把侧栏的过滤清掉就行。文件不在了……你大概是 `rm -rf` 了某个老工作区。

### 为什么有些任务被标 `cancelled` 而不是 `failed`？

三种终态：

- **done** —— exit code 0
- **failed** —— exit code 非零，或超时
- **cancelled** —— 这个任务在执行或排队时被 `maestro cancel-run` 中断

`cancelled` 单独存在是为了让报告能区分对待，并让 `maestro rerun --from` 知道这种任务是可以重试的。

### 能把 `.maestro/` commit 到项目仓库吗？

视情况而定：

- `projects.yaml`、`skills/`、`memory/l1_facts/` —— **可以**，commit 给团队当 playbook
- `memory/l2_decisions/`、`runs/`、`chat/sessions/` —— **不建议**，自动生成且很噪
- `*_models.json` / `cursor_models.json` —— **不要**，机器相关

典型 `.gitignore`：

```
.maestro/runs/
.maestro/chat/
.maestro/*_models.json
.maestro/cursor_models.json
.maestro/memory/l2_decisions/
```

### 我能把 `maestro` 部署到服务器吗？

不要。`maestro` 是本地开发工具。服务器侧请用真正的 CI：GitHub Actions、Buildkite 等。本地跑过的计划可以作为起点，但应该转换成 CI 自己的格式。
