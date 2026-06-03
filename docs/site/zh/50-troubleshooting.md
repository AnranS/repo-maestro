# 排错指南

## macOS "Operation not permitted (os error 1)"

`maestro` 在 Desktop 下某些子目录里调用 `getcwd()` 会失败，这是 macOS TCC（Transparency, Consent, Control）限制。第一次从 `~/Desktop/...` 启动经常踩到。

修复：

1. 系统设置 → 隐私与安全 → 文件与文件夹 → Terminal（或 iTerm / 你用的 shell host）→ 勾上 "桌面文件夹" 和 "下载文件夹"。
2. 或者把工作区从 `~/Desktop` 挪到 `~/work/` 或 `~/code/`。

## 模型下拉空

`/api/models` 返回带 provider 标记的本地缓存（`cursor_models.json`、`<provider>_models.json`）和内置 fallback。首次启动时 Cursor 缓存可能不存在，server 会尝试后台跑 `cursor-agent --list-models`，失败则使用 fallback。

如果失败（离线、没登录、二进制不在 `$PATH`），你会看到只有 8 条的硬编码 fallback。修复：

```bash
cursor-agent status        # 登录状态？
cursor-agent --list-models | head     # 它本身能返回数据吗？
maestro models --refresh       # 命令行手动刷新 Cursor
```

## 面板空白

打开浏览器 devtools 网络/控制台 tab，常见原因：

1. **浏览器缓存了老 JS bundle**，硬刷新（Cmd-Shift-R）。
2. **/api/state 返回 500** —— 工作区权限失败，参考 TCC 修复。
3. **控制台报 `Cannot convert undefined or null to object`** —— 你跑的是旧 binary，重新构建。

## Run 卡在 running 不动

两种可能：

1. Agent 本身卡了。`maestro logs <task-id> -f` 看，没新输出就是模型卡了。`maestro cancel-run` 重试。
2. 一个 `verify` 任务本身很慢。默认 5 分钟，给它加 `timeout_minutes: 30`。

## "missing field `spec`"

`PLAN.yaml` 必须有顶层 `spec: <一行描述>`。Agent 应该总是生成它；手写时容易忘。

## "agent task X has empty prompt"

`kind: agent` 的任务 `prompt` 必填。如果这个任务本来就是想跑命令，把 `kind` 改成 `verify` 并用 `command`。

## 改了 skill / memory，Agent 看不到

skill 和 memory 是在**任务派发时**读取的，不是回复时。要让改动生效得派发一个新任务（或发一条新的聊天消息）。面板永远读最新，UI 不会陈旧。

## "unknown model" 警告，给的建议看着也不对

跑 `maestro models --refresh`——你的 Cursor 账户可能刚拿到一个还没在本地缓存里的新模型。

## CARGO_TARGET_DIR 污染构建

Cursor 的 shell 沙箱有时把 `CARGO_TARGET_DIR` 设到了 tmp。看到陈旧构建时：

```bash
unset CARGO_TARGET_DIR
cargo build --release
```

只在当前 shell 生效，仓库的 `target/` 就会被正确填充。

## 日志在哪儿？

| 来源 | 文件 |
|---|---|
| `maestro ui` 前台 | 你 shell 的 stdout/stderr |
| `maestro open` 后台 | `/tmp/maestro-ui.log` |
| 任务执行 | `.maestro/runs/<id>/logs/<task>.log` |
| 聊天 session | `.maestro/chat/sessions/<id>.json`（完整 transcript 在里面） |

提 bug 时通常给 run 目录 + `maestro ui` 日志就够了。
