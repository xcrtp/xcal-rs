# xcal-app

> **组装与启动层**。基于 `eframe 0.36`（egui + wgpu），把
> `xcal-scene / xcal-render / xcal-render-backends / xcal-egui`
> 提供的所有 System、Resource、Backend 按阶段装配成一个可运行的桌面应用。

```bash
cargo run -p xcal-app
```

---

## 启动后会看到什么

> **P5**：本轮把 2D 内容换成**首个数学动画原型**（分镜脚本在 `src/storyboard.rs`，
> 六条分镜见计划 §1.1）。原先的 2D demo（振动圆 / `sin(x+φ)` 曲线 / 圆↔方形变 /
> `x²`↔`x³` 语义形变）已被分镜取代；3D demo（线框立方体 + 参数曲面 + 内容纹理公式）
> 保持不动。

1. **中央视口（wgpu 合成）**——六条分镜：
   1. **常驻**：网格与坐标轴（`Axes2D`，逐帧按相机 refresh）；
   2. **镜头初始位**：2D 相机是场景对象（`Framing2D`），脚本取出 `Camera2DHandle`；
   3. **0–2s**：曲线 `y = sin x` 从左到右**被"写"出来**（R0 书写动画：`Reveal` 组件 +
      `RevealRange` 目标把区间 `[0, end]` 从 0 推到 1 —— 几何静态、逐帧只变 uniform）；
   4. **2–2.5s**：`wait`；
   5. **2.5–3.5s**：**镜头推进**到曲线峰值 `(π/2, 1)`（`scale` 40 → 110）；
   6. **3.5–4.5s**：公式 `y = sin x` **被"写"出来**（每个描边子轮廓一个 `Reveal`，
      **逐轮廓依次**占 `1/N` 段 —— 连续 `play` 顺序排期，见 `Timeline` 的顺序游标）；
   另有 3D demo：线框立方体持续绕 Y 轴做轨道相机运动 + 蓝色参数曲面（采样内容纹理，
   纹理源是一条静态 Typst 公式 `x² + 1`）。
2. **egui 面板**：左侧时间/相机/网格 + **`2D 相机` / `3D 相机` 复选框**
   （关掉即不再声明该图层：2D 关掉则网格/曲线/公式全部不画，3D 关掉则立方体/曲面不画）、
   **`视口操作` 单选**（K3：2D 平移缩放 / 3D 轨道（拖拽+滚轮）/ 3D 第一人称（拖拽+WASD/QE））
   与 **`2D 相机自由（关闭=分镜驱动）` 复选框**（决定 2D 相机由分镜的镜头推进驱动，
   还是交给用户平移/缩放）、
   **`合成后滤镜` 开关 + 强度**（C 线：亮度 / 桶形畸变 / 3×3 模糊）、
   **`翻转图层顺序` 复选框**（S3-2 验收开关：验证叠放顺序确实由 `SceneView::layers`
   的声明序决定，默认关），
   左二时间控制面板：播放/暂停、**`⟲ 重新播放`**（拨回 `t = 0` 并开始播放，
   用于重看分镜；全部绑定是 O1 纯函数 ⇒ 回到 0 即回到初始画面）、速度、时间轴 seek，
   右侧动画参数
   （圆半径 / 曲线相位 / 3D 轨道相机 slider → `ParamPanelSystem` 写回 `BindingTable`）
3. **右上角 Health Overlay（F1 切换）**：FPS、tick、实体数、绘制调用数
4. **快捷键**：
   - `F1` — 切换性能面板
   - `F10` — 导出当前 `hecs::World + Resources` 快照到 `snapshots/tick_{n}.json`
   - 滚轮缩放、拖拽平移（host 翻译为 `CameraCommand` → `CameraInputSystem`，
     **相机状态在场景对象里**，P1）

---

## App::new 初始化流程（关键代码）

> ⚠️ 以下为**当前**装配流程的压缩版（P1~P5 后）。完整代码见 `src/lib.rs`；
> 2D 分镜在 `src/storyboard.rs`。

```rust
// ── 1. 字体 / 视觉（宿主平台资产归 host） ──
if EguiSceneHost::install_cjk_font(&ctx.egui_ctx).is_none() { eprintln!("…中文可能显示为方框…"); }
Self::setup_visuals(&ctx.egui_ctx);

// ── 2. 渲染环境：一行装好后端与合成器（细节在 backends 侧） ──
let render_ctx = xcal_render_backends::install_standard_backends(&rs);

// ── 3. 3D demo + 内容纹理公式 ──
let mut scene = Scene::new();
scene.spawn((Arc::new(wireframe_cube(1.0)), Material3D::color(WHITE)));
scene.spawn((Arc::new(parametric_surface(..)), surface_material, Transform { .. }));
let camera3d_entity = scene.spawn((Camera3D::new(..),));               // eye 由 Binding 驱动
let engine = formula::build_engine();                                  // Typst / placeholder
if let Some(ir) = formula::rebuild(&engine, &spec /* "x^2 + 1" */) {
    for (path, color, width, space) in formula::compile_to_stroke_paths(&ir, &spec) {
        scene.spawn((Path::new(path), StrokeSolid::new_in(width, color, space), ContentTextureItem));
    }
    content_camera = Some(base_camera.fit_bounds(bounds, 0.8));         // 内容纹理取景
}

// ── 4. 首个数学动画原型（§1.1 六条分镜，声明式）──
// 内部：Axes2D::build + spawn_camera2d（取 handle）+ 曲线 MorphShape + 公式浮现实体
//       + Timeline 把"先…再…"编译成 ClockNode(offset+span+Hold) + Binding
let sb = storyboard::build(&mut scene, &engine);                        // → forest / bindings / camera
let world = scene.into_world();
let mut forest = sb.forest;
let mut bindings = sb.bindings;
// 「2D 相机自由」开关托管的分镜绑定：指 2D 相机（`Camera2DFraming`）的 center/scale 各一条。
// 开关打开时它们被**移出**绑定表（否则每帧无条件写 center/scale，用户的 Pan/Zoom 同帧被抹掉）、
// 关闭时放回。
let cam2d_bindings: Vec<Binding> =
    bindings.bindings.iter().filter(|b| b.target.kind() == TargetKind::Camera2DFraming).cloned().collect();
// 唯一一条手工绑定：3D 轨道相机（与分镜编排无关，时间源 = 全局 t）
// K3：这条绑定由 `CameraDriveSystem` **托管** —— 「视口操作」切到 3D 时它会从表里
// **移出**（让位给交互：否则 `CameraEye` 每帧无条件写 eye，拖拽效果立刻被覆盖），
// 切回 2D 时放回；同时把 3D 相机的 `CameraMode` 组件在 `Path` ↔ `Orbit`/`Fps` 间切换。
// 两台相机的让位判定由**同一个** System 原子完成（它们让位的是同一张 `BindingTable`，
// 同相位两个写者会被 `Scheduler::add_system` 的静态借用冲突检查拦下）。
let orbit = Binding { clock: forest.insert(ClockNode::new(None))?, curve: Curve::Orbit { .. }, target: Arc::new(CameraEye { .. }), seek_kind: SeekKind::O1 };
bindings.add(orbit.clone());

// ── 5. Resources 注入 ──
let mut r = Resources::new();
r.insert(sb.camera);            // P1-1/P2：相机状态在场景对象里，这里只放**句柄**
r.insert(TimeState::default());
r.insert(UiConfig::default());
r.insert(ViewRect::default());  // P4：来自 xcal-render
r.insert(InputEvents::default());
r.insert(DrawQueue::new());
r.insert(DrawQueue3D::new());
r.insert(ContentTextureSpec { .. });      // S3-3
r.insert(ContentTextureQueue::default());
r.insert(MorphQueue::default());          // S4.3
r.insert(RevealQueue::default());         // R0 书写动画
r.insert(render_ctx.clone());
r.insert(forest);
r.insert(bindings);
r.insert(DirectorState::new());
r.insert(GroupVisibility::default());     // N3：组级可见性开关（默认空 ⇒ 所有组可见）

// ── 6. Scheduler 注册 System（参见下方"阶段分配"） ──
let mut sched = Scheduler::new(world, r);
sched.add_system(Phase::Input,        DirectorAdvanceSystem);
sched.add_system(Phase::Input,        CameraDriveSystem::new(cam2d_bindings, orbit)); // App 层（K3 + 2D 自由开关）
sched.add_system(Phase::Input,        CameraInputSystem);          // xcal-scene（P1 归位）
sched.add_system(Phase::LogicUpdate,  TimeControlSystem);
sched.add_system(Phase::LogicUpdate,  ParamPanelSystem);           // App 层
sched.add_system(Phase::SceneSync,    BindingApplySystem);
sched.add_system(Phase::Prepare,      AxesRefreshSystem);          // M4：几何在对象层
sched.add_system(Phase::Prepare,      FunctionGraphRefreshSystem);
sched.add_system(Phase::RenderCommit, SceneRecordSystem);          // xcal-render（P4 归位）
sched.add_system(Phase::RenderCommit, ContentTextureRecordSystem); // xcal-render
sched.add_system(Phase::RenderCommit, MorphRecordSystem);          // xcal-render
sched.add_system(Phase::RenderCommit, RevealRecordSystem);         // xcal-render（R0 书写动画）
sched.add_system(Phase::RenderCommit, SceneRecord3DSystem);        // 3D 专用（App 层）
// FrameEnd 为空——内容提交由 host.present 承担（原 RenderSubmit 职责）
```

> UI 与场景只经 `SceneView` / `SceneEvent` 契约交互：相机与面板 chrome 全部收敛在
> `EguiSceneHost`（`build_scene_view` / `begin_frame` / `present`），System 侧不再有 egui 类型。

---

## 阶段分配（完整）

| Phase | System | 资源写集 | 资源读集 |
|-------|--------|----------|----------|
| **Input**       | DirectorAdvance | DirectorState, TimeState | — |
|                 | CameraDrive     | BindingTable             | UiConfig, Camera2DHandle, Camera3DHandle |
|                 | CameraInput     | —（写**场景对象** `Framing2D`）| InputEvents, Camera2DHandle |
| **LogicUpdate** | TimeControl     | DirectorState            | InputEvents |
|                 | ParamPanel      | BindingTable             | InputEvents |
| **SceneSync**   | BindingApply    | hecs 组件（无 Resource） | TimeState, ClockForest, BindingTable |
| **Prepare**     | AxesRefresh (M4) | —（写 hecs 组件）         | ViewRect, Camera2DHandle, UiConfig |
|                 | FunctionGraphRefresh (M4) | —（写 hecs 组件）| — |
| **RenderCommit**| SceneRecord (2D) | DrawQueue (2D)          | ViewRect, Camera2DHandle, **GroupVisibility (N3)** |
|                 | ContentTextureRecord (S3-3) | ContentTextureQueue | ContentTextureSpec |
|                 | MorphRecord (S4.3) | MorphQueue            | ViewRect, Camera2DHandle, **GroupVisibility (N3)** |
|                 | RevealRecord (R0) | RevealQueue             | ViewRect, Camera2DHandle, **GroupVisibility (N3)** |
|                 | SceneRecord3D   | DrawQueue3D              | —（读 hecs::World 组件） |
| **FrameEnd**    | —（空）         | —                        | — |

前四个录制 System 与 `CameraInput` **不在 app**：它们分别住在
`xcal-render::record_systems`（P4 / R0）与 `xcal-scene::camera_input`（P1）。

同 phase 内 `TimeControl` / `ParamPanel` 同读 `InputEvents`（合法）；
四个录制 System 同读 `ViewRect` / `Camera2DHandle`、写集两两不相交
——debug 模式下 `BorrowTracker` 不会触发冲突。

> **`BindingTable` 单写者约束（实测坑）**：`CameraDrive` 与 `ParamPanel` 是**仅有的**两个
> `BindingTable` 写者，且必须分处不同 phase（`Input` / `LogicUpdate`）。两者同 phase 会被
> `Scheduler::add_system` 的 debug 静态冲突检查在启动时 panic（`Scheduler conflict in phase
> LogicUpdate ...`）。两台相机的让位判定（2D 自由开关 + 3D 视口目标）也因此必须由**同一个**
> `CameraDriveSystem` 原子完成 —— 它们让位的是同一张表。

`FrameEnd` 为空：内容提交（2D + 3D）由 `EguiSceneHost::present` 在 `run_frame` 之后承担，
面板 chrome 与 `ViewRect` 写入由 `begin_frame` 在 `run_frame` 之前完成。

---

## eframe::App::ui 每帧主循环

```rust
fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
    // 墙钟 dt：用 Instant 单调时钟算，不随渲染帧率波动。
    let now = Instant::now();
    let dt = (now - self.last_frame).as_secs_f32().min(0.1);
    self.last_frame = now;

    // 1. 场景 → UI：构建本帧视图声明（2D/3D 图层 + 相机）。
    let view = EguiSceneHost::build_scene_view(self.scheduler.world(), self.scheduler.resources());

    // 2. UI → 场景：收集事件、画面板 chrome、写 ViewRect。
    //    必须在 run_frame 之前：System 消费 InputEvents，Axes/FunctionGraph 刷新当帧读 ViewRect。
    EguiSceneHost::begin_frame(ui, &view, self.scheduler.resources_mut());

    self.scheduler.run_frame(dt);

    // 3. 内容提交：2D 队列 + 3D 队列 → painter（原 RenderSubmit 职责）。
    EguiSceneHost::present(ui, &view, self.scheduler.resources_mut());

    // ── 快捷键 F1（性能面板）/ F10（导出快照）──
    // health.update(..., EguiSceneHost::draw_call_count(res))：绘制调用数由 host 取数
    //（口径 = 2D 命令数 + 3D mesh/line 数；FrameReport / FrameCacheStats 均无该字段）。

    // ── 帧末：清空已被 present 消费的 DrawQueue(2D)（3D 队列由回调内部 take_snapshot 清空）──
    EguiSceneHost::end_frame(self.scheduler.resources_mut());

    // ⚠️ egui 默认是"事件驱动"，无输入时不重画 → 动画停住。
    //    每帧主动请求，保证时间推进与 repaint 持续。
    ui.ctx().request_repaint();
}
```

---

## 公式接入（`src/formula.rs`）

把已完成的 Typst 内容管线（`xc-content` IR + `xc-prerender` 引擎）接进主应用，
让公式以**普通 2D 图元**出现在场景里。**不新增任何 System**，只做接线。

```
Typst 源码 → xc-prerender::engine（typst [feature] / placeholder）
           → xc-content::ContentIR（StyledPath，pt，Y-up）
           → xcal-render::ir_to_draw_cmds（scale = font_size / nominal_font_size）
           → formula::compile_to_stroke_paths（flatten_subpaths + apply_point）
           → 每个子轮廓一条 (Path, StrokeSolid) 实体
           → 现有 SceneRecordSystem::record_scene → DrawQueue → WgpuBackend2D（GPU 离屏）
```

关键点：

- **复用 `ir_to_draw_cmds`**：缩放与平移全部由它完成，桥接层不另写变换。
- **每个子轮廓单独一条路径**：用 `Path2D::flatten_subpaths` 按 `MoveTo` 分组
  （`flatten` 会把所有子路径拼成一条，多轮廓字形 `o`/`B`/`8` 会出现横穿字形内部
  的桥接线）；组内首尾点重合即视为闭合，`Path2D::from_polyline(..., closed=true)` 追加 `Close`。
- **字号解耦**：`FormulaSpec.nominal_font_size`（默认 12.0）是 Typst 编译标称字号，
  决定曲线提取精度与 IR 本地点尺度；`FormulaSpec.font_size` 是目标显示大小。
  展平容差是相对标称尺度的绝对值，显示尺寸缩小不会放大折线误差。
- **空心字**：公式以 Stroke（轮廓）呈现。原为规避旧 CPU 管线 Fill 分支
  `convex_polygon` 把非凸字形画成凸包的缺陷；S4.1 删除该路径后缺陷不复存在，
  此处保留空心字形作为既定视觉。
- **描边属世界空间**（`MeasureSpace::World`）：`ir_to_draw_cmds` 把宽度乘
  `target / nominal_font_size`（与几何同系数）换算为世界单位；fill-only 字形的默认细线宽
  同样按 `× scale` 换算。实机：缩放时公式描边与字形**成比例**变化
  （nominal=12 / target=1.3 / scale=40 → 约 4.3px）。
- **静态烘焙**：启动时编译一次并 spawn 实体；保留 `rebuild()`，将来接交互只需加 UI。

> 公式源在 `App::new` 里硬编码为 `"x^2 + 1"`。注意 Typst 0.14 数学模式下连续字母
> （如 `abcde`）会被解析为**单个未定义标识符**，多字母需空格分隔（`a b c d e`）。

---

## 形变（S1 插值内核）

**P5 后**：形变由**分镜脚本**消费 —— 分镜 3（曲线生长）与分镜 6（公式浮现）都是
`MorphShape(退化点, 目标折线)` + `MorphT` 绑定，走同一条 GPU 形变链路。

- **链路**：`MorphT`（`xcal_scene::animation::target::MorphT`）写 `t`；
  `MorphRecordSystem`（`xcal-render::record_systems`，RenderCommit）把实体录成
  `MorphItem`（`from`/`to` + `t`），由 2D 后端的**形变管线**做 GPU 插值
  （双 vertex buffer + `t` uniform，见 S4.3）。两端几何经弧长等点重采样，
  三角化结果与 GPU 缓冲都**帧间命中缓存** —— 每帧只多写一个 `t` uniform。
- **退化点起点**：`from` 的全部点重合于目标路径的首点（曲线）或包围盒中心（公式）
  ⇒ 形变两端点数对齐、视觉上"从一点绽开"。
  ⚠️ **零长度一侧由 2D 后端合成**（P5 实测修）：lyon 对零长度路径**不产出几何**，
  若直接跳过整条形变项就消失（画面空白，而 `MorphQueue` 看不出异常）。
  后端在 `prepare` 阶段用另一侧的拓扑合成它（顶点位置塌到该点、法线归零）
  ⇒ `t = 0` 不可见、随 `t` 平滑长出。用例：
  `morph_from_degenerate_point_grows_into_target_line`（GPU 像素级）。
- **开口 / 闭合口径**（实测修）：形变端点的闭合性**由两端共同决定** ——
  任一端自带 `Close` 即按闭合环重采样，**两端都开口则保持开口**。
  早期版本恒按闭合环处理，开口曲线（分镜 3 的 `y = sin x`）会因此多出一条
  **首尾相接的弦**（画面上是 x 轴上多出的一条直线）。用例：
  `open_morph_endpoints_stay_open_without_chord` / `closed_morph_endpoints_stay_closed`。
- **CPU 回退**：`xcal_scene::animation::MorphApplySystem` 仍可把形变落成 `Path`
  （实体同时带 `MorphShape` + `Path` 时生效，供 headless / 无 GPU 宿主使用）。
  App **不注册**它，形变走 GPU。
- **S1a/S1b 旧 demo**（圆↔方、`x²`↔`x³` 语义形变）**已从 App 移除**（被分镜取代）；
  `formula::compile_morph_pairs` 与其文档保留（`#[allow(dead_code)]`，见该函数 doc）。

> 铁律（路线甲）：`xc-time` 可依赖 `xc-notion`，但**不得**依赖 `xc-content`——
> 语义配对留在 `xc-content`，`xc-time` 只提供插值原语。
>
> 度量空间：分镜的曲线 / 公式形变 / 网格 / 坐标轴均为 `MeasureSpace::Screen`
> （粗细恒定）；只有内容纹理那条公式描边是 `World`（随缩放成比例）。唯一换算点在
> `xcal-render::record_scene`。

---

## 图层合成关键：为什么在单一 SceneCompositorCallback 里

`egui-wgpu` 的 paint callback 有两个重要特性：

1. 与 egui 主 render pass **同一次 render pass** 内执行，省掉一次 subpass
   切换开销；
2. scissor 可限制在中央视口，**天然把内容限制在非侧栏区域**。

所以**绝对不能**用 `Order::Background`（会在主 pass 之前提交、被侧栏覆盖），
而是 `ui.painter_at(central_rect).add(Shape::Callback(...))`——同一 layer 内
"后提交 = 在上面"。详见 `xcal-render-backends` 文档。

> **为什么只有一个回调（S3-2 修复）**：`egui_wgpu::CallbackResources` 是**全局共享**的
> TypeMap，不是 per-callback 的。若每个图层各注册一个回调并各自 `insert::<CompositeFrame>()`，
> 后插入会覆盖先插入 → 两个 `paint()` 画同一帧（实机现象：顺序 A 只见 3D、顺序 B 只见 2D）。
> 因此 2D / 3D 合并为**单个** `SceneCompositorCallback`，一次性合成全部图层。

### 2D 渲染路径（S3-1 / S4.1）

S4.1 后 2D 只有**一条**路径（旧 CPU 回放实现与其编译期 feature 已删除）：

2D 进 `SceneCompositorCallback` → `WgpuBackend2D` 离屏 `Rgba8Unorm` + MSAA 4x，
再经 `Compositor` 按声明顺序混合回主 pass（与 3D 图层同在**同一个** `CompositeFrame` 内）。

> ⚠️ 合成四边形必须写死全 NDC 盒（`compositor::COMPOSITE_QUAD`）：egui-wgpu 会把 render pass
> viewport 设为**回调自身矩形**，若按整屏尺寸换算裁剪坐标会导致视口被二次应用
> （2D 视图 X 方向被压扁）。见 `xcal-render-backends` README 的 viewport 陷阱一节。

---

## 可观测性（`xcal-scene/feature="telemetry"`）

Scheduler 会在每 phase 开始/结束埋点 `tracing::info_span!`；F10 触发的
`HealthOverlay::dump_snapshot` 会把 hecs World 所有组件（通过 `hecs::World::archetypes()`
迭代 + 类型名反射）+ Resource 快照导出 JSON：

```
snapshots/
  tick_000123.json   →  { tick, timestamp, archetypes: [...], resources: {...} }
```

可配合外部工具（Python/Pandas）做离线时序分析。

---

## 依赖

> **P5 文档修正（先核查）**：本节此前写作 `xcal-scene = { features = ["egui"] }`，
> 但 `crates/scene/xcal-scene/Cargo.toml` **没有 `[features]` 段**（实测，全仓库无该
> feature 定义）——以代码为准，已改为无 feature 的普通依赖。

```toml
xcal-scene                              # 场景组件 / 装配对象 / 动画（ClockForest + Binding + Timeline）
xcal-schedule                           # 调度内核 + Telemetry
xcal-render                             # 2D 管线（含四个录制 System：P4 三个 + R0 书写动画 + ViewRect）
xcal-render-backends   = { features = ["egui-wgpu"] }
xcal-egui
xc-macro
xc-content                              # 公式 IR 类型
xc-prerender           = { default-features = false }   # Typst / placeholder 引擎
xc-notion                             # 分镜：直接构造 Path2D（MorphShape 的 from/to）
xc-object                             # 相机句柄 / 取景参数 / 图元

eframe = { version = "0.36", default-features = false, features = ["wgpu","x11","wayland"] }
egui / egui-wgpu / wgpu 30
hecs / glam
derive-new / derive_builder
tracing 0.1

[features]
default = ["typst"]
typst   = ["xc-prerender/typst"]        # 关闭时回退 placeholder 引擎
```

> **铁律**：`xcal-render` 只能依赖轻的 `xc-content`，**不得**依赖 `xc-prerender`
> （否则关掉 feature 也会把 typst 拖进渲染层）。验收：
> `cargo tree -p xcal-render | grep -c typst` 结果为 0。

---

## 运行 / 测试

```bash
cargo run   -p xcal-app                               # 启动主 GUI（桌面环境，真公式）
cargo run   -p xcal-app --no-default-features         # placeholder 兜底（显示方块）
cargo test  -p xcal-app                               # formula.rs / param_panel.rs 单测
cargo build -p xcal-app --no-default-features         # 两种 feature 组合都需通过
cargo clippy -p xcal-app --all-targets -- -D warnings # 严格检查
```
