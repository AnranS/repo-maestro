# `projects.yaml`

这个文件是 `maestro` 知道的所有项目的**注册表**。它是单一可信来源：哪些目录可达、哪个项目用哪个 Agent、它们对外暴露什么契约、应该自动接收什么记忆。

文件路径是 `.maestro/projects.yaml`，由 `maestro init` 创建。可以用 `maestro add` / `maestro rm` 增删，也可以手动编辑。

## 完整 schema

```yaml
version: 1                        # schema 版本，目前永远为 1

defaults:                         # 没有覆盖时套用到每个项目
  agent: cursor                   # cursor | codex | shell | mock
  agent_model: ""                 # 为空 = 让当前 agent 自己挑
  tagger_model: ""                # 自动打标签用的便宜模型
  model_profile: balanced         # 可选：默认模型 fallback 链
  model_profiles:
    balanced:
      preferred: gpt-5.2
      fallback: [composer-2, composer-2-fast]
  branch_prefix: feat/            # maestro 建分支时用的前缀
  max_parallel: 4                 # DAG 并发上限
  max_total_tasks: 1000           # 单次 run 的任务总数上限，超过即拒绝（0 = 不限制）
  refute_on_high_risk: false      # 给高风险任务自动挂一个对抗式 refuter 审查（无显式 review_by 时）
  sibling_workspaces:             # 同时索引这些 sibling repo 的契约 provider（建议用相对路径）
    - ../backend-monorepo         # 一个有自己 .maestro/projects.yaml 的 workspace 根

projects:
  login-api:
    path: ./login-api             # 必填：相对路径或 ~/绝对路径
    type: backend                 # backend|frontend|mobile|tool|library|<自定义>
    stack:                        # 自由标签，驱动技术栈模板
      - python
      - fastapi
    agent: cursor                 # 覆盖默认 agent；选 "shell" 跑裸命令
    agent_model: gpt-5.2          # 项目级模型覆盖
    model_profile: balanced       # fallback 链覆盖；优先级高于 agent_model
    memory_scope:                 # 哪些 L1 事实主题会被自动注入
      - api
      - schema
    contracts:                    # 跨项目边界（见[契约]章节）
      provides: schemas/openapi.yaml
      consumes: ""

  login-web:
    path: ./login-web
    type: frontend
    stack: [react, vite, tailwind]
    memory_scope: [ui, design]
    contracts:
      provides: ""
      consumes: schemas/openapi.yaml
```

## 字段表

### `defaults`

| 字段 | 默认值 | 含义 |
|---|---|---|
| `agent` | `cursor` | 项目 / 任务没显式选时用哪个 Adapter。 |
| `agent_model` | `""` | 传给 `cursor` 或 `codex` 的适配器中立模型名。空 = 用当前 agent 的账户/配置默认。 |
| `cursor_model` | 未设置 | `agent_model` 的旧别名。旧配置仍可读取，新写入统一用 `agent_model`。 |
| `tagger_model` | `""` | 聊天自动打标签用的模型。建议选便宜的。空时回退到 `agent_model`。 |
| `model_profile` | 未设置 | 未设置任务/运行/项目 profile 时使用的默认 profile。 |
| `model_profiles` | `{}` | 命名 fallback 链。每个 profile 有 `preferred` 和有序 `fallback`。 |
| `branch_prefix` | `feat/` | 任何创建分支的 agent 都会用这个前缀。 |
| `max_parallel` | `4` | DAG 内同时跑的最大任务数。 |
| `max_total_tasks` | `1000` | 单次 run 可执行的任务总数硬上限（在 `project_each` 展开后统计）。超过上限的失控 plan 会被**拒绝而非截断**，并给出可操作的错误。`0` 表示不限制。不作用于 `rerun`/`resume`（它们恢复的是已 admit 的历史 plan）。 |
| `refute_on_high_risk` | `false` | 为 true 时，没有显式 `review_by` 的高风险任务会自动挂上内置 `refuter` 角色做对抗式审查——专找漏掉的调用点、未迁移的 consumer、被破坏的契约、没测的边界，过了才 integrate。refute 失败走正常的 retry / circuit-breaker 恢复。显式 `review_by` 优先。在 `gate_on_high_risk` 审批门之前跑。成本靠 default-off + 仅高风险作用域控制。 |
| `sibling_workspaces` | `[]` | 额外索引这些 sibling workspace 根（各自有 `.maestro/projects.yaml`）的**契约 provider**，让本仓的 consumer 能把 `consumes` 连到另一个 repo 里的 producer。只索引 sibling 里已设 `contracts.provides` 的项目（不往 sibling 跑 discovery）。本地 provider 永远优先于同名 sibling；多个 sibling 之间按声明顺序先到先得。**建议用相对路径**（相对本 workspace 根解析），可移植可提交；绝对路径是机器相关的。若 consumer 与 sibling 契约之间算不出相对路径，该对会被跳过（绝不写绝对 `consumes`）。空 = 单仓行为。 |

### `projects.<name>`

| 字段 | 必填 | 含义 |
|---|---|---|
| `path` | 是 | 磁盘路径。相对路径以工作区根（即装着 `.maestro/` 的目录）为参照。 |
| `type` | 否 | 提示字符串，影响图标和技术栈模板。常见取值：`backend`、`frontend`、`mobile`、`tool`、`library`。 |
| `stack` | 否 | 自由标签列表。skills 的 `trigger_match` 会用它判断是否注入。 |
| `agent` | 否 | `cursor` / `codex` / `shell` / `mock`。`shell` 直接跑任务自带的 `command`，对验证步骤很好用。 |
| `agent_model` | 否 | 项目级模型覆盖，在任务/运行/profile 之后生效。 |
| `cursor_model` | 否 | `agent_model` 的旧别名；保留给已有工作区。 |
| `model_profile` | 否 | 项目级 fallback 链覆盖，优先级高于 `agent_model`。 |
| `memory_scope` | 否 | `.maestro/memory/l1_facts/` 下要自动注入到此项目任务 prompt 的子目录名列表。 |
| `contracts.provides` | 否 | 此项目对外产出的一个契约文件路径，其他项目可以 `consumes` 它声明依赖。 |
| `contracts.consumes` | 否 | 此项目依赖的一个契约文件路径。 |

> 目前 `provides` 和 `consumes` 都只接受一个字符串。多契约支持在 roadmap 上。

## 解析优先级

调度任务时，`maestro` 用下列顺序决定使用哪个模型和 agent（高优先级先生效）：

1. **任务级**：`PLAN.yaml` 里 `task.model` / `task.agent`
2. **运行级模型**：`maestro run --model ...`
3. **任务级 profile**：`task.model_profile`
4. **项目级 profile**：`projects.<name>.model_profile`
5. **角色同名 profile**：`defaults.model_profiles.<role>`
6. **默认 profile**：`defaults.model_profile`
7. **项目 / defaults 模型**：`projects.<name>.agent_model`，再到 `defaults.agent_model`（也会读取旧的 `cursor_model`）
8. **Adapter 默认**：当前 agent 自己决定的模型

记忆注入的优先级一致——任务级 `memory_inject` 一定盖过项目级 `memory_scope`。

## 通过 CLI 增删改

```bash
maestro add ./api --name api --type backend --stack python
maestro ls
maestro rm api
maestro validate
```

`maestro validate` 会检查路径是否存在、契约是否引用真实文件（如果有契约）、有没有重名冲突。

## 在面板里添加

**架构** tab 顶上有 **+ 添加项目** 按钮。它打开一个表单，写下来的内容会落到 `projects.yaml`，跟 `maestro add` 等价，外加可选的 `contracts.provides` / `consumes`。新项目会立刻出现在依赖图里。
