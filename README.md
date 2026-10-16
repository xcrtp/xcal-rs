# xcal-rs

基于 ECS（hecs）与 wgpu 的 2D/3D 渲染实验框架：分阶段的 System 调度器、
Painter 抽象、egui UI overlay，以及 3D 渲染层（Camera3D /
MeshGeometry / Material3D / WgpuBackend3D）。

## 架构总览

workspace 采用分层目录结构 `crates/<layer>/`，通用基础 crate 用 `xc-` 前缀，
领域/应用层保留 `xcal-` 前缀。依赖方向单向：**xc-* ← xcal-* ← app**，反向禁止。

```text
crates/
├── kernel/          # 纯数据层（xc-prerender 为例外：kernel 层唯一重 crate，重型依赖 feature-gated）
│   ├── xc-notion       理念层/数学元语（Color32 / Rect2D / Path2D / Path3D / 图元 / Transform2D）
│   ├── xc-time           时钟树 + 曲线 + 导演（Director 唯一时间源，纯数据纯函数）
│   ├── xc-content        内容中间表示（ContentIR + 源码归一化 + 确定性 SVG + 占位排版）
│   ├── xc-prerender      内容预渲染引擎接入（Typst / placeholder + 哈希缓存），默认零重依赖
│   ├── xc-vertex         顶点格式契约（VertexLayout / AaModel / CacheStrategy，无 wgpu 依赖）
│   ├── xc-resource       GPU 资源契约与缓存（ResourceKey / ResourceCache / GpuHandle / DeferredDestroyer，纯 std）
│   └── xc-macro          Transform2DInto 派生宏
│
├── graph/           # 节点图契约层（禁止依赖 egui / xcal-* / 领域类型）
│   ├── xc-node           节点图内核（Node / Graph / Socket trait + NodeInteraction）
│   ├── xc-graph-algo     图算法（TopoSort / SCC / 脏标记传播）
│   ├── xc-graph-eval     惰性拉取求值（LazyPullEval）
│   ├── xc-node-schema    属性模型（Property / NodeSpec / DynamicObject / NodeRegistry）
│   └── xc-node-schema-macros  #[derive(NodeSpec)] 过程宏
│
├── schedule/        # 通用 ECS 调度框架（不含任何场景/渲染概念）
│   └── xcal-schedule     Resource / System trait / 六阶段 Scheduler + Telemetry（依赖仅 hecs）
│
├── render/          # 场景层 + 渲染层
│   ├── xcal-scene         场景层：场景组件 + 场景 System + domain 契约 + Transform2D + 3D 场景类型
│   ├── xcal-render       2D/3D 渲染抽象（Painter / DrawQueue / Shape / RenderLayer）
│   └── xcal-render-backends  wgpu / egui-wgpu 具体后端（WgpuBackend3D / WgpuBackend2D）
│
├── ui/              # UI 与编辑器层
│   ├── xcal-egui         egui UI overlay System（控制面板 / 网格 / 相机）
│   ├── xcal-node-editor  纯逻辑节点编辑器状态机（Command / Undo / HitTestPolicy）
│   ├── xcal-node-egui    egui 画布（pan/zoom / 卡片 / 连线 / resize / 磁吸）
│   └── xcal-node-egui-ui 属性→egui 控件映射（PropertyNodeView / draw_property）
│
└── app/             # 应用组装层
    └── xcal-app          eframe 入口（Scheduler 构建 / Resource 注入 / System 注册）
```

依赖铁律：
- `xc-*` crate 不得依赖任何 `xcal-*` crate 或 egui/wgpu
- `xcal-*` crate 可依赖 `xc-*`，但不得依赖 `xcal-app`
- `xcal-app` 可依赖一切，是唯一组装入口

## Crate 一览

| Crate | 层 | 职责 |
|-------|----|------|
| `xc-notion` | kernel | 理念层（数学元语）：Vec/Mat 再导出 / Color32 / Rect2D / Path2D·Path3D / 2D 图元 / Transform2D·3D / Lerp / 函数·曲线·采样器 / 投影辅助（全仓唯一 glam 出口） |
| `xc-time` | kernel | 纯逻辑时间内核：ClockForest（时钟树+环检测+eval_local）/ Curve（Const/Sin/Orbit/Phase/Sampled 纯函数 sample）/ Director（global_t + speed + playing，唯一时间源） |
| `xc-content` | kernel | 内容中间表示：ContentIR / StyledPath / Span 锚点 + 源码归一化 + 确定性 SVG 量具 + 零依赖占位排版 |
| `xc-prerender` | kernel | 内容预渲染：ContentEngine 异步形状 API + InMemoryEngine + 哈希缓存 + Typst 接入（feature 全 optional，默认零重依赖） |
| `xc-vertex` | kernel | 顶点格式契约（VertexLayout / VertexAttrib / AaModel / CacheStrategy；无 wgpu 依赖，可序列化） |
| `xc-resource` | kernel | GPU 资源契约与缓存（GpuResourceId / GpuHandle / ResourceKey / ResourceCache / DeferredDestroyer；纯 std 零依赖） |
| `xc-macro` | kernel | `Transform2DInto` 派生过程宏 |
| `xc-node` | graph | 节点图内核（Node / Graph / Socket trait + NodeInteraction 交互策略） |
| `xc-graph-algo` | graph | 图算法（TopoSort / SCC / 脏标记传播） |
| `xc-graph-eval` | graph | 惰性拉取求值（LazyPullEval） |
| `xc-node-schema` | graph | 属性模型（Property / NodeSpec / DynamicObject / NodeRegistry），不依赖 egui |
| `xc-node-schema-macros` | graph | `#[derive(NodeSpec)]` 过程宏：`#[node(...)]`（含 `resizable/draggable/selectable`）+ `#[property(...)]` |
| `xcal-schedule` | schedule | 通用 ECS 调度框架（Resource / System trait / 六阶段 Scheduler + Telemetry）；仅依赖 hecs，不含任何场景/渲染概念 |
| `xcal-scene` | render | 场景层：场景组件 + 场景 System（BindingApply / DirectorAdvance / MorphApply）+ domain 契约（CameraCommand / SceneView / ContentHandle）+ Transform2D 体系 + 3D 场景类型 + TimeState；依赖 `xcal-schedule` |
| `xcal-render` | render | 2D/3D 渲染抽象（Painter / DrawQueue / Shape / RenderLayer / Viewport） |
| `xcal-render-backends` | render | wgpu / egui-wgpu 具体后端（WgpuBackend3D / WgpuBackend2D / Compositor / SceneCompositorCallback） |
| `xcal-egui` | ui | egui UI overlay System（控制面板 / 网格 / 相机输入 / **TimeControlSystem 时间控制**） |
| `xcal-node-editor` | ui | 纯逻辑节点编辑器状态机（Command / Undo / 可插拔 HitTestPolicy） |
| `xcal-node-egui` | ui | egui 画布（pan/zoom / 卡片 / 连线 / resize / 磁吸） |
| `xcal-node-egui-ui` | ui | 属性→egui 控件映射（PropertyNodeView / draw_property / spawn_menu） |
| `xcal-app` | app | 应用组装：Scheduler 构建 / Resource 注入 / System 注册 / egui-wgpu 回调 |

## 动画架构（Director 唯一时间源）

旧：`TimeUpdateSystem` 直接累积 `elapsed` → `AnimationApplySystem` 组件组合查询写动画字段。
新：**时间单向管道**，全部纯函数，不依赖渲染帧率：

```
墙钟 Instant::now()
  → DirectorAdvanceSystem (Input 阶段, Writes=DirectorState+TimeState)
      director.advance(dt)     // global_t = (global_t + speed*dt) 或 seek
      TimeState.elapsed = global_t
  → BindingApplySystem (SceneSync 阶段, Reads=TimeState+ClockForest+BindingTable)
      对每条 Binding:
        local_t = ClockForest.eval_local(clock, global_t)   // 纯函数，支持 Loop/PingPong 等
        value   = Curve.sample(local_t)                      // Const/Sin/Orbit/Phase/Sampled
        写入目标组件字段 (Circle.radius / Camera3D.eye / ...)
  → SceneRecord → 渲染
```

铁律：
- 动画纯函数：永不累积状态 → seek/倒放免费正确
- Binding 表取代组件耦合：新增动画 = 加一行 Binding，不改 System 源码
- 时钟树纯数据 + 纯函数：脱离 hecs，可序列化可测试
- 墙钟时间：`Instant::now()` 算 dt，帧率波动不影响时间流速
- UI slider 的 seek 只在 `drag_stopped()` 触发，step_by 会把 global_t 对齐到网格导致卡死

## 3D 帧流水线

1. **SceneSync**：`BindingApplySystem` 根据 `BindingTable` 把动画值写入
   `Camera3D.eye`（轨道）等组件，取代旧的 `OrbitAnim` 组件 + `OrbitApplySystem`。
2. **RenderCommit**：`SceneRecord3DSystem` 遍历 World 中的 `MeshGeometry`
   （+ 可选 `Material3D` / `Transform`），清空并填充 `DrawQueue3D`。
3. **FrameEnd**：`EguiSceneHost::present` 取中央视口矩形，构造**单个**
   `SceneCompositorCallback`（含全部 `SceneLayer` 声明 + `DrawQueue`/`DrawQueue3D` 快照）
   注册为 egui paint callback；egui-wgpu 在主 render pass 内回调：
   - `prepare`：逐层渲染到各自离屏纹理（`WgpuBackend2D` / `WgpuBackend3D::render_layer_offscreen`），
     收集 `CompositeOp` → 一次 `Compositor::prepare` 得到单个 `CompositeFrame`；
   - `paint`：`Compositor::composite_layers` 按声明顺序混合（层序 = `SceneView::layers` 索引序）。

## 运行

```shell
# 完整应用（2D 网格 + 3D 线框立方体 + 轨道相机 + 控制面板）
cargo run -p xcal-app

# 独立 3D demo（不依赖 xcal-app / scheduler，直接演示 WgpuBackend3D）
cargo run -p xcal-render-backends --example cube_3d

# 节点编辑器 demo（多输入/多输出 pin、卡片 resize、连线磁吸、撤销重做）
cargo run -p xcal-node-egui --example node_editor_demo

# 全量测试 / 文档
cargo test --workspace
cargo doc --no-deps -p xcal-scene -p xcal-render
```
