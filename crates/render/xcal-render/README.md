# xcal-render

> **渲染 trait + 调度 + 资源管理层**。位于三层架构的中间层：
>
> ```text
> xc-notion / xcal-scene  ←── xcal-render ──→  xcal-render-backends
> ```
>
> 定义 2D 与 3D 的绘制器 trait、绘制队列、视口/渲染目标/渲染层抽象与新图元体系。
> **无 egui 依赖**；`wgpu` 仅通过 `feature = "wgpu"` 引入（纯数据/队列/trait 定义不依赖 wgpu）。
> 渲染契约层（顶点格式 `xc-vertex`、资源缓存纯数据 `xc-resource`）已下沉 kernel，
> 本 crate 通过 `pub use` 与过渡 shim 保留原有公开路径。

---

## 管线概览（2D 统一 + 3D 新）

xcal-render 同时承载两套渲染抽象：

### ① 2D 管线（M0 图元体系迁移后，`draw.rs` + re-export）
载荷已统一为 `Shape2D` / `Style2D`，旧 `Shape` / `Style` 枚举**已删除**：
- `Painter2D` trait：`fn paint(&self, shape: &Shape2D, style: &Style2D)`
- `Shape2D` enum（`shape.rs`）：`Empty / Line / Polyline / Polygon / Rect /
  Circle / Ellipse / Path / Group` + `Custom(CustomShapeData)` 扩展钩子
  （`Text/Image` 属预留，尚未纳入）
- `Style2D`（`shape.rs`）：fill / stroke 独立（`Option<Color32>`）+
  `stroke_width` + `space: MeasureSpace`（声明式，`content_ir` 产出 `World`）
- `DrawCommand` = `ZIndex` + `Shape2D` + `Style2D`；`Ord` **只比 `z`**
- `DrawQueue`：`push(DrawCommand)` + 便捷构造（`circle_fill/stroke`、
  `path_fill/stroke`、`line_stroke`、`rect_fill/stroke`）；
  `ensure_sorted()` 用 `Vec::sort`（**稳定**排序，同 `ZIndex` 保插入序）；
  `execute(&painter)` 排序后逐条 `paint`；另有 `merge`（稳定归并）/
  `commands` / `clear` / `with_capacity`
- `Drawable` / `record_drawables` / `record_scene` / `SceneRender`：
  `record_drawables` 是 **ECS 无关**的录制入口，只接受实现了 `Drawable`
  （`to_primitive(&camera) -> Option<(Shape2D, Style2D)>`）的对象——
  `(形状, 样式)` 引用对经泛型 blanket impl 自动获得 `Drawable`；
  `record_scene` 负责把**契约层 `SceneRead`**（`for_each_pair_opt`）里的
  `Circle/Path/Line/Rect` 组合成 `Drawable` 后调用它。**M3-7：不认识 hecs** ——
  hecs 侧经 `xcal-scene` 的 `WorldView` 只读适配器接入。层级（M4）：对象上
  **可选**的 `ZOrder` 组件优先（网格 z = -100 / 坐标轴 z = -90），缺席用默认层（0）。
  旧「逐组件穷举宏」已删除 ⇒
  **新增图元只需实现 `ToShape2D`，不必改动录制逻辑**。
- **描边宽度的度量空间**（`StrokeSolid.space: MeasureSpace`，默认 `Screen`）：
  `record_scene` 是**唯一换算点**——按 `camera.scale` 用 `xc_notion::resolve_width`
  解析为屏幕像素，其它地方不得乘除 `scale`。`MeasureSpace` 由本 crate 再导出；
  `StrokeSolid::new(w, c)` 两参保持（默认 `Screen`），显式声明用 `new_in(w, c, space)`。
  `World` = 随缩放变化（如公式描边），`Screen` = 恒定（网格 / 轴 / 曲线 / 圆 / 形变）。
- **预留可替换后端**（`painter_2d.rs::Painter2DTarget`，已有生产实现 `WgpuBackend2D`）：
  面向 SVG / PDF / TikZ 等非 GPU 后端的**指令级接口**（`begin_frame` / `draw_shape` / `end_frame`）。
  > 注：原 `draw_queue_2d.rs`（`DrawQueue2D` / `DrawCommand2D`，123 行）**已删除** ——
  > 全仓引用仅其自身测试与 `lib.rs` re-export，无生产消费方；保留下来只会成为
  > "纸面能力"（建了没人用的设施比不建更糟）。将来真要接非 GPU 后端时，
  > 应直接实现 `Painter2DTarget`（接口已在），而不是复活一个空队列类型。

### ② S3-0 三角化适配层（`tessellate.rs`，纯 CPU / headless）
`Shape2D + Transform2D + Style2D` → 索引化顶点缓冲，顶点结构**严格**符合自有顶点
契约（[`VertexLayout::standard`]，stride = 20，三槽 position/attrib0/attrib1）：
- `Vertex2D { position, normal, side }`（`#[repr(C)]`，`size == 20` 编译期断言）。
  **描边几何参数化**：`position` 槽存 lyon `position_on_path()`（路径点，**不含宽度**），
  轮廓点由后端 shader 现场补偿 `position + normal * (stroke_width * 0.5)`
  （`normal` 保持 lyon 原样、含 miter 缩放，**不得归一化**）。因此几何与
  `stroke_width` / `camera.scale` **无关**；填充顶点 `normal = [0,0]` → 偏移恒 0。
  宽度仍由 CPU 侧唯一换算点 `draw.rs::resolve_style` 解析（shader 只做几何偏移）
- `Tessellation2D { vertices, indices, layout, kind: Fill | Stroke }`
- `tessellate(shape, transform, style, tolerance) -> Vec<Tessellation2D>`
  （`Group` 递归展开；`Empty` 返回**空 Vec**）
- `tolerance_for_scale(scale) = (0.25 / scale).clamp(1e-4, 1.0)`（≈0.25 屏幕像素误差）
- 不自研三角化：内部用 `lyon_tessellation` 1.0 做填充 / 描边
- `tessellation_layout()` 声明 `(AaModel::Analytic, CacheStrategy::PerFrame)`；
  2D 后端的缓存决策**只读内容哈希**、从不读 `CacheStrategy`，故该标签**仅作声明**
  （改它不改变任何缓存行为）
- **无 feature 门控**：纯 CPU，`--no-default-features` 下同样可用

### ③ 3D 管线
- `DrawQueue3D`：双列表存储 `MeshDraw { mesh, transform, color }` 与
  `LineDraw { start, end, color }`，类型无交集便于并行写入
  （便捷方法 `draw_mesh` / `draw_line`）
- `Painter3D` trait：`fn begin(&mut self)` / `draw_mesh` / `draw_line` / `end`
- `RenderLayer` (`feature="wgpu"`)：渲染层 / framebuffer 组合描述
- `RenderTarget` (`feature="wgpu"`)：对 wgpu `TextureView` 的 RAII 包装
- `Viewport`：纯数据 `(origin, size)`，无 wgpu 依赖，任何 feature 组合均可用
- `RenderBackend / RenderBackend3D` (`feature="wgpu"`)：设备/队列/格式抽象
- `ResourceCache<T>` / `GpuResourceCache`：generation-aware slot + key→id 索引的资源缓存
  （已下沉 `xc-resource`，本 crate 经 `resource_cache` shim 重导出）
- `GpuHandle`：纯 `Any` 包装，**无 wgpu 依赖**，任何上层都可持有（已下沉 `xc-resource`）

---

## Features

| Feature | 默认 | 引入 |
|---------|------|------|
| `wgpu` | ✅ | `wgpu = 30`；编译 `render_layer.rs / render_target.rs / resource_cache.rs` 以及 `render_backend.rs` 中的 `RenderBackend / RenderBackend3D` trait。关闭时仍保留 `GpuHandle`、`Viewport`、`DrawQueue*D`、`Painter*D` 等纯数据与 trait。 |

**典型组合**：
- `xcal-render` (默认) — 用于 xcal-app 装配
- `xcal-render, default-features = false` — 用于 xcal-egui 2D 管线
  （不关心 wgpu 层，仅用 `Painter2D` / `DrawQueue` / `Shape2D` / `Style2D`；
  `tessellate` 是纯 CPU 模块，任何 feature 组合都可用）

---

## 模块速览

| 模块 | 说明 |
|------|------|
| `components` | 2D 管线的组件统一出口（**纯转发**，无自有组件）：`xc-notion` 的数学元语（`FunctionX2Y, Sampler1D/2D, ParameterCurve*, Interpolator, SyncSendFn`）、`xc-object` 的图元与摆放（`Circle, Line, Path, Rect, Transform`）、`xc-render-ir` 的样式组件（`StrokeSolid, FillSolid, StrokeStyle, FillStyle`）、`xcal-scene` 的纯逻辑组件（`PhaseAnim, marker, mesh, PathMesh, …`） |
| `draw` | `Painter2D + DrawQueue + DrawCommand + Shape2D/Style2D + Drawable + record_drawables + record_scene + push_morph_item`（统一 2D 管线；`record_scene<S: SceneRead>` 不认识 hecs，层级读对象上的 `ZOrder`） |
| `record_systems` | P4 从 `xcal-egui` 归位的三个录制 System：`SceneRecordSystem` / `MorphRecordSystem` / `ContentTextureRecordSystem`（**渲染动作住渲染层**；只读 `ViewRect` + `Camera2DHandle`，不读 UI 类型） |
| `view_rect` | `ViewRect`（P4 下沉）：本帧 2D 渲染目标矩形；相机在录制时绑定它得到 `Camera2D`（P1-2 解绑） |
| `tessellate` | S3-0 lyon 三角化适配层：`Vertex2D` / `Tessellation2D` / `tessellate` / `tolerance_for_scale`（纯 CPU，无 feature 门控） |
| `draw_queue_3d` | 3D 双列表队列（`DrawQueue3D` / `MeshDraw` / `LineDraw`）|
| `painter_2d` / `painter_3d` | 绘制器 trait 定义（`Painter2DTarget` / `Painter3D`） |
| `shape` | `Shape2D` + `Style2D` + `CustomShapeData/CustomTypeId` 自定义图元钩子 |
| `viewport` | `Viewport`（纯数据） |
| `render_backend` | 重导出 `GpuHandle / GpuResourceId / AccessError`（kernel 来源，始终可用）+ `RenderBackend / RenderBackend3D` (`wgpu`) |
| `render_layer` (`wgpu`) | `RenderLayer`（framebuffer + 清屏描述 + 视口） |
| `render_target` (`wgpu`) | `RenderTarget`（`TextureView` RAII + 描述符） |
| `resource_cache` (`wgpu`) | 过渡 shim：glob 重导出 `xc-resource` 的 `ResourceCache / ResourceKey / DeferredDestroyer / GpuResourceCache`（M3-7 起**不再有 hecs 的 `CacheKey` 兼容层**）|
| `scene` (mod) | `use xcal_render::scene::{comp, Scene}` 重新导出 components + xcal_scene Scene |
| `content_ir` | `ir_to_draw_cmds`：xc-content `ContentIR` → `Shape2D + Style2D + Transform2D`（scale = target / nominal_font_size，描边宽度同步换算为世界单位并标 `MeasureSpace::World`） |
| `vertex` | 顶点格式契约（已下沉 kernel `xc-vertex`，经 `pub use xc_vertex as vertex;` 保持路径）：`VertexLayout` / `VertexAttrib` / `AaModel` / `CacheStrategy` / `StaticConsistencyChecker`；需 `serde` 可序列化 |

---

## 使用示例（2D 管线 + 自定义 Painter2D）

```rust
use xcal_render::{DrawCommand, DrawQueue, Painter2D, Shape2D, Style2D, ZIndex};
use xc_notion::{Color32, Rect2D, Vec2};

struct ConsolePainter;
impl Painter2D for ConsolePainter {
    fn paint(&self, shape: &Shape2D, style: &Style2D) {
        println!("draw {shape:?} with {style:?}");
    }
}

let mut dq = DrawQueue::new();
dq.push(DrawCommand::new(
    ZIndex::new(0),
    Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(100.0, 50.0))),
    Style2D { stroke: Some(Color32::RED), stroke_width: 2.0, ..Default::default() },
));

dq.ensure_sorted();          // 稳定排序（同 ZIndex 保插入序）
dq.execute(&ConsolePainter); // 遍历执行
```

## 使用示例（2D 三角化 → 自有顶点契约）

```rust
use xcal_render::{tessellate, tolerance_for_scale, Style2D, Shape2D};
use xc_notion::{Color32, Rect2D, Transform2D, Vec2};

let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(100.0, 50.0)));
let style = Style2D { fill: Some(Color32::GREEN), stroke: None, ..Default::default() };

let items = tessellate(&shape, &Transform2D::identity(), &style, tolerance_for_scale(40.0));
assert!(items.iter().all(|t| t.layout == xcal_render::tessellation_layout()));
```

> `Transform2D::identity()` 的实际构造以 `xc-notion` 为准（示例仅示意流程）。

## 使用示例（3D DrawQueue3D）

```rust
use std::sync::Arc;
use xcal_render::DrawQueue3D;
use xcal_scene::scene::components::Material3D;
use xcal_scene::scene::primitives::wireframe_cube;
use xc_notion::Color32;

let white = Color32::new(255, 255, 255, 255);
let mut q3 = DrawQueue3D::new();
q3.draw_mesh(Arc::new(wireframe_cube(1.0)), glam::Mat4::IDENTITY, Material3D::color(white));
q3.draw_line(glam::Vec3::ZERO, glam::Vec3::X, Color32::new(255, 0, 0, 255));
assert_eq!(q3.mesh_count() + q3.line_count(), 2);
```

---

## 测试

```bash
cargo test -p xcal-render                      # 75 个单元测试（顶点契约 / 资源缓存主体已随模块迁至 xc-vertex / xc-resource）
cargo test -p xcal-render --no-default-features # 验证纯数据/队列无 wgpu 依赖时通过
cargo doc  -p xcal-render --no-deps --all-features
```

覆盖点：
- `DrawQueue`：稳定排序（同 `ZIndex` 保插入序）、`merge` 多批次归并、
  `execute` 顺序验证
- `DrawQueue3D`：Mesh/Line 双列表追加、清空、`mesh_count` / `line_count`
- `Shape2D`（rect / circle / path / line）端点一致性 + `record_scene` 经契约层 `SceneRead`
  （测试用**非 ECS 的 `TestScene`** 驱动，证明"可替换"不是纸面承诺）+ `ZOrder` 层级优先；
  闭合路径以 `PathCommand::Close` 结尾（M0 方案 b）
- `tessellate`：`Vertex2D` size == 20 布局契约、`side ∈ {±1}`、
  填充面积 / 描边环面积、Group 展开、
  圆近似误差 ≤ tolerance、`tolerance_for_scale` 随 scale 下降并钳制、
  Custom 钩子（`to_triangles` / `to_path` / 无钩子）
- 描边几何参数化（R1）门卫：`stroke_geometry_is_independent_of_width`
  （同路径宽度 2/8 → 顶点数 / index 表 / `position` / `side` 相同，`normal` 只差
  f32 末位）、`fill_vertices_are_unaffected_by_offset`（任意宽度下偏移恒 0）、
  `shader_offset_reproduces_lyon_contour_points`（`position + normal * w/2`
  与旧管线 lyon `position()` 逐顶点 < 1e-6，即"与旧 CPU 管线同源"的数值证明）、
  `stroke_area_scales_linearly_with_width`（宽 2→8 面积比 = 4）
- `resource_cache`（shim 层）：`ResourceKey` 版本隔离、`get_or_insert` 命中/未命中、
  `mark_used` 保活、`evict_unused`、脏标记与 `upload_dirty`、stale id 返回 `AccessError`
  （`ResourceCache`/`GpuHandle` 主体与 pending 有界性压测已随模块迁至 `xc-resource`）
- `GpuHandle` 跨线程可 Send（测试用 `std::sync::mpsc` 验证）

---

## 依赖

```toml
xcal-scene        = { path = "../../scene/xcal-scene", default-features = false }
xcal-schedule     = { path = "../../schedule/xcal-schedule" }    # P4：录制 System 需要 System / ExecutorContext
xc-notion         = { path = "../../kernel/xc-notion" }
xc-render-ir      = { path = "../../kernel/xc-render-ir" }       # Shape2D / Style2D / 样式组件 / 降级 trait
xc-object         = { path = "../../object/xc-object" }          # 图元 newtype / Camera2D / ZOrder
xc-scene-contract = { path = "../../kernel/xc-scene-contract" }  # record_scene 的唯一入参抽象（SceneRead）
xc-content        = { path = "../../kernel/xc-content" }         # content_ir 消费侧：ir_to_draw_cmds
xc-vertex         = { path = "../../kernel/xc-vertex" }          # 顶点格式契约（已下沉 kernel）
xc-resource       = { path = "../../kernel/xc-resource" }        # 资源缓存 / GpuHandle（已下沉 kernel）
serde             = { workspace = true }                        # VertexLayout 可序列化（pipeline 变体缓存 key）
glam              = "0.33"
derive_more / derive-new / derive_builder
thiserror         = "2.0"
lyon_tessellation = "1.0"   # S3-0 三角化（不自研）
lyon_path         = "1.0"
wgpu              = "30"   (optional, feature = "wgpu")
```

> **无 hecs（库本体）**（M3-7）：`record_scene<S: SceneRead>` 与 P4 归位的三个录制 System
> 都只吃契约层抽象（`WorldView` 是 hecs 侧适配器，在 `xcal-scene`）——
> `Cargo.toml` 的 `[dependencies]` **没有 hecs**。
> ⚠️ P4 新增 `[dev-dependencies] hecs`（**仅测试**）：驱动 `Scheduler` 的容器就是
> `hecs::World`，录制 System 的用例无法绕开。库本体的 hecs 零引用不变。
> （树中一直会出现传递的 hecs，因为本 crate 依赖 `xcal-scene` —— 那是 ECS 实现层。）
