# 审批门与取消

DAG 跑起来以后**默认是自治的**——你点了同意之后，调度器会一路把能跑的任务都跑完。你可以通过两种机制让人重新进入决策回路。

## `requires_approval_after`

任务带 `requires_approval_after: true` 时，它会正常做完工作、写完日志，**然后暂停整个 DAG** 直到有人显式释放。

```yaml
tasks:
  - id: T_design_review
    project: _global
    prompt: review schemas/openapi.yaml 的 API 设计，确认可以进入实现。
    requires_approval_after: true

  - id: T_impl
    project: login-api
    depends_on: [T_design_review]
    prompt: 实现这个路由。
```

`T_design_review` 完成后：

- 它的节点会变成琥珀色
- 出现一个小的"暂停"徽章
- 下游 `T_impl` 维持 pending —— 调度器不会推进

要继续：

```bash
maestro approve T_design_review
```

或者在 UI 任务卡上点 **approve**。调度器在下一个调度周期（不到 1 秒）就会发现 approval marker 并触发下游。

### 什么时候用

- 契约变更后、下游 Agent 即将消费时
- 真正破坏性的动作之前（部署、push、drop 表）
- "设计 review 通过才能进入实现"这种过渡

如果一份计划完全不带 `requires_approval_after`，整个运行就是 fire-and-forget——适合你已经信任这条 pipeline、想快速迭代时。

## 取消

两种方式：

```bash
maestro cancel-run                  # 取消当前运行
maestro cancel-run 20260516-082011_d4c95577   # 取消指定 run
```

或者在任务 tab 的 run 头部点 **取消** 按钮。

取消是通过往 `.maestro/control/cancels/` 丢一个 marker 文件实现的。调度器会监听这个目录：

1. 停止派发新任务
2. 给在跑的子进程发 SIGTERM（shell adapter 适用）
3. 把还没开始的任务标记成 `cancelled`
4. 写一份最终 `REPORT.md`，包含已完成的部分

被取消的 run 可以用 `maestro rerun --from <id>` 继续往下跑。

## 超时

每个任务都有 `timeout_minutes` 字段（按 kind 默认值不同：agent 任务 15 分钟，verify 任务 5 分钟）。墙钟超时后任务被标记为 `failed`，下游 skip。这里**没有软超时**——如果你想要"轮询"那种语义，请写成 `kind: verify` 任务，shell `command` 里自己实现重试循环。
