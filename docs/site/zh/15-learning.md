# 失败驱动的学习（guardrails）

Maestro 本就有**被动学习**:一次通过验证的 run 结束后会归档一条 **L2 决策**,并在后续相关任务里 fan-in 注入(见[共享记忆](12-memory.md))。主动学习加上了第一个**可选**循环 —— 把一次 run 的**失败**变成可复用的**护栏(guardrail)**,把它的**成功**变成可复用的**skill 行动手册(playbook)** —— 同时保持 Maestro 的确定性、可审计、人治。

整个特性**默认关闭**,且只遵循一条规则:Maestro 永远只**提议**;在人**采纳**之前,什么都不会改变未来的 run。

## 提议 → 审阅 → 采纳(PROPOSE → REVIEW → PROMOTE)

1. **提议(惰性)。** 三个可选的"生产者"会把**提议**写到 `.maestro/proposals/` —— 都**完全确定性、无 LLM**,各自以**指纹**为键,所以**复发**的信号只会让 `occurrences` 计数 +1,而不是产生重复。提议**不是 skill**、**不在任何注入路径**里 —— 它不可能影响任何 run。

   - **失败 → 护栏**(`learning.propose_guardrails`):一次**失败**的 run(验收失败、熔断、集成冲突或任务失败)被蒸馏成护栏,以失败签名的指纹为键(与熔断器同款归一化)。
   - **成功 → 行动手册**(`learning.synthesize_skills`):一次**通过验证的复杂** run 被蒸馏成一份草稿 skill **playbook**。"复杂"指它做了真实工作 —— **至少 1 个 agent task** —— 且具备广度信号(多项目、契约接线、或任务较多);纯 verify 的 run 没有可复用的工作形态，不会被合成。Maestro 识别出可复用的"形状",起草骨架(有序步骤、契约、由什么验证通过),以 run 的形状为键,并给出确定性计算的 confidence。你在 promote 时把骨架补成真正的 how-to。
   - **记忆自管理**(`maestro learn scan-memory`,按需触发):扫描归档的 [L2 决策](12-memory.md),在同一项目内找出**近重复**记录(本地 TF-IDF 余弦),为每个簇提议合并。你**手动编辑过**的记录("what to remember"段落被改过)会被**排除**。采纳时**保留最新**、淘汰旧的近重复 —— 不合并内容,所以两个不同契约版本永远不会被揉成一条。

2. **审阅。** 人按复发次数从高到低查看队列:

   ```bash
   maestro learn list                 # 待处理提议,×复发次数
   maestro learn show <fingerprint>   # 完整正文 + 来源(哪些 run)
   ```

3. **采纳(唯一会改变行为的操作)。**

   ```bash
   maestro learn promote <fingerprint> --trigger "<让该 skill 触发的短语>"
   maestro learn reject  <fingerprint>   # 留作审计记录;不再被提议
   ```

   对**护栏/playbook**:采纳会通过与手写 skill 相同的路径写出一条普通 **skill**(见 [Skills](13-skills.md))—— 因此会镜像到 `.cursor/rules` 和 `.claude/skills`,此后通过**不变的** skill 机制对匹配的未来任务生效。对**记忆自管理**提议:采纳改为合并 L2 簇(保留最新、淘汰其余)—— 不需要 trigger。

## 为什么必须给 trigger

护栏就是一条 skill,而 skill 在其 `trigger` 短语出现在任务 prompt 里时触发。自动推导的 trigger 被刻意保持**保守** —— 只取高区分度的 token(路径、带点的名字、标识符),绝不取 `test`/`build` 这类泛词 —— 而且常常**留空**。因此采纳时**必须**给一个具体的 trigger(你来写或确认),这样学到的护栏永远不会悄悄淹没无关的 prompt。

## 治理与确定性

- **默认关闭。** 在 `.maestro/settings.yaml` 设 `learning.propose_guardrails: true` 才会产生提议。
- **提议 ≠ 生效。** 提议是惰性文件;只有 `maestro learn promote` 会改变未来行为,且产出的是一条普通、可 review 的 skill。
- **可审计。** `.maestro/proposals/` 是 `.maestro/` 里**唯一不被 gitignore** 的部分,所以提议(以及被保留作记录的 reject)可以作为 diff 审阅 —— 也可选择提交。每条都带 `source_runs` 来源,可回溯到产生它的那些 run。
- **确定性。** 指纹和蒸馏不用 LLM,所以相同的失败永远产生相同的提议。

## 启用

```yaml
# .maestro/settings.yaml
learning:
  propose_guardrails: true   # 失败的 run → 护栏
  synthesize_skills: true    # 通过验证的复杂 run → playbook 草稿
```

两个开关相互独立、默认都关。照常跑;之后看 `maestro learn list`。
