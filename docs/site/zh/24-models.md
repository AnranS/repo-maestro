# Agent 模型选择

`maestro` 把 LLM 调用交给某个适配器（默认 `cursor`，也可以选 `codex`），并提供一个分层的覆盖机制让你给不同任务挑不同模型。

## 解析优先级

高优先级先生效：

1. **任务级**：`PLAN.yaml` 里的 `task.model`
2. **本次运行级覆盖**：`maestro run --model <id>`
3. **任务级 profile**：`PLAN.yaml` 里的 `task.model_profile`
4. **项目级 profile**：`projects.<name>.model_profile`
5. **角色同名 profile**：和任务 role 同名的 `defaults.model_profiles.<role>`
6. **工作区 profile**：`defaults.model_profile`
7. **项目级**：`projects.<name>.agent_model`
8. **工作区默认**：`defaults.agent_model`
9. **账户默认**：未传 `--model` 时由当前 agent 自己挑

每一层都为空时，`maestro` 调当前适配器时不会传 `--model`，由你的账户/配置默认值兜底。`agent_model` 是正式的适配器中立字段；旧配置里的 `cursor_model` 仍会作为兼容别名读取。

## Model profiles

Profile 可以把一组 fallback 链命名，然后在任务、项目、角色或 defaults 中复用：

```yaml
defaults:
  model_profile: balanced
  model_profiles:
    balanced:
      preferred: gpt-5.2
      fallback: [composer-2, composer-2-fast]
    reviewer:
      preferred: gpt-5.3-codex-high
      fallback: [gpt-5.2, composer-2]

projects:
  api:
    path: ./api
    model_profile: reviewer
```

模型缓存可用时，`maestro` 会从 profile 里挑第一个当前账号存在的候选（包括 alias）。缓存为空或不存在时，直接把第一个候选传给适配器。

**对话** tab 还多两层：

- **Session provider 钉子**：通过 provider 选择器或 `--provider` 设置，覆盖 `MAESTRO_CHAT_PROVIDER` / `.maestro/settings.yaml`
- **Session 模型钉子**：在模型选择器里点中的模型仅作用于这个 session

## 模型列表

`maestro` 在 `.maestro/` 下维护本地模型缓存：Cursor 是 `cursor_models.json`，其他 provider 是 `<provider>_models.json`。`/api/models` 返回按 provider 标记的模型；`/api/models?provider=codex` 可只看某个 provider。Cursor 首次加载会尽量自动 refresh；Codex/Claude 在有 live catalog 前使用内置 fallback。

```bash
maestro models                # 看缓存
maestro models --refresh      # 刷新 Cursor live catalog，其他 provider 用 fallback
```

Settings 和模型选择器里都有 **刷新模型列表** 按钮。选中 provider 时只刷新该 provider；不选时返回组合列表。

## 校验

你写了一个缓存里没有的模型时，CLI 和 UI 都会给警告（不会阻断），并按 Levenshtein 距离给出最接近的建议：

```bash
$ maestro run plans/foo.yaml --model gpt5
  ⚠ 未知模型 `gpt5`（不在缓存中）。是不是 `gpt-5.2`?
  如果是最近新加入的，跑 `maestro models --refresh`。
```

距离阈值经过调参，避免对很短的拼写错误给出离谱建议（不会把 `gpt5` 建议成 `auto` 之类）。

## Tagger 模型

会话自动打标签用的是**另一个**模型：

```yaml
defaults:
  agent_model: gpt-5.2-codex      # 干活的大脑
  tagger_model: gpt-5-mini        # 便宜 —— 只生成 1-3 个标签
```

`tagger_model` 不填则回退到 `agent_model`（或旧的 `cursor_model`）；两者都不填则回退到账户默认。

## 实用建议

| 工作负载 | 建议模型 |
|---|---|
| 复杂的多文件重构 | `gpt-5.3-codex-high` / `claude-4.6-opus-max-thinking` |
| 常规 endpoint 实现 | `composer-2-fast` / `gpt-5.2` |
| 快速聊天 / 规划 | `gpt-5-mini` / `composer-2-fast` |
| 自动打标签 | `gpt-5-mini` / `claude-4-sonnet` |
| Debug 难题 | `gpt-5.3-codex-xhigh` |

这只是起点——你的账户访问级别和任务形态可能不一样。覆盖层级的存在就是为了让你**按任务精细调整**而不动其他东西。

## 关于 token 成本

`maestro` 不做预算追踪——它会高兴地烧光你的 Cursor token。所以：

- 编排对话 Agent 用便宜模型（它做了大量规划工作）。
- 复杂任务用 `task.model` 升级到大模型。
- Tagger 是最容易省的——挑最便宜的就行。
