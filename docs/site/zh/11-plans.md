# `PLAN.yaml` 与 DAG 执行

一份**计划**就是一个声明式的任务 DAG。它是真正会被版本控制、被 review、被执行的产物。计划存在工作区的 `plans/` 目录里；编排 Agent 产出它，你可以手动修改，`maestro run` 来执行。

## 结构

```yaml
spec: 给 api + web 加上用户名/密码登录

tasks:
  - id: T1_api_schema
    project: login-api
    agent: cursor            # 可选；不填用项目的 agent
    model: composer-2-fast   # 可选；任务级模型覆盖
    prompt: |
      定义 POST /login，参数为用户名+密码。
      返回 { token: string }，token 是一小时 TTL 的 JWT。
      把 schema 写到 schemas/openapi.yaml。

  - id: T2_api_impl
    project: login-api
    depends_on: [T1_api_schema]
    prompt: |
      实现路由处理函数。用 bcrypt 校验密码。
      加一个 happy-path 单元测试。

  - id: T3_web_form
    project: login-web
    depends_on: [T1_api_schema]   # 与 T2_api_impl 并行
    prompt: |
      加一个 /login 路由，里面是登录表单。
      成功后把 token 存到 localStorage 的 "auth_token" 字段，跳回首页。

  - id: T4_verify
    project: _global
    agent: shell
    kind: verify
    depends_on: [T2_api_impl, T3_web_form]
    command: |
      cd login-api && pytest -q
      cd ../login-web && pnpm test --run
```

## 任务字段

| 字段 | 必填 | 含义 |
|---|---|---|
| `id` | 是 | 稳定的 kebab-case 标识。依赖、日志、报告里都用它。 |
| `project` | 必填*| `projects.yaml` 里的项目名。特殊值 `_global` 表示"和具体仓库无关"。 |
| `project_each` | 必填*| 项目名列表——任务会被展开成 `<id>__<name>` 一个项目一个任务。与 `project` 互斥。 |
| `kind` | 否 | `agent`（默认）或 `verify`。verify 跑的是 `command`，不是 prompt。 |
| `prompt` | 视 kind 而定 | `kind: agent` 必填。 |
| `command` | 视 kind 而定 | `kind: verify` 必填。多行 shell 通过 `bash -c` 跑。 |
| `agent` | 否 | `cursor` / `codex` / `shell` / `mock`。覆盖项目/默认设置。 |
| `model` | 否 | 仅此任务用的 Agent 模型覆盖。 |
| `depends_on` | 否 | 任务 id 列表。所有上游 `done` 之后才会触发。 |
| `outputs` | 否 | 任务成功后捕获的命名输出，可 snapshot 文件。 |
| `inputs` | 否 | 来自上游输出的命名输入（`from: <task>.<output>`），会自动补 `depends_on`。 |
| `parallel_group` | 否 | 自由字符串。同组任务通过 `max_parallel` 互相限流。 |
| `requires_approval_after` | 否 | 为 `true` 时任务跑完会暂停 DAG，直到 `maestro approve <id>` 才推进。 |
| `timeout_minutes` | 否 | 墙钟超时。按 `kind` 不同有不同默认值。 |
| `memory_inject` | 否 | 在项目 `memory_scope` 之外额外注入的记忆。 |
| `skills` | 否 | 显式注入到 agent 任务的 Maestro skill。默认先查项目 scope，再查 `_global`；可用 `_global/<name>` 消歧。显式 skill 缺失会让任务在派发前失败。 |

\* `project` 和 `project_each` 必须有且只有一个。

## 工具权限

角色可以通过 mode 系统给任务带上 `allowed_tools`（`shell`、`git_write`、`network`、`allowed_commands`）。这里的执行边界会明确区分硬约束和软策略：

- `agent: shell` 是硬约束。设置了 `allowed_commands` 时，Maestro 只接受匹配 allowlist 的单条 argv 风格命令，并拒绝 `&&`、`;`、`|`、重定向、命令替换等 shell 组合。
- `agent: codex` 总是在 Codex workspace sandbox 里运行；当 `network: false` 时，会额外传 Codex 的 workspace network-off sandbox 配置。细粒度的 `shell` 和 `git_write` 暂时还是 prompt policy，因为 Codex CLI 还没有稳定的单工具禁用参数。
- `agent: cursor` 遇到受限角色时会去掉 `--force`，并开启 Cursor sandbox。细粒度的 `shell`、`git_write`、`network` 在 Cursor 暴露稳定硬开关前仍是 prompt policy。

每个任务日志都会写明哪些权限是硬执行，哪些只是 provider 侧软策略。

## 用 `project_each` 做扇出

这是"在每个仓库里干同一件事"最便宜的写法：

```yaml
tasks:
  - id: T_lint
    project_each: [login-api, login-web, mobile-app]
    agent: shell
    command: |
      pre-commit run --all-files

  - id: T_summary
    project: _global
    depends_on: [T_lint]
    prompt: 汇总三个仓库的 lint 结果。
```

加载时 `maestro` 会把 `T_lint` 展开成 `T_lint__login-api`、`T_lint__login-web`、`T_lint__mobile-app`。下游写 `depends_on: [T_lint]` 会被自动重写为依赖**全部三个**子任务。

## Run 是怎么跑的

1. `maestro run plans/foo.yaml` 校验计划，在 `.maestro/runs/<时间戳>_<短-uuid>/` 下建目录。
2. 调度器构建一个 `petgraph` DAG，做拓扑排序。
3. 在 `max_parallel` 上限内派分所有 ready 任务。
4. 每个任务有自己的日志文件 `logs/<id>.log`，通过 SSE 实时回传到面板。
5. 任务结束后下游变成 ready，循环继续直到 DAG 跑干净。
6. 最后写 `REPORT.md`，并把 L2 决策归档到各项目目录下。

## 实时状态

正在跑的时候 `RUN_STATE.json`（在 run 目录顶层）是权威状态。它每次状态切换都会被原子替换；面板通过 `notify` watcher 监听变化，是秒级响应的。终端里：

```bash
maestro status
maestro logs T1_api_schema -f
maestro cancel-run
maestro approve T_design_review
```

## 重跑

```bash
maestro rerun <run-id> --from T2_api_impl
```

从指定的 run fork 一份，从 `T2_api_impl` 开始重跑。之前的任务会被继承为已完成。某一步失败、只想重试那一步时非常好用。

## 静态分析

```bash
maestro plan validate plans/foo.yaml
```

会报告：

- 未知项目
- 环
- verify 任务里的 shell 语法错误（通过 `bash -n -c`）
- 不安全的契约竞争（两个并行任务写同一个 `provides` 文件）
- 计划太大的提醒（> 50 个任务）

`maestro plan validate` 也会在每次 `maestro run` 之前自动跑一遍。
