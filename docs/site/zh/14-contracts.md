# 跨项目契约

一个**契约**就是一个文件（通常是 schema 或共享模块）——它由一个项目产出，被一个或多个其他项目依赖。有了契约，`maestro` 就能：

- 画出架构依赖图
- **自动接线**：让消费方任务排在生产方之后（绝不对着旧契约开工）
- 检测两个并行任务会不会同时改同一份 `provides` 文件
- 把契约的**真实内容**自动注入到消费方任务的 prompt 里（让 Agent 对着真实接口写，而不是猜）

你可以手写契约，但多数情况下**发现阶段会自动推断**——见下。

## 自动发现（无需声明）

`maestro init --analyze` / `maestro work --root <dir>` 扫描项目时，除了清单依赖（package.json、Cargo 等），还会跟踪**跨项目边界的相对源码 import**。当项目 B import 了项目 A 的文件：

- A 把那个文件作为它的 `provides`，
- B 得到指向它的 `consumes`，
- 记录一条 `A → B` 的边（架构 tab 里画成虚线 + 置信度）。

于是 monorepo / polyrepo 里靠 `import "../shared/..."` 串起来的项目，**零手写配置**就能得到可用的契约图。推断出的契约写进 `.maestro/projects.yaml`，边的来源（provenance）持久化到 `.maestro/topology.json`。

## 声明（可选 / 覆盖）

```yaml
projects:
  login-api:
    path: ./login-api
    contracts:
      provides: schemas/openapi.yaml

  login-web:
    path: ./login-web
    contracts:
      consumes: schemas/openapi.yaml

  mobile-app:
    path: ./mobile-app
    contracts:
      consumes: schemas/openapi.yaml
```

路径是**相对于各自项目目录**的。上例中真实文件是 `login-api/schemas/openapi.yaml`；兄弟仓里的消费方可以写 `consumes: ../login-api/schemas/openapi.yaml`。两边**不需要写成同一字符串**——maestro 按逻辑路径（末两段）匹配契约，生产方相对写法和消费方相对写法指向同一文件即可。

## 图视图

**架构** tab 把契约画成边。产出方节点带绿色 ▲（provides），消费方节点带蓝色 ▼（consumes）。边上有流动效果，表示方向。

## 自动接线

`maestro run` 跑之前会把契约隐含的依赖边补上：没有传递依赖到生产方的消费方任务，会自动加一条 `depends_on` 指向生产方的终端任务。你会看到：

```text
→ wired 2 contract dependency edge(s) so consumers don't race ahead of producers
  + T_change_web depends_on T_change_api  (contract `schemas/openapi.yaml`)
```

加 `--no-wire-contracts` 可按原样跑。每条接的边都会进运行的自动决策账本（REPORT.md、跑完摘要、dashboard）。

## 静态安全检查

`maestro plan validate`（以及每次 `maestro run` 前的隐式校验）会检查：

- 两个并行任务都 `provides` 同一文件 → **错误**
- 消费方任务没有传递依赖到它的生产方 → **警告**（"可能竞态"）；自动接线会修掉大部分这种情况

## 自动注入

消费方任务被派发时，所消费契约的**当前内容**会注入到 prompt：

```text
## Consumed contract: `schemas/openapi.yaml`

<契约内容——读自生产方刚刚集成的改动>
```

内容优先读自生产方的集成 worktree，所以即使在 polyrepo（文件不在消费方自己磁盘上）里，下游任务也能看到上游的改动。`login-web` 的任务只要说"让登录表单跑起来"就够了——Agent 会看到精确 schema，不用猜 endpoint 形状。

## Roadmap

今天的契约只支持一个方向一个字符串。计划中的扩展：

- 每个项目多个 `provides` / `consumes`
- `PLAN.yaml` 的 `contracts_change` 字段（数据模型已就位）携带 diff 摘要
- 契约版本戳，消费方可以钉到某个形状
