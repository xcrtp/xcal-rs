# xcal-render-backends

> **具体渲染后端实现**：wgpu 3D、wgpu 2D（S3-1，**唯一 2D 路径**）、eframe 集成、顶点契约翻译层。
> **所有模块按 feature-gate 提供**，最终用户按需组合。
>
> （旧 `svg_recorder` / Typst / LaTeX 空壳已删除——内容管线改走
> `xc-prerender::engine → xc-content::ContentIR → xcal-render::ir_to_draw_cmds`。）

```
xcal-scene / xcal-render / xc-notion
              │
              ▼
    xcal-render-backends  ← 本 crate
              │
              ├ wgpu 3D 后端 (feature = "wgpu")          ← 图层离屏渲染（S3-2）
              ├ wgpu 2D 后端 (feature = "wgpu")          ← S3-1：离屏 MSAA；S3-2：透明清屏
              ├ 图层合成器 (feature = "wgpu")            ← S3-2：N 层按声明顺序混合
              ├ 顶点契约 → wgpu 翻译 (feature = "wgpu")
              └ eframe + egui_wgpu 集成 (feature = "eframe")
```

---

## Feature 矩阵

| feature | 引入的依赖 | 可用模块 |
|---------|-----------|----------|
| `default` (= `wgpu`) | 同 `wgpu` | 同 `wgpu`（默认开启以便 `cargo test -p xcal-render-backends` 直接跑顶点契约测试） |
| `wgpu` | `wgpu 30`, `pollster 0.3`, `xcal-render/wgpu` | `render_context`, `wgpu_3d`, `wgpu_2d`, `vertex_layout`, `compositor`, `standard_backends`, `scene_callback` |
| `egui-wgpu` | `egui` + `egui-wgpu` + `wgpu` + `pollster` + 同 `wgpu` | 同 `wgpu`（`scene_callback` 额外提供 `CallbackTrait` 实现） |
| `eframe` | 同 `egui-wgpu` | 同 `egui-wgpu` + `eframe_backend`（`EframeWgpuBackend` / `install_standard_backends`）。**默认关闭**：浏览器产物不带 egui 链 |
| `text` | — | 预留空 feature：Cargo.toml 保留声明，`src/` 无对应模块（旧 Typst/LaTeX 空壳已删除） |

> **S4.1**：旧 CPU 2D 回放的 feature（含其历史别名）与 `egui_2d` 模块已删除——
> 2D 统一走 `wgpu_2d`，不再有逐变体手写的 egui 回放路径。

---

## 关键模块

### 1. `render_context` (`wgpu` / `egui-wgpu`)

`RenderContext` 是跨阶段共享的 GPU 句柄三件套（`Arc<Device> + Arc<Queue> +
TextureFormat`），实现了 `Send + Sync + Clone`，可作为 Resource 放入
`xcal_schedule::Resources` 供任何 System 取用：

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
  - `triangle list` 管线：实心网格、solid_cube、参数曲面
  - `line list` 管线：wireframe_cube、坐标轴
  - 两条管线均带 `Depth32Float` 深度测试（`Less`，写入开）——RC.3。
- **材质绑定**（RC.1）：bind group 3 entry = uniform（dynamic offset）+
  `texture_2d<f32>` + sampler；着色式为**墨迹 over 已受光材质色**
  （`tex.rgb + color.rgb * lit * (1 - tex.a)`，见 `cube.wgsl`）——纹理作为 decal
  不参与光照，材质未指定纹理时绑定 1×1 透明黑兜底纹理，此时退化为 `color * lit`。
  顶点布局 32 字节 = `Float32x3` 位置 + `Float32x2` UV
  + `Float32x3` 法线（RC 补充；法线缺省填 `(0,0,1)`）。
- **Uniform 绑定**：`ViewProj (Mat4) + TintColor (Vec4)` 每 draw 一提交，经
  `UniformArena`（256B 对齐 dynamic offset），使用 `wgpu 30` 新 API
  （`draw(0..n, 0..1)` 范围语法）。
- **每帧流程**：
  1. `prepare_draws(&device, &queue, view_proj, &meshes, &lines) -> Frame3DData`
     按几何版本/内容寻址复用 vertex buffer、uniform 走 arena，生成 Vec\<PreparedDraw3D\>
  2. `record_prepared(&mut rpass, &draws)` 提交 draw
  3. `render_layer_offscreen(encoder, layer_id, view_proj, viewport, meshes, lines)`
     渲染到该图层**自己的离屏纹理 + 深度纹理**（透明清屏），交合成器按声明序混合
- **Cube WGSL Shader**：位于 `shaders/cube.wgsl`，单 VS + FS；VS 输入
  `vec3<f32>` 位置 + `vec2<f32>` UV + `vec3<f32>` 法线，FS 返回
  `color * lit * textureSample(t, s, uv)`，其中 `lit` 为固定头顶方向
  `L = (0,0,1)` 的 Lambert（`ambient = 0.2`）；无法线几何 `lit = 1` 故外观不变
  （RC 补充）。
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

- `SceneCompositorCallback::new(viewport_points, layers, queue_2d, queue_3d, content, morphs)`：
  `layers` 是**有效序**（翻转开关已生效）的 `Vec<SceneLayer { id, kind, blend, opacity }>`；
  `content` 是内容纹理层（S3-3）的当帧快照（层 id → 已降级的 2D 命令）；
  `morphs` 是形变项（S4.3）快照，随 2D 图层走形变管线。
- `prepare`：逐层渲染离屏（2D → `WgpuBackend2D`，3D → `WgpuBackend3D::render_layer_offscreen`）
  → 按**声明序**收集全部 `CompositeOp` → **一次** `Compositor::prepare` → 单个
  `CompositeFrame`，与各层 uniform arena buffer（保活）一起存入 TypeMap 的
  `SceneCompositorFrame`（合成条目**只有一处**）。
- `paint`：设 scissor → `Compositor::composite_layers`（按声明顺序混合，含 opacity）。
- **不使用 `Order::Background`**；用 `ui.painter_at(central_rect).add(Shape::Callback(...))`
  与 egui 同层提交，回调落在有效序中**首个 GPU 图层**的位置（CPU 2D 层就地回放）。
- `WgpuBackend2D` / `WgpuBackend3D` / `Compositor` 存放在
  `rs.renderer.write().callback_resources` TypeMap 中，回调以 `&self` 访问。
- **离屏目标按图层持有**（RC.1）：2D/3D 后端各自以 `HashMap<LayerId, _>` 管理离屏
  目标，同一类可有多个图层（S3-2 的"每类最多一个图层"`debug_assert` 已解除）；
  目标尺寸随 `viewport` 变化时按图层重建。

### 4. `wgpu_2d` (`wgpu` / `egui-wgpu`) — S3-1 / S3-2 / S4.2 / S4.3 / R0

xcal-rs 的 GPU 2D 后端（S4.1 起是**唯一** 2D 路径）：**离屏纹理 + MSAA 4x**；
合成自 S3-2 起外移到 `compositor`（本文件不再自带合成管线）。

- **`WgpuBackend2D`**：`new(device, queue)` 建**两条**管线——
  `shaders/shape_2d.wgsl`（图元 + **R0 书写动画**；硬边 + MSAA）与
  `shaders/shape_2d_morph.wgsl`（S4.3 形变；双 vertex buffer + `t` uniform）。
  **R0 的书写动画复用图元管线**（区间是 uniform 里的分支，不是管线维度）⇒ 管线数不变。
  合成管线、绑定布局、采样器与 `composite()` 已移除（统一由 §6 `Compositor` 承担）。
- 实现 `Painter2DTarget`（`begin_frame` / `draw_shape` / `end_frame`），
  另有直达录制 API `push(shape, transform, style, z)` / `push_morph(MorphItem)` /
  `push_reveal(RevealItem)`。
- **每帧流程**（`finish_frame(encoder, layer_id, geometry) -> Frame2DData`）：
  1. 录制项按 `z` 稳定排序（同 z 保录制序，画家算法）；
  2. **趟 1**：每项 `tessellate`（S3-0）→ 顶点哈希 → 三角化结果（**CPU 侧**）进
     `ResourceCache<()>`，命中即免三角化；**命中要 `mark_used`**（`get` 不刷新
     `last_used_frame`，否则帧末 `evict_unused(0)` 会把稳态内容也驱逐，缓存跨帧失效）；
     请求/未命中数记入 `FrameCacheStats`；
  3. **趟 2（S4.2 合批）**：按 `(color, stroke_width)` 分组 → 成员顶点按序拼接、
     成员索引按顶点基址**重排**（rebase）成**一套** GPU 缓冲 → 整批**一次**
     `draw_indexed`。合批缓冲按"批内容"哈希缓存 ⇒ 稳态零装配零上传；
     分组保序 ⇒ 同 `z` 下覆盖关系与逐条绘制一致；
  4. **趟 3（S4.3 形变）**：`MorphItem` 的 `from`/`to` 先**弧长等点重采样**到
     `samples` 点（两侧顶点一一对应），各三角化一次并按内容哈希缓存
     —— 缓存键**不含 `t`**，故 `t` 逐帧变化仍全程命中；每帧只多写一个
     `t` uniform（`params.y`），shader 侧 `mix(from, to, t)`（位置与法线都插值）。
     形变项排在全部 `DrawQueue` 几何**之后**（同为 z = 0 的内容；网格/轴线 z < 0 不受影响）；
  5. **趟 4（R0 书写动画）**：`RevealItem` 的路径用 `tessellate_reveal`（按
     `REVEAL_STEP_PX` 预细分 + 每顶点归一化弧长 `s`）三角化，按内容哈希缓存
     —— 缓存键**不含区间 `[a,b]`**，故区间逐帧变化仍全程命中；每帧只多写
     `params.z/w`（区间）。区间逐项不同 ⇒ **不进合批**，每项一次 `draw_indexed`。
     区间外的**片元**由 `shape_2d.wgsl` 的 `discard` 丢弃（§详见该 shader 头注：
     顶点塌缩方案被像素级验收证伪）；
  6. uniform 经 `UniformArena`（复用 `wgpu_3d`，256B 对齐 dynamic offset）；
     宽度由 CPU 侧唯一换算点 `draw.rs::resolve_style` 解析，**shader 不做空间换算**；
     几何本身与宽度无关（`position` 槽存 lyon `position_on_path()`），
     故同一路径换宽度只需换 uniform；
  7. 在离屏 MSAA 纹理上绘制并 resolve；**离屏清屏透明**（S3-2/C2，背景由 host 提供）。
- **`draw_count` 语义保持"几何条数"不变**（S4.2 起 = 合批前三角化条数 + 形变项数
  + 书写动画项数）；新增 **`batch_count` = 实际 `draw_indexed` 次数**
  （合批后 ≤ `draw_count`）—— 同色多条几何时 `batch_count < draw_count`
  （可证伪断言见 §测试）。
- **离屏格式** `OFFSCREEN_FORMAT = Rgba8Unorm`（**非 sRGB**，存 gamma 值），
  `MSAA_SAMPLES = 4`；纹理开 `COPY_SRC` 支持像素回读。
- **`Frame2DData`** = `{ offscreen, draw_count, batch_count, cache, _arena_buffer }`，
  `cache: FrameCacheStats { geom_requests, geom_misses, morph_requests, morph_misses,
  reveal_requests, reveal_misses }`。
- **`Frame2DGeometry`**：`viewport_points` + `pixels_per_point` + `screen_size_px`；
  `viewport_px()` 复刻 `epaint::ViewportInPixels::from_points` 的取整 + 钳制口径，
  使**离屏纹理尺寸恒等于 egui-wgpu 为该回调设置的 render pass viewport 尺寸**。
- **`WgpuBackend2D`**（`feature = "wgpu"` / `"egui-wgpu"`）的 egui 回调职责已并入
  §3 `scene_callback`（`SceneCompositorCallback`）；本模块只提供渲染 API。
- 导出：`WgpuBackend2D` / `Frame2DGeometry` / `Frame2DData` / `FrameCacheStats` /
  `Recorded2D` / `snapshot_queue` / `MSAA_SAMPLES` / `OFFSCREEN_FORMAT` /
  `SHAPE_2D_SHADER` / `SHAPE_2D_MORPH_SHADER`。

> ⚠️ **egui-wgpu 回调 viewport 陷阱**：egui-wgpu 处理 `Primitive::Callback` 时会
> `set_viewport(callback.viewport_in_pixels())`（下一个 primitive 处才复位）。
> 即**回调内 NDC `[-1,1]²` 对应回调自身矩形**，而非整屏。因此合成四边形必须写死
> 全 NDC 盒（`compositor::COMPOSITE_QUAD`）；若按整屏尺寸把视口像素矩形换算成裁剪坐标，
> 视口会被**二次应用**（宽度 × 视口宽/屏幕宽并右移），表现为 2D 视图在 X 方向被压扁。
> 回归测试 `composite_lands_inside_scoped_viewport_on_real_device` 在旧实现下必然失败。

### 5. `compositor` (`wgpu` / `egui-wgpu`) — S3-2

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
- **无动态 blend / 无动态入口点** ⇒ 管线**键空间**是 9 个组合
  `(LayerBlend, Conversion)`（3 blend × 3 conversion）—— 但**不再预建**
  （C3 起惰性，见本节末尾"管线惰性建"）。
- 混合（预乘 alpha）：`Opaque` = REPLACE；`Alpha` = 色/alpha 均 `One / OneMinusSrcAlpha`；
  `Additive` = `One / One`。
- **sRGB 域转换**：`textureSample` 对 sRGB 格式纹理**自动解码**，故按
  (源 sRGB-ness, 目标 sRGB-ness) 选入口 —— 同域 `fs_identity`、
  源非 sRGB → sRGB 目标 `fs_gamma_to_linear`、源 sRGB → 非 sRGB 目标 `fs_linear_to_gamma`
  （`Conversion::for_formats`）。3D 离屏沿用主目标格式（源 = 目标 → Identity）；
  2D 离屏为 `Rgba8Unorm`（存 gamma）。
- 导出：`Compositor` / `CompositeOp` / `CompositeFrame`。

### 6. `eframe_backend` (`eframe`)

`EframeWgpuBackend`：实现 `xcal_render::RenderBackend` trait 的 eframe 版本。
对写纯 eframe/wgpu 示例时可直接复用。

`install_standard_backends(rs) -> RenderContext`：**为 egui 宿主准备渲染环境的一行入口**。
**W1 起它只是两步的第二步的包装**：

1. `StandardBackends::new(&RenderContext)`（见第 8 节）—— 纯构造（**零 egui 依赖**）；
2. `StandardBackends::install_into(&mut CallbackResources)` —— 装进
   `Renderer::callback_resources`（全局 TypeMap），返回 `RenderContext`。

- 为什么装 TypeMap：回调执行时只有 `&self`，不能持有"重建中"的 GPU 资源（见 `scene_callback`）。
- 为什么拆两步（W1）：装配知识此前**只能**吃 `egui_wgpu::RenderState`；拆开后
  app / headless / 其他宿主共用同一份装配（headless 直接用第 1 步）。
  **对外签名与行为不变**，插入顺序仍是 3D → 2D → 合成器。

### 7. `vertex_layout` (`wgpu` / `egui-wgpu`)

顶点格式契约（`xcal-render::vertex`）→ wgpu 的翻译层：`to_wgpu(layout, &mut attrs)`
把 `VertexLayout` 转成 `wgpu::VertexBufferLayout`（调用方提供属性数组存储）；
多 buffer 用 `to_wgpu_buffers(&VertexInputDesc, &mut …)`，shader location 跨 buffer
全局连续（wgpu 语义）。入口先 `validate()` fail-fast。**不碰 `wgpu_3d.rs`。**

---

### 8. `standard_backends` (`wgpu` / `egui-wgpu`) — W1

**不依赖 egui 的渲染环境装配 + headless 渲染入口**。

```text
StandardBackends::new(&RenderContext)          ← 纯构造，零 egui 依赖
   ├── install_into(&mut CallbackResources)    ← egui 宿主（见第 6 节）
   └── render_frame_headless(..)               ← 非 GUI 路径
```

`StandardBackends` 自持有 `WgpuBackend3D` + `WgpuBackend2D`（+ **惰性**的内容层实例）
+ `Compositor`；`BackendStore` trait 是"取用口"的两个实现（TypeMap / 自持有），
**帧体只有一份**（`SceneCompositorCallback::prepare_frame` / `paint_frame`）⇒
"两条宿主路径逐位一致"是**结构保证**。

`render_frame_headless(ctx, &mut backends, &callback, screen, clear) -> Vec<u8>`：
按回调的视口矩形自建目标纹理 → 跑帧体 → 显式设 viewport/scissor → 合成 → 回读
**紧凑 RGBA8**（行距 `w*4`，左上原点，可直接喂 PNG 编码器）。
`render_single_frame(..)` 是"抓单帧"的糖（⚠️ 序列帧导出**别**用它：每帧重建后端
会丢管线与几何缓存）。

- **验收**：`headless_path_matches_egui_path_bit_for_bit`（内容层 + 2D + 3D 三层齐全，
  空链与非空链各跑一遍，两条路径的目标纹理**逐字节相同**）。
- **回读 API**：`RenderContext::read_texture(texture, (w, h))` —— 256 字节
  `bytes_per_row` 对齐在它内部处理（此前 `wgpu_2d` / `compositor` / `scene_callback`
  三份重复实现已消除）。
- **`scene_callback` 不再整体门控在 `egui-wgpu`**：`SceneCompositorCallback` 的字段全是
  纯数据，它同时是 headless 的"一帧渲染请求"；只有 `impl egui_wgpu::CallbackTrait` 在门控内。

## 合成后滤镜链（C 线：C0–C3）

`Compositor` 支持把全部图层合成到**中间全屏纹理**、再由 `shaders/post_filter.wgsl`
（**uber shader + 4 固定槽位**）经滤镜链 blit 进主 pass。

- **空链 = 直通**：`PostFilterChain::is_empty()` ⇒ 不建中间纹理、不多一次全屏 pass
  ⇒ 画面**逐位不变**（`scene_callback.rs` 的 `post_blit: Option<PostBlit>`）。
- **滤镜不是管线维度**：管线仍是 `3 blend × 3 域转换`；滤镜只在 uniform 里
  （`FilterKind::code()` 与 WGSL 常量表一一对应）⇒ **新增滤镜不改管线代码**。
- **预乘 alpha 硬约束**：中间纹理里是预乘颜色 —— 增益对 `rgb`/`a` 同乘、模糊在
  **预乘空间**做（否则半透明边缘出黑边/halo）。
- **管线惰性建**（C3）：`ensure_for(&mut self, ..)` 按本帧组合现场建并缓存；
  `new` 不再预建 9 条。它只在 `prepare`（有 `&mut`）里调用 ⇒ `composite_layers`
  的借用形式与签名**零改动**（`paint` 拿到的是 `&CallbackResources`，改不动它）。
- **GPU 验收**：`gpu_post_route_with_identity_filter_matches_direct_bit_exactly` /
  `gpu_post_filters_point_geometry_neighborhood` /
  `gpu_filters_add_no_pipelines_and_pipelines_are_lazy`。

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
xcal-scene              = { default-features = false }
xcal-render            = { default-features = false }
xc-notion
glam
thiserror
pollster               (optional)  ← wgpu  / egui-wgpu
wgpu                   (optional)  ← wgpu  / egui-wgpu
egui                   (optional)  ← egui-wgpu
egui-wgpu              (optional)  ← egui-wgpu
```

> **dev-dependency**：`hecs`（仅测试）—— N6 的 GPU 像素级验收要构造真实 ECS 场景，
> 而 `record_scene_tagged` 的入参是契约层 `SceneRead`（hecs 侧适配器 `WorldView`
> 需要 `&hecs::World`）。**库本体仍不认识 hecs**。

## 测试

```bash
cargo test -p xcal-render-backends --features egui-wgpu   # 3D/2D 离屏渲染、图层合成、顶点布局翻译
cargo test -p xcal-render-backends --no-default-features  # 空 crate（全部模块被 feature 门控关闭）
cargo check -p xcal-render-backends --features wgpu
cargo check -p xcal-render-backends --features egui-wgpu
```

`--features egui-wgpu` 下的 2D 相关测试：`Frame2DGeometry::viewport_px` 取整/钳制口径、
离屏尺寸下限、局部变换与裁剪矩阵、`gpu_frame_rasterizes_and_recreates_targets_on_resize`
（离屏像素回读 + 视口 200×100 → 400×300 目标重建）、
`gpu_pipeline_builds_for_srgb_and_non_srgb_targets`、`pipeline_vertex_layout_comes_from_contract`、
`animated_content_keeps_cache_and_arena_bounded`。

S4.2 / S4.3 新增的可证伪断言（真实设备门控）：
- `same_color_geometries_batch_into_one_draw_call`：6 条同色几何 + 1 条异色 →
  `draw_count == 7`、`batch_count == 2` 且 `batch_count < draw_count`；
- `steady_state_geometry_cache_misses_stop_growing`：跑 10 帧同内容 →
  首帧 `geom_misses == 1`、之后恒为 0（`mark_used` 保活）；
- `morph_t_uniform_drives_geometry_and_stays_cached`：同一个形变项跑 `t = 0…1` →
  描边边界随 `t` 逐帧外移（像素级），且 `morph_misses` 首帧后恒为 0（`t` 不进缓存键）。

N6（对象级可见性 / 不透明度，**走完整录制路径**：`record_scene_tagged` 消费真实 ECS
场景 → `DrawQueue` → 2D 后端 → 回读像素；hecs 仅作 **dev-dependency**）：
- `visibility_and_opacity_absent_match_global_full`：`Visibility::global` + `Opacity::FULL`
  与**两者都缺席逐位一致**（缺席 = 默认值）；
- `visibility_camera_tag_filters_pixels`：相机 A 下有像素、相机 B 下**零覆盖像素**；
- `visibility_group_switch_filters_pixels`：组开有像素、组关零像素；
- `opacity_half_premultiplies_and_has_no_dark_halo`：`Opacity = 0.5` ⇒ 预乘后
  `r ≈ a ≈ 128`，且全图半透明像素 `|r − a| ≤ 2`（**无黑边/halo**）——
  可证伪点：若 `rgb` 与 `a` 同乘，预乘后 `r` 会落到 `o²` 档（≈64）而非 128；
- `opacity_zero_yields_no_pixels`：`Opacity = 0` ⇒ 零覆盖像素（预乘后 rgb 与 a 同时为 0）。

`compositor` 测试（真实设备门控，无适配器时跳过）：
`blend_state_matches_declared_semantics`、`composite_quad_is_full_ndc_box_with_uv`、
`quad_bytes_are_twenty_per_vertex`、`composite_quad_fills_callback_viewport_at_any_position`、
`conversion_for_formats_matches_srgb_domains`（纯 CPU）、
`gpu_composite_respects_declaration_order`（红 + 蓝 ×0.5 → (128,0,128)；反序 → 纯红）、
`gpu_composite_preserves_op_order_with_distinct_blends`、
`gpu_composite_opacity_zero_invisible_one_opaque`、
`gpu_composite_builds_all_three_blend_pipelines`（sRGB / 非 sRGB 目标各建一次）。

3D 侧新增 `test_render_layer_offscreen_clears_transparent`（C3：空队列 → 离屏全 `[0,0,0,0]`）。
