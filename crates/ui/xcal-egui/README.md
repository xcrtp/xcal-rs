# xcal-egui

> **UI overlay 层**：把 `egui 0.36` 的每一帧 UI 拆成一组符合 xcal-core 六阶段
> 调度约定的 `System`，提供控制面板、网格背景、相机输入、视口矩形、
> 2D 绘制提交等功能。

```
xcal-core (System/Resource/Scheduler)
xcal-render (冻结 2D 管线 Painter2D / DrawQueue)
xcal-render-backends (EguiPainter)
xcal-macro (Transform2DInto 派生)
      │
      ▼
xcal-egui  ← 本 crate
      │
      ▼
xcal-app (装配)
```

---

## 资源（Resource）一览

所有资源都实现了 `xcal_core::resource::Resource` (blanket for
`Send + Sync + 'static`)，可直接放入 `Resources` 并被各 System 声明读写：

| Resource | 读/写在哪些阶段 | 说明 |
|----------|-----------------|------|
| `TimeState { elapsed, dt, tick }` | Input **W**; LogicUpdate **R**; SceneSync **R** | 每帧累计时钟（由 `TimeUpdateSystem` 加 `dt`）|
| `AppCamera { zoom, offset, anchor }` | Input **W**; Prepare **R**; RenderCommit **R** | 2D 视图相机（pan/zoom，由鼠标/键盘驱动）|
| `UiConfig { grid, snap, palette, show_axis }` | LogicUpdate **W**; Prepare **R** | 由控制面板修改的全局 UI 配置 |
| `ViewRect { min, max }` | SceneSync **W**; Prepare **R**; RenderCommit **R**; FrameEnd **R** | 中央视口在 egui 像素坐标中的矩形（`ViewRectUpdateSystem` 从 `EguiFrameContext` 获取）|
| `EguiFrameContext(&'ui egui::Ui)` | Input 注入/FrameEnd 移除; 各阶段 **R** | 将每帧短暂的 `&egui::Ui` 降级为 Resource，实现 system 间传递 |
| `DrawQueue` (来自 xcal-render) | Prepare **W**; RenderCommit **W**; FrameEnd **R+W** | 2D 绘制命令队列 |

---

## System（按 Phase 分配）

每一 phase 的 Read/Write 资源集合都经过精心分配，保证**同 phase 内任意两
System 不冲突**（debug 下 BorrowTracker 会验证）：

### Phase::Input
| System | 读写 | 功能 |
|--------|------|------|
| `TimeUpdateSystem` | W=TimeState | `time.elapsed += dt; time.tick += 1` |
| `CameraInputSystem` | R=EguiFrameContext; W=AppCamera | 处理滚轮缩放（鼠标锚点）、中键拖拽平移、方向键 pan |

### Phase::LogicUpdate
| System | 读写 | 功能 |
|--------|------|------|
| `ControlPanelSystem` | R=TimeState, AppCamera, EguiFrameContext; W=UiConfig | 绘制 egui 侧栏：网格开关/颜色、缩放重置、相机显示、FPS 文本 |

### Phase::SceneSync
| System | 读写 | 功能 |
|--------|------|------|
| `ViewRectUpdateSystem` | R=EguiFrameContext; W=ViewRect | 从当前 ui.max_rect() 减去侧栏宽度得到中央视口 |
| `AnimationApplySystem` | R=TimeState; W=() (写 hecs 组件) | 按 `LocalClock + CircleAnim/PhaseAnim` 更新 Circle 半径 / FunctionX2Y 相位 |

### Phase::Prepare
| System | 读写 | 功能 |
|--------|------|------|
| `GridBuildSystem` | R=ViewRect, AppCamera, UiConfig; W=DrawQueue | 根据视口 + 缩放级别绘制 1×1, 10×10, 100×100 多级网格线 |

### Phase::RenderCommit
| System | 读写 | 功能 |
|--------|------|------|
| `SceneRecordSystem` | R=ViewRect, AppCamera; W=DrawQueue | 调 `xcal_render::record_scene(world, camera, view, dq)` 遍历 hecs World |

### Phase::FrameEnd
| System | 读写 | 功能 |
|--------|------|------|
| `RenderSubmitSystem` | R=ViewRect, EguiFrameContext, DrawQueue; W=()（通过 EguiPainter 消费 DrawQueue）| 取 `ui.painter_at(central_rect)`，以 `EguiPainter` flush 命令 |

> 注意：`AnimationApplySystem` / 其他 scene 系统通过
> `ctx.with_world_and_resource_mut::<R, _>(...)` 实现同时访问 `&hecs::World`
> 和 `&mut Resource` 的拆分借用。

---

## 控制面板面交互

`ControlPanelSystem` 暴露的 UI 控件：

- **时间**：elapsed/tick 显示，暂停/恢复按钮（设置 `ClockState`）
- **相机**：当前 zoom + offset 显示，一键 `重置视图`
- **网格**：`显示/隐藏`、主次网格颜色、轴线颜色、`吸附到网格` 开关
- **调试**：Health Overlay (F1) 切换 / World Snapshot (F10) 开关按钮

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
xcal-core              = { features = ["egui"] }     # EguiFrameContext 需要 feature = "egui"
xcal-render            = { default-features = false }# 仅用冻结 2D 管线（无 wgpu）
xcal-render-backends   = { features = ["egui"] }     # EguiPainter + color_to_egui
xcal-macro
egui                   = "0.36"
hecs / glam / derive-new / derive-builder
```

## 测试

```bash
cargo test -p xcal-egui
cargo doc  -p xcal-egui --no-deps
```

主要覆盖资源类型的 Default / round-trip 属性；系统行为依赖 `xcal-app`
端到端运行与 `cargo test --workspace` 中的 hecs 查询集成。
