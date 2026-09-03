# xcal-core

> **xcal-rs 的 ECS 与调度核心层**。封装 [`hecs`](https://docs.rs/hecs/0.11)
> 为分阶段 System 调度器、带冲突检查的 Resource 管理、以及 2D/3D 场景类型，
> 是整个 workspace 中除纯几何层外所有 crate 的依赖根。

---

## 在架构中的位置

```
xcal-geometry (纯数据)          xcal-macro (proc-macro)
        │                               │
        └───────────────┬───────────────┘
                        ▼
                  xcal-core  ← 本 crate
                        │
         ┌──────────────┼──────────────┐
         ▼              ▼              ▼
    xcal-render    xcal-egui     (node tech stack 复用 core 的 ECS 惯例)
         │              │
         └──────┬───────┘
                ▼
           xcal-app / xcal-render-backends
```

---

## 关键特性

### 六阶段 Scheduler

每一帧按固定顺序 flush 六个独立 `CommandBuffer`：

| Phase | 典型工作 |
|-------|----------|
| `Input` | 时间/输入采集（TimeUpdate, CameraInput）|
| `LogicUpdate` | UI 状态、业务逻辑（ControlPanel）|
| `SceneSync` | 从 UI 资源同步到视图/动画（ViewRectUpdate, AnimationApply, OrbitApply）|
| `Prepare` | 构建渲染准备数据（GridBuild）|
| `RenderCommit` | 遍历 World 写入 DrawQueue（SceneRecord, SceneRecord3D）|
| `FrameEnd` | 真正提交 GPU / egui（RenderSubmit, RenderSubmit3D）|

同 phase 内多个 System 在 **debug 模式下** 会做读写资源集合冲突校验；
release 模式零开销。接口上已预留并行调度能力（当前串行）。

### `System` trait + 关联类型 Reads/Writes

```rust
pub trait System: Send + Sync + 'static {
    type Reads:  ResourceTuple;
    type Writes: ResourceTuple;
    fn run(&mut self, ctx: &mut ExecutorContext<'_>);
}
```

- 使用 `sys::<R, W>(|ctx| {...})` 以闭包形式注册系统（`Sys<SysClosure, R, W>`）。
- `ResourceTuple` 对 `()` ~ 16 元组递归实现；`ResourceTupleDefault` 子 trait
  支持 `Scheduler::init_resources` 自动注入。
- 对象安全的运行时 trait：`ErasedSystem`（桥接 `run` / `init` / `reads_ids` /
  `writes_ids`），Scheduler 内部持有 `Box<dyn ErasedSystem>`。

### 调试期 BorrowTracker

`#[cfg(debug_assertions)]` 下 `ExecutorContext::get/get_mut` 实际记录每 system
的 borrow TypeId；`Scheduler::add_system` 在加入前静态比对同 phase 系统是否冲突，
冲突则 panic 并给出具体类型名。release 模式完全剔除。

### `with_world_and_resource_mut` 拆分借用

`ExecutorContext` 的 `get_mut(&mut self)` 会把 `&mut self`
完整借走，不能同时持有 `&hecs::World`。对需要同时读 World 又写某 Resource 的
场景（典型如 SceneRecordSystem）：

```rust
ctx.with_world_and_resource_mut::<DrawQueue, _>(|world, dq| {
    for (_e, (circle, stroke)) in world.query::<(&Circle, &StrokeSolid)>().iter() {
        dq.push(/* draw command */);
    }
});
```

内部通过 `ManuallyDrop` 两次解构 `self`，避免 NLL 的"整结构体借用"限制。

---

## 模块速览

| 模块 | 核心类型 |
|------|----------|
| [`resource`] | `Resource`(blanket for `Send+Sync+'static`), `Resources`(TypeMap), `ResourceTuple`, `ResourceTupleDefault` |
| [`access`] | `Access { reads, writes: HashSet<TypeId> }`, `conflicts_with`, `union`, `from_tuples::<R,W>` |
| [`context`] | `ExecutorContext { world, commands, dt, time, resources: 🔒 }` (+ debug `BorrowTracker`) |
| [`system`] | `System`, `Sys<F,R,W>`, `sys()`, `InitSystem`(blanket auto-impl), `ErasedSystem` |
| [`scheduler`] | `Phase` (6 阶段 enum), `Scheduler { phases: Vec<Vec<Box<dyn ErasedSystem>>> }` |
| [`transform`] | `Transform2D` / `Transform2DInto` trait 系统（配合 `xcal-macro` 派生）|
| [`types`] | `Camera2D`, `Rect`, `Vec2/3/4`, `Color32`, `U8Vec4 / I8Vec4` (与 xcal-geometry 再导出一致) |
| [`scene`] | `Scene`(hecs World 包装), `components::{Camera3D, Projection, Mesh3D, OrbitAnim, Circle, Path, Line, FunctionX2Y, ...}`, `primitives::{wireframe_cube, solid_cube}` |
| [`errors`] | `Error { ... }` + `pub type Result<T> = Result<T, Error>` |
| [`telemetry`] `feature="telemetry"` | `FrameReport`, `SnapshotRegistry`, `init_tracing` (Scheduler 埋点 + World 快照导出) |
| [`utils`] | `auto_into!` 等宏工具 |

---

## 最小示例：构建 + 注册系统

```rust
use xcal_core::{
    Phase, Scheduler, Resources, System, sys,
    resource::Resource,
    types::Vec2,
};

// 1) 自定义 Resource
#[derive(Debug, Default)]
struct MousePos(Vec2);
impl Resource for MousePos {}  // 空 blanket impl

#[derive(Debug, Default)]
struct Time(f32);
impl Resource for Time {}

// 2) 闭包系统：R = (Time,), W = (MousePos,)
let update = sys::<(Time,), (MousePos,)>(|ctx| {
    let time = ctx.get::<Time>().unwrap();
    let mut pos = ctx.get_mut::<MousePos>().unwrap();
    pos.0.x = time.0.sin();
    pos.0.y = time.0.cos();
});

// 3) 组装 scheduler
let resources = {
    let mut r = Resources::new();
    r.insert(Time(0.0));
    r.insert(MousePos::default());
    r
};
let mut sched = Scheduler::new(Default::default(), resources);
sched.add_system(Phase::LogicUpdate, update);

// 4) 每帧跑 dt
for _ in 0..100 { sched.run_frame(1.0 / 60.0); }
```

---

## Features

| Feature | 默认 | 含义 |
|---------|------|------|
| `egui` | ✅ | 引入 `egui = 0.36`；资源类型（如 `EguiFrameContext` 虽然定义在 xcal-egui，但核心类型适配需要）|
| `telemetry` | ✅ | 引入 `tracing / tracing-subscriber / serde / serde_json`；编译 `src/telemetry/`，Scheduler 内埋点。关闭时观测代码完全剔除。|

## Benchmark

`cargo bench -p xcal-core --bench scheduler_bench`（`criterion` harness=false，
生成 HTML 报告）：测 100/1_000/10_000 实体 × 6 阶段的调度吞吐。

---

## 测试

```bash
cargo test  -p xcal-core            # ~91 个单元测试
cargo bench -p xcal-core            # criterion 基准
cargo doc   -p xcal-core --no-deps  # API 文档
```

---

## 依赖

```toml
xcal-macro = { path = "../xcal-macro" }      # Transform2DInto 派生
xcal-geometry = { path = "../xcal-geometry" }# 纯数据层
glam         = "0.33.6"
derive_more  = "2.1"   (full)
thiserror    = "2.0"
smart-default= "0.7"
derive-new   = "0.7"
derive_builder= "0.20"
hecs         = "0.11"
egui         = "0.36"   (optional, feature = "egui")
tracing / tracing-subscriber / serde / serde_json
                      (optional, feature = "telemetry")
```
