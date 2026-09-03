# xcal-rs

基于 ECS（hecs）与 wgpu 的 2D/3D 渲染实验框架：分阶段的 System 调度器、
Painter 抽象、egui UI overlay，以及 3D 渲染层（Camera3D / Mesh3D /
WgpuBackend3D）。

## 架构总览

```text
┌────────────────────────────────────────────────────────────────────┐
│ xcal-app（组装层 / eframe 入口）                                    │
│  Scheduler 构建、Resource 注入、System 注册、egui-wgpu 回调挂载      │
│  2D：GridBuild → SceneRecord → RenderSubmit                        │
│  3D：OrbitApply → SceneRecord3D → RenderSubmit3D（Layer3DCallback） │
└──────────────┬─────────────────────────────┬───────────────────────┘
               │                             │
┌──────────────▼──────────┐      ┌───────────▼───────────────────────┐
│ xcal-egui（UI overlay）  │      │ xcal-render（渲染层）              │
│  控制面板 / 网格 / 相机    │      │  2D：Painter2D / DrawQueue / Shape │
│  输入等 egui System       │      │  3D：RenderLayer / Viewport /      │
│                          │      │      RenderTarget / DrawQueue3D /  │
│  依赖 xcal-core + render │      │      Painter3D / RenderBackend3D   │
└──────────────┬───────────┘      │  WGPU 后端：                       │
               │                  │   backends/wgpu_2d（2D）           │
               │                  │   backends/wgpu_3d（WgpuBackend3D +│
               │                  │     Layer3DCallback, cube.wgsl）   │
               │                  └───────────┬───────────────────────┘
               │                              │
┌──────────────▼──────────────────────────────▼───────────────────────┐
│ xcal-core（ECS + 场景类型）                                          │
│  hecs World / Resource / System / Scheduler（六阶段调度）             │
│  Transform2D 体系（xcal-macro 派生）                                  │
│  3D：Camera3D / Projection / Mesh3D / PrimitiveTopology / Transform  │
│      primitives::wireframe_cube / solid_cube                         │
│      animation::OrbitAnim                                            │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────────┐
│ xcal-macro：Transform2DInto 过程宏                                   │
└─────────────────────────────────────────────────────────────────────┘
```

依赖方向单向向上：**xcal-macro ← xcal-core ← xcal-render ← xcal-egui ← xcal-app**，
上层可依赖下层，下层不依赖上层。

## Crate 一览

| Crate | 职责 |
|-------|------|
| `xcal-core` | ECS 基础设施（hecs World、Resources、System trait、六阶段 Scheduler）；2D Transform 体系；3D 场景类型（Camera3D / Mesh3D / Primitive / OrbitAnim）；`telemetry` feature 下的帧计时与 World 快照 |
| `xcal-render` | 2D 渲染抽象（Painter2D / DrawQueue / Shape / Style）；**3D 渲染层（RenderLayer / Viewport / RenderTarget / DrawQueue3D / Painter3D / RenderBackend3D）**；wgpu 后端 `WgpuBackend3D`（双管线：triangle list + line list，uniform 传 MVP + color）与 egui-wgpu `Layer3DCallback` |
| `xcal-egui` | egui UI overlay System：控制面板、网格背景、相机输入等 2D 交互 |
| `xcal-app` | 应用组装：创建 Scheduler、注入 Resource、注册 System，通过 egui-wgpu paint callback 在主 render pass 内合成 2D/3D |
| `xcal-macro` | `Transform2DInto` 派生过程宏 |
| `xcal-node` | 节点图内核（Node/Graph/Socket trait 与数据模型，零内部依赖）；`NodeMeta.interaction` 携带每节点类型的交互策略（可拖拽/可选中/可 resize + min/max 尺寸） |
| `xcal-graph-algo` / `xcal-graph-eval` | 图拓扑算法（TopoSort / 环检测 / 脏标记）与惰性拉取求值 |
| `xcal-node-ui` | 属性模型（Property / NodeSpec / NodeRegistry），不依赖 egui |
| `xcal-node-ui-macros` | `#[derive(NodeSpec)]` 过程宏：`#[node(...)]`（含 `resizable/draggable/selectable` 交互属性）+ `#[property(...)]` 零样板生成 |
| `xcal-node-editor` | 纯逻辑编辑器状态机（Command/Undo/Validation）；命中检测为可插拔 `HitTestPolicy`，默认链：输出 pin→输入 pin→resize 握把→卡片→边 |
| `xcal-node-egui` | egui 画布（pan/zoom、卡片/连线绘制、pointer→Command 输入状态机、可拖拽 resize 握把、连线磁吸） |
| `xcal-node-egui-ui` | 属性 → egui 控件映射（`PropertyNodeView` / `draw_property` / `spawn_menu`）；控件随卡片 resize 实时拉伸 |

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
cargo run -p xcal-render --example cube_3d

# 节点编辑器 demo（多输入/多输出 pin、卡片 resize、连线磁吸、撤销重做）
cargo run -p xcal-node-egui --example node_editor_demo

# 全量测试 / 文档
cargo test --workspace
cargo doc --no-deps -p xcal-core -p xcal-render
```
