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
   - 可多级缩放的网格背景（1 / 10 / 100 三档，视缩放自动淡入淡出）
2. **左侧控制面板（egui）**：时间、相机、网格、调试切换
3. **右上角 Health Overlay（F1 切换）**：FPS、tick、实体数、绘制调用数
4. **快捷键**：
   - `F1` — 切换性能面板
   - `F10` — 导出当前 `hecs::World + Resources` 快照到 `snapshots/tick_{n}.json`
   - 滚轮缩放、中键/右键拖拽平移、方向键 pan（CameraInputSystem）

---

## App::new 初始化流程（关键代码）

```rust
// ── 1. 字体：尝试加载微软雅黑，否则用 egui 默认（中文会变成方框） ──
Self::setup_fonts(&ctx.egui_ctx);

// ── 2. RenderContext + WgpuBackend3D（回调资源） ──
let rs = ctx.wgpu_render_state.clone().unwrap();
let backend_3d = WgpuBackend3D::with_default_shader(&rs.device, &rs.queue, rs.target_format);
rs.renderer.write().callback_resources.insert(backend_3d);  // &self 访问
let render_ctx = RenderContext::from_render_state(&rs);

// ── 3. Scene 填充 hecs World ──
let mut scene = Scene::new();
scene.spawn((wireframe_cube(1.0, Color32::WHITE),));
scene.spawn((Camera3D::new(...), OrbitAnim::new(0.5, 5.0, Vec3::Y, 0.0)));
scene.spawn((Circle, StrokeSolid, LocalClock, CircleAnim));      // 振动圆
scene.spawn((FunctionX2Y::new(|x| x.sin()), Sampler1D, ...));    // sin 曲线

// ── 4. Resources 注入 ──
let mut r = Resources::new();
r.insert(AppCamera::default());
r.insert(TimeState::default());
r.insert(UiConfig::default());
r.insert(ViewRect::default());
r.insert(DrawQueue::new());
r.insert(DrawQueue3D::new());
r.insert(render_ctx.clone());

// ── 5. Scheduler 注册 System（参见"阶段分配"） ──
let mut sched = Scheduler::new(scene.into_world(), r);
sched.add_system(Phase::Input,        TimeUpdateSystem);
sched.add_system(Phase::Input,        CameraInputSystem);
sched.add_system(Phase::LogicUpdate,  ControlPanelSystem);
sched.add_system(Phase::SceneSync,    ViewRectUpdateSystem);
sched.add_system(Phase::SceneSync,    AnimationApplySystem);
sched.add_system(Phase::SceneSync,    OrbitApplySystem);         // 3D 专用
sched.add_system(Phase::Prepare,      GridBuildSystem);
sched.add_system(Phase::RenderCommit, SceneRecordSystem);
sched.add_system(Phase::RenderCommit, SceneRecord3DSystem);      // 3D 专用
sched.add_system(Phase::FrameEnd,     RenderSubmitSystem);
sched.add_system(Phase::FrameEnd,     RenderSubmit3DSystem);     // 3D 专用
```

---

## 阶段分配（完整）

| Phase          | System                     | 资源写集                 | 资源读集 |
|----------------|----------------------------|--------------------------|----------|
| **Input**      | TimeUpdate                 | TimeState                | —        |
|                | CameraInput                | AppCamera                | EguiFrameContext |
| **LogicUpdate**| ControlPanel               | UiConfig                 | TimeState, AppCamera, EguiFrameContext |
| **SceneSync**  | ViewRectUpdate             | ViewRect                 | EguiFrameContext |
|                | AnimationApply (2D)        | hecs 组件 (无 Resource)  | TimeState |
|                | OrbitApply (3D)            | hecs Camera3D + OrbitAnim| TimeState |
| **Prepare**    | GridBuild                  | DrawQueue (2D)           | ViewRect, AppCamera, UiConfig |
| **RenderCommit**| SceneRecord (2D)          | DrawQueue (2D)           | ViewRect, AppCamera |
|                | SceneRecord3D (3D)         | DrawQueue3D              | hecs::World 组件 |
| **FrameEnd**   | RenderSubmit (2D)          | —  (consume DrawQueue)   | ViewRect, EguiFrameContext, DrawQueue |
|                | RenderSubmit3D (3D)        | —  (提交 egui 回调)      | EguiFrameContext, DrawQueue3D, ViewRect |

同 phase 中 `ViewRectUpdate` / `AnimationApply` / `OrbitApply` 读写集互不相交；
`SceneRecord` 写 `DrawQueue` vs `SceneRecord3D` 写 `DrawQueue3D`（类型无交集）
——debug 模式下 `BorrowTracker` 不会触发冲突。

---

## eframe::App::ui 每帧主循环

```rust
fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
    let dt = ui.input(|i| i.unstable_dt).min(0.1);

    // ── 注入临时资源（每帧都换，因为 &egui::Ui 生命周期短暂） ──
    self.scheduler.resources_mut().insert(EguiFrameContext::new(ui));

    // ── 六阶段主循环 ──
    self.scheduler.run_frame(dt);

    // ── 快捷键：F1/F10；健康面板更新；清理 EguiFrameContext ──
    // ... (health.toggle + dump_snapshot + health.update + health.show)

    self.scheduler.resources_mut().remove::<EguiFrameContext>();

    // ── 清空 DrawQueue(2D)（3D 队列由回调内部 take_snapshot 清空） ──
    if let Some(q) = self.scheduler.resources_mut().get_mut::<DrawQueue>() {
        q.clear();
    }

    // ⚠️ egui 默认是"事件驱动"，无输入时不重画 → 动画停住。
    //    每帧主动请求，保证 TimeState.elapsed 持续增长。
    ui.ctx().request_repaint();
}
```

---

## 3D 合成关键：为什么在 Layer3DCallback 里

`egui-wgpu` 的 paint callback 有两个重要特性：

1. 与 egui 主 render pass **同一次 render pass** 内执行，省掉一次 subpass
   切换开销；
2. scissor 可限制在中央视口，**天然把 3D 限制在非侧栏区域**。

所以**绝对不能**用 `Order::Background`（会在主 pass 之前提交、被侧栏覆盖），
而是 `ui.painter_at(central_rect).add(Shape::Callback(...))`——同一 layer 内
"后提交 = 在上面"。详见 `xcal-render-backends` 文档。

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

eframe = { version = "0.36", default-features = false, features = ["wgpu","x11","wayland"] }
egui / egui-wgpu / wgpu 30
hecs / glam
derive-new / derive-builder
tracing 0.1
```

---

## 运行 / 测试

```bash
cargo run   -p xcal-app                               # 启动主 GUI（桌面环境）
cargo test  -p xcal-app                               # health.rs 单元测试
cargo clippy -p xcal-app --all-targets -- -D warnings # 严格检查
```
