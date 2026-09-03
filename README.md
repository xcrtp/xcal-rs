# xcal-rs

基于 ECS（hecs）与 wgpu 的 2D/3D 渲染实验框架：分阶段的 System 调度器、
Painter 抽象、egui UI overlay，以及 3D 渲染层（Camera3D / Mesh3D /
WgpuBackend3D）。

## 架构总览

workspace 采用分层目录结构 `crates/<layer>/`，通用基础 crate 用 `xc-` 前缀，
领域/应用层保留 `xcal-` 前缀。依赖方向单向：**xc-* ← xcal-* ← app**，反向禁止。

```text
crates/
├── kernel/          # 纯数据层
│   └── xc-geometry       纯几何数据（Color32 / Rect2D / Path2D / Transform2D）
│
├── graph/           # 节点图契约层（禁止依赖 egui / xcal-* / 领域类型）
│   ├── xc-node           节点图内核（Node / Graph / Socket trait + NodeInteraction）
│   ├── xc-graph-algo     图算法（TopoSort / SCC / 脏标记传播）
│   ├── xc-graph-eval     惰性拉取求值（LazyPullEval）
│   ├── xc-node-schema    属性模型（Property / NodeSpec / DynamicObject / NodeRegistry）
│   └── xc-node-schema-macros  #[derive(NodeSpec)] 过程宏
│
├── render/          # 渲染层
│   ├── xcal-core         ECS（hecs）+ Scheduler（六阶段）+ Transform2D + 3D 场景类型
│   ├── xcal-render       2D/3D 渲染抽象（Painter / DrawQueue / Shape / RenderLayer）
│   └── xcal-render-backends  wgpu / egui-wgpu 具体后端（WgpuBackend3D / EguiPainter）
│
├── ui/              # UI 与编辑器层
│   ├── xcal-macro        Transform2DInto 派生宏
│   ├── xcal-egui         egui UI overlay System（控制面板 / 网格 / 相机）
│   ├── xcal-node-editor  纯逻辑节点编辑器状态机（Command / Undo / HitTestPolicy）
│   ├── xcal-node-egui    egui 画布（pan/zoom / 卡片 / 连线 / resize / 磁吸）
│   └── xcal-node-egui-ui 属性→egui 控件映射（PropertyNodeView / draw_property）
│
└── app/             # 应用组装层
    └── xcal-app          eframe 入口（Scheduler 构建 / Resource 注入 / System 注册）
```

依赖铁律：
- `xc-*` crate 不得依赖任何 `xcal-*` crate 或 egui/wgpu
- `xcal-*` crate 可依赖 `xc-*`，但不得依赖 `xcal-app`
- `xcal-app` 可依赖一切，是唯一组装入口

## Crate 一览

| Crate | 层 | 职责 |
|-------|----|------|
| `xc-geometry` | kernel | 纯几何数据（Color32 / Rect2D / Path2D / Transform2D / Point3D） |
| `xc-node` | graph | 节点图内核（Node / Graph / Socket trait + NodeInteraction 交互策略） |
| `xc-graph-algo` | graph | 图算法（TopoSort / SCC / 脏标记传播） |
| `xc-graph-eval` | graph | 惰性拉取求值（LazyPullEval） |
| `xc-node-schema` | graph | 属性模型（Property / NodeSpec / DynamicObject / NodeRegistry），不依赖 egui |
| `xc-node-schema-macros` | graph | `#[derive(NodeSpec)]` 过程宏：`#[node(...)]`（含 `resizable/draggable/selectable`）+ `#[property(...)]` |
| `xcal-core` | render | ECS（hecs World / Resources / System trait / 六阶段 Scheduler）；Transform2D 体系；3D 场景类型（Camera3D / Mesh3D / OrbitAnim） |
| `xcal-render` | render | 2D/3D 渲染抽象（Painter / DrawQueue / Shape / RenderLayer / Viewport） |
| `xcal-render-backends` | render | wgpu / egui-wgpu 具体后端（WgpuBackend3D / EguiPainter / Layer3DCallback） |
| `xcal-macro` | ui | `Transform2DInto` 派生过程宏 |
| `xcal-egui` | ui | egui UI overlay System（控制面板 / 网格 / 相机输入） |
| `xcal-node-editor` | ui | 纯逻辑节点编辑器状态机（Command / Undo / 可插拔 HitTestPolicy） |
| `xcal-node-egui` | ui | egui 画布（pan/zoom / 卡片 / 连线 / resize / 磁吸） |
| `xcal-node-egui-ui` | ui | 属性→egui 控件映射（PropertyNodeView / draw_property / spawn_menu） |
| `xcal-app` | app | 应用组装：Scheduler 构建 / Resource 注入 / System 注册 / egui-wgpu 回调 |

## 3D 帧流水线

1. **SceneSync**：`OrbitApplySystem` 根据 `TimeState` 更新带 `OrbitAnim` 的
   `Camera3D`（eye 绕 target 轨道运动）。
2. **RenderCommit**：`SceneRecord3DSystem` 遍历 World 中的 `Mesh3D`
   （+ 可选 `Transform`），清空并填充 `DrawQueue3D`。
3. **FrameEnd**：`RenderSubmit3DSystem` 取中央视口矩形，构造
   `Layer3DCallback`（camera + aspect + DrawQueue3D 快照）注册为
   egui paint callback；egui-wgpu 在主 render pass 内回调：
   - `prepare`：`WgpuBackend3D::prepare_draws` 计算 VP 矩阵、每帧重建
     vertex/uniform buffer，生成 `Frame3DData`；
   - `paint`：`record_prepared` 设置 scissor（中央视口，避开侧栏）、
     绑定 pipeline 与 bind group，按 topology 发出 draw 命令。

## 运行

```shell
# 完整应用（2D 网格 + 3D 线框立方体 + 轨道相机 + 控制面板）
cargo run -p xcal-app

# 独立 3D demo（不依赖 xcal-app / scheduler，直接演示 WgpuBackend3D）
cargo run -p xcal-render-backends --example cube_3d

# 节点编辑器 demo（多输入/多输出 pin、卡片 resize、连线磁吸、撤销重做）
cargo run -p xcal-node-egui --example node_editor_demo

# 全量测试 / 文档
cargo test --workspace
cargo doc --no-deps -p xcal-core -p xcal-render
```
