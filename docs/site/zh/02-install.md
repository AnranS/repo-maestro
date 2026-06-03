# 安装与卸载

## 从源码构建

`maestro` 是一个单 Rust 二进制：

```bash
git clone <这个仓库>
cd multi_projects_cooperation

# 1. 先构建内嵌的 web 面板 —— Rust 构建会把这些产物嵌进二进制。
cd web && pnpm install && pnpm build && cd ..

# 2. 再编译二进制。
cargo build --release
```

二进制在 `./target/release/maestro`。把它放到 `$PATH` 任意位置：

```bash
cp ./target/release/maestro /usr/local/bin/
```

## 系统要求

| 工具 | 用途 | 检测方式 |
|---|---|---|
| Rust 1.76+ | 编译工具链 | `rustc --version` |
| Node.js 18+ + pnpm | 编译内嵌的 web 面板 | `pnpm --version` |
| `cursor-agent` | 默认 Agent 后端 | `cursor-agent status` |
| `codex` | 可选 Codex 任务后端 | `codex --version` |
| git | 检查仓库和运行 diff | `git --version` |

`cargo build` 会通过 `rust-embed` 把 `web/dist` 里**已经构建好**的面板产物嵌进二进制——它**不会**替你跑 web 构建，所以要先构建 web 包（上面的第 1 步）。之后如果你改了 `web/` 下的代码，请先在那个目录里重新跑一次 `pnpm build`，再回到根目录重新 `cargo build`：

```bash
cd web && pnpm build && cd ..
cargo build --release
```

## 验证安装

```bash
maestro --version
maestro --help
```

在任意目录里 `maestro init` 都会创建一个 `.maestro/` 工作区。你可以同时有任意数量的工作区——它们彼此互不感知。

## 卸载

`maestro` 不会向工作区目录和 `~/.cursor/projects/<workspace>/` 之外的位置写任何东西。完全清理：

```bash
rm /usr/local/bin/maestro                # 二进制
rm -rf <工作区>/.maestro                 # 每个工作区自己的状态
```

如果你运行过 `maestro skill sync`，还会有几个旁路镜像：

- 每个项目里的 `.cursor/rules/*.mdc`
- 每个项目里的 `.claude/skills/<name>/SKILL.md`

它们都是**追加式**的，删除它们不会影响 `maestro` 本身。
