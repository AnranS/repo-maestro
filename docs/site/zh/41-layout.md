# 磁盘布局 (`.maestro/`)

`maestro` 把所有状态都放在工作区下的 `.maestro/`。除了 `maestro skill sync` 可选写入的 `.cursor/rules/` 和 `.claude/skills/` 之外，不会向其他位置写任何东西。

```text
你的工作区/
├── .maestro/
│   ├── projects.yaml             # 项目注册表（见[projects.yaml]）
│   ├── cursor_models.json        # Cursor 模型缓存
│   ├── <provider>_models.json    # 可选 provider 模型缓存
│   │
│   ├── memory/
│   │   ├── l1_facts/             # 手动维护的主题知识
│   │   │   └── <topic>/<id>.md
│   │   └── l2_decisions/         # 自动归档的 run 摘要
│   │       └── <project>/<run-id>-<slug>.md
│   │
│   ├── skills/                   # markdown 行动手册
│   │   ├── _global/<id>.md
│   │   └── <project>/<id>.md
│   │
│   ├── proposals/                # 失败驱动的 guardrail 提议（不被 gitignore）
│   │   └── <fingerprint>.md      # 用 `maestro learn` 审阅
│   │
│   ├── chat/
│   │   └── sessions/             # session 元数据 + Cursor chat id
│   │       └── <session-id>.json
│   │
│   ├── runs/                     # 每次 `maestro run` 一个目录
│   │   └── <ts>_<short-uuid>/
│   │       ├── PLAN.yaml         # 执行的计划快照
│   │       ├── RUN_STATE.json    # 实时 + 最终状态
│   │       ├── REPORT.md         # 人可读总结
│   │       └── logs/<task-id>.log
│   │
│   └── control/
│       ├── approvals/            # touch 一个文件释放暂停的任务
│       └── cancels/              # touch 一个文件取消运行
│
├── plans/                        # PLAN.yaml 草稿（建议跟项目一起版本化）
│   └── 20260516-add-login.yaml
│
└── (你的项目)
    ├── login-api/
    ├── login-web/
    └── ...
```

## 为什么放这里

| 路径 | 为什么 |
|---|---|
| `.maestro/projects.yaml` | 工作区级别的注册表；不应该塞进任何项目仓库。推荐：放在一个轻量"工作区 meta" 仓库里 commit，或者 `.gitignore`。 |
| `.maestro/runs/` | 大、生成的，一律 `.gitignore`。 |
| `plans/` | 一等公民产物；建议跟实际代码改动一起 commit，让未来的维护者能看到"什么时候跑了什么计划"。 |
| `.maestro/memory/l1_facts/` | 手动维护；建议 commit（或同步到团队知识库）。 |
| `.maestro/memory/l2_decisions/` | 自动生成；commit 可以留审计痕迹，`.gitignore` 也可。 |
| `.maestro/proposals/` | 失败驱动的 guardrail 提议（可选开启）。`.maestro/` 里唯一**不**被 gitignore 的部分，可作为 diff 审阅/提交。用 `maestro learn` promote。 |
| `.maestro/skills/` | 手动维护；建议 commit 让团队共享。 |

## 控制面文件

`control/` 目录是 CLI 给调度器发信号的方式，不走 IPC。调度器用 `notify::recommended_watcher` 监听。Marker 文件只用**文件名**表达信号：

- `control/approvals/<task-id>` —— 释放此任务
- `control/cancels/<run-id>` 或 `current` —— 取消此 run

这是有意为之——任何外部工具甚至 shell `touch` 都能触发，不需要走 HTTP。

## 原子性

`maestro` 写每个状态文件都用 temp-file + rename：

- `RUN_STATE.json.tmp` → `mv` → `RUN_STATE.json`
- `projects.yaml.tmp` → `mv` → `projects.yaml`
- Skill / memory 同上

被中断也最多丢失最新一次写入，绝不会留下半写状态。

## 清理

旧 run 会越积越多，目前没有自动清理。手动：

```bash
# 保留最近 20 个 run
ls -1t .maestro/runs | tail -n +21 | xargs -I{} rm -rf .maestro/runs/{}
```

未来可能会落地成 `maestro runs --prune --keep 20`。
