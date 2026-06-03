# `maestro-action` 协议详解

总览在 [maestro-action 协议](#docs/actions)，本页是精简后的协议层级参考。

## 块格式

````
```maestro-action
verb: <verb>
description: 可选的人类可读标签
<key>: <value>
```
````

规则：

- info string 必须**精确等于** `maestro-action`。
- body 必须能被解析为 YAML。
- 未知 verb 会被执行器忽略。
- 同一条消息允许多个块，按出现顺序渲染。

## Verb 目录

### `work`

优先用这个。它可以初始化工作区、扫描项目目录、注册发现到的项目、按依赖顺序生成计划并校验。

```yaml
verb: work
description: 扫描、计划并校验 JSON 输出
spec: 给 CLI 加 JSON 输出
root: ~/work/monorepo   # 可选
agent: codex            # 可选
out: plans/json.yaml    # 可选
run: true               # 可选
dry: true               # 可选
```

`spec` 必填。`root` 是要扫描的项目文件夹。想只看生成的 prompt、不调用 agent 时用 `dry: true`。

### `run`

```yaml
verb: run
description: 跑提议的 login 计划
plan: plans/20260516-login.yaml
```

`plan` 必填，相对工作区根。

### `rerun`

```yaml
verb: rerun
plan: 20260516-082011_d4c95577
from: T2_api_impl       # 可选
```

### `approve`

```yaml
verb: approve
task: T_design_review
```

### `status`

```yaml
verb: status
```

### `plan_validate`

```yaml
verb: plan_validate
plan: plans/20260516-login.yaml
```

## 执行语义

- 用户在 UI 里点 **运行**（或在终端 `maestro action run <chat-id> <action-idx>`）之后才执行。
- 每个 verb 对应一个已有 CLI 子命令，执行等价于手动跑那个子命令。
- stdout/stderr 实时流到聊天 UI，显示在 action 卡片的输出面板里。
- exit code 透传：非零 = 红色卡片，零 = 绿色卡片。
- 失败的 action 不会偷偷推进任何后续步骤——下游 Agent 推理会看到这个失败。

## 为什么不用真正的 function call

Cursor 的对话补全 API 在部分模型上支持 function call，但不是所有模型都稳定。Action 块在你账户支持的每个模型上都能正常工作，可以在 markdown 里被翻页/检视，可以手动改。值得为此付出一点 YAML 解析的代价。
