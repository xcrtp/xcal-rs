# zela-scene-ecs

> **场景层的 ECS 实现**（B3-1d 起；原 `zela-scene`）。只装"实现"：
> ① `Scene`（hecs 适配）/ 资源 / 延迟命令 / 生命周期；
> ② 全部 System（`BindingApply` / `MorphApply` / `CameraDrive` / `DirectorAdvance` / …）；
> ③ 调度接线（`register`）。
>
> 概念（对象 / 组件 / 纯逻辑）在 `ze-scene-core`；语言无关能力在 `ze-scene-api`；
> 作品（分镜 / demo）在 `zela-authored`。下游仍可经壳层 `zela-scene` 使用旧路径。
>
> 调度框架本身**不在这里**——它在独立的 `zela-schedule` crate 中，本 crate 依赖它。

本 crate 是**纯数据 + 纯逻辑**：不得出现任何 `egui::` 类型，也不含任何 GPU 实现。

---

## 在架构中的位置

```
        kernel:  ze-notion / ze-time / ze-macro   (纯数据 + proc-macro)
                            │
                            ▼
                   ze-scene-core  ← 对象 / 组件 / 纯逻辑（零 hecs）
                            ▲
   zela-schedule  ◄───  zela-scene-ecs   ← 本 crate（ECS 实现）
   (ECS 调度框架)            │
                    ┌────────┴─────────┐
                    ▼                  ▼
               zela-render         zela-egui
                    │                  │
                    └────────┬─────────┘
                             ▼
                zela-render-backends / zela-app
```

依赖方向是**单向**的：

- `zela-scene` → `zela-schedule`：本 crate 的 System 实现的是
  `zela_schedule::system::System`，上下文是 `zela_schedule::context::ExecutorContext`；
  Phase 归属由本 crate 的 `register` 声明，`zela-app` 只负责调用它；UI 层**不再注册**
  任何场景 System。
- `zela-render` / `zela-egui` → `zela-scene`：渲染与 UI 消费场景数据与契约。
- 反向依赖不存在：`zela-schedule` 不认识任何场景/渲染概念。

---

## 关键特性

### 场景数据：`Scene`

`Scene` 是 `hecs::World` + 类型化 `ResourceMap` 的薄包装，提供 `spawn` /
`add_resource` / `borrow_resource(_mut)` / `len` / `is_empty`，
以及 `world()` / `world_mut()` / `into_world()` 的 ECS 特定入口
（**替换实现时随之重写**；契约层能力见下）。

> **M3-8**：原先的 `render(|world, resources| ...)` 闭包入口**已删除** —— 它把
> `&hecs::World` 交给任意调用方，是 ECS 泄漏最重的一处。需要读场景时走契约层
> `ze_scene_contract::SceneRead`（`for_each` / `for_each_pair` / `for_each_pair_opt`），
> 需要「同时拿 world 与 resource」时用 `zela-schedule` 的
> `ExecutorContext::with_world_and_resource_mut`。

### 契约层适配：`contract`

`zela-scene` 是**契约层 `Scene` trait 的 hecs 实现**（唯一接触面）：

| 类型 | 能力 | 用途 |
|------|------|------|
| `WorldView<'a>` | `SceneRead`（只读） | 渲染录制：`record_scene(&WorldView::new(world), camera, queue)`（不可变借用足够）|
| `WorldScene<'a>` | `SceneRead + Scene`（读写） | 装配对象（`Axes2D::refresh` / `FunctionGraph::refresh`）|
| `Scene`（本地类型） | `SceneRead + Scene` | 持有 `hecs::World` 的场景；App 启动期装配 |
| `object_id_of` / `entity_of` | 句柄互转 | `ObjectId ↔ hecs::Entity`（位模式直通）|

`spawn_dyn` 走**组件登记表**（`ze-object` 的图元 / 相机 / 摆放 / 装配对象 + 样式 +
动画组件（含 `Reveal`）+ **组织组件（`Name` / `Group` / `Visibility` / `Opacity`）** +
`Parent`）；未登记的组件类型 ⇒ 返回 `None`（不造半成品对象）。

组织与可见性（N1–N4，契约层定义、本 crate 登记与转发）：

- `find_by_name(&impl SceneRead, &str) -> Option<ObjectId>`：按 `for_each::<Name>` 遍历，
  返回**第一个**命中（**不新增契约层 trait 方法** —— 加方法要动 4 个实现，不划算）。
- `Name` / `Group` / `GroupId` / `Visibility` / `VisibilityTag` / `GroupVisibility`
  经 `zela_scene::contract` **对外转发**（消费方不必直连 `ze-scene-contract`）。
- `Opacity`（对象级不透明度，落点在 `ze-render-ir`）同样进登记表。

组件分两处：

- 2D 纯逻辑组件（`Circle` / `Path` / `Line` / `MorphShape` / `Reveal` / `PhaseAnim` / marker …）
  与 3D 类型（`Camera3D` / `Projection` / `MeshGeometry` / `Material3D`）都在 `scene::components`
  （以 `comp` 别名再导出）。
- **渲染专属**的组件（`StrokeSolid` / `FillSolid` 等）**不在这里**，它们在 `zela-render`。
- **连续数学对象**（`FunctionX2Y` / `Sampler1D` / `Sampler2D` / `ParameterCurve*` /
  `Interpolator`）M1 起归位理念层 `ze-notion`（`func` / `curve` / `sample`），
  `zela-render::components` 直接从理念层转发，本 crate **不再 re-export**。

### 场景 System

三个 System 都只实现 `zela_schedule::system::System`，自身不含调度逻辑：

| System | 阶段 | 职责 |
|--------|------|------|
| `DirectorAdvanceSystem` | `Input` | **唯一**能推进时间的 System：`Director::advance(dt)` → 同步 `TimeState.elapsed` |
| `BindingApplySystem` | `SceneSync` | 查 `BindingTable`，把 `Curve::sample(forest.eval_local(clock, t))` 写进组件字段 |
| `MorphApplySystem` | `SceneSync` | `MorphShape{from,to,t}` → 插值出 `Path` 的 `Path2D`（必须在 `BindingApply` 之后，后者写 `t`） |

动画是**纯函数** `f(t) → 值`（由 `ze_time::Curve::sample` 保证），永不累积状态；
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

egui 原生输入的读取与翻译只发生在 `zela-egui` 的 host。

### 相机模式与控制器（K 线：K0–K3）

`CameraMode` 是**相机对象的组件**（`spawn` 时挂上；未挂 ⇒ `Default` = `PanZoom`）：

| 模式 | 消费的命令 | 控制器（纯函数，可 headless 测试） |
|---|---|---|
| `PanZoom` | `Pan` / `Zoom` | `apply_pan_zoom(&mut Framing2D, cmd)` |
| `Orbit` | `Orbit` / `Dolly` | `apply_orbit(&mut Camera3D, cmd)`（保持半径 + 俯仰钳制） |
| `Fps` | `Look` / `Move` | `apply_fps(&mut FpsView, &mut Camera3D, cmd)`（pitch 钳 ±(π/2−ε)） |
| `Path` | —（动画驱动） | —（交互命令全部忽略） |

- **分派**：`CameraInputSystem::run` 读相机对象的模式 → `apply_command_2d` / `apply_command_3d`；
  各模式对不属于自己的命令是 **no-op** ⇒ "命令路由"不需要 layer→相机 映射
  （app 里 2D / 3D 图层占据同一视口矩形，坐标无法区分；见 `camera_input.rs` 头注释）。
- `CameraMode` 是 `#[non_exhaustive]` ⇒ 下游用 `CameraMode::{PAN_ZOOM,ORBIT,FPS,PATH}`
  关联常量取值/构造。
- 新增模式只需：加变体 + 加控制器 + 在 `apply_command_*` 的 match 里加一支 ⇒
  **事件契约（`SceneEvent` / `CameraCommand`）零改动**。

### 坐标转换：`Transform2D` / `Transform2DInto`

- `Transform2D`：`&self` 版本；`Transform2DInto`：消耗 `self` 版本（省 clone）。
- 对实现 `Transform2DInto + Clone` 的类型，`Transform2D` 由 blanket impl 自动提供。
- 结构体派生：`#[derive(ze_macro::Transform2DInto)]`，字段可用
  `#[transform(skip)]` / `#[transform(iter)]` 调整转换模式。
- 派生宏生成的代码使用绝对路径 `zela_scene::…`，本 crate 通过
  `extern crate self as zela_scene;` 让该路径在 crate 内也可解析。

---

## 模块速览

| 模块 | 核心类型 |
|------|----------|
| [`scene`] | `Scene`(hecs World + ResourceMap), `components`(= `comp`: `Camera3D`, `Projection`, `MeshGeometry`, `Material3D`, `Circle`, `Path`, `MorphShape`, `Line`, `PhaseAnim`, …), `primitives`(`wireframe_cube` / `solid_cube` / `parametric_surface`) |
| [`contract`] | 契约层 hecs 实现：`WorldView` / `WorldScene` / `object_id_of` / `entity_of` + `SceneRead` 转发 |
| [`animation`] | `Binding` / `BindingTable` / `SeekKind` / `Target`（P0 trait 化：`CircleRadius` / `Phase` / `CameraEye` / `MorphT` / `Camera2DFraming` / **`RevealRange`** / **`OpacityValue`** / **`TransformValue`**；T0-② 后 target 自报 `name()` / `target_id()`，分类清单已删除）/ `Timeline`（P3 顺序编排）+ `BindingApplySystem` / `MorphApplySystem` |
| [`camera_input`] | `CameraInputSystem`（P1：相机状态归场景对象；K0：**按 `CameraMode` 分派**）+ 控制器纯函数 `apply_pan_zoom` / `apply_orbit` / `apply_fps` |
| [`director_state`] | `DirectorState`(包装 `ze_time::Director`), `DirectorAdvanceSystem` |
| [`domain`] | `CameraCommand` / `CameraMode`；`SceneEvent` / `InputEvents` / `WidgetId`；`SceneView` / `LayerDescriptor` / `LayerKind` / `LayerId` / `LayerBlend` / `ContentHandle` |
| [`transform`] | `Transform2D` / `Transform2DInto` trait 系统（结构体派生用 `ze_macro::Transform2DInto`）|
| [`types`] | 视口相关类型已归位：`Camera2D` 在 `ze-object`，视口 `Rect`（重复定义）已删除；此处只转发 `ze-notion` 的 `Vec2/3/4` / `Rect2D` / `Color32`（整数向量 `U8Vec4` / `I8Vec4` 亦由 `ze-notion::vec` 提供） |
| [`time_state`] | `TimeState`（全局累积时间，Resource） |
| [`errors`] | `Error` + `pub type Result<T> = Result<T, Error>` |
| [`utils`] | `auto_into!` 宏 |

`lib.rs` 同时再导出 `ze_time` 的时间原语：
`AnimValue` / `ClockForest` / `ClockId` / `ClockNode` / `Curve` / `Director` / `PlayMode`。

---

## 最小示例：场景 + 场景 System

调度器（`Scheduler` / `Phase` / `Resources`）来自 `zela-schedule`，
本 crate 只提供被注册的 System：

```rust
use zela_scene::animation::{BindingApplySystem, MorphApplySystem};
use zela_scene::{DirectorAdvanceSystem, DirectorState, Scene, TimeState};
use zela_schedule::{Phase, Resources, Scheduler};

// 1) 场景数据
let mut scene = Scene::new();
scene.spawn((zela_scene::components::Circle::new((0.0, 0.0), 1.0),));

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
见 `zela-schedule` 的 README。

---

## Features

本 crate **没有 feature**。

`egui` 适配已迁至 `zela-egui`；`telemetry`（日志 / `FrameReport` / World 快照）
随 ECS 调度内核一起位于 `zela-schedule`。

## 测试

```bash
cargo test  -p zela-scene            # 场景层单元测试（含 headless 形变接缝测例）
cargo doc   -p zela-scene --no-deps  # API 文档
```

---

## 依赖

```toml
ze-macro           = { path = "../../kernel/ze-macro" }           # Transform2DInto 派生
ze-notion          = { path = "../../kernel/ze-notion" }          # 理念层（数学元语）
ze-render-ir       = { path = "../../kernel/ze-render-ir" }       # 样式组件 / 降级 trait
ze-scene-contract  = { path = "../../kernel/ze-scene-contract" }  # 契约层（本 crate 是 hecs 实现）
ze-object          = { path = "../../object/ze-object" }          # 图元 / 相机 / 摆放 / 装配对象
ze-time            = { path = "../../kernel/ze-time" }            # 时钟树 / 曲线 / Director
zela-schedule      = { path = "../../schedule/zela-schedule" }    # ECS 调度框架 + Telemetry

derive_more (full) / thiserror / smart-default / derive-new / derive_builder
hecs = "0.11"
```

> **2026-10-19 收口**：`glam` 依赖已从 Cargo.toml 删除；源码 0 处直接引用
> （数学类型一律走 `ze-notion` 或本层的转发模块 [`types`](src/types.rs)），
> 仅剩 2 行说明性注释（`types.rs` 历史沿革 / `transform.rs` 的 Y 翻转约定）。

无 `egui`、无 `wgpu`、无 optional feature。
