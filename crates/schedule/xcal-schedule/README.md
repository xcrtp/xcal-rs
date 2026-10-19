# xcal-schedule

> **通用的 ECS 调度框架 + Telemetry**（可观测性基础设施）。
> 从场景层抽离出来的「与业务无关」的那一半：**只做「谁、按什么顺序、访问了什么」**，
> 不知道任何场景 / 渲染 / UI 概念。

---

## 职责

| 模块 | 内容 |
|------|------|
| [`resource`] | `Resource` marker（`Send + Sync + 'static` 的 blanket impl）、`Resources`（TypeMap：`insert` / `get` / `get_mut` / `remove`）、`ResourceTuple` / `ResourceTupleDefault`（`()` ~ 16 元组递归实现） |
| [`access`] | `Access { reads, writes: HashSet<TypeId> }`、`conflicts_with` / `union` / `from_tuples::<R, W>`——System 声明式读写集合的冲突判定 |
| [`context`] | `ExecutorContext { world, commands, dt, time, resources }`（含 `with_world_and_resource_mut` 拆分借用、`spawn` / `despawn` / `insert` 命令缓冲）+ debug-only `BorrowTracker` |
| [`system`] | `System`（关联类型 `Reads` / `Writes: ResourceTuple`）、`Sys<F, R, W>` 闭包适配器与 `sys::<R, W>(...)`、`InitSystem`（blanket auto-impl）、object-safe 的 `ErasedSystem` |
| [`scheduler`] | `Phase`（六阶段 enum）、`Scheduler`（`add_system` / `init_resources` / `run_frame` / `run_frame_with_time` / `tick` / `frame_report`） |
| [`telemetry`] `feature="telemetry"` | `FrameReport` / `SystemTelemetry`（每帧 System 计时）、`FrameClock` / `ZeroClock` / `InstantClock`（**可注入、默认安全**的时间源；浏览器注入 `performance.now()`）、`init_tracing`（结构化日志）、`SnapshotRegistry` / `WorldSnapshot` / `write_snapshot_json`（World 状态导出） |

### 六阶段

每帧按固定顺序 flush 六个独立 CommandBuffer：

| Phase | 典型工作 |
|-------|----------|
| `Input` | 时间 / 输入采集 |
| `LogicUpdate` | UI 状态、业务逻辑 |
| `SceneSync` | 从 UI 资源同步到视图 / 动画 |
| `Prepare` | GPU 资源准备：CPU → GPU 上传、缓存填充 |
| `RenderCommit` | 遍历 World 写入 DrawQueue |
| `FrameEnd` | 真正提交 GPU / egui |

同 phase 内多个 System 在 **debug 模式**下做读写资源集合冲突校验（panic 带系统名），
release 模式零开销；接口上已预留并行调度能力（当前串行）。

---

## 🔴 为什么 `scheduler` 与 `telemetry::frame_report` 必须在同一 crate

这两个模块之间是**双向引用**：

- [`scheduler.rs`](src/scheduler.rs) 使用 `telemetry::{FrameReport, SystemTelemetry}`
  —— 每帧采集并持有 `FrameReport`；
- [`telemetry/frame_report.rs`](src/telemetry/frame_report.rs) 使用
  `crate::scheduler::Phase` —— `SystemTelemetry.phase` 字段的类型。

Rust 的 crate 是编译与依赖的**最小单位**：crate 内模块循环引用完全合法，
跨 crate 循环依赖则被 Cargo 直接禁止。因此这两者**只能同 crate**。

**这是刻意保留的设计，不要试图把它们拆成两个 crate。**
（拆分需要把 `Phase` 下沉到第三个 crate，会让 `Phase` 脱离它唯一的语义归处。）

---

## 依赖

```toml
hecs = "0.11"                       # 唯一的必选依赖

# 以下四个仅在 feature = "telemetry"（默认开启）时引入
tracing / tracing-subscriber / serde / serde_json
```

关闭 feature（`--no-default-features`）时整个 `telemetry` 模块在编译期完全剔除，
观测代码零运行时开销。

> **时间源（默认安全）**：`Scheduler::new` 默认使用 `ZeroClock`（耗时恒 0），
> **不注入也不会 panic**。native 宿主（`xcal-app`）显式注入 `InstantClock` 以保持
> 真实计时；浏览器入口注入基于 `performance.now()` 的实现。

**不依赖**：`xcal-scene` / `xcal-render` / `xcal-egui` / `wgpu`。
反过来说：本 crate 之内**不含任何场景或渲染概念**。

### ⚠️ 与 `xc-resource` 重名但无关

kernel 层还有一个 `xc-resource`（**GPU 资源缓存**：`GpuHandle` / `ResourceKey` /
缓存索引，属渲染契约层）。它与本 crate 的 [`resource`] 模块
（**ECS Resource**：任意 `Send + Sync + 'static` 的全局状态，TypeMap 存储）
只是名字撞了，语义、层次、用途全无关系。不要混淆。

---

## 最小示例

```rust
use xcal_schedule::{Phase, Resources, Resource, Scheduler, sys};

// 1) 自定义 Resource（空 blanket impl）
#[derive(Debug, Default)]
struct Counter(u32);
impl Resource for Counter {}

// 2) 闭包系统：显式声明 R = (Counter,)，W = (Counter,)
let bump = sys::<(Counter,), (Counter,)>(|ctx| {
    if let Some(c) = ctx.get_mut::<Counter>() {
        c.0 += 1;
    }
});

// 3) 组装 scheduler
let resources = {
    let mut r = Resources::new();
    r.insert(Counter::default());
    r
};
let mut sched = Scheduler::new(Default::default(), resources);
sched.add_system(Phase::LogicUpdate, bump);

// 4) 每帧跑 dt
for _ in 0..100 {
    sched.run_frame(1.0 / 60.0);
}
```

---

## 位置

`crates/schedule/xcal-schedule/` —— 与 `app / graph / kernel / render / ui` 并列。
它不是渲染层：放在 `render/` 下语义不符。
