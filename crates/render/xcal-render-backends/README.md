# xcal-render-backends

> **具体渲染后端实现**：wgpu 3D、wgpu 2D（S3-1）、egui 2D、eframe 集成、顶点契约翻译层。
> **所有模块按 feature-gate 提供**，最终用户按需组合。
>
> （旧 `svg_recorder` / Typst / LaTeX 空壳已删除——内容管线改走
> `xc-prerender::engine → xc-content::ContentIR → xcal-render::ir_to_draw_cmds`。）

```
xcal-core / xcal-render / xc-geometry
              │
              ▼
    xcal-render-backends  ← 本 crate
              │
              ├ wgpu 3D 后端 (feature = "wgpu")          ← 图层离屏渲染（S3-2）
              ├ wgpu 2D 后端 (feature = "wgpu")          ← S3-1：离屏 MSAA；S3-2：透明清屏
              ├ 图层合成器 (feature = "wgpu")            ← S3-2：N 层按声明顺序混合
              ├ 顶点契约 → wgpu 翻译 (feature = "wgpu")
              ├ egui 2D 后端 (feature = "egui")
              └ eframe + egui_wgpu 集成 (feature = "egui-wgpu")
```

---

## Feature 矩阵

| feature | 引入的依赖 | 可用模块 |
|---------|-----------|----------|
| `default` (= `wgpu`) | 同 `wgpu` | 同 `wgpu`（默认开启以便 `cargo test -p xcal-render-backends` 直接跑顶点契约测试） |
| `wgpu` | `wgpu 30`, `pollster 0.3`, `xcal-render/wgpu` | `render_context`, `wgpu_3d`, `wgpu_2d`, `vertex_layout` |
| `egui` | `egui 0.36`, `xcal-core/egui` | `egui_2d` (`EguiPainter`, `color_to_egui`) |
| `egui-wgpu` | `egui` + `egui-wgpu` + `wgpu` + `pollster` + 同 `wgpu` + `xcal-core/egui` | `render_context`, `wgpu_3d`, `wgpu_2d`, `vertex_layout`, `compositor`, `eframe_backend`, `scene_callback` |
| `text` | — | 预留空 feature：Cargo.toml 保留声明，`src/` 无对应模块（旧 Typst/LaTeX 空壳已删除） |

四种 feature 组合都通过 CI 的 `cargo check -F` 验证。

---

## 关键模块

### 1. `render_context` (`wgpu` / `egui-wgpu`)

`RenderContext` 是跨阶段共享的 GPU 句柄三件套（`Arc<Device> + Arc<Queue> +
TextureFormat`），实现了 `Send + Sync + Clone`，可作为 Resource 放入
`xcal_core::Resources` 供任何 System 取用：

```rust
use xcal_render_backends::RenderContext;

let rs: &egui_wgpu::RenderState = /* from eframe CreationContext */;
let ctx = RenderContext::from_render_state(rs);
```

同时导出 `DEFAULT_TARGET_FORMAT = wgpu::TextureFormat::Bgra8UnormSrgb`，
与 eframe/wgpu 默认格式一致。

### 2. `wgpu_3d` (`wgpu` / `egui-wgpu`)

**WgpuBackend3D**：xcal-rs 的官方 3D MVE 后端。

- **双渲染管线**：
  - `triangle list` 管线：实心网格、solid_cube
  - `line list` 管线：wireframe_cube、坐标轴
- **Uniform 绑定**：`ViewProj (Mat4) + TintColor (Vec4)` 每实例一提交，使用
  `wgpu 30` 新 API（`draw(0..n, 0..1)` 范围语法）。
- **每帧流程**：
  1. `prepare_draws(&device, &queue, camera, aspect, &draw_queue_3d) -> Frame3DData`
     重算 VP 矩阵、重建 vertex/uniform buffer，生成 Vec\<PreparedDraw3D\>
  2. `record_prepared(&mut rpass, &frame_data, viewport_scissor)` 提交 draw
- **Cube WGSL Shader**：位于 `shaders/cube.wgsl`（48 行，单 VS + FS，顶点位置 `vec3<f32>` + 颜色 `vec4<u8>` unpack）。
- 兼容 `glam 0.33`：使用
  `glam::camera::rh::proj::directx::perspective` / `orthographic` 与
  `glam::camera::rh::view::look_at_mat4`（移除旧 `perspective_rh` /
  `look_at_rh`）。

### 3. `scene_callback` (`egui-wgpu`) — S3-2 修复

`SceneCompositorCallback` 实现 `egui_wgpu::CallbackTrait`，是**唯一**的 egui
绘制回调：它一次性渲染并合成**本帧全部图层**（2D / 3D），是场景与 egui UI
**在同一主 render pass** 内合成的桥梁。

> **为什么必须合并为一个回调**：`egui_wgpu::CallbackResources` 是**全局共享**的
> `TypeMap`（`HashMap<TypeId, _>`），**不是 per-callback 的**。早期"每层一个回调"
> 的设计让两个回调各自 `insert::<CompositeFrame>()` —— 后插入覆盖先插入，两个
> `paint()` 拿到**同一帧**，于是把同一张离屏纹理画两遍（现象：顺序 A 只见 3D、
> 顺序 B 只见 2D）。合成本就是"一次性消费全部图层"的操作，结构上不该每层一个回调。

- `SceneCompositorCallback::new(viewport_points, layers, queue_2d, queue_3d)`：
  `layers` 是**有效序**（翻转开关已生效）的 `Vec<SceneLayer { id, kind, blend, opacity }>`；
  仅快照**有效序里真的存在**的那一类队列（`gpu_2d = false` 时白拷 2D 几何无意义）。
- `prepare`：逐层渲染离屏（2D → `WgpuBackend2D`，3D → `WgpuBackend3D::render_layer_offscreen`）
  → 按**声明序**收集全部 `CompositeOp` → **一次** `Compositor::prepare` → 单个
  `CompositeFrame`，与各层 uniform arena buffer（保活）一起存入 TypeMap 的
  `SceneCompositorFrame`（合成条目**只有一处**）。
- `paint`：设 scissor → `Compositor::composite_layers`（按声明顺序混合，含 opacity）。
- **不使用 `Order::Background`**；用 `ui.painter_at(central_rect).add(Shape::Callback(...))`
  与 egui 同层提交，回调落在有效序中**首个 GPU 图层**的位置（CPU 2D 层就地回放）。
- `WgpuBackend2D` / `WgpuBackend3D` / `Compositor` 存放在
  `rs.renderer.write().callback_resources` TypeMap 中，回调以 `&self` 访问。
- **本阶段容量限制**：2D/3D 后端各只有**一份**离屏目标 → 每类最多一个图层
  （`prepare` 中有 `debug_assert`）；per-`LayerId` 目标属 S3-3。

### 4. `wgpu_2d` (`wgpu` / `egui-wgpu`) — S3-1 / S3-2

xcal-rs 的首个 GPU 2D 后端：**离屏纹理 + MSAA 4x**；合成自 S3-2 起外移到
`compositor`（本文件不再自带合成管线）。

- **`WgpuBackend2D`**：`new(device, queue)` 只建一条管线
  `shaders/shape_2d.wgsl`（图元；硬边 + MSAA）。合成管线、绑定布局、采样器与
  `composite()` 已移除（统一由 §6 `Compositor` 承担）。
- 实现 `Painter2DTarget`（`begin_frame` / `draw_shape` / `end_frame`），
  另有直达录制 API `push(shape, transform, style, z)`。
- **每帧流程**（`finish_frame(encoder, geometry) -> Frame2DData`）：
  1. 录制项按 `z` 稳定排序（同 z 保录制序，画家算法）；
  2. 每项 `tessellate`（S3-0）→ 顶点哈希 → `ResourceCache<()>` 命中即复用、
     miss 则建缓冲（帧末 `evict_unused(0)` + deferred destroy，有界）；
  3. uniform 经 `UniformArena`（复用 `wgpu_3d`，256B 对齐 dynamic offset）；
     宽度由 CPU 侧唯一换算点 `draw.rs::resolve_style` 解析，**shader 不做空间换算**；
     几何本身与宽度无关（`position` 槽存 lyon `position_on_path()`），
     故同一路径换宽度只需换 uniform；
  4. 在离屏 MSAA 纹理上绘制并 resolve；**离屏清屏透明**（S3-2/C2，背景由 host 提供）。
- **离屏格式** `OFFSCREEN_FORMAT = Rgba8Unorm`（**非 sRGB**，存 gamma 值），
  `MSAA_SAMPLES = 4`；纹理开 `COPY_SRC` 支持像素回读。
- **`Frame2DData`** = `{ offscreen, draw_count, _arena_buffer }`
  （不再含 quad / 合成绑定组）。
- **`Frame2DGeometry`**：`viewport_points` + `pixels_per_point` + `screen_size_px`；
  `viewport_px()` 复刻 `epaint::ViewportInPixels::from_points` 的取整 + 钳制口径，
  使**离屏纹理尺寸恒等于 egui-wgpu 为该回调设置的 render pass viewport 尺寸**。
- **`WgpuBackend2D`**（`feature = "wgpu"` / `"egui-wgpu"`）的 egui 回调职责已并入
  §3 `scene_callback`（`SceneCompositorCallback`）；本模块只提供渲染 API。
- 导出：`WgpuBackend2D` / `Frame2DGeometry` / `Frame2DData` /
  `Recorded2D` / `snapshot_queue` / `MSAA_SAMPLES` / `OFFSCREEN_FORMAT`。

> ⚠️ **egui-wgpu 回调 viewport 陷阱**：egui-wgpu 处理 `Primitive::Callback` 时会
> `set_viewport(callback.viewport_in_pixels())`（下一个 primitive 处才复位）。
> 即**回调内 NDC `[-1,1]²` 对应回调自身矩形**，而非整屏。因此合成四边形必须写死
> 全 NDC 盒（`compositor::COMPOSITE_QUAD`）；若按整屏尺寸把视口像素矩形换算成裁剪坐标，
> 视口会被**二次应用**（宽度 × 视口宽/屏幕宽并右移），表现为 2D 视图在 X 方向被压扁。
> 回归测试 `composite_lands_inside_scoped_viewport_on_real_device` 在旧实现下必然失败。

### 6. `compositor` (`wgpu` / `egui-wgpu`) — S3-2

**图层合成器**：每个图层（2D / 3D）渲染到自己的离屏纹理，由本模块按
`SceneView::layers` 的**索引顺序**（先 = 下）混合进主 pass —— 于是 2D / 3D
可任意叠放（此前 3D 硬编码在 2D 之上），并为 S3-3（互为纹理）铺路。

```rust
let frame = compositor.prepare(&device, &[CompositeOp { texture, src_format,
                                    blend, opacity, rect_px }, /* ... */]);
compositor.composite_layers(&mut render_pass, &frame);   // 层序 = 输入序，不重排
```

- `CompositeOp { texture, src_format, blend, opacity, rect_px }`；
  `CompositeFrame` 提供 `len` / `is_empty` / `blend_at(i)` / `conversion_at(i)`。
- `opacity` 走 uniform（x 分量乘进源色），不走 blend constant（部分后端受限）；
  `LayerDescriptor::clamped_opacity()` 是唯一取值口径。
- **无动态 blend / 无动态入口点** → 预建 9 条管线
  `HashMap<(LayerBlend, Conversion), RenderPipeline>`（3 blend × 3 conversion）。
- 混合（预乘 alpha）：`Opaque` = REPLACE；`Alpha` = 色/alpha 均 `One / OneMinusSrcAlpha`；
  `Additive` = `One / One`。
- **sRGB 域转换**：`textureSample` 对 sRGB 格式纹理**自动解码**，故按
  (源 sRGB-ness, 目标 sRGB-ness) 选入口 —— 同域 `fs_identity`、
  源非 sRGB → sRGB 目标 `fs_gamma_to_linear`、源 sRGB → 非 sRGB 目标 `fs_linear_to_gamma`
  （`Conversion::for_formats`）。3D 离屏沿用主目标格式（源 = 目标 → Identity）；
  2D 离屏为 `Rgba8Unorm`（存 gamma）。
- 导出：`Compositor` / `CompositeOp` / `CompositeFrame`。

### 7. `egui_2d` (`egui`)

`EguiPainter`：xcal-render 统一 2D 管线 (`Painter2D`) 的 egui 实现。
按 `Shape2D` 的 `Circle / Line / Rect / Polyline / Polygon / Ellipse / Path`
分支，把 `fill` / `stroke` **独立**翻译成 egui 原生 `Shape::*`
（`Path` 经 `FLATTEN_TOLERANCE = 0.1` 展平）。同时提供
`color_to_egui(xc_geometry::Color32) -> egui::Color32` 辅助函数。
由 xcal-egui 直接 re-export。

### 6. `eframe_backend` (`egui-wgpu`)

`EframeWgpuBackend`：实现 `xcal_render::RenderBackend` trait 的 eframe 版本。
对写纯 eframe/wgpu 示例时可直接复用。

### 7. `vertex_layout` (`wgpu` / `egui-wgpu`)

顶点格式契约（`xcal-render::vertex`）→ wgpu 的翻译层：`to_wgpu(layout, &mut attrs)`
把 `VertexLayout` 转成 `wgpu::VertexBufferLayout`（调用方提供属性数组存储）；
多 buffer 用 `to_wgpu_buffers(&VertexInputDesc, &mut …)`，shader location 跨 buffer
全局连续（wgpu 语义）。入口先 `validate()` fail-fast。**不碰 `wgpu_3d.rs`。**

---

## 示例：独立 3D demo

`examples/cube_3d.rs` 展示了一条**不依赖 xcal-app / scheduler** 的极简 3D
渲染路径，便于单独测试 WgpuBackend3D：

```bash
cargo run -p xcal-render-backends --example cube_3d --features egui-wgpu
```

流程：
1. `eframe::run_native` 启动应用；
2. 在 `CreationContext` 中创建 `WgpuBackend3D`，塞入 `callback_resources`；
3. `ui()` 每帧构造 `SceneCompositorCallback`（含 `SceneLayer` 声明），以 egui paint callback 方式注册；
4. egui-wgpu 主 render pass 调 `prepare()` + `paint()`，线框立方体被渲染。

---

## 依赖

```toml
xcal-core              = { default-features = false }
xcal-render            = { default-features = false }
xc-geometry
glam
hecs
thiserror
pollster               (optional)  ← wgpu  / egui-wgpu
wgpu                   (optional)  ← wgpu  / egui-wgpu
egui                   (optional)  ← egui  / egui-wgpu
egui-wgpu              (optional)  ← egui-wgpu
```

## 测试

```bash
cargo test -p xcal-render-backends --features egui-wgpu   # 3D/2D 离屏渲染、图层合成、顶点布局翻译
cargo test -p xcal-render-backends --no-default-features  # 空 crate（全部模块被 feature 门控关闭）
cargo check -p xcal-render-backends --features wgpu
cargo check -p xcal-render-backends --features egui
```

`--features egui-wgpu` 下的 2D 相关测试：`Frame2DGeometry::viewport_px` 取整/钳制口径、
离屏尺寸下限、局部变换与裁剪矩阵、`gpu_frame_rasterizes_and_recreates_targets_on_resize`
（离屏像素回读 + 视口 200×100 → 400×300 目标重建）、
`gpu_pipeline_builds_for_srgb_and_non_srgb_targets`、`pipeline_vertex_layout_comes_from_contract`。

`compositor` 测试（真实设备门控，无适配器时跳过）：
`blend_state_matches_declared_semantics`、`composite_quad_is_full_ndc_box_with_uv`、
`quad_bytes_are_twenty_per_vertex`、`composite_quad_fills_callback_viewport_at_any_position`、
`conversion_for_formats_matches_srgb_domains`（纯 CPU）、
`gpu_composite_respects_declaration_order`（红 + 蓝 ×0.5 → (128,0,128)；反序 → 纯红）、
`gpu_composite_preserves_op_order_with_distinct_blends`、
`gpu_composite_opacity_zero_invisible_one_opaque`、
`gpu_composite_builds_all_three_blend_pipelines`（sRGB / 非 sRGB 目标各建一次）。

3D 侧新增 `test_render_layer_offscreen_clears_transparent`（C3：空队列 → 离屏全 `[0,0,0,0]`）。
