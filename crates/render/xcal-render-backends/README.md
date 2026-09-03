# xcal-render-backends

> **具体渲染后端实现**：wgpu 3D、egui 2D、SVG 录制、eframe 集成、Typst/LaTeX
> 文本引擎占位。**所有模块按 feature-gate 提供**，最终用户按需组合。

```
xcal-core / xcal-render / xc-geometry
              │
              ▼
    xcal-render-backends  ← 本 crate
              │
              ├ wgpu 后端 (feature = "wgpu")
              ├ egui 2D 后端 (feature = "egui")
              ├ eframe + egui_wgpu 集成 (feature = "egui-wgpu")
              ├ SVG 录制 (always)
              └ text 引擎占位 (feature = "text")
```

---

## Feature 矩阵

| feature | 引入的依赖 | 可用模块 |
|---------|-----------|----------|
| `default` (空) | 无 | `svg_recorder`, `text::{mod, typst_engine, latex_engine}` (空壳) |
| `wgpu` | `wgpu 30`, `pollster 0.3`, `xcal-render/wgpu` | `render_context`, `wgpu_3d` |
| `egui` | `egui 0.36`, `xcal-core/egui` | `egui_2d` (`EguiPainter`, `color_to_egui`) |
| `egui-wgpu` | `egui` + `egui-wgpu` + `wgpu` + `pollster` + 同 `wgpu` + `xcal-core/egui` | `render_context`, `wgpu_3d`, `eframe_backend`, `layer_callback` |
| `text` | — | 占位文本引擎预留接口 |

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

### 4. `egui_2d` (`egui`)

`EguiPainter`：xcal-render 冻结 2D 管线 (`Painter2D`) 的 egui 实现。
把 `Shape::{Line, Rect, Circle, Path, Text, Callback}` 翻译成 egui 原生
`Shape::*`。同时提供 `color_to_egui(xc_geometry::Color32) -> egui::Color32`
辅助函数。由 xcal-egui 直接 re-export。

### 5. `eframe_backend` (`egui-wgpu`)

`EframeWgpuBackend`：实现 `xcal_render::RenderBackend` trait 的 eframe 版本。
对写纯 eframe/wgpu 示例时可直接复用。

### 6. `svg_recorder`（始终编译）

空壳占位后端。保留给未来：把 `DrawQueue2D` 的 `Shape2D` 序列化为 `.svg` 文件，
用于无头导出。

### 7. `text`（始终编译，`feature="text"` 预留）

- `typst_engine.rs`：Typst 数学公式 → 位图
- `latex_engine.rs`：LaTeX/DviSvg → 位图
两者目前仅保留模块骨架（不做实际调用），避免在默认 CI 引入系统级 texlive 依赖。

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
cargo test -p xcal-render-backends --features egui-wgpu   # ~12 个测试（含 3D 离线渲染）
cargo test -p xcal-render-backends --no-default-features  # 仅 SVG + text 空壳
cargo check -p xcal-render-backends --features wgpu
cargo check -p xcal-render-backends --features egui
```
