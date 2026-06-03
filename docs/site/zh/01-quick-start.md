# 快速开始（10 分钟）

这是当前 workflow 的最短路径：把 `maestro` 指向一个装着相关项目的文件夹，
让它分析项目、推断依赖 DAG、生成 prompt；确认没问题后再真正运行。

## 0. 准备

- `maestro` 二进制已加入 `$PATH`（从本仓库 `cargo build --release` 编出来）
- 至少装好一个 agent 后端：`codex` 或 `cursor-agent`
- 一个包含本地项目的文件夹

## 1. 选一个工作区

**工作区**就是带 `.maestro/` 的目录。`maestro` 会以启动它的目录为参照。

```bash
mkdir -p ~/work/maestro-demo
cd ~/work/maestro-demo
```

不需要单独跑 `maestro init`。`maestro work` 会在需要时自动初始化工作区。

如果是新机器，建议先跑一次引导式设置，补齐内置 skills、检查 provider 可用性并运行诊断：

```bash
maestro setup
maestro demo --run
```

`maestro demo --run` 会创建一个零配置的两任务 shell DAG，直接运行并打印报告路径。
如果只想先看 dry-run prompt，用不带 `--run` 的 `maestro demo`。

## 2. 扫描、计划、校验

把 `--root` 指向项目所在文件夹：

```bash
maestro work "给 calculator CLI 加 JSON 输出" \
  --root ~/work/projects \
  --agent codex
```

这条命令会：

1. 在需要时创建 `.maestro/`；
2. 扫描 `--root` 下的项目；
3. 把发现的项目和依赖写入 `.maestro/projects.yaml`；
4. 生成按依赖排序的 `PLAN.yaml`；
5. 对计划做静态校验。

默认模式只生成并校验，然后打印下一步运行命令。

## 3. 先看 prompt

用 `--dry` 可以只渲染完整 prompt，不调用 agent：

```bash
maestro work "给 calculator CLI 加 JSON 输出" \
  --root ~/work/projects \
  --agent codex \
  --dry
```

dry-run 输出会指向 `.maestro/runs/<dry-run-id>/dry/*.prompt.md`。

## 4. 运行 DAG

确认计划和 prompt 没问题后：

```bash
maestro work "给 calculator CLI 加 JSON 输出" \
  --root ~/work/projects \
  --agent codex \
  --run
```

也可以手动运行已生成的计划：

```bash
maestro run plans/2026-05-22-add-json-output.yaml
```

## 5. 打开 Web 面板

```bash
maestro open
```

它会在 `127.0.0.1:7777` 启动 Web UI 并打开浏览器。想让服务以前台进程跑：

```bash
maestro ui
```

关键 tab：

- **对话**：确认编排 Agent 提出的 `maestro-action` 块。
- **任务**：查看 DAG 执行和任务日志。
- **架构**：查看项目依赖和契约依赖边。
- **上下文**：编辑注入任务 prompt 的 skills 和 memory。
- **文档**：内置 EN/ZH 参考。

## 6. 落盘内容

第一次 `maestro work` 后会看到：

```text
.maestro/
├── projects.yaml
├── memory/
├── skills/
└── runs/
```

真实运行或 dry-run 会增加：

- `.maestro/runs/<run-id>/PLAN.yaml`
- `.maestro/runs/<run-id>/RUN_STATE.json`
- `.maestro/runs/<run-id>/REPORT.md`
- `.maestro/runs/<run-id>/logs/<task>.log`
- dry-run 时的 `.maestro/runs/<dry-run-id>/dry/<task>.prompt.md`

## 刚刚发生了什么？

| 步骤 | 组件 | 产物 |
|---|---|---|
| 输入一个目标 | `maestro work` | 标准化 workflow 意图 |
| 发现项目 | discovery engine | `.maestro/projects.yaml` |
| 排列依赖顺序 | planner | `plans/*.yaml` 或指定的 `--out` |
| 准备 prompt | scheduler dry-run / run | `.maestro/runs/<id>/dry/` 或日志 |
| 执行任务 | Codex / Cursor / shell adapter | `.maestro/runs/<id>/RUN_STATE.json` |
| 汇总结果 | reports module | `.maestro/runs/<id>/REPORT.md` |

## 下一步去哪儿

- **理解生成的计划** -> [PLAN.yaml 与 DAG 执行](#docs/plans)
- **调整项目元数据** -> [projects.yaml](#docs/projects-yaml)
- **控制对话动作** -> [maestro-action 协议](#docs/protocols)
- **使用项目 skills** -> [Skills 行动手册](#docs/skills)
