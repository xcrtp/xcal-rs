# xcal-egui

> **UI 适配层**：`EguiSceneHost` 是 egui 与场景之间的**唯一**适配层——把每帧 egui
> 输入翻译为 `SceneEvent`、绘制面板 chrome、写 `ViewRect`、提交 2D/3D 内容；
> 另有 4 个符合 xcal-scene 六阶段调度约定的场景侧 `System`（相机输入、时间控制、
> 网格背景、场景录制），只依赖纯数据 Resource，可 headless 测试。

```
xcal-scene (场景组件 + 场景 System + domain 契约)
xcal-schedule (ECS 调度框架：Resource / System trait / 六阶段 Scheduler)
xcal-render (2D 管线 Painter2D / DrawQueue / Shape2D)
xcal-render-backends (EguiPainter / Compositor / SceneCompositorCallback)
xc-geometry (Rect2D：声明 GPU 2D 回调视口)
xc-macro (Transform2DInto 派生)
      │
      ▼
xcal-egui  ← 本 crate
      │
      ▼
xcal-app (装配 + ParamPanelSystem)
```

---

## 资源（Resource）一览

所有资源都实现了 `xcal_schedule::resource::Resource` (blanket for
`Send + Sync + 'static`)，可直接放入 `Resources` 并被各 System 声明读写：

| Resource | 归属 | 说明 |
|----------|------|------|
| `AppCamera { center: Vec2, scale: f32 }` | 本 crate | 2D 相机**纯状态**；输入处理已删除，控制器是纯函数 `apply_command`（消费 domain 的 `CameraCommand`）|
| `UiConfig { show_grid, show_field, gpu_2d, flip_layer_order }` | 本 crate | 面板显示开关 + S3-1 运行期 2D 管线开关（默认 `gpu_2d = false`）+ S3-2 图层顺序翻转（默认 `false`）|
| `ViewRect { rect }` | 本 crate | 中央视口在 egui 点坐标中的矩形（由 `EguiSceneHost::update_view_rect` 写入）|
| `TimeState` | 再导出自 xcal-scene | 每帧累计时钟（由 `DirectorAdvanceSystem` 推进）|
| `DirectorState` | xcal-scene | 时间轴 / 播放 / 速度（由 `TimeControlSystem` 消费事件修改）|
| `InputEvents`（`Vec<SceneEvent>`）| xcal-scene::domain | 帧首由 host 整帧替换写入；各阶段 System 只读 |
| `DrawQueue`（来自 xcal-render）| — | 2D 绘制命令队列（Prepare 写网格、RenderCommit 录场景）|
| `DrawQueue3D`（来自 xcal-render）| — | 3D 绘制命令队列（`EguiSceneHost::present` 消费）|

> `EguiFrameContext` **已删除**（B 阶段 UI 解耦）：egui 访问收敛到 `EguiSceneHost`，
> 场景侧 System 只依赖纯数据 `InputEvents`，因此可 headless 测试。

---

## System（按 Phase 分配）

本 crate 只保留 4 个**场景侧** System（不含任何 egui 类型）；纯 UI chrome
（控制面板 / 时间面板 / 参数面板）与内容提交都在 `EguiSceneHost`。

| Phase | System | 读写 | 功能 |
|-------|--------|------|------|
| Input | `CameraInputSystem` | R=`InputEvents`; W=`AppCamera` | 过滤 `SceneEvent::Camera`，委托 `AppCamera::apply_command`（PanZoom 模式）|
| LogicUpdate | `TimeControlSystem` | R=`InputEvents`; W=`DirectorState` | 消费 `Button(PLAY_PAUSE)` / `Slider(SPEED)` / `Slider(SEEK)` |
| Prepare | `GridBuildSystem` | R=`ViewRect`, `AppCamera`, `UiConfig`; W=`DrawQueue` | 视口底色 + 多级网格 / 坐标轴（`MeasureSpace::Screen`）|
| RenderCommit | `SceneRecordSystem` | R=`ViewRect`, `AppCamera`; W=`DrawQueue` | `record_scene(world, camera, dq)` 遍历 hecs World + `FunctionX2Y` 曲线 |

其他阶段由别的 crate 提供：Input 的 `DirectorAdvance`、SceneSync 的 `BindingApply`
（xcal-scene），LogicUpdate 的 `ParamPanel`（xcal-app）。`Phase::FrameEnd` 目前为空。

> `SceneRecordSystem` 同时需要 `&hecs::World` 与 `&mut DrawQueue`，通过
> `ctx.with_world_and_resource_mut::<DrawQueue, _>(...)` 做拆分借用。

---

## 面板 chrome

`EguiSceneHost`（`host.rs`）绘制的三个面板：

- **控制面板（左）**：`t` 读数、`显示网格` / `显示向量场` 复选框、
  **`GPU 2D 管线` 复选框**（S3-1 运行期开关，默认关）、
  **`翻转图层顺序` 复选框**（S3-2 验收开关，默认关）、相机 center/scale 读数。
- **时间面板（左）**：播放/暂停按钮、速度滑块、时间轴 seek 滑块。
- **参数面板（右）**：按 (curve, target) 展示绑定滑块——圆半径 `base/amp/omega`、
  函数相位 `omega`、3D 轨道相机 `radius/speed/angle`。

所有控件交互都以 `SceneEvent`（按 `WidgetId` 标识）上报，由场景侧 System 消费。

---

## `EguiSceneHost`：唯一 egui 触点

- `build_scene_view(world, res) -> SceneView`：场景 → UI，纯数据（2D 图层取
  `AppCamera` + `ViewRect`，3D 图层取 World 中第一个 `Camera3D`）。
- `begin_frame(ui, view, res)`：收集视口输入（滚轮 → `Zoom`、右键/中键拖拽 → `Pan`，
  位移在 host 内换算为**世界坐标**并做 Y 翻转）+ 三面板 chrome，整帧替换写入
  `InputEvents`；随后 `update_view_rect` 写 `ViewRect`（必须在 `run_frame` 之前，
  否则 resize 时内容慢一帧）。
- `present(ui, view, res)`：提交 2D 与 3D 内容，替代原 `RenderSubmitSystem` /
  `RenderSubmit3DSystem` / `GridBuild` 的全屏背景矩形。**顺序语义（S3-2）**：
  `SceneView::layers` 的**索引序 = 合成序**（先 = 下），本函数**遍历该列表**依次
  `painter.add`（调用顺序即绘制顺序）—— 不再是"3D 永远盖在 2D 上"；
  `UiConfig::flip_layer_order`（控制面板复选框）仅用于验收时翻转一次顺序。
  先铺 `SceneView::background`（原 `GridBuild` 的 `-1000` 矩形职责，C2 外移），
  图层全部在其之上。**2D 两条管路由 `UiConfig::gpu_2d` 运行期二选一**：

  | `gpu_2d` | 路径 |
  |----------|------|
  | `false`（默认）| `DrawQueue` 经 `EguiPainter` 在 CPU 侧回放（迁移前基线）|
  | `true` | `SceneCompositorCallback`：`prepare` 渲染离屏 MSAA 4x 透明底纹理，`paint` 经**合成器**混合回主 pass |

  3D 图层与 GPU 2D 图层进**同一个** `SceneCompositorCallback`（离屏 + 合成器），
  回调落在有效序中首个 GPU 图层的位置；CPU 2D 层在其声明位置就地回放。
  两个图层都声明 `LayerBlend::Alpha`：离屏是透明底，用 `Opaque`(REPLACE) 会把透明像素
  写进主 pass、抹掉背景与下层。CPU 路径**不支持真合成**（退化为顺序绘制，2D 仍在下，S3-4 退役后消失）。

> **单一回调（S3-2 修复）**：`egui_wgpu::CallbackResources` 是全局共享的 TypeMap，
> 每层一个回调会互相覆盖 `CompositeFrame`（实机现象：顺序 A 只见 3D、顺序 B 只见 2D）。
> 故 `present` 只注册一个回调，一次性合成全部图层。

---

## EguiPainter 的定位

`xcal_egui::EguiPainter` 实际由 **xcal-render-backends/egui_2d** 提供，本 crate
通过 `pub use xcal_render_backends::EguiPainter;` 重新导出。下游用户统一写：

```rust
use xcal_egui::EguiPainter;
```

不用关心具体模块归属。

---

## 依赖

```toml
xcal-scene              = { features = ["egui"] }              # egui feature 门控（如需）
xcal-render            = { default-features = false }         # 2D 管线（无 wgpu）
xcal-render-backends   = { features = ["egui", "egui-wgpu"] } # EguiPainter + Compositor/SceneCompositorCallback
xc-geometry                                                   # S3-1：以 Rect2D 声明回调视口
xc-macro
egui                   = "0.36"
egui-wgpu                                                     # present 提交 3D/GPU2D 回调
hecs / glam / derive-new / derive-builder
```

## 测试

```bash
cargo test -p xcal-egui
cargo doc  -p xcal-egui --no-deps
```

- `resources`：`AppCamera` / `UiConfig` Default（含 `!gpu_2d` / `!flip_layer_order` 断言）/ `Resources` 增删
- `systems`：`CameraInputSystem` zoom / clamp / pan / 无事件 4 例；`AppCamera::apply_command`
  忽略 Orbit/Dolly 1 例；`GridBuildSystem` 不再产出全屏背景矩形 1 例
- `time_control`：`TimeControlSystem` 播放切换 / speed+seek / 无关事件忽略 3 例
- `host`：`build_scene_view` 图层声明（含 `LayerBlend::Alpha` / `opacity` / 默认背景断言）/
  缺层跳过 / 2D 相机与 AppCamera 一致 3 例

以上均为 **headless**（无窗口、无 egui 上下文）构造 `Scheduler + Resources` 驱动。
