# xcal-render

> **渲染 trait + 调度 + 资源管理层**。位于三层架构的中间层：
>
> ```text
> xc-geometry / xcal-core  ←── xcal-render ──→  xcal-render-backends
> ```
>
> 定义 2D 与 3D 的绘制器 trait、绘制队列、视口/渲染目标/渲染层抽象、
> 新图元体系与文本渲染接口。**无 egui 依赖**；`wgpu` 仅通过
> `feature = "wgpu"` 引入（纯数据/队列/trait 定义不依赖 wgpu）。

---

## 管线概览（2D 统一 + 3D 新）

xcal-render 同时承载两套渲染抽象：

### ① 2D 管线（M0 图元体系迁移后，`draw.rs` + re-export）
载荷已统一为 `Shape2D` / `Style2D`，旧 `Shape` / `Style` 枚举**已删除**：
- `Painter2D` trait：`fn paint(&self, shape: &Shape2D, style: &Style2D)`
- `Shape2D` enum（`shape.rs`）：`Empty / Line / Polyline / Polygon / Rect /
  Circle / Ellipse / Path / Group` + `Custom(CustomShapeData)` 扩展钩子
  （`Text/Image` 属预留，尚未纳入）
- `Style2D`（`painter_2d.rs`）：fill / stroke 独立（`Option<Color32>`）+
  `stroke_width` + `space: MeasureSpace`（声明式，`content_ir` 产出 `World`）
- `DrawCommand` = `ZIndex` + `Shape2D` + `Style2D`；`Ord` **只比 `z`**
- `DrawQueue`：`push(DrawCommand)` + 便捷构造（`circle_fill/stroke`、
  `path_fill/stroke`、`line_stroke`、`rect_fill/stroke`）；
  `ensure_sorted()` 用 `Vec::sort`（**稳定**排序，同 `ZIndex` 保插入序）；
  `execute(&painter)` 排序后逐条 `paint`；另有 `merge`（稳定归并）/
  `commands` / `clear` / `with_capacity`
- `SceneRender` / `record_scene`：把 hecs World 的 `Circle/Path/Line/Rect/...`
  组件拍平到 `DrawQueue`（z = 0）
- **描边宽度的度量空间**（`StrokeSolid.space: MeasureSpace`，默认 `Screen`）：
  `record_scene` 是**唯一换算点**——按 `camera.scale` 用 `xc_geometry::resolve_width`
  解析为屏幕像素，其它地方不得乘除 `scale`。`MeasureSpace` 由本 crate 再导出；
  `StrokeSolid::new(w, c)` 两参保持（默认 `Screen`），显式声明用 `new_in(w, c, space)`。
  `World` = 随缩放变化（如公式描边），`Screen` = 恒定（网格 / 轴 / 曲线 / 圆 / 形变）。
- **预留可替换后端**（`painter_2d.rs::Painter2DTarget` / `draw_queue_2d.rs` /
  `vector_content.rs`）：面向 SVG / PDF / TikZ 的指令级队列 + 无损矢量描述。

### ② S3-0 三角化适配层（`tessellate.rs`，纯 CPU / headless）
`Shape2D + Transform2D + Style2D` → 索引化顶点缓冲，顶点结构**严格**符合自有顶点
契约（[`VertexLayout::standard`]，stride = 20，三槽 position/attrib0/attrib1）：
- `Vertex2D { position, normal, side }`（`#[repr(C)]`，`size == 20` 编译期断言）
- `Tessellation2D { vertices, indices, layout, kind: Fill | Stroke }`
- `tessellate(shape, transform, style, tolerance) -> Vec<Tessellation2D>`
  （`Group` 递归展开；`Empty` 返回**空 Vec**）
- `tolerance_for_scale(scale) = (0.25 / scale).clamp(1e-4, 1.0)`（≈0.25 屏幕像素误差）
- 不自研三角化：内部用 `lyon_tessellation` 1.0 做填充 / 描边
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
- `ResourceCache<K,V>` / `GpuResourceCache`：TypeMap + `Arc<dyn Any>` GPU 资源缓存
- `GpuHandle`：纯 `Any` 包装，**无 wgpu 依赖**，任何上层都可持有
- `text_engine.rs`：`TextRenderEngine` trait + `Typst/LaTeX` 空壳占位（实现层在 backends）

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
| `components` | 统一 2D 管线需要的 hecs 组件（`Circle, StrokeSolid, FillSolid, Line, Path, FunctionX2Y, Sampler1D, LocalClock, ClockState, CircleAnim, PhaseAnim`）|
| `draw` | `Painter2D + DrawQueue + DrawCommand + Shape2D/Style2D + SceneRender + record_scene`（统一 2D 管线） |
| `tessellate` | S3-0 lyon 三角化适配层：`Vertex2D` / `Tessellation2D` / `tessellate` / `tolerance_for_scale`（纯 CPU，无 feature 门控） |
| `draw_queue_2d` / `draw_queue_3d` | 预留 2D 指令队列 + 3D 双列表队列 |
| `painter_2d` / `painter_3d` | 绘制器 trait 定义 + `Style2D` / `Painter2DTarget` |
| `shape` | `Shape2D` + `CustomShapeData/CustomTypeId` 自定义图元钩子 |
| `vector_content` | 矢量内容容器（可序列化给 SVG/TikZ 后端） |
| `text_engine` | `TextRenderEngine` trait + `BitmapFormat / TextRenderOutput` |
| `viewport` | `Viewport`（纯数据） |
| `render_backend` | `GpuHandle`（始终可用）+ `RenderBackend / RenderBackend3D` (`wgpu`) |
| `render_layer` (`wgpu`) | `RenderLayer`（framebuffer + 清屏描述 + 视口） |
| `render_target` (`wgpu`) | `RenderTarget`（`TextureView` RAII + 描述符） |
| `resource_cache` (`wgpu`) | `GpuResourceCache`（`CacheKey -> Arc<GpuHandle>` LRU 占位） |
| `scene` (mod) | `use xcal_render::scene::{comp, Scene}` 重新导出 components + xcal_core Scene |
| `content_ir` | `ir_to_draw_cmds`：xc-content `ContentIR` → `Shape2D + Style2D + Transform2D`（scale = target / nominal_font_size，描边宽度同步换算为世界单位并标 `MeasureSpace::World`） |
| `vertex` | 顶点格式契约与管线缓存：`VertexLayout` / `VertexAttribute` 定义、`layout.rs`（装配与判重）、`cache.rs`（pipeline 变体缓存）、`aa.rs`（抗锯齿）、`check.rs`（契约自校验）；需 `serde` 可序列化 |

---

## 使用示例（2D 管线 + 自定义 Painter2D）

```rust
use xcal_render::{DrawCommand, DrawQueue, Painter2D, Shape2D, Style2D, ZIndex};
use xc_geometry::{Color32, Rect2D, Vec2};

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
use xc_geometry::{Color32, Rect2D, Transform2D, Vec2};

let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(100.0, 50.0)));
let style = Style2D { fill: Some(Color32::GREEN), stroke: None, ..Default::default() };

let items = tessellate(&shape, &Transform2D::identity(), &style, tolerance_for_scale(40.0));
assert!(items.iter().all(|t| t.layout == xcal_render::tessellation_layout()));
```

> `Transform2D::identity()` 的实际构造以 `xc-geometry` 为准（示例仅示意流程）。

## 使用示例（3D DrawQueue3D）

```rust
use xcal_render::DrawQueue3D;
use xcal_core::scene::primitives::wireframe_cube;
use xc_geometry::Color32;

let white = Color32::new(255, 255, 255, 255);
let mut q3 = DrawQueue3D::new();
q3.draw_mesh(wireframe_cube(1.0, white), glam::Mat4::IDENTITY, white);
q3.draw_line(glam::Vec3::ZERO, glam::Vec3::X, Color32::new(255, 0, 0, 255));
assert_eq!(q3.mesh_count() + q3.line_count(), 2);
```

---

## 测试

```bash
cargo test -p xcal-render                      # 103 个单元测试
cargo test -p xcal-render --no-default-features # 验证纯数据/队列无 wgpu 依赖时通过
cargo doc  -p xcal-render --no-deps --all-features
```

覆盖点：
- `DrawQueue`：稳定排序（同 `ZIndex` 保插入序）、`merge` 多批次归并、
  `execute` 顺序验证
- `DrawQueue3D`：Mesh/Line 双列表追加、清空、`mesh_count` / `line_count`
- `Shape2D`（rect / circle / path / line）端点一致性 + `record_scene` 遍历 hecs World；
  闭合路径以 `PathCommand::Close` 结尾（M0 方案 b）
- `tessellate`：`Vertex2D` size == 20 布局契约、`side ∈ {±1}`、
  `position_on_path` 恒等式往返、填充面积 / 描边环面积、Group 展开、
  圆近似误差 ≤ tolerance、`tolerance_for_scale` 随 scale 下降并钳制、
  Custom 钩子（`to_triangles` / `to_path` / 无钩子）
- `GpuHandle` 跨线程可 Send（测试用 `std::sync::mpsc` 验证）

---

## 依赖

```toml
xcal-core     = { path = "../xcal-core", default-features = false }
xc-geometry = { path = "../xc-geometry" }
xc-content  = { path = "../../kernel/xc-content" }   # content_ir 消费侧：ir_to_draw_cmds
serde       = { workspace = true }                   # VertexLayout 可序列化（pipeline 变体缓存 key）
glam          = "0.33"
derive_more / derive-new / derive_builder
hecs          = "0.11"
thiserror     = "2.0"
lyon_tessellation = "1.0"   # S3-0 三角化（不自研）
lyon_path    = "1.0"
wgpu          = "30"   (optional, feature = "wgpu")
```
