# xcal-app

> **组装与启动层**。基于 `eframe 0.36`（egui + wgpu），把
> `xcal-core / xcal-render / xcal-render-backends / xcal-egui`
> 提供的所有 System、Resource、Backend 按阶段装配成一个可运行的桌面应用。

```bash
cargo run -p xcal-app
```

---

## 启动后会看到什么

1. **中央视口（wgpu 合成）**：
   - 线框立方体（white, `wireframe_cube(1.0)`）持续绕 Y 轴做轨道相机运动
   - 振动的 2D 绿色圆（`radius ∈ [0.6, 1.4]`，sin 振荡驱动）
   - 一条青色 `sin(x + φ)` 曲线（`φ` 随时间平移）
   - 一条白描边 Typst 公式 `x² + 1`（**空心字轮廓**；由 `formula.rs` 把
     `ContentIR` 烘焙成旧 `(Path, StrokeSolid)` 实体，见下方"公式接入"）
   - 一个橙色**形变形状**：圆 ↔ 方 平滑来回（S1a；`MorphShape` + `MorphApplySystem`）
   - 一段粉色**语义形变公式**：`x²` ↔ `x³`（S1b；按源码锚点配对 —— `x` 配到 `x`）
   - 可多级缩放的网格背景（1 / 10 / 100 三档，视缩放自动淡入淡出）
2. **egui 面板**：左侧时间/相机/网格 + **`GPU 2D 管线` 复选框**
   （S3-1 运行期开关，默认关；勾选后 2D 走 `SceneCompositorCallback` 离屏 MSAA 合成，
   不勾则走 `EguiPainter` CPU 回放）+ **`翻转图层顺序` 复选框**
   （S3-2 验收开关：验证叠放顺序确实由 `SceneView::layers` 的声明序决定，默认关），
   右侧动画参数
   （圆半径 / 曲线相位 / 3D 轨道相机 slider → `ParamPanelSystem` 写回 `BindingTable`）
3. **右上角 Health Overlay（F1 切换）**：FPS、tick、实体数、绘制调用数
4. **快捷键**：
   - `F1` — 切换性能面板
   - `F10` — 导出当前 `hecs::World + Resources` 快照到 `snapshots/tick_{n}.json`
   - 滚轮缩放、拖拽平移（host 翻译为 `CameraCommand` → `CameraInputSystem`）

---

## App::new 初始化流程（关键代码）

```rust
// ── 1. 字体 / 视觉：微软雅黑优先，否则 egui 默认（中文变方框） ──
Self::setup_fonts(&ctx.egui_ctx)?;
Self::setup_visuals(&ctx.egui_ctx);

// ── 2. RenderContext + WgpuBackend3D / WgpuBackend2D / Compositor（回调资源） ──
let rs = ctx.wgpu_render_state.clone().unwrap();
let backend_3d = WgpuBackend3D::with_default_shader(&rs.device, &rs.queue, rs.target_format);
rs.renderer.write().callback_resources.insert(backend_3d);  // 回调为 &self，资源存 TypeMap

// S3-1：同一 TypeMap 再放一份 WgpuBackend2D（pipeline / 缓存 / 离屏目标惰性重建；
// `UiConfig::gpu_2d` 关闭时不产生任何 GPU 工作）。
let backend_2d = WgpuBackend2D::new(&rs.device, &rs.queue);
rs.renderer.write().callback_resources.insert(backend_2d);

// S3-2：图层合成器（2D / 3D 各画到自己的离屏纹理后按声明顺序混合进主 pass）。
let compositor = Compositor::new(&rs.device, rs.target_format);
rs.renderer.write().callback_resources.insert(compositor);

let render_ctx = RenderContext::from_render_state(&rs);

// ── 3. Scene 填充 hecs World（纯逻辑组件） ──
let mut scene = Scene::new();
scene.spawn((wireframe_cube(1.0, Color32::WHITE),));            // 3D 线框立方体
scene.spawn((Camera3D::new(...),));                            // 3D 相机（eye 由 Binding 驱动）
scene.spawn((Circle::new((0.,0.),1.0), StrokeSolid));          // 振动圆（半径由 Binding 驱动）
scene.spawn((FunctionX2Y::new(|x| x.sin()), Sampler1D,
             StrokeSolid, PhaseAnim));                         // sin 曲线（相位由 Binding 驱动）
// 公式：IR → 旧 (Path, StrokeSolid) 实体（每个子轮廓一条），见"公式接入"
let engine = formula::build_engine();
let spec   = formula::FormulaSpec::new("x^2 + 1", 1.3, glam::Vec2::new(-2.8, 3.0));
if let Some(ir) = formula::rebuild(&engine, &spec) {
    for (vertices, color, width, closed, space) in formula::compile_to_stroke_paths(&ir, &spec) {
        scene.spawn((
            Path::new(vertices, closed),
            // 公式描边是世界空间：与字形一同随缩放成比例变化。
            StrokeSolid::new_in(width, color, space),
        ));
    }
}

// ── 4. 动画唯一驱动链路：ClockForest + BindingTable ──
let mut forest = ClockForest::new();      // 3 个 root：振动圆 / 曲线相位 / 3D 轨道
let mut bindings = BindingTable::new();   // Curve::Sin / Phase / Orbit → BindingTarget

// ── 5. Resources 注入 ──
let mut r = Resources::new();
r.insert(AppCamera::default());
r.insert(TimeState::default());
r.insert(UiConfig::default());
r.insert(ViewRect::default());
r.insert(InputEvents::default());   // 每帧由 host 整帧替换
r.insert(DrawQueue::new());
r.insert(DrawQueue3D::new());
r.insert(render_ctx.clone());
r.insert(forest);
r.insert(bindings);
r.insert(DirectorState::new());

// ── 6. Scheduler 注册 System（参见"阶段分配"） ──
let mut sched = Scheduler::new(scene.into_world(), r);
sched.add_system(Phase::Input,        DirectorAdvanceSystem);
sched.add_system(Phase::Input,        CameraInputSystem);
sched.add_system(Phase::LogicUpdate,  TimeControlSystem);
sched.add_system(Phase::LogicUpdate,  ParamPanelSystem);        // App 层
sched.add_system(Phase::SceneSync,    BindingApplySystem);
sched.add_system(Phase::Prepare,      GridBuildSystem);
sched.add_system(Phase::RenderCommit, SceneRecordSystem);       // 2D
sched.add_system(Phase::RenderCommit, SceneRecord3DSystem);     // 3D 专用（App 层）
// FrameEnd 为空——内容提交由 host.present 承担（原 RenderSubmit 职责）
```

> UI 与场景只经 `SceneView` / `SceneEvent` 契约交互：相机与面板 chrome 全部收敛在
> `EguiSceneHost`（`build_scene_view` / `begin_frame` / `present`），System 侧不再有 egui 类型。

---

## 阶段分配（完整）

| Phase | System | 资源写集 | 资源读集 |
|-------|--------|----------|----------|
| **Input**       | DirectorAdvance | DirectorState, TimeState | — |
|                 | CameraInput     | AppCamera                | InputEvents |
| **LogicUpdate** | TimeControl     | DirectorState            | InputEvents |
|                 | ParamPanel      | BindingTable             | InputEvents |
| **SceneSync**   | BindingApply    | hecs 组件（无 Resource） | TimeState, ClockForest, BindingTable |
|                 | MorphApply      | hecs `Path`（无 Resource）| hecs `MorphShape`（读组件） |
| **Prepare**     | GridBuild       | DrawQueue (2D)           | ViewRect, AppCamera, UiConfig |
| **RenderCommit**| SceneRecord (2D)| DrawQueue (2D)           | ViewRect, AppCamera |
|                 | SceneRecord3D   | DrawQueue3D              | —（读 hecs::World 组件） |
| **FrameEnd**    | —（空）         | —                        | — |

同 phase 内 `TimeControl` / `ParamPanel` 同读 `InputEvents`（合法）；
`SceneRecord` 写 `DrawQueue` 与 `SceneRecord3D` 写 `DrawQueue3D` 类型无交集
——debug 模式下 `BorrowTracker` 不会触发冲突。

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
    //    必须在 run_frame 之前：System 消费 InputEvents，GridBuild 当帧读 ViewRect。
    EguiSceneHost::begin_frame(ui, &view, self.scheduler.resources_mut());

    self.scheduler.run_frame(dt);

    // 3. 内容提交：2D 队列 + 3D 队列 → painter（原 RenderSubmit 职责）。
    EguiSceneHost::present(ui, &view, self.scheduler.resources_mut());

    // ── 快捷键 F1（性能面板）/ F10（导出快照）+ health.update/show ──

    // ── 清空 DrawQueue(2D)（3D 队列由回调内部 take_snapshot 清空） ──
    if let Some(q) = self.scheduler.resources_mut().get_mut::<DrawQueue>() {
        q.clear();
    }

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
           → 现有 SceneRecordSystem::record_scene → DrawQueue → EguiPainter
```

关键点：

- **复用 `ir_to_draw_cmds`**：缩放与平移全部由它完成，桥接层不另写变换。
- **每个子轮廓单独一条路径**：用 `Path2D::flatten_subpaths` 按 `MoveTo` 分组
  （`flatten` 会把所有子路径拼成一条，多轮廓字形 `o`/`B`/`8` 会出现横穿字形内部
  的桥接线）；组内首尾点重合即传 `closed=true`。
- **字号解耦**：`FormulaSpec.nominal_font_size`（默认 12.0）是 Typst 编译标称字号，
  决定曲线提取精度与 IR 本地点尺度；`FormulaSpec.font_size` 是目标显示大小。
  展平容差是相对标称尺度的绝对值，显示尺寸缩小不会放大折线误差。
- **空心字**：旧管线 Fill 分支用 `convex_polygon`，非凸字形会被画成凸包，因此公式
  暂走 Stroke（轮廓）路径；实心填充待 S3 三角化后接管（挂 `CustomShapeData::to_triangles`）。
- **描边属世界空间**（`MeasureSpace::World`）：`ir_to_draw_cmds` 把宽度乘
  `target / nominal_font_size`（与几何同系数）换算为世界单位；fill-only 字形的默认细线宽
  同样按 `× scale` 换算。实机：缩放时公式描边与字形**成比例**变化
  （nominal=12 / target=1.3 / scale=40 → 约 4.3px）。
- **静态烘焙**：启动时编译一次并 spawn 实体；保留 `rebuild()`，将来接交互只需加 UI。

> 公式源在 `App::new` 里硬编码为 `"x^2 + 1"`。注意 Typst 0.14 数学模式下连续字母
> （如 `abcde`）会被解析为**单个未定义标识符**，多字母需空格分隔（`a b c d e`）。

---

## 形变（S1 插值内核）

两段 demo 共用同一条形变时钟 + `MorphT` 绑定链路（均落在 `SceneSync`）：

- **S1a 几何形变**：橙色形状 `MorphShape(圆, 方)`。`BindingTarget::MorphT` 写 `t`，
  `MorphApplySystem`（BindingApply 之后）每帧把
  `Path2D::morph_points(from, to, t, samples)` 写进同实体的 `Path`（`closed = true`），
  再由现有 `record_scene` 消费 —— **零录制改动**。
- **S1b 语义形变**：`formula::compile_morph_pairs("x^2", "x^3")` 用
  `xc_content::pair_by_anchor` 按**源码锚点**配对（`x` 配到 `x`，而非按位置配到 `3`），
  每对路径 spawn 一个 `MorphShape` 实体，共用同一形变时钟同步形变。

时钟：`PlayMode::PingPong` span `(0, 1)` + `Curve::Phase { omega: 1.0 }`
→ 形变参数 `t` 在 `[0, 1]` 平滑往返。**起点对齐**（原型约定）：圆的起笔方向与方的
首顶点方向取一致（均指向 -45°），避免形变过程"绕圈"。

> 铁律（路线甲）：`xc-time` 可依赖 `xc-geometry`，但**不得**依赖 `xc-content`——
> 语义配对留在 `xc-content`，`xc-time` 只提供插值原语。
>
> 度量空间：形变形状 / 网格 / 坐标轴 / sin 曲线 / 圆均为 `MeasureSpace::Screen`
> （粗细恒定）；只有公式描边是 `World`（随缩放成比例）。唯一换算点在
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

### 2D 管线开关（S3-1）

2D 内容有两条**并存**管线，由 `UiConfig::gpu_2d`（面板复选框）在
`EguiSceneHost::present` 运行期二选一：

- `gpu_2d = false`（默认）：`DrawQueue` 经 `EguiPainter` 在 CPU 侧就地回放（基线）；
- `gpu_2d = true`：2D 进 `SceneCompositorCallback` → `WgpuBackend2D` 离屏 `Rgba8Unorm` + MSAA 4x，
  再经 `Compositor` 按声明顺序混合回主 pass（与 3D 图层同在**同一个** `CompositeFrame` 内）。

> ⚠️ 合成四边形必须写死全 NDC 盒（`compositor::COMPOSITE_QUAD`）：egui-wgpu 会把 render pass
> viewport 设为**回调自身矩形**，若按整屏尺寸换算裁剪坐标会导致视口被二次应用
> （2D 视图 X 方向被压扁）。见 `xcal-render-backends` README 的 viewport 陷阱一节。

---

## 可观测性（`xcal-core/feature="telemetry"`）

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

```toml
xcal-core              = { features = ["egui"] }
xcal-render                             # 默认 feature="wgpu"
xcal-render-backends   = { features = ["egui-wgpu"] }
xcal-egui
xcal-macro
xc-content                              # 公式 IR 类型
xc-prerender           = { default-features = false }   # Typst / placeholder 引擎
xc-geometry                             # 形变 demo：直接构造 Path2D（MorphShape 的 from/to）

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
