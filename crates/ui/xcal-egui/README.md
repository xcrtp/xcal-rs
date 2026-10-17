# xcal-egui

> **UI 适配层**：`EguiSceneHost` 是 egui 与场景之间的**唯一**适配层——把每帧 egui
> 输入翻译为 `SceneEvent`、绘制面板 chrome、写 `ViewRect`、提交 2D/3D 内容；
> 另有 2 个符合 xcal-scene 六阶段调度约定的场景侧 `System`（坐标轴刷新、函数图像
> 刷新），只依赖纯数据 Resource，可 headless 测试。
>
> **P1 / P4 归位**：相机状态搬进场景（本层不再持 `AppCamera`）、三个录制 System
> 搬到 `xcal-render`（R0 起该模块共**四个**：P4 三个 + 书写动画录制 `RevealRecordSystem`）、
> `ViewRect` 随之下沉 —— 详见下方"已迁出"。

```
xcal-scene (场景组件 + 场景 System + domain 契约)
xcal-schedule (ECS 调度框架：Resource / System trait / 六阶段 Scheduler)
xcal-render (2D 管线 Painter2D / DrawQueue / Shape2D)
xcal-render-backends (Compositor / SceneCompositorCallback)
xc-notion (Rect2D：声明 GPU 2D 回调视口)
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
| `UiConfig { show_grid, show_field, show_2d, show_3d, viewport_camera, free_2d_camera, flip_layer_order, …(滤镜四参) }` | 本 crate | 面板显示开关（`show_2d` / `show_3d` = 图层开关，默认都开）+ K3 视口操作目标（默认 `TwoD`）+ **`free_2d_camera`：2D 相机由交互还是分镜驱动（默认 `false` = 分镜）** + S3-2 图层顺序翻转（默认 `false`）|
| `TimeState` | 再导出自 xcal-scene | 每帧累计时钟（由 `DirectorAdvanceSystem` 推进）|
| `DirectorState` | xcal-scene | 时间轴 / 播放 / 速度（由 `TimeControlSystem` 消费事件修改）|
| `InputEvents`（`Vec<SceneEvent>`）| xcal-scene::domain | 帧首由 host 整帧替换写入；各阶段 System 只读 |
| `Camera2DHandle`（来自 xc-object）| — | 主 2D 相机的**引用型句柄**（P2）：指向场景里的相机对象（持 `Framing2D`）。相机状态本身**不是 Resource**（P1-1）|
| `ViewRect`（来自 xcal-render）| — | 本帧渲染目标矩形（P4 下沉）。由 `EguiSceneHost::update_view_rect` 写入；相机在录制时绑定它得到 `Camera2D`（P1-2 解绑）|
| `DrawQueue` / `DrawQueue3D` / `ContentTextureQueue` / `MorphQueue` / `RevealQueue`（来自 xcal-render）| — | 各渲染队列；由 `xcal-render::record_systems` 的四个录制 System 与 app 的 3D 录制写入（`RevealQueue` = R0 书写动画）|

> `AppCamera` **已删除**（P1-1）：相机状态是**场景对象**（`xc_object::Framing2D`），
> UI 只发 `CameraCommand`（控制器纯函数 `xcal_scene::camera_input::apply_pan_zoom`）。

> `EguiFrameContext` **已删除**（B 阶段 UI 解耦）：egui 访问收敛到 `EguiSceneHost`，
> 场景侧 System 只依赖纯数据 `InputEvents`，因此可 headless 测试。

---

## System（按 Phase 分配）

本 crate 只保留**场景侧** System（不含任何 egui 类型）；纯 UI chrome
（控制面板 / 时间面板 / 参数面板）与内容提交都在 `EguiSceneHost`。

| Phase | System | 读写 | 功能 |
|-------|--------|------|------|
| LogicUpdate | `TimeControlSystem` | R=`InputEvents`; W=`DirectorState` | 消费 `Button(PLAY_PAUSE)` / `Slider(SPEED)` / `Slider(SEEK)` |
| Prepare | `AxesRefreshSystem`（M4）| R=`ViewRect`, `Camera2DHandle`, `UiConfig`; W=— | 取本帧相机 → 调对象层 `Axes2D::refresh`（几何在 `xc-object`，本层只接线）|
| Prepare | `FunctionGraphRefreshSystem`（M4）| R=—; W=— | 取实体相位（`PhaseAnim`）→ 调对象层 `FunctionGraph::refresh`（写路径 = 契约层 `Scene::set`）|

### 已迁出（P1 / P4）

| System | 新家 | 理由 |
|---|---|---|
| `CameraInputSystem` | `xcal-scene::camera_input` | 相机状态归**场景对象**；控制器纯函数 `apply_pan_zoom` 一并搬走 |
| `SceneRecordSystem` | `xcal-render::record_systems` | 渲染动作住渲染层；不再读 UI 的 `ViewRect`（已下沉） |
| `MorphRecordSystem` | 同上 | 同上 |
| `ContentTextureRecordSystem` | 同上 | 同上；其**内联 hecs 查询**一并改为契约层 `for_each*`（"换场景实现要整体重写"→"只需换适配器"） |

其他阶段由别的 crate 提供：Input 的 `DirectorAdvance` / `CameraInput`、SceneSync 的
`BindingApply`（xcal-scene），LogicUpdate 的 `ParamPanel`（xcal-app）。
`Phase::FrameEnd` 目前为空。

> **M4：UI 层不含几何计算。** 原网格构建 System（相机裁剪 / 步长自适应 / 刻度线 /
> 坐标轴，279 行）已退役，四段逻辑整体归 `xc_object::Axes2D`；函数采样归
> `xc_object::FunctionGraph`。本 crate 因此不再读写相机缩放，也不产生任何
> 世界↔屏幕换算（换算只在 `xcal-render` 的唯一换算点）。
>
> P1：`AxesRefreshSystem` 的相机来自**场景对象**（经 `Camera2DHandle` 指认 + `ViewRect`
> 绑定视口）；只读世界解析相机、再取世界可变借用写线段，一读一写不重叠。

---

## 面板 chrome

`EguiSceneHost`（`host.rs`）绘制的三个面板：

- **控制面板（左）**：`t` 读数、`显示网格` / `显示向量场` 复选框、
  **`2D 相机` / `3D 相机` 复选框**（图层开关：关掉即 `build_scene_view` 不再声明该图层，
  等于关掉对应相机。**N3：这是图层级"总闸"、优先级最高** —— 关掉即整层不画，
  **不看**对象的场景级 `Visibility`；总闸开时才轮到录制路径按对象 tag 逐对象判）、
  **`视口操作` 单选**（K3：`2D 相机（平移/缩放）` / `3D 轨道（拖拽/滚轮）` /
  `3D 第一人称（拖拽 + WASD/QE）` —— 2D/3D 同屏时由它决定拖拽/滚轮操作谁）、
  **`2D 相机自由（关闭=分镜驱动）` 复选框**（K3 补：决定 2D 相机由分镜的
  `Camera2DFraming` 绑定驱动，还是交给用户平移/缩放 —— 打开即让分镜绑定让位）、
  **`合成后滤镜` 开关 + 强度滑块**（C 线：亮度 / 桶形畸变 / 模糊；由
  `UiConfig::post_filters()` 编成 `PostFilterChain` 写回 Resource，全关 ⇒ 空链 ⇒ 直通）、
  **`翻转图层顺序` 复选框**（S3-2 验收开关，默认关）、相机 center/scale 读数
  （值取自本帧 `SceneView` 的 2D 图层 —— UI 不再持有相机状态）。
- **时间面板（左）**：播放/暂停按钮、**`⟲ 重新播放` 按钮**（拨回 `t = 0` 并恢复播放 →
  `Button(REPLAY)` → `TimeControlSystem`）、速度滑块、时间轴 seek 滑块。
- **参数面板（右）**：**按曲线类型**展示绑定滑块——圆半径 `base/amp/omega`、
  函数相位 `omega`、3D 轨道相机 `radius/speed/angle`（标签取 `Target::name()`，
  可选中文映射缺失时回退原名；写回由场景侧按 `WidgetId` 完成）。

所有控件交互都以 `SceneEvent`（按 `WidgetId` 标识）上报，由场景侧 System 消费。

---

## `EguiSceneHost`：唯一 egui 触点

- `build_scene_view(world, res) -> SceneView`：场景 → UI，纯数据（2D 图层取
  `Camera2DHandle` 指向的场景相机对象 + `ViewRect` 绑定，3D 图层取 World 中第一个
  `Camera3D`）。
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
  图层全部在其之上。**S4.1 后 2D / 3D / 内容纹理全部走 GPU**，统一进同一个
  `SceneCompositorCallback`：

  | 路径 | 说明 |
  |------|------|
  | `SceneCompositorCallback`（唯一路径）| `prepare` 逐层渲染离屏（2D 走 `WgpuBackend2D` 的 MSAA 4x 透明底纹理），`paint` 经**合成器**按声明序混合回主 pass |

  两个图层都声明 `LayerBlend::Alpha`：离屏是透明底，用 `Opaque`(REPLACE) 会把透明像素
  写进主 pass、抹掉背景与下层。
  （S4.1 前的 CPU 回放路径不支持真合成，已随其编译期 feature 一并删除。）
- `install_cjk_font(ctx) -> Option<PathBuf>`：安装 CJK 字体 —— 候选路径覆盖
  Windows / macOS / 常见 Linux 发行版，可用 `XCAL_CJK_FONT` 指定；找不到**不 panic**
  （返回 `None`，提示文案由调用方决定）。
- `draw_call_count(res) -> usize`：本帧绘制调用数（2D 命令 + 3D mesh/line），供 health overlay。
  ⚠️ `FrameReport` / `FrameCacheStats` 均无该字段（实测），故保留"查队列"口径、只把取数收进 host。
- `end_frame(res)`：帧末清理（清空已被 `present` 消费的 2D 队列）—— 帧管理归 host，app 只表达"这一帧结束了"。

> **单一回调（S3-2 修复）**：`egui_wgpu::CallbackResources` 是全局共享的 TypeMap，
> 每层一个回调会互相覆盖 `CompositeFrame`（实机现象：顺序 A 只见 3D、顺序 B 只见 2D）。
> 故 `present` 只注册一个回调，一次性合成全部图层。

---

## 2D 渲染路径（S4.1 后）

`xcal_egui` 不再导出任何 `Painter2D` 实现。旧 CPU 回放实现
（`xcal-render-backends` 的 `egui_2d` 模块）与其编译期 feature
已在 S4.1 一并删除——2D 统一走 `xcal-render-backends::wgpu_2d`
（lyon 三角化 + 离屏 MSAA + 合成），与 3D 汇入同一个 `SceneCompositorCallback`。

---

## 依赖

```toml
xcal-scene                                                    # 场景组件 + System + domain 契约
xcal-schedule                                                 # System / ExecutorContext / Resources
xcal-render            = { default-features = false }         # 2D 录制管线 + `ViewRect`（P4 下沉）
xcal-render-backends   = { default-features = false, features = ["egui-wgpu"] } # Compositor / SceneCompositorCallback
xc-notion                                                   # S3-1：以 Rect2D 声明回调视口
xc-object                                                   # 相机句柄 / 取景参数 / 图元
xc-macro
egui                   = "0.36"
egui-wgpu                                                     # present 提交 3D/GPU2D 回调
hecs / glam / derive-new / derive-builder
```

本 crate **无自有 feature**（S4.1 删除了转发旧 CPU 回放的那个）。

## 测试

```bash
cargo test -p xcal-egui
cargo doc  -p xcal-egui --no-deps
```

- `resources`：`UiConfig` Default（含 `!flip_layer_order` 断言）/ `Resources` 增删
- `systems`：`AxesRefreshSystem` 每帧清旧建新 + `show_grid` 开关 + 无相机句柄跳过 3 例；
  `FunctionGraphRefreshSystem` 按实体相位重采样 1 例
- `time_control`：`TimeControlSystem` 播放切换 / **重播（拨回 0 并恢复播放）** /
  speed+seek / 无关事件忽略 4 例
- `host`：`build_scene_view` 图层声明（含 `LayerBlend::Alpha` / `opacity` / 默认背景断言）/
  缺层跳过 / 2D 相机 = 取景参数绑定 `ViewRect` 3 例

> 迁出的 System 由新家自带测试：`xcal-scene::camera_input`（zoom/clamp/pan/无事件/失效句柄/
> 控制器忽略 Orbit）与 `xcal-render::record_systems`（相机→视口绑定、失效句柄跳过、
> 形变项、内容纹理过滤）。

以上均为 **headless**（无窗口、无 egui 上下文）构造 `Scheduler + Resources` 驱动。
