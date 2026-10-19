# xc-time

> **纯逻辑时间内核** — 时钟树 + 动画曲线 + Director（导演）。
> 零渲染 / 零 ECS / 零 UI 依赖；全部求值为**纯函数**，全部状态可序列化。

---

## 设计定位

xcal-rs 动画架构的时间单向管道中，本 crate 承担全部「时间 → 数值」的计算：

```
墙钟 Instant::now()
  → Director.advance(dt)                    # 唯一可推进时间的对象（可变状态仅此一处）
  → ClockForest.eval_local(id, global_t)    # 纯函数：时钟树映射全局时间 → 本地时间
  → Curve.sample(local_t)                   # 纯函数：本地时间 → AnimValue
  → 写入目标字段（由上层 BindingApplySystem 完成）
```

**为什么是纯函数**：求值不累积任何状态，同一 `t` 永远得到同一结果——
seek（跳转）、倒放（负 rate/speed）、循环回绕因此**免费且必然正确**，
且与渲染帧率完全解耦。

**硬约束**：仅依赖 `xc-notion` / `serde` / `thiserror`；
`glam` 为**禁用项**（数学类型一律经 `xc-notion`，见 `Cargo.toml` 依赖铁律注释）；
`hecs` / `egui` / `wgpu` / 任何 `xcal-*` 绝对禁止。
**路线甲**：可依赖 `xc-notion`（插值原语），但**不得**依赖 `xc-content`——
语义配对（按源码锚点）的知识留在外层。

---

## 模块一览

| 模块 | 关键类型 | 说明 |
|------|----------|------|
| `clock` | `ClockForest` / `ClockNode` / `ClockId` / `PlayMode` | 多根时钟森林；`eval_local` 纯函数求值；`set_parent` 构造期环检测 |
| `curve` | `Curve` / `AnimValue` / `Interp` | 动画曲线（Const/Sin/Orbit/Orbit2D/Phase/Sampled/Morph/PathMorph/Keyframes），`sample(t)` 纯函数 |
| `easing` | `Easing` | 时间重映射 `f(t)→t'`（Linear/EaseIn/EaseOut/EaseInOut/Cubic/Step） |
| `director` | `Director` | 唯一时间源：`global_t` + `speed` + `playing` |
| `error` | `TimeError` | `ClockNotFound` / `CycleDetected` / `ReverseNotSeekable` |

---

## ClockForest — 时钟树

每个节点持有 `parent` / `local_offset` / `rate` / `span` / `mode`，
求值公式（自根向叶递归）：

```
local(t) = map_into_span( (parent_local(t) - offset) * rate, span, mode )
```

| `PlayMode` | span 语义 |
|------------|-----------|
| `Step` | 直接映射，span 无效果（默认） |
| `Loop` | 在 span 边界循环取模（`rem_euclid`，边界连续） |
| `PingPong` | 三角波反弹，边界连续 |
| `Hold` | clamp 到 span（「暂停」语义由 `Director.playing` 负责） |

```rust
use xc_time::{ClockForest, ClockNode, PlayMode};

let mut forest = ClockForest::new();
let root = forest.insert(ClockNode::new(None)).unwrap();

let mut half = ClockNode::new(Some(root));
half.rate = 0.5;                       // 半速子时钟
let child = forest.insert(half).unwrap();

// 纯函数：不改任何状态，可任意次重复求值
let t = forest.eval_local(child, 20.0);   // root_local = 20 → child = 10.0

let mut looper = ClockNode::new(None);
looper.span = Some((0.0, 1.0));
looper.mode = PlayMode::Loop;
let id = forest.insert(looper).unwrap();
// t = 0.5 / 1.5 / 2.5 → 全部映射回 0.5
```

**环检测**：`insert` 天然不可能成环（新 id 不在任何已有节点的祖先链上）；
`set_parent` 沿祖先链检测，遇环返回 `Err(TimeError::CycleDetected)` 且
林结构保持不变（测试锁定：失败后求值行为不受影响）。

---

## Curve — 纯函数动画曲线

```rust
use xc_time::{Curve, AnimValue};

let orbit = Curve::Orbit { radius: 5.0, axis: xc_notion::Vec3::Y, speed: 1.0, initial_angle: 0.0 };
let eye = orbit.sample(0.0);               // AnimValue::Vec3(0, 0, 5)，与旧 OrbitAnim 起点一致
let v = eye.try_vec3().unwrap();           // 类型安全取值：不匹配返回 None
```

| 变体 | 输出 | 用途 |
|------|------|------|
| `Const(v)` | 常量 | 静态绑定 |
| `Sin { base, amp, omega }` | `Scalar` | `base + amp·sin(ωt)` |
| `Orbit { radius, axis, speed, initial_angle }` | `Vec3` | 3D 轨道相机偏移（Rodrigues 轴角旋转） |
| `Orbit2D { radius, speed, initial_angle }` | `Vec3` | **2D 公转**：`(r·cosθ, r·sinθ, 0)`，`θ = speed·t + initial_angle`（XY 平面固定；3D `Orbit` 的基向量是 +Z，投影会退化） |
| `Phase { omega }` | `Scalar` | 线性相位 `ωt`（供 FunctionX2Y 水平滑动 / 形变参数 t） |
| `Sampled { points, interpolation }` | 任意 | 兜底采样点数组（**外部数据**），`Linear`/`Step` 插值 |
| `Morph { from, to, span, easing }` | 任意 | 两条曲线按 `span` 归一化时间经 `easing` 混合（span 外取端点） |
| `PathMorph { from, to, easing, samples }` | `Path` | Path→Path 形变（弧长对齐），顶点数恒为 `samples`（**已就绪未接线**，见下） |
| `Keyframes { keys, interpolation }` | 任意 | 用户编排的关键帧（帧间按 `Interp` 插值） |

> `Sampled` 与 `Keyframes` 数据结构一致，分工是**语义**（外部数据 vs 用户编排），
> 共享同一套采样逻辑；若后续确认无差别，优先合并（S1 计划 §3.2 / §6）。

> **`PathMorph` 状态：已就绪、未接线。** 已实现并有单测覆盖，但当前**无生产接线**
> （`render/`、`app/` 无引用）。场景侧（首个数学动画原型的分镜 3 / 分镜 6）走的是
> `xcal_scene::animation::MorphT` + `MorphShape` 路线 —— 持有 from/to、只写 `t`，
> 由 GPU 形变管线插值（from/to 上传一次、每帧一个 float）；`PathMorph` 是"路径对
> **直接**插值、一次性 CPU 产出路径"的另一条用法，供将来按需接线。

`AnimValue` 支持 `Scalar` / `Vec2` / `Vec3` / `Color([f32;4])` / `Path(Path2D)`：

- 热路径取值 `scalar()` / `vec3()`：类型不匹配在 **debug panic**（尽早暴露 bug）、
  release 返回零值；类型不确定时用 `try_scalar()` / `try_vec2()` / `try_vec3()` /
  `try_path()`（`try_path` 返回引用）；
- 自引入 `Path` 后 `AnimValue` **不再是 `Copy`**；
- `Sampled` 相邻点类型不一致时退化为取区间起点值（**不静默编造插值**）。

`Easing`（时间重映射，纯函数）：

```rust
use xc_time::Easing;
assert_eq!(Easing::EaseInOut.apply(0.5), 0.5);
let e = Easing::Cubic([0.25, 0.1, 0.25, 1.0]);  // CSS cubic-bezier（二分求根，确定性）
```

---

## Director — 唯一时间源

```rust
use xc_time::Director;

let mut d = Director::new();     // global_t = 0, speed = 1, playing = true
d.advance(1.0 / 60.0);           // 只在 playing 时累积 global_t += dt * speed
d.set_speed(-1.0);               // 负速 = 倒放
d.seek(10.0);                    // 纯置位，不累积
d.pause();  d.resume();  d.toggle();
```

可变状态只有 `Director` 一个；`ClockForest` / `Curve` 全程只读纯函数——
这就是「seek / 倒放免费正确」的结构保证。

---

## 序列化

`ClockForest` / `ClockNode` / `Curve` / `AnimValue` / `Director` 全部
`Serialize + Deserialize`（`Vec2`/`Vec3` 的 `serde` 由 `xc-notion` 的 `serde`
feature 转开）。
round-trip 测试锁定：**反序列化后在相同时刻采样结果完全相等**——
时钟树与曲线可安全导出 / 导入。

---

## 测试

```bash
cargo test -p xc-time          # 37 个单元测试
cargo tree -p xc-time          # 依赖卫生：不含 xc-content / hecs / egui / wgpu / xcal-*
```

覆盖点：

- 时钟树 — 嵌套 rate/offset 复合、Loop 周期连续性、PingPong 三角波、
  Hold 钳制、负 rate 倒放、`set_parent` 成环拒绝且结构不被破坏、
  求值无状态确定性（同一 t 同一结果）、serde round-trip
- 曲线 — Sin 公式千点扫描、Orbit 起点/四分之一周期、Phase 线性、
  Sampled 线性插值、Color 插值、类型不匹配退化、try_* 安全取值、
  `PathMorph` 端点与稳定顶点数、`Morph` span 内混合 / span 外取端点、
  `Keyframes` 关键帧精确 + 帧间插值、Path 值参与 lerp、serde round-trip
- 缓动 — 各曲线端点精确、单调不减、`Step` 量化、`Cubic` 等价直线控制点、
  serde round-trip
- Director — advance 累积 / seek 覆盖 / pause 停止 / 负速倒放 / serde round-trip

---

## 依赖

```toml
[dependencies]
xc-notion = { path = "../xc-notion", features = ["serde"] }  # 唯一数学入口 + 插值原语
serde.workspace = true                                       # ClockForest / Curve 可导出
thiserror.workspace = true                                   # 错误类型
```

> `glam` **不是本 crate 的依赖**；直接引用 glam 会被 CI 门禁拦下
> （`No direct glam outside xc-notion`）。

`hecs` / `egui` / `wgpu` / 任何 `xcal-*` 绝对禁止；
`xc-content` 亦禁止（路线甲：语义配对不进时间内核）。
验收：`cargo tree -p xc-time | grep -c xc-content` = 0。
