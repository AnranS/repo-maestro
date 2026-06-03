# 与编排 Agent 对话

**对话** tab 是你的主要入口。它是一个 provider-backed 对话框（默认 `cursor`，配置后也可用 `codex` / `claude`），并带有两个超能力：

1. **系统上下文** —— 你发出的每条消息都会被自动包上一段当前工作区状态前言：项目、运行状态、记忆、相关 skill。
2. **`maestro-action` 块** —— Agent 可以输出结构化 YAML 块，面板把它们渲染成可点击执行的卡片（`work`、`run`、`approve`、`status`、`rerun`、`plan_validate`）。

## 会话

每个对话就是一个 session，持久化在 `.maestro/chat/sessions/`。Cursor session 会额外保存 provider chat id 以便 resume。一个 session 有：

- 标题（首条 user 消息后由 tagger 模型自动生成，可以编辑）
- 一组自由标签（自动生成，可以编辑）
- 一个固定 provider（可选；不设则依次用 `MAESTRO_CHAT_PROVIDER`、`.maestro/settings.yaml`、`cursor`）
- 一个固定模型（可选；不设则用 `defaults.agent_model`）

侧边栏列出所有 session，可按标签过滤。**+ 新建** 起一个空会话；垃圾桶图标删除（带二次确认）。

## 系统前言

每条用户消息送出去之前，`maestro` 会拼一段隐形前言，结构大致是：

```text
## 工作区
- 工作区根：~/work/login-demo
- 项目：login-api (backend, python/fastapi)、login-web (frontend, react/vite)

## 当前运行
- run id: 20260516-082011_d4c95577
- 2 done, 0 running, 0 failed

## 最近决策（L2，每项目最多 5 条）
- login-api / 20260515-110203: 切到了 bcrypt
- ...

## Action 协议
... （教 Agent 如何输出一段小的 maestro-action 块，通常是 `verb: work`）
```

Agent 因此永远知道局势如何，不用你重复说。

## 流式输出

Assistant 回复是 token 级别流式回传的，通过 SSE。流式过程中你能看到：

- assistant 气泡旁有一个闪烁的光标
- action 块在 fenced YAML 闭合时就实时出现
- **停止** 按钮可以中断生成

部分 provider 会在 delta 之后再发一次完整消息，`maestro` 用 provider 的最终结果作为权威文本去重，UI 不会出现内容重复。

## 单 session provider / 模型覆盖

输入框左下方有 **provider 选择器** 和 **模型选择器**：

- provider default —— 用当前配置的 chat provider
- `cursor`、`codex`、`claude` —— 仅这个 session 固定
- agent default model —— 不传 `--model`，让当前 provider 自己选
- 当前 provider 的缓存 / fallback 模型

多 provider 同时可见时，模型列表会按 provider 分组。选中某个 provider 后刷新模型，只刷新该 provider；使用 provider default 时刷新组合列表。

## 自动打标签

发完第一条 user 消息后，后台一个轻量任务会调用 **tagger 模型**（默认 `defaults.tagger_model`，回退到 `agent_model`），给会话生成 1-3 个标签。你可以在侧栏直接改。

为什么要单独配 tagger 模型？因为给每条消息都用大模型打标签太贵了。tagger 应该选个便宜的（`gpt-5-mini` / `composer-2-fast` 之类）。

## 隐私

Tagger **只看用户侧消息**，从来不会把 assistant 输出送过去——这一约束写在 `chat/tagger.rs` 里。如果完全不想要自动打标签，把 `defaults.tagger_model` 设成一个不存在的 id，tagger 会静默失败，对其他功能没有影响。
