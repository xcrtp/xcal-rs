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

## 两大管线（2D 冻结 + 3D 新）

xcal-render 同时承载两套渲染抽象：

### ① 冻结 2D 管线（`draw.rs` + re-export）
与 xcal-egui / xcal-app 保持二进制稳定接口，不会变更：
- `Painter2D` trait：`fn paint(&self, shape: &Shape, style: &Style)`
- `Shape`：`Line / Rect / Circle / Path / Text / Callback`
- `Style`：`FillSolid / StrokeSolid / StrokeDashed` + `ZIndex`
- `DrawQueue`：`Vec<DrawCommand>`，`merge_sorted_by_z_index()` 使用**稳定归并排序**，
  相同 ZIndex 的命令保留插入顺序（ECS 查询遍历的天然顺序）
- `SceneRender` / `record_scene`：把 hecs World 的 `Circle/Path/Line/...`
  组件拍平到 `DrawQueue`
- **描边宽度的度量空间**（`StrokeSolid.space: MeasureSpace`，默认 `Screen`）：
  `record_scene` 是**唯一换算点**——按 `camera.scale` 用 `xc_geometry::resolve_width`
  解析为屏幕像素，其它地方不得乘除 `scale`。`MeasureSpace` 由本 crate 再导出；
  `StrokeSolid::new(w, c)` 两参保持（默认 `Screen`），显式声明用 `new_in(w, c, space)`。
  `World` = 随缩放变化（如公式描边），`Screen` = 恒定（网格 / 轴 / 曲线 / 圆 / 形变）。

### ② 新版图元体系（`shape.rs` / `painter_2d.rs` / `draw_queue_2d.rs` / `vector_content.rs`）
面向未来的渲染后端抽象（可替换 SVG / PDF / TikZ）：
- `Shape2D` enum：`Rect / RoundedRect / Circle / Line / Polyline / Polygon /
  Path / Text / Image` + `Custom(CustomShapeData)` 钩子
- `Style2D`：独立的 stroke + fill，支持 line_width / cap / join / miter_limit；
  含 `space: MeasureSpace`（声明式，`ir_to_draw_cmds` 产出 `World`）
- `Painter2DTrait`（新）：按 `Shape2D` 逐个 dispatch
- `DrawQueue2D`：指令级队列，配合 `VectorContent` 做无损矢量描述

### ③ 3D 管线
- `DrawQueue3D`：双列表存储 `MeshDraw { mesh, transform, material }` 与
  `LineDraw { start, end, color }`，类型无交集便于并行写入
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
- `xcal-render, default-features = false` — 用于 xcal-egui 冻结 2D 管线
  （不关心 wgpu 层，仅用 Painter2D/DrawQueue/Shape）

---

## 模块速览

| 模块 | 说明 |
|------|------|
| `components` | 冻结 2D 管线需要的 hecs 组件（`Circle, StrokeSolid, FillSolid, Line, Path, FunctionX2Y, Sampler1D, LocalClock, ClockState, CircleAnim, PhaseAnim`）|
| `draw` | `Painter2D + DrawQueue + Shape + Style + record_scene`（冻结 2D 管线） |
| `draw_queue_2d` / `draw_queue_3d` | 新 2D 队列 + 3D 双列表队列 |
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

---

## 使用示例（冻结 2D 管线 + 自定义 Painter2D）

```rust
use xcal_render::{
    Painter2D, Shape, Style, DrawQueue, ZIndex,
    components::StrokeSolid,
};
use xc_geometry::{Color32, Rect2D, Vec2};

struct ConsolePainter;
impl Painter2D for ConsolePainter {
    fn paint(&self, shape: &Shape, style: &Style) {
        println!("draw {:?} with {:?}", shape, style);
    }
}

let mut dq = DrawQueue::new();
dq.push(Shape::rect(Rect2D::from_min_max(
    Vec2::ZERO, Vec2::new(100.0, 50.0),
)), Style::stroke(StrokeSolid::new(2.0, Color32::RED)), ZIndex(0));

dq.sort_by_z_index();          // 稳定排序
dq.flush(&ConsolePainter);     // 遍历执行
```

## 使用示例（3D DrawQueue3D）

```rust
use xcal_render::{DrawQueue3D, MeshDraw, LineDraw};
use xcal_core::scene::primitives::wireframe_cube;
use xc_geometry::Color32;

let mut q3 = DrawQueue3D::new();
q3.push_mesh(MeshDraw { mesh: wireframe_cube(1.0, Color32::WHITE).0, transform: glam::Mat4::IDENTITY });
q3.push_line(LineDraw { from: glam::Vec3::ZERO, to: glam::Vec3::X, color: [255,0,0,255] });
assert_eq!(q3.mesh_count() + q3.line_count(), 2);
```

---

## 测试

```bash
cargo test -p xcal-render                      # ~40 个测试 + 3 doctest
cargo test -p xcal-render --no-default-features # 验证纯数据/队列无 wgpu 依赖时通过
cargo doc  -p xcal-render --no-deps --all-features
```

覆盖点：
- `DrawQueue`：稳定排序 (10_000 条乱序命令 + 多批次 merge 验证插入序)
- `DrawQueue3D`：Mesh/Line 双列表追加、清空、`take_snapshot`
- `Shape::rect/circle/path` 端点一致性 + `record_scene` 遍历 hecs World
- `GpuHandle` 跨线程可 Send（测试用 `std::sync::mpsc` 验证）

---

## 依赖

```toml
xcal-core     = { path = "../xcal-core", default-features = false }
xc-geometry = { path = "../xc-geometry" }
glam          = "0.33"
derive_more / derive-new / derive_builder
hecs          = "0.11"
thiserror     = "2.0"
wgpu          = "30"   (optional, feature = "wgpu")
```
