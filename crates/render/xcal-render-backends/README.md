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
              ├ wgpu 3D 后端 (feature = "wgpu")
              ├ wgpu 2D 后端 (feature = "wgpu")          ← S3-1：离屏 MSAA + 合成
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
| `egui-wgpu` | `egui` + `egui-wgpu` + `wgpu` + `pollster` + 同 `wgpu` + `xcal-core/egui` | `render_context`, `wgpu_3d`, `wgpu_2d`, `vertex_layout`, `eframe_backend`, `layer_callback` |
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

### 3. `layer_callback` (`egui-wgpu`)

`Layer3DCallback` 实现了 `egui_wgpu::CallbackTrait`，是 2D egui UI 与 3D wgpu
绘制**在同一主 render pass** 内合成的桥梁。注意：

- **不能使用 `Order::Background`**（会把 3D 画在 egui 后导致被遮盖）；
- 使用 `ui.painter_at(central_rect).add(Shape::Callback(...))` 在同一 layer
  内以较晚的 shape order 保证在 UI 之上、裁剪自动局限在中央视口。
- `WgpuBackend3D` 存放在 `rs.renderer.write().callback_resources` TypeMap
  中，回调以 `&self` 访问，回调内部通过内部可变性重建缓冲区。

### 4. `wgpu_2d` (`wgpu` / `egui-wgpu`) — S3-1

xcal-rs 的首个 GPU 2D 后端：**离屏纹理 + MSAA 4x + 合成**。

- **`WgpuBackend2D`**：`new(device, queue, target_format)` 建两条管线——
  `shaders/shape_2d.wgsl`（图元；硬边 + MSAA）与 `shaders/composite_2d.wgsl`
  （合成），后者按主目标 `is_srgb()` 选 `fs_linear` / `fs_gamma` 入口，
  sRGB 与非 sRGB 主目标都能启动而不 panic。
- 实现 `Painter2DTarget`（`begin_frame` / `draw_shape` / `end_frame`），
  另有直达录制 API `push(shape, transform, style, z)`。
- **每帧流程**（`finish_frame(encoder, geometry) -> Frame2DData`）：
  1. 录制项按 `z` 稳定排序（同 z 保录制序，画家算法）；
  2. 每项 `tessellate`（S3-0）→ 顶点哈希 → `ResourceCache<()>` 命中即复用、
     未命中则上传（`DeferredDestroyer` 延迟销毁）；
  3. uniform 经 **复用 `wgpu_3d` 的 `UniformArena`**（256B 对齐 dynamic offset，
     `UNIFORM_SIZE = 80`）写入；
  4. 在离屏 MSAA 纹理上绘制并 resolve。
- **离屏格式** `OFFSCREEN_FORMAT = Rgba8Unorm`（**非 sRGB**，存 gamma 值），
  `MSAA_SAMPLES = 4`；纹理开 `COPY_SRC` 支持像素回读。
- **合成**：`composite(frame, render_pass)` 用常量 `COMPOSITE_QUAD`（NDC
  `[-1,1]²` 全屏四边形）把离屏纹理贴回主 pass。
- **`Frame2DGeometry`**：`viewport_points` + `pixels_per_point` + `screen_size_px`；
  `viewport_px()` 复刻 `epaint::ViewportInPixels::from_points` 的取整 + 钳制口径，
  使**离屏纹理尺寸恒等于 egui-wgpu 为该回调设置的 render pass viewport 尺寸**。
- **`Layer2DCallback`**（`feature = "egui-wgpu"`）：实现 `CallbackTrait`，
  `prepare` 渲染离屏、`paint` 合成；经
  `ui.painter_at(rect).add(Shape::Callback(...))` 与 egui UI 同层提交。
- 导出：`WgpuBackend2D` / `Layer2DCallback` / `Frame2DGeometry` / `Frame2DData` /
  `Recorded2D` / `snapshot_queue` / `MSAA_SAMPLES` / `OFFSCREEN_FORMAT`。

> ⚠️ **egui-wgpu 回调 viewport 陷阱**：egui-wgpu 处理 `Primitive::Callback` 时会
> `set_viewport(callback.viewport_in_pixels())`（下一个 primitive 处才复位）。
> 即**回调内 NDC `[-1,1]²` 对应回调自身矩形**，而非整屏。因此合成四边形必须写死
> 全 NDC 盒（`COMPOSITE_QUAD`）；若按整屏尺寸把视口像素矩形换算成裁剪坐标，视口会被
> **二次应用**（宽度 × 视口宽/屏幕宽并右移），表现为 2D 视图在 X 方向被压扁。
> 回归测试 `composite_lands_inside_scoped_viewport_on_real_device` 在旧实现下必然失败。

### 5. `egui_2d` (`egui`)

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
3. `ui()` 每帧构造 `Layer3DCallback`，以 egui paint callback 方式注册；
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
cargo test -p xcal-render-backends --features egui-wgpu   # 36 个测试（3D/2D 离线渲染、合成、顶点布局翻译）
cargo test -p xcal-render-backends --no-default-features  # 空 crate（全部模块被 feature 门控关闭）
cargo check -p xcal-render-backends --features wgpu
cargo check -p xcal-render-backends --features egui
```

`--features egui-wgpu` 下的 2D 相关测试：`Frame2DGeometry::viewport_px` 取整/钳制口径、
离屏尺寸下限、局部变换与裁剪矩阵、`COMPOSITE_QUAD` 全 NDC 盒、合成在真实设备上
落在 scoped viewport 内（端到端）、`gpu_frame_rasterizes_and_recreates_targets_on_resize`
（离屏像素回读 + 视口 200×100 → 400×300 目标重建）、
`gpu_pipeline_builds_for_srgb_and_non_srgb_targets`、`pipeline_vertex_layout_comes_from_contract`。
