# CLI 参考

`maestro <子命令> --help` 能告诉你的一切，按主题整理。

## 工作区

```bash
maestro setup                         # 一条命令完成首次设置
maestro init                          # 在当前目录建 .maestro/
maestro add <path> [选项]             # 注册一个项目
maestro work "<目标>" --root <dir>     # 一条命令扫描/注册/计划/校验
maestro ls                            # 列出已注册项目
maestro rm <name>                     # 移除一个项目
maestro validate                      # 检查 projects.yaml 一致性
```

`setup` 是新机器或新工作区推荐先跑的命令。它会创建 `.maestro/`，安装或刷新内置
skills（默认保留本地改动），汇总已知 agent provider，可用 `--refresh-models` 立即刷新
模型列表，并在最后运行 `doctor`；不想跑诊断可加 `--skip-doctor`。

### `maestro add` 参数

| 参数 | 含义 |
|---|---|
| `--name <id>` | 逻辑名（默认用目录 basename） |
| `--type <t>` | 类型标签（backend / frontend / mobile / tool / library / ...） |
| `--stack <a,b,c>` | 逗号分隔的 stack 标签 |
| `--agent <a>` | 此项目用的 agent（cursor / codex / shell / mock） |

契约和 `memory_scope` 目前只能后续手动编辑 `.maestro/projects.yaml`。

### `maestro work`

```bash
maestro work "给 CLI 加 JSON 输出" --root ~/work/monorepo --agent codex
maestro work "给 CLI 加 JSON 输出" --project api,web --run
maestro work "给 CLI 加 JSON 输出" --root ~/work/monorepo --dry
```

`work` 是日常使用的短路径。它会在需要时初始化 `.maestro/`，可选扫描
`--root` 并写入发现的项目，基于依赖关系生成 workflow PLAN，然后立即做静态校验。
加 `--run` 会直接调度执行；加 `--dry` 只渲染完整 prompt，不调用 agent。

## 计划

```bash
maestro plan validate <path>          # 静态分析
maestro run <path> [--model <id>]     # 跑 PLAN.yaml
maestro rerun <run-id> [--from <task>]  # fork 一次 run
maestro approve <task-id>             # 放行一个被卡住的任务
maestro cancel-run [<run-id>]         # 取消当前 / 指定 run
maestro status                        # 当前运行状态
maestro runs                          # 历史 run 列表
maestro runs evidence [run-id]         # 查看证据：任务时间线、并发重叠、worktree
maestro runs replay [run-id]           # 从事件 + evidence 复盘时间线
maestro runs pr-body [run-id] --write   # 基于 evidence 写 PR_BODY.md
maestro runs events [run-id]           # 查看追加式 run 事件
maestro tui [--run <id>]               # 终端 run 状态/evidence dashboard
maestro logs <task-id> [-f]           # 跟踪任务日志
maestro providers [--adapters-only]   # provider 状态与 adapter 覆盖
```

## 对话

```bash
maestro chat                          # 终端交互 REPL（使用当前 chat provider）
maestro chat ls                       # 列出 session
maestro chat new                      # 新建并切到此 session
maestro chat rm <id>                  # 删除 session
maestro chat tag <id> <tag>...        # 给 session 加标签
maestro chat use <id>                 # 设置当前 session
```

## 记忆

```bash
maestro memory ls
maestro memory get <topic>/<id>
maestro memory put <topic>/<id> < file.md    # 或粘贴后 Ctrl-D
maestro memory rm <topic>/<id>
```

## Skills

```bash
maestro skill ls
maestro skill new <scope> <id>        # 新建模板
maestro skill show <scope> <id>
maestro skill update [id] [--force]   # 安装/刷新内置 skills
maestro skill rm <scope> <id>
maestro skill sync                    # 镜像到 .cursor/rules + .claude/skills
```

`<scope>` 是 `_global` 或一个项目名。

## Learn（失败驱动的 guardrails）

```bash
maestro learn list                                  # 待处理提议，×复发次数
maestro learn show <fingerprint>                    # 完整正文 + 来源
maestro learn promote <fingerprint> --trigger "<短语>" [--scope <scope>]
maestro learn reject <fingerprint>                  # 审计记录；不再被提议
maestro learn scan-memory                           # 为近重复 L2 起草去重提议
```

可选开启（`.maestro/settings.yaml` 里 `learning.propose_guardrails: true`）；失败的
run 会蒸馏出 guardrail 提议，供你审阅并 promote 成 skill。详见
[失败驱动的学习](15-learning.md)。

## 模型

```bash
maestro models                        # 看缓存
maestro models --refresh              # 刷新 Cursor live catalog，其他 provider 用 fallback
```

`maestro providers --json` 会输出机器可读的 provider 注册表，包括安装状态、
binary 路径、是否已接入 adapter、是否支持模型覆盖。

## 面板

```bash
maestro ui [--host 127.0.0.1] [--port 7777]   # 前台 server
maestro open [--port 7777] [--no-browser]     # 后台 server + 开浏览器
maestro doc                                    # 打开文档站
maestro doc ls [--lang zh]                     # 终端列出文档目录
maestro doc show <page> [--lang zh]            # 终端打印一页
```

## 外部历史（只读）

```bash
maestro history                       # 列出 Cursor IDE / Claude Code / Codex 的 session
maestro history show <id>             # 打印一条会话内容
```

## 常用模式

```bash
# 从零起一个工作区
mkdir myproj && cd myproj && maestro setup && maestro open

# 重跑某次失败任务以后
maestro rerun 20260516-082011_d4c95577 --from T_failed_task

# 一条命令：扫描、注册、计划、校验
maestro work "给 CLI 加 JSON 输出" --root ~/work/monorepo --agent codex

# 看一个 verify 任务
maestro logs T_verify -f
```
