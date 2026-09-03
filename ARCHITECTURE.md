# Architecture

> **本文件的『依赖图 / 分层 / 合规校验』由 `gen_arch.py` 自动生成。**
> 修改 `Cargo.toml` 后重新运行 `python3 gen_arch.py` 即可刷新。
> **不要手动编辑下方自动生成部分**——手写内容（设计原则、待决问题）请放在末尾『## 设计说明』。

## 分层总览

| 层 | 前缀 | 职责 |
|---|---|---|
| `kernel/` | `xc-` | 纯计算/数据内核（无 egui、无渲染、无领域）|
| `graph/`  | `xc-` | 图论 + 节点结构契约（`NodeSpec`/`Property`/`DynamicObject`）|
| `render/` | `xcal-` | 渲染抽象 + 后端（含 ECS/场景）|
| `ui/`     | `xcal-` | egui 前端 + 编辑器 + proc-macro |
| `app/`    | `xcal-` | 组装 / 可执行 |

## 依赖图

```
依赖方向: 自上而下 (下层被上层依赖). xc-* → xcal-* 单向.

[kernel]
  xc-geometry

[graph]
  xc-graph-algo  ←  xc-node
  xc-graph-eval  ←  xc-graph-algo, xc-node
  xc-node
  xc-node-schema  ←  xc-graph-algo, xc-graph-eval, xc-node, xc-node-schema-macros
  xc-node-schema-macros  ←  xc-graph-algo, xc-graph-eval, xc-node, xc-node-schema

[render]
  xcal-core  ←  xc-geometry, xcal-macro
  xcal-render  ←  xc-geometry, xcal-core
  xcal-render-backends  ←  xc-geometry, xcal-core, xcal-render

[ui]
  xcal-egui  ←  xcal-core, xcal-macro, xcal-render, xcal-render-backends
  xcal-macro
  xcal-node-editor  ←  xc-graph-algo, xc-node
  xcal-node-egui  ←  xc-graph-algo, xc-graph-eval, xc-node, xc-node-schema, xc-node-schema-macros, xcal-node-editor, xcal-node-egui-ui
  xcal-node-egui-ui  ←  xc-node, xc-node-schema, xcal-node-egui

[app]
  xcal-app  ←  xcal-core, xcal-egui, xcal-macro, xcal-render, xcal-render-backends

```

## 完整依赖矩阵

| crate | 依赖 (内部) |
|---|---|
| `xc-geometry` | _(无内部依赖)_ |
| `xc-graph-algo` | `xc-node` |
| `xc-graph-eval` | `xc-graph-algo`, `xc-node` |
| `xc-node` | _(无内部依赖)_ |
| `xc-node-schema` | `xc-graph-algo`, `xc-graph-eval`, `xc-node`, `xc-node-schema-macros` |
| `xc-node-schema-macros` | `xc-graph-algo`, `xc-graph-eval`, `xc-node`, `xc-node-schema` |
| `xcal-app` | `xcal-core`, `xcal-egui`, `xcal-macro`, `xcal-render`, `xcal-render-backends` |
| `xcal-core` | `xc-geometry`, `xcal-macro` |
| `xcal-egui` | `xcal-core`, `xcal-macro`, `xcal-render`, `xcal-render-backends` |
| `xcal-macro` | _(无内部依赖)_ |
| `xcal-node-editor` | `xc-graph-algo`, `xc-node` |
| `xcal-node-egui` | `xc-graph-algo`, `xc-graph-eval`, `xc-node`, `xc-node-schema`, `xc-node-schema-macros`, `xcal-node-editor`, `xcal-node-egui-ui` |
| `xcal-node-egui-ui` | `xc-node`, `xc-node-schema`, `xcal-node-egui` |
| `xcal-render` | `xc-geometry`, `xcal-core` |
| `xcal-render-backends` | `xc-geometry`, `xcal-core`, `xcal-render` |

## 合规校验

❌ 发现 2 个问题:
- 循环依赖: xc-node-schema → xc-node-schema-macros → xc-node-schema
- 循环依赖: xcal-node-egui → xcal-node-egui-ui → xcal-node-egui

## 铁律 (手写)

- 依赖方向单向: `xcal-*` → `xc-*`, 反向禁止
- `xc-*`（kernel/graph）禁止依赖 egui / render / 任何 `xcal-*`（领域/渲染/前端）
- `xc-node-schema` 禁止引用 `FunctionDef`/`AST`/动画领域类型
- 新 crate 命名前先判: 是否与动画/渲染无关 → `xc-`, 否则 `xcal-`
- 新增 crate 必须同时更新 `gen_arch.py` 的 `LAYER_RULES`

## 设计说明 (手写区)

> 在此处手写架构原则、关键决策、待决问题。此区域不被脚本覆盖。
