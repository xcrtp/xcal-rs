# ze-object

> **场景对象层** — 用户可见的**场景对象**：图元 newtype（`Circle` / `Path` / `Line` / `Rect`）、
> 相机（`Camera2D` / `Camera3D`）、摆放（`Transform` / `ZOrder`）与**装配对象**
> （`Axes2D` / `FunctionGraph`）。只依赖 kernel 层，可编译到 `wasm32`。

---

## 设计定位

```
ze-notion          理念层      数学值 + 代数运算（Circle2D / Rect2D / Transform2D / …）
ze-render-ir       渲染 IR     Shape2D / Style2D / ToShape2D / ResolveStyle
ze-scene-contract  契约层      Scene / ObjectId / Describable / *Desc
ze-object          场景对象层  Circle / Camera2D / Transform / Axes2D / …   ← 本 crate
zela-scene         ECS 实现    Scene trait 的 hecs 实现 + Transform2DInto 实现
zela-render        渲染管线    record_scene / 三角化 / 队列
```

**硬约束**：

- 只依赖 kernel 层（`ze-notion` / `ze-render-ir` / `ze-scene-contract` / `ze-macro`）；
- **不得依赖任何 `zela-*`**（反向依赖禁令）—— 装配对象通过**契约层 trait** `Scene`
  与场景交互，而不是认识 ECS。

**为什么本层依赖契约层是"真实使用"而非凑数**：`#[derive(Describable)]` 在**本层的类型上**
展开，展开代码引用 `ze_scene_contract::{Describable, *Desc}`，装配对象的方法签名吃
`ze_scene_contract::Scene` —— 契约层因此有了第一批真实消费方（FanIn 0 → 3）。

**契约消费方（FanIn = 5）**：`zela-scene`（组件类型 + hecs 登记表 + `Transform2DInto`）、
`zela-render`（`record_scene` 的图元/样式载荷）、`zela-render-backends`、`zela-egui`、`zela-app`。

---

## 模块一览

| 模块 | 类型 | 说明 |
|------|------|------|
| `primitives` | `Circle` / `Path` / `Line` / `Rect` | 图元 **newtype**（`pub` 内层值 + `Deref`/`DerefMut`）+ `impl ToShape2D` + `#[derive(Describable)]` |
| `camera` | `Camera2D` | `{ center, scale, rect }`：`screen_to_world` / `world_to_screen` / `screen_delta_to_world` / `to_transform2d` / **`fit_bounds`**（`rect` = 渲染目标视口，由渲染层在录制时绑定）|
| `camera` | `Framing2D` | **2D 取景参数**（P1-2，**无 `rect`**）：`{ center, scale }` + `to_camera_2d(rect)`。相机作为**场景对象**的形态 |
| `camera` | `Camera2DHandle` / `spawn_camera2d` | **引用型相机句柄**（P2）：`Camera2DHandle(pub ObjectId)` + `resolve(scene, rect) -> Option<Camera2D>`；`spawn_camera2d(scene, framing)` 建相机对象并取句柄 |
| `camera` | `Camera3D` / `Projection` | 3D 相机（`view_matrix` / `view_projection`）与投影（`Perspective` / `Orthographic`，自带 `aspect`）|
| `camera` | `Camera3DHandle` / `spawn_camera3d` | **3D 相机句柄**（K3，与 `Camera2DHandle` 对称）：`Camera3DHandle(pub ObjectId)`；`spawn_camera3d(scene, camera)` 建 3D 相机对象并取句柄 |
| `camera` | `FpsView` / `FPS_PITCH_LIMIT` | **FPS 朝向状态**（K1，组件）：显式 `{ yaw, pitch }`（增量旋转会漂移、pitch 无法 clamp）+ `from_camera` / `forward` / `clamp_pitch`；俯仰钳 `±(π/2 − 0.01)` |
| `placement` | `Transform` | 3D 摆放（`position` / `rotation` / `scale` + `matrix()`）|
| `zorder` | `ZOrder(pub i32)` | **绘制层级**（可缺席）：值越小越先绘制 |
| `axes` | `Axes2D` / `TickSpec` / `AxesLine` | 坐标轴 / 网格**装配对象**：裁剪 + 步长自适应 + 刻度线 + 轴线 |
| `graph` | `FunctionGraph` | 函数图像**装配对象**：采样规格 + 纯采样 + 刷新 |
| `content_texture` | `CONTENT_TEXTURE_ID` / `_WIDTH` / `_HEIGHT` | 内容纹理（S3-3）默认配置：`1` / `512` / `512`（值由 F1 决定，勿随手改）|
| `affine` | `map_path_affine` / `uniform_scale` | 理念层仿射作用到 `Path2D`（**椭圆弧解析改写**，不做折线采样）/ 取统一缩放因子 |

根导出：

```rust
pub use affine::{map_path_affine, uniform_scale};
pub use axes::{AXIS_Z, Axes2D, AxesLine, GRID_Z, TickSpec};
pub use camera::{Camera2D, Camera2DHandle, Camera3D, Framing2D, Projection, spawn_camera2d};
pub use content_texture::{CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH};
pub use graph::{CONTENT_Z, FunctionGraph};
pub use placement::Transform;
pub use primitives::{Circle, Line, Path, Rect};
pub use zorder::ZOrder;
```

---

## 特性速览

### 图元是 newtype + `Deref` —— 消掉"同一个圆两处定义"

```
理念层 ze-notion      Circle2D { center, radius }      ← 唯一的字段定义
对象层 ze-object      Circle(pub Circle2D)             ← 本层（+ Deref ⇒ circle.radius 直接可用）
渲染 IR  Shape2D      Circle { center, radius }        ← 由 ToShape2D 从 Circle2D 构造（冻结载荷形态）
```

- 字段只在理念层定义**一次**；对象层与 IR 层都不再各写一份 ⇒ 两处真相消失。
- `impl ToShape2D for Circle/Line/Path/Rect` 必须在本层：trait 在 IR 层、图元在本层，
  只有"实现里出现本地类型"才合法（**孤儿规则**）。
- `Describable` 的槽位由 `#[describable(..)]` 逐项决定 ⇒ **"线段不能填色"在类型层面成立**
  （`Line::Descriptors` 里根本没有 `FillDesc` 槽位）：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Default, ze_macro::Describable)]
#[describable(Stroke, Fill)]     // Circle / Path / Rect
pub struct Circle(pub Circle2D);

#[describable(Stroke)]           // Line：只有描边槽位（编译期约束）
pub struct Line(pub Line2D);
```

> 注：`Transform2DInto`（`to_screen` / `to_logical`）的实现**不在本层**，而在
> `zela-scene::transform`（那里是 trait 的本地定义处），本层只提供 `affine` 的两个纯函数。

### 相机：`fit_bounds` 与 `screen_delta_to_world`

```rust
// 让世界空间包围盒适配视口：center = 盒中心；scale = margin × min(视口宽/盒宽, 视口高/盒高)
let cam = Camera2D::new(Vec2::ZERO, 40.0, texture_rect).fit_bounds(bbox, 0.8);
// 退化保护：盒宽/盒高非有限或 ≤ 1e-3 ⇒ 保持基底 scale（调用方用"兜底 scale"的相机作基底）

// 屏幕位移增量（像素，Y-down）→ 世界位移（Y-up）：只做缩放与 Y 翻转，不含视口平移
let world_delta = cam.screen_delta_to_world(drag_px);
```

- `to_transform2d()` 是**相机降级为仿射矩阵**的唯一入口：IR 层只吃矩阵不吃相机（避免成环），
  Y 翻转只在该矩阵里存在**一份**（其它地方不得再写翻转）；
- `fit_bounds` 的 `margin` 是显式参数：它是**视觉选择**（0.8 = 内容占视口 80%），不是几何常量。

### 装配对象：数据纯、行为不纯

`Axes2D` / `FunctionGraph` 把"每帧重建几何"从 UI 层搬回对象层（M4）：
**规格是纯数据**（可序列化 / 可测试 / 脚本能描述），**装配方法吃契约层 `Scene`**。

```rust
// 坐标轴 / 网格：build 一次，refresh 逐帧（线段数量随缩放变化 ⇒ 清旧建新）
let axes = Axes2D::unbounded();                       // 也可 Axes2D::new(x_range, y_range)
let handle = axes.build(&mut scene).expect("已登记");   // 容器对象
axes.refresh(&mut scene, handle, &camera);            // 逐帧：裁剪 → 步长 → 生成线段实体
axes.clear(&mut scene, handle);                       // show_grid 关闭时清空（容器保留）

// 函数图像：build 一次，refresh 用**写路径**覆盖既有 Path（采样点数固定 ⇒ 无需清旧建新）
let graph = FunctionGraph::new(FunctionX2Y::new(|x| x.sin()), Sampler1D::new(-10.0, 10.0, 0.1));
let handle = graph.build(&mut scene, StrokeSolid::new_in(2.0, CYAN, MeasureSpace::Screen)).unwrap();
graph.refresh(&mut scene, handle, phase);             // ← 契约层 Scene::set 的消费方
```

- `build(&self, ..)` **消耗 `&self` 而非 `self`**：同一规格可装配多次（两个视口各一套坐标轴）；
- **层级**：网格 z = [`GRID_Z`] = -100、轴线 [`AXIS_Z`] = -90、内容 [`CONTENT_Z`] = 0，
  由 `ZOrder` 组件承载（与迁移前的即写队列逐位一致）；
- `FunctionGraph::refresh` **不取相机**：采样在世界空间完成，相机换算只发生在渲染录制
  （`Path::to_screen`）—— 唯一换算点纪律的必然结果。

> ⚠️ **性能记账（`Axes2D::refresh`）**：当前实现每帧 despawn + spawn ≈ **39** 个实体
> （默认视口 800×600、scale 40）。这是"Scene 化"相对即时模式（直写 `DrawQueue`）的**真实代价**：
> F1 overlay 的实体计数上升、每帧约 39 次 spawn/despawn，视觉无变化。
> 按「图元下沉判据」（实体数爆炸 + 每帧重算），`Axes2D` 是将来**第一个该下沉成单一图元**的候选；
> **触发条件** = 真实动画中成为性能瓶颈（而不是"看起来不够优雅"）。

### `ZOrder` 为什么是可缺席组件

绝大多数对象不需要显式分层（用录制默认层），只有坐标轴/网格这类"观测工具"必须压在内容之下。
故契约层提供 `for_each_pair_opt`（可选第三组件）而不是要求所有对象都带 `ZOrder`；
渲染层缺省时用内容默认层 0。

---

## 测试

```bash
cargo test -p ze-object
cargo doc  -p ze-object --no-deps
```

覆盖：`Deref` 暴露内层字段、**线段无 Fill 槽位（编译期判据）**、`Circle/Rect/Path` 的
`ToShape2D` 降级数值、`Path` 降级保留 `Close`、`screen_delta_to_world` 与"屏幕两点之差"口径一致、
`fit_bounds` 的两式等价（宽 ≥ 高时）与退化盒保护、`Axes2D` 的步长自适应 / 可见范围裁剪 /
刻度含原点 / 网格与轴线线段数、`FunctionGraph` 采样口径 / 相位加在自变量上 / 退化采样规格。

---

## 依赖

```toml
[dependencies]
ze-notion          = { path = "../../kernel/ze-notion" }          # 数学值
ze-render-ir       = { path = "../../kernel/ze-render-ir" }       # Shape2D / ToShape2D / ResolveStyle
ze-scene-contract  = { path = "../../kernel/ze-scene-contract" }  # Describable / Scene（装配）
ze-macro           = { path = "../../kernel/ze-macro" }           # #[derive(Describable)]
derive_builder.workspace = true    # Transform 的 Builder
derive-new.workspace = true        # Camera / ZOrder 的 new
```

无任何 `zela-*`、无 hecs / egui / wgpu。本 crate **没有 dev-dependencies**：
装配路径（`build` / `refresh` 经 `Scene`）由实现侧 `zela-scene` 的契约测试覆盖（那里有 hecs）。
