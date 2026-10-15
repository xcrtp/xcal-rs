# xcal-scene

> **xcal-rs 的场景层**。装三类东西：
> ① **场景数据**（`Scene` / 组件 / 图元生成器）；
> ② **场景 System**（`BindingApply` / `MorphApply` / `DirectorAdvance`）；
> ③ **UI ↔ 场景契约**（`domain`：`CameraCommand` / `SceneView` / `ContentHandle`）。
>
> 调度框架本身**不在这里**——它在独立的 `xcal-schedule` crate 中，本 crate 依赖它。

本 crate 是**纯数据 + 纯逻辑**：不得出现任何 `egui::` 类型，也不含任何 GPU / 调度实现。

---

## 在架构中的位置

```
        kernel:  xc-geometry / xc-time / xc-macro   (纯数据 + proc-macro)
                            │
                            ▼
   xcal-schedule  ◄───  xcal-scene   ← 本 crate
   (ECS 调度框架)            │
                    ┌────────┴─────────┐
                    ▼                  ▼
               xcal-render         xcal-egui
                    │                  │
                    └────────┬─────────┘
                             ▼
                xcal-render-backends / xcal-app
```

依赖方向是**单向**的：

- `xcal-scene` → `xcal-schedule`：本 crate 的 System 实现的是
  `xcal_schedule::system::System`，上下文是 `xcal_schedule::context::ExecutorContext`；
  但「谁注册、谁按什么 Phase 跑」由上游（`xcal-app` / `xcal-egui`）决定。
- `xcal-render` / `xcal-egui` → `xcal-scene`：渲染与 UI 消费场景数据与契约。
- 反向依赖不存在：`xcal-schedule` 不认识任何场景/渲染概念。

---

## 关键特性

### 场景数据：`Scene`

`Scene` 是 `hecs::World` + 类型化 `ResourceMap` 的薄包装，提供 `spawn` /
`add_resource` / `borrow_resource(_mut)` / `render(|world, resources| ...)`，
以及 `world()` / `world_mut()` / `into_world()` 的所有权转移入口。

组件分两处：

- 2D 纯逻辑组件（`Circle` / `Path` / `Line` / `MorphShape` / `FunctionX2Y` / marker …）
  与 3D 类型（`Camera3D` / `Projection` / `MeshGeometry` / `Material3D`）都在 `scene::components`
  （以 `comp` 别名再导出）。
- **渲染专属**的组件（`StrokeSolid` / `FillSolid` 等）**不在这里**，它们在 `xcal-render`。

### 场景 System

三个 System 都只实现 `xcal_schedule::system::System`，自身不含调度逻辑：

| System | 阶段 | 职责 |
|--------|------|------|
| `DirectorAdvanceSystem` | `Input` | **唯一**能推进时间的 System：`Director::advance(dt)` → 同步 `TimeState.elapsed` |
| `BindingApplySystem` | `SceneSync` | 查 `BindingTable`，把 `Curve::sample(forest.eval_local(clock, t))` 写进组件字段 |
| `MorphApplySystem` | `SceneSync` | `MorphShape{from,to,t}` → 插值出 `Path` 的 `Path2D`（必须在 `BindingApply` 之后，后者写 `t`） |

动画是**纯函数** `f(t) → 值`（由 `xc_time::Curve::sample` 保证），永不累积状态；
`BindingTable` 是时钟树/曲线与组件之间的**显式**绑定表，取代旧的动画组件组合查询。

### `domain`：UI ↔ 场景契约

两个方向缺一不可，且**都是纯数据**（红线：不得出现 `egui::`）：

- **向下（场景 → UI）**：[`domain::view`] 的 `SceneView` / `LayerDescriptor` /
  `LayerKind` / `LayerId` / `LayerBlend` / `ContentHandle`。场景只声明「有什么图层」，
  UI 决定「怎么画」。`ContentHandle` 刻意保持不透明（S3 统一管线后指向渲染队列分片）。
- **向上（UI → 场景）**：[`domain::event`] 的 `SceneEvent` / `InputEvents` / `WidgetId`
  把原生输入翻译成场景语义事件；[`domain::camera`] 的 `CameraCommand` / `CameraMode`
  是**语义化**相机命令（不引用任何相机字段名，也不含 egui 类型），
  同一命令在不同 `CameraMode` 下语义不同——这就是可插拔控制器的接口。

egui 原生输入的读取与翻译只发生在 `xcal-egui` 的 host。

### 坐标转换：`Transform2D` / `Transform2DInto`

- `Transform2D`：`&self` 版本；`Transform2DInto`：消耗 `self` 版本（省 clone）。
- 对实现 `Transform2DInto + Clone` 的类型，`Transform2D` 由 blanket impl 自动提供。
- 结构体派生：`#[derive(xc_macro::Transform2DInto)]`，字段可用
  `#[transform(skip)]` / `#[transform(iter)]` 调整转换模式。
- 派生宏生成的代码使用绝对路径 `xcal_scene::…`，本 crate 通过
  `extern crate self as xcal_scene;` 让该路径在 crate 内也可解析。

---

## 模块速览

| 模块 | 核心类型 |
|------|----------|
| [`scene`] | `Scene`(hecs World + ResourceMap), `components`(= `comp`: `Camera3D`, `Projection`, `MeshGeometry`, `Material3D`, `Circle`, `Path`, `MorphShape`, `Line`, `FunctionX2Y`, …), `primitives`(`wireframe_cube` / `solid_cube`) |
| [`animation`] | `Binding` / `BindingTarget` / `BindingTable` / `SeekKind` + `BindingApplySystem` / `MorphApplySystem` |
| [`director_state`] | `DirectorState`(包装 `xc_time::Director`), `DirectorAdvanceSystem` |
| [`domain`] | `CameraCommand` / `CameraMode`；`SceneEvent` / `InputEvents` / `WidgetId`；`SceneView` / `LayerDescriptor` / `LayerKind` / `LayerId` / `LayerBlend` / `ContentHandle` |
| [`transform`] | `Transform2D` / `Transform2DInto` trait 系统（结构体派生用 `xc_macro::Transform2DInto`）|
| [`types`] | `Camera2D`, `Rect`, `Vec2/3/4`, `Color32`, `U8Vec4 / I8Vec4` |
| [`time_state`] | `TimeState`（全局累积时间，Resource） |
| [`errors`] | `Error` + `pub type Result<T> = Result<T, Error>` |
| [`utils`] | `auto_into!` 宏 |

`lib.rs` 同时再导出 `xc_time` 的时间原语：
`AnimValue` / `ClockForest` / `ClockId` / `ClockNode` / `Curve` / `Director` / `PlayMode`。

---

## 最小示例：场景 + 场景 System

调度器（`Scheduler` / `Phase` / `Resources`）来自 `xcal-schedule`，
本 crate 只提供被注册的 System：

```rust
use xcal_scene::animation::{BindingApplySystem, MorphApplySystem};
use xcal_scene::{DirectorAdvanceSystem, DirectorState, Scene, TimeState};
use xcal_schedule::{Phase, Resources, Scheduler};

// 1) 场景数据
let mut scene = Scene::new();
scene.spawn((xcal_scene::components::Circle::new((0.0, 0.0), 1.0),));

// 2) 资源：时间 / 导演状态 /（时钟树、绑定表由上层填充）
let mut resources = Resources::new();
resources.insert(TimeState::new());
resources.insert(DirectorState::new());

// 3) 按 Phase 注册场景 System
let mut sched = Scheduler::new(Default::default(), resources);
sched.add_system(Phase::Input, DirectorAdvanceSystem);
sched.add_system(Phase::SceneSync, BindingApplySystem);
sched.add_system(Phase::SceneSync, MorphApplySystem);

// 4) 每帧推进 dt
for _ in 0..60 {
    sched.run_frame(1.0 / 60.0);
}
```

调度框架自身的用法（自定义 Resource / 闭包系统 / 冲突校验 / Telemetry）
见 `xcal-schedule` 的 README。

---

## Features

本 crate **没有 feature**。

`egui` 适配已迁至 `xcal-egui`；`telemetry`（日志 / `FrameReport` / World 快照）
随 ECS 调度内核一起位于 `xcal-schedule`。

## 测试

```bash
cargo test  -p xcal-scene            # 场景层单元测试（含 headless 形变接缝测例）
cargo doc   -p xcal-scene --no-deps  # API 文档
```

---

## 依赖

```toml
xc-macro      = { path = "../../kernel/xc-macro" }            # Transform2DInto 派生
xc-geometry   = { path = "../../kernel/xc-geometry" }         # 纯几何数据层
xc-time       = { path = "../../kernel/xc-time" }             # 时钟树 / 曲线 / Director
xcal-schedule = { path = "../../schedule/xcal-schedule" }     # ECS 调度框架 + Telemetry

glam = "0.33.6"
derive_more (full) / thiserror / smart-default / derive-new / derive_builder
hecs = "0.11"
```

无 `egui`、无 `wgpu`、无 optional feature。
