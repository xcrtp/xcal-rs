# zela-render

> **渲染 trait + 调度 + 资源管理层**。位于三层架构的中间层：
>
> ```text
> ze-notion / zela-scene  ←── zela-render ──→  zela-render-backends
> ```
>
> 定义 2D 与 3D 的绘制器 trait、绘制队列、视口/渲染目标/渲染层抽象与新图元体系。
> **无 egui 依赖，B3-0.5e 起无 wgpu 依赖**（渲染无关层：纯数据 / 队列 / trait / 顶点生产）。
> GPU 侧契约（`RenderTarget` / `RenderLayer` / `RenderBackend` / `ResourceCache`）已迁入
> `zela-render-backends`；契约纯数据（顶点格式 `ze-vertex`、资源缓存 `ze-resource`）在 kernel。

---

## 管线概览（2D 统一 + 3D 新）

zela-render 同时承载两套渲染抽象：

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
  hecs 侧经 `zela-scene` 的 `WorldView` 只读适配器接入。层级（M4）：对象上
  **可选**的 `ZOrder` 组件优先（网格 z = -100 / 坐标轴 z = -90），缺席用默认层（0）。
  旧「逐组件穷举宏」已删除 ⇒
  **新增图元只需实现 `ToShape2D`，不必改动录制逻辑**。
- **对象级可见性与不透明度**（N3–N5）：录制入口除**签名冻结**的
  `record_scene`（= 不过滤形态）外，另有 `record_scene_tagged(scene, camera,
  camera_tag: Option<ObjectId>, groups: &GroupVisibility, queue)` —— 它承载
  `VisibilityTag::Camera` 判定所需的相机 `ObjectId`（`record_scene` 只有 `&Camera2D`，
  **拿不到 id**，故拆函数而不是改冻结签名）。过滤与不透明度施加都在 `record_group`
  闭包内：闭包首参即 `ObjectId` ⇒ 用 `scene.with::<Visibility, _>(id, ..)` /
  `with::<Opacity, _>(id, ..)` **点查询**（契约层不必新增访客）。
  缺席语义 = 默认值：`Visibility` 缺席 = 全局可见、`Opacity` 缺席 = 1.0，
  故**未挂组件的对象画面逐位不变**。施加点唯一：`apply_opacity`（**只缩 `Style2D`
  的直色 alpha**；预乘在 GPU 侧发生，`rgb` 不能再乘一次 —— 否则会偏暗一档）。
- **描边宽度的度量空间**（`StrokeSolid.space: MeasureSpace`，默认 `Screen`）：
  `record_scene` 是**唯一换算点**——按 `camera.scale` 用 `ze_notion::resolve_width`
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
契约（[`VertexLayout::standard`]，R0 后 stride = 24，四槽
position/attrib0/attrib1/attrib2）：
- `Vertex2D { position, normal, side, advance }`（`#[repr(C)]`，`size == 24` 编译期断言）。
  **描边几何参数化**：`position` 槽存 lyon `position_on_path()`（路径点，**不含宽度**），
  轮廓点由后端 shader 现场补偿 `position + normal * (stroke_width * 0.5)`
  （`normal` 保持 lyon 原样、含 miter 缩放，**不得归一化**）。因此几何与
  `stroke_width` / `camera.scale` **无关**；填充顶点 `normal = [0,0]` → 偏移恒 0。
  宽度仍由 CPU 侧唯一换算点 `draw.rs::resolve_style` 解析（shader 只做几何偏移）
- `advance`（**R0 书写动画**）：lyon `advancement()` 归一化到 `[0,1]`（除以本条几何的
  实测最大推进量）。`lyon` 在子路径起点沿用上一条的推进量 ⇒ **跨子路径累加**，
  故书写顺序是"轮廓依次写出"（`reveal_advance_accumulates_across_subpaths` 锁死）
- `Tessellation2D { vertices, indices, layout, kind: Fill | Stroke }`
- `tessellate(shape, transform, style, tolerance) -> Result<Vec<Tessellation2D>, TessellateError>`
  （`Group` 递归展开；`Empty` 返回 `Ok(空 Vec)`；`Custom` 未注册规格 / 规格无几何 ⇒ `Err`）
- `tessellate_with(shape, transform, style, tolerance, &CustomSpecs) -> Result<…>`
  （带**具名规格表**的版本；`Custom` 的 `SpecId` 按名字精确查找）
- `tessellate_reveal(path, transform, style, tolerance, step) -> Option<Tessellation2D>`
  （**R0**：进 lyon 之前按 `step`（`REVEAL_STEP_PX = 2.0` 输出空间 px）**预细分** ——
  lyon 不细分直线段，不补点则切口精度 = 顶点间距；只做描边，填充无 `advancement`）
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
- `RenderLayer` / `RenderTarget` / `RenderBackend / RenderBackend3D`：**已迁入**
  `zela-render-backends`（B3-0.5e；GPU 侧契约与 wgpu 类型同处一个 crate）
- `Viewport`：纯数据 `(origin, size)`，无 wgpu 依赖
- `ResourceCache<T>` / `GpuResourceCache`：generation-aware slot + key→id 索引的资源缓存
  （定义在 `ze-resource`；后端侧 shim 在 `zela-render-backends::resource_cache`）
- `GpuHandle`：纯 `Any` 包装，**无 wgpu 依赖**，任何上层都可持有（已下沉 `ze-resource`）

---

## Features

**无 feature 开关**（`default = []`，且没有可选 feature）：本 crate 恒为渲染无关层。

**典型组合**：
- `zela-render` — 用于 zela-app / zela-egui / zela-render-backends
  （`Painter2D` / `DrawQueue` / `Shape2D` / `Style2D` / `DrawQueue3D` / `tessellate` /
  `vertex_record` 等；不含任何 wgpu 类型）

---

## 模块速览

| 模块 | 说明 |
|------|------|
| `components` | 2D 管线的组件统一出口（**纯转发**，无自有组件）：`ze-notion` 的数学元语（`FunctionX2Y, Sampler1D/2D, ParameterCurve*, Interpolator, DynFn`）、`ze-object` 的图元与摆放（`Circle, Line, Path, Rect, Transform`）、`ze-render-ir` 的样式组件（`StrokeSolid, FillSolid, StrokeStyle, FillStyle`）、`zela-scene` 的纯逻辑组件（`PhaseAnim, marker, mesh, PathMesh, …`） |
| `draw` | `Painter2D + DrawQueue + DrawCommand + Shape2D/Style2D + Drawable + record_drawables + record_scene + **record_scene_tagged（N5）** + push_morph_item + push_reveal_item + **visible_for / opacity_of / apply_opacity（N5）**`（统一 2D 管线；`record_scene<S: SceneRead>` 不认识 hecs，层级读对象上的 `ZOrder`，可见性读对象上的 `Visibility`） |
| `record_systems` | P4 从 `zela-egui` 归位的录制 System：`SceneRecordSystem` / `MorphRecordSystem` / `ContentTextureRecordSystem` + **R0 新增 `RevealRecordSystem`**（**渲染动作住渲染层**；只读 `ViewRect` + `Camera2DHandle` + **N5 的 `GroupVisibility`**，不读 UI 类型）。**N5**：三个 2D 录制 System 都消费对象级 `Visibility` / `Opacity`（相机 tag 取自 `Camera2DHandle`）；`ContentTextureRecordSystem` 是纹理源、**本轮不参与过滤**（记账） |
| `view_rect` | `ViewRect`（P4 下沉）：本帧 2D 渲染目标矩形；相机在录制时绑定它得到 `Camera2D`（P1-2 解绑） |
| `filters` | **C 线（纯数据）**：`PostFilterChain`（Resource；顺序即应用顺序、最多 `MAX_FILTER_SLOTS = 4` 个）+ `FilterSlot` + `FilterKind`（`code()` 与 WGSL 常量表一一对应）+ 160 B uniform 编码；**空链 ⇒ 渲染层走直通、画面逐位不变** |
| `tessellate` | S3-0 lyon 三角化适配层：`Vertex2D` / `Tessellation2D` / `tessellate` / `tessellate_with` / `TessellateError` / **`tessellate_reveal`（R0 书写动画）** / `REVEAL_STEP_PX` / `tolerance_for_scale`（纯 CPU，无 feature 门控） |
| `draw_queue_3d` | 3D 双列表队列（`DrawQueue3D` / `MeshDraw` / `LineDraw`）|
| `painter_2d` / `painter_3d` | 绘制器 trait 定义（`Painter2DTarget` / `Painter3D`） |
| `shape` | `Shape2D` + `Style2D` + `CustomShapeData`/`SpecId`/`CustomSpec`/`CustomSpecs`（自定义图元按**具名规格**解析） |
| `viewport` | `Viewport`（纯数据） |
| `render_backend`（导出项） | 根导出 `GpuHandle / GpuResourceId / AccessError`（kernel 来源）；`RenderBackend / RenderBackend3D` 已迁入 `zela-render-backends` |
| `scene` (mod) | `use zela_render::scene::{comp, Scene}` 重新导出 components + zela_scene Scene |
| `content_ir` | `ir_to_draw_cmds`：ze-content `ContentIR` → `Shape2D + Style2D + Transform2D`（scale = target / nominal_font_size，描边宽度同步换算为世界单位并标 `MeasureSpace::World`） |
| `vertex` | 顶点格式契约（已下沉 kernel `ze-vertex`，经 `pub use ze_vertex as vertex;` 保持路径）：`VertexLayout` / `VertexAttrib` / `AaModel` / `CacheStrategy` / `StaticConsistencyChecker`；需 `serde` 可序列化 |

---

## 使用示例（2D 管线 + 自定义 Painter2D）

```rust
use zela_render::{DrawCommand, DrawQueue, Painter2D, Shape2D, Style2D, ZIndex};
use ze_notion::{Color32, Rect2D, Vec2};

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
use zela_render::{tessellate, tolerance_for_scale, Style2D, Shape2D};
use ze_notion::{Color32, Rect2D, Transform2D, Vec2};

let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(100.0, 50.0)));
let style = Style2D { fill: Some(Color32::GREEN), stroke: None, ..Default::default() };

let items = tessellate(&shape, &Transform2D::identity(), &style, tolerance_for_scale(40.0))
    .expect("内置图元不会失败");
assert!(items.iter().all(|t| t.layout == zela_render::tessellation_layout()));
```

> `Transform2D::identity()` 的实际构造以 `ze-notion` 为准（示例仅示意流程）。

## 使用示例（3D DrawQueue3D）

```rust
use std::sync::Arc;
use zela_render::DrawQueue3D;
use zela_scene::scene::components::Material3D;
use zela_scene::scene::primitives::wireframe_cube;
use ze_notion::Color32;

let white = Color32::new(255, 255, 255, 255);
let mut q3 = DrawQueue3D::new();
q3.draw_mesh(Arc::new(wireframe_cube(1.0)), ze_notion::Mat4::IDENTITY, Material3D::color(white));
q3.draw_line(ze_notion::Vec3::ZERO, ze_notion::Vec3::X, Color32::new(255, 0, 0, 255));
assert_eq!(q3.mesh_count() + q3.line_count(), 2);
```

---

## 软失败口径（三角化）

**结论：软失败被接受，但必须可观测。**

| 情形 | 行为 |
|---|---|
| 内置图元三角化失败 | 不适用（不会失败） |
| `Custom` 的 `SpecId` 未注册 / 规格无几何 | `tessellate*` 返回 `Err(TessellateError)` |
| 后端收到 `Err` | **记录命名原因 → 跳过该绘制 → 继续渲染**；不中断整帧、不改变退出码 |
| 事后发现 | 每帧末打印 `本帧 N 个绘制被跳过`；`zela-render` 退出前打印累计值 |

口径写死的理由：单个绘制失败不应让整段导出失败（尤其离线批量），但旧行为
"静默空几何"不可接受。计数与命名原因让失败**可枚举**。

`Bundle::check_restorable` 是 **bundle 路径的前置硬闸**；`--script` 路径没有这道闸，
靠上述计数发现。**本轮不改硬失败**；若将来要改，收敛点是
`zela_render_backends::diagnostics::record_skipped_draw` 的调用处。

---

## 测试

```bash
cargo test -p zela-render                      # 本 crate 无 feature 开关、不含 wgpu 代码，一次命令即全量
cargo doc  -p zela-render --no-deps
```

覆盖点：
- `DrawQueue`：稳定排序（同 `ZIndex` 保插入序）、`merge` 多批次归并、
  `execute` 顺序验证
- `DrawQueue3D`：Mesh/Line 双列表追加、清空、`mesh_count` / `line_count`
- `Shape2D`（rect / circle / path / line）端点一致性 + `record_scene` 经契约层 `SceneRead`
  （测试用**非 ECS 的 `TestScene`** 驱动，证明"可替换"不是纸面承诺）+ `ZOrder` 层级优先；
  闭合路径以 `PathCommand::Close` 结尾（M0 方案 b）
- `tessellate`：`Vertex2D` size == 24 布局契约、`side ∈ {±1}`、
  填充面积 / 描边环面积、Group 展开、
  圆近似误差 ≤ tolerance、`tolerance_for_scale` 随 scale 下降并钳制、
  Custom 具名规格（`to_triangles` / `to_path`）、**未注册 / 无几何 ⇒ 显式 `TessellateError`**
- 描边几何参数化（R1）门卫：`stroke_geometry_is_independent_of_width`
  （同路径宽度 2/8 → 顶点数 / index 表 / `position` / `side` 相同，`normal` 只差
  f32 末位）、`fill_vertices_are_unaffected_by_offset`（任意宽度下偏移恒 0）、
  `shader_offset_reproduces_lyon_contour_points`（`position + normal * w/2`
  与旧管线 lyon `position()` 逐顶点 < 1e-6，即"与旧 CPU 管线同源"的数值证明）、
  `stroke_area_scales_linearly_with_width`（宽 2→8 面积比 = 4）
- `resource_cache`（shim 层测试在 `zela-render-backends`）：`ResourceKey` 版本隔离、`get_or_insert` 命中/未命中、
  `mark_used` 保活、`evict_unused`、脏标记与 `upload_dirty`、stale id 返回 `AccessError`
  （`ResourceCache`/`GpuHandle` 主体与 pending 有界性压测在 `ze-resource`）
- `GpuHandle` 不再要求 `Send + Sync`（wasm 上 wgpu 资源含 `Rc`）；native 的
  egui TypeMap 边界由后端类型上的显式徽章局部承担

---

## 依赖

```toml
zela-scene        = { path = "../../scene/zela-scene", default-features = false }
zela-schedule     = { path = "../../schedule/zela-schedule" }    # P4：录制 System 需要 System / ExecutorContext
ze-notion         = { path = "../../kernel/ze-notion" }
ze-render-ir      = { path = "../../kernel/ze-render-ir" }       # Shape2D / Style2D / 样式组件 / 降级 trait
ze-object         = { path = "../../object/ze-object" }          # 图元 newtype / Camera2D / ZOrder
ze-scene-contract = { path = "../../kernel/ze-scene-contract" }  # record_scene 的唯一入参抽象（SceneRead）
ze-content        = { path = "../../kernel/ze-content" }         # content_ir 消费侧：ir_to_draw_cmds
ze-vertex         = { path = "../../kernel/ze-vertex" }          # 顶点格式契约（已下沉 kernel）
ze-resource       = { path = "../../kernel/ze-resource" }        # 资源缓存 / GpuHandle（已下沉 kernel）
serde             = { workspace = true }                        # VertexLayout 可序列化（pipeline 变体缓存 key）
derive_more / derive-new / derive_builder
thiserror         = "2.0"
lyon_tessellation = "1.0"   # S3-0 三角化（不自研）
lyon_path         = "1.0"
# 无 wgpu：B3-0.5e 起 GPU 侧契约在 zela-render-backends
```

> **无 hecs（库本体）**（M3-7）：`record_scene<S: SceneRead>` 与 P4 归位的三个录制 System
> 都只吃契约层抽象（`WorldView` 是 hecs 侧适配器，在 `zela-scene`）——
> `Cargo.toml` 的 `[dependencies]` **没有 hecs**。
> ⚠️ P4 新增 `[dev-dependencies] hecs`（**仅测试**）：驱动 `Scheduler` 的容器就是
> `hecs::World`，录制 System 的用例无法绕开。库本体的 hecs 零引用不变。
> （树中一直会出现传递的 hecs，因为本 crate 依赖 `zela-scene` —— 那是 ECS 实现层。）
