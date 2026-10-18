# xcal-rs

基于 ECS（hecs）与 wgpu 的 2D/3D 渲染实验框架：分阶段的 System 调度器、
Painter 抽象、egui UI overlay、3D 渲染层，以及语言无关的 Pod 边界
（Lua / Python 在装配期编排场景）。

## 架构总览

workspace 采用分层目录结构 `crates/<layer>/`，通用基础 crate 用 `xc-` 前缀，
领域/应用层保留 `xcal-` 前缀。分层顺序为
**kernel → object / graph / script → schedule → scene → render → ui → app**
（允许上层依赖下层，禁止反向）。

```text
crates/
├── kernel/          # 纯数据层（xc-prerender 为例外：kernel 层唯一重 crate，重型依赖 feature-gated）
│   ├── xc-notion           理念层/数学元语（Color32 / Rect2D / Path2D / Path3D / 图元 / Transform2D）
│   ├── xc-render-ir        渲染 IR（Shape2D / Style2D / ToShape2D / ResolveStyle；不认识 hecs）
│   ├── xc-scene-contract   场景契约（Scene / ObjectId / Parent / Describable；hecs 只在实现方）
│   ├── xc-time             时钟树 + 曲线 + 导演（Director 唯一时间源，纯数据纯函数）
│   ├── xc-content          内容中间表示（ContentIR + 源码归一化 + 确定性 SVG + 占位排版）
│   ├── xc-prerender        内容预渲染引擎接入（Typst / placeholder + 哈希缓存），默认零重依赖
│   ├── xc-vertex           顶点格式契约（VertexLayout / AaModel / CacheStrategy，无 wgpu 依赖）
│   ├── xc-resource         GPU 资源契约与缓存（ResourceKey / ResourceCache / GpuHandle，纯 std）
│   ├── xc-macro            Transform2DInto 派生宏
│
├── pod/             # 领域无关的 Pod 层（生产依赖不含任何 xcal-*）
│   ├── xc-pod              Pod 机制（capability 协商 / 不透明句柄 / Injector / Broker）
│   ├── xc-pod-binding      语言无关绑定驱动（BindingPlan / ValueCodec / JSON 文档）
│   ├── xc-pod-macro        #[capability] 与 #[derive(PodType)] 过程宏
│   ├── xc-pod-lua          Lua 适配器（mlua，解释器 vendored）：装配期脚本 → SceneScript
│   ├── xc-pod-py           Python 适配器（pyo3，feature 门控）：装配期批量能力 + .pyi 导出
│   └── xc-pod-tests        集成测试 + 基准（publish = false；mini pod / topic / Ord / flush）
│
├── object/          # 场景对象层（只依赖 kernel）
│   └── xc-object           图元 newtype / 相机 / 变换 / 装配对象（Axes2D · FunctionGraph）
│
├── graph/           # 节点图契约层（禁止依赖 egui / xcal-* / 领域类型）
│   ├── xc-node             节点图内核（Node / Graph / Socket trait + NodeInteraction）
│   ├── xc-graph-algo       图算法（TopoSort / SCC / 脏标记传播）
│   ├── xc-graph-eval       惰性拉取求值（LazyPullEval）
│   ├── xc-node-schema      属性模型（Property / NodeSpec / DynamicObject / NodeRegistry）
│   └── xc-node-schema-macros  #[derive(NodeSpec)] 过程宏
│
├── schedule/        # 通用 ECS 调度框架（不含任何场景/渲染概念）
│   └── xcal-schedule       Resource / System trait / 六阶段 Scheduler + Telemetry（依赖仅 hecs）
│
├── scene/           # 场景层
│   ├── xcal-scene          ECS 场景数据 + 场景 System + domain 契约 + Pod domain（SceneApi / ContentApi）
│   └── xcal-scene-bundle   XSB 装配产物（格式 / payload 哈希 / 可还原性检查；无脚本依赖）
│
├── render/          # 渲染抽象与具体后端
│   ├── xcal-render         2D/3D 渲染抽象（Painter / DrawQueue / Shape2D / RenderLayer）
│   └── xcal-render-backends  wgpu / egui-wgpu 具体后端（WgpuBackend3D / WgpuBackend2D / Compositor）
│
├── ui/              # UI 与编辑器层
│   ├── xcal-egui           egui 适配层（输入翻译 / 面板 chrome / 内容提交；不含场景 System）
│   ├── xcal-node-editor    纯逻辑节点编辑器状态机（Command / Undo / HitTestPolicy）
│   ├── xcal-node-egui      egui 画布（pan/zoom / 卡片 / 连线 / resize / 磁吸）
│   └── xcal-node-egui-ui   属性→egui 控件映射（PropertyNodeView / draw_property）
│
└── app/             # 应用外壳层
    ├── xcal-app          组装（无 UI）：Runtime / Pod 装配 / Resource 注入 / System 注册
    ├── xcal-gui          GUI 可执行壳（eframe::App）
    └── xcal-compile      XSB 独立编译器（脚本 → .xsb）
```

依赖铁律：
- `xc-*` crate 不得依赖任何 `xcal-*` crate 或 egui/wgpu
- `xcal-*` crate 可依赖 `xc-*`，但不得依赖 `xcal-app`
- `xcal-app` 是唯一组装入口；`xcal-gui` / `xcal-compile` 只是调用它的壳
  （**`xcal-app` 不依赖 `xcal-egui`**）
- 语言适配器（`xc-pod-lua` / `xc-pod-py`）属于 `crates/pod/` 的语言适配器组，**生产依赖不含任何 `xcal-*`**；
  domain 能力由 App 通过 `register_capability::<SceneApi>(..)` 注入
- `crates/pod/` 的核心组（`xc-pod` / `xc-pod-binding` / `xc-pod-macro`）平台无关，必须过 wasm gate；
  语言适配器组（mlua / pyo3）**不在** wasm gate 名单内（CI 注释已注明原因）

## Crate 一览

| Crate | 层 | 职责 |
|-------|----|------|
| `xc-notion` | kernel | 理念层（数学元语）：Vec/Mat 再导出 / Color32 / Rect2D / Path2D·Path3D / 2D 图元 / Transform2D·3D / Lerp / 函数·曲线·采样器 / 投影辅助（全仓唯一 glam 出口） |
| `xc-render-ir` | kernel | 渲染 IR：`Shape2D` / `Style2D` / 样式组件 / `ToShape2D` + `ResolveStyle`（唯一换算点） |
| `xc-scene-contract` | kernel | 场景契约：`Scene` / `ObjectId` / `Parent` / `Describable`；具体 ECS 只出现在 `xcal-scene` |
| `xc-time` | kernel | 纯逻辑时间内核：ClockForest（时钟树+环检测+eval_local）/ Curve（纯函数 sample）/ Director（唯一时间源） |
| `xc-content` | kernel | 内容中间表示：ContentIR / StyledPath / Span 锚点 + 源码归一化 + 确定性 SVG 量具 + 零依赖占位排版 |
| `xc-prerender` | kernel | 内容预渲染：ContentEngine 异步形状 API + InMemoryEngine + 哈希缓存 + Typst 接入（feature 全 optional） |
| `xc-vertex` | kernel | 顶点格式契约（VertexLayout / VertexAttrib / AaModel / CacheStrategy；无 wgpu 依赖，可序列化） |
| `xc-resource` | kernel | GPU 资源契约与缓存（GpuResourceId / GpuHandle / ResourceKey / ResourceCache / DeferredDestroyer） |
| `xc-macro` | kernel | `Transform2DInto` 派生过程宏 |
| `xc-pod` | pod | Pod 机制：capability 协商（typed 句柄）/ 不透明句柄 / Injector / Broker（需求校验、拓扑、环检测、注入） |
| `xc-pod-binding` | pod | 语言无关绑定驱动：`BindingPlan` / `ValueCodec` + `invoke` / JSON 文档真源；只依赖 `xc-pod` |
| `xc-pod-macro` | pod | `#[capability]` 与 `#[derive(PodType)]` 过程宏（`xc-pod` 保持零依赖） |
| `xc-object` | object | 场景对象层：图元 newtype / 相机 / 摆放 / 装配对象（`Axes2D` / `FunctionGraph`）；只依赖 kernel |
| `xc-node` | graph | 节点图内核（Node / Graph / Socket trait + NodeInteraction 交互策略） |
| `xc-graph-algo` | graph | 图算法（TopoSort / SCC / 脏标记传播） |
| `xc-graph-eval` | graph | 惰性拉取求值（LazyPullEval） |
| `xc-node-schema` | graph | 属性模型（Property / NodeSpec / DynamicObject / NodeRegistry），不依赖 egui |
| `xc-node-schema-macros` | graph | `#[derive(NodeSpec)]` 过程宏：`#[node(...)]`（含 `resizable/draggable/selectable`）+ `#[property(...)]` |
| `xcal-schedule` | schedule | 通用 ECS 调度框架（Resource / System trait / 六阶段 Scheduler + Telemetry）；仅依赖 hecs |
| `xcal-scene` | scene | 场景层：场景组件 + 场景 System（BindingApply / DirectorAdvance / MorphApply / Axes / FunctionGraph / 输入事件 / 父子变换）+ domain 契约 + Pod domain（`SceneApi` / `ContentApi`）；依赖 `xcal-schedule` |
| `xcal-scene-bundle` | scene | XSB 装配产物：容器 / payload SHA-256 / Meta（cap+pod 版本）/ 可还原性检查；**无脚本依赖** |
| `xc-pod-lua` | pod（语言适配器组） | Lua 适配器（mlua vendored）：Lua codec + 能力表 + run 入口；不依赖任何 `xcal-*` |
| `xc-pod-py` | pod（语言适配器组） | Python 适配器（pyo3，`python` feature 门控）：批量能力 / run 入口 / `.pyi` 导出 |
| `xc-pod-tests` | pod（测试组） | 集成测试 + 基准（`publish = false`）：mini pod、topic 扇出、逻辑时序、flush、Lua/Python 对称性 |
| `xcal-render` | render | 2D/3D 渲染抽象（Painter / DrawQueue / Shape2D / RenderLayer / Viewport）+ 录制 System |
| `xcal-render-backends` | render | wgpu / egui-wgpu 具体后端（WgpuBackend3D / WgpuBackend2D / Compositor / SceneCompositorCallback） |
| `xcal-egui` | ui | egui 适配层：输入翻译 / 面板 chrome / 写 `ViewRect` / 内容提交（**不含任何场景 System**） |
| `xcal-node-editor` | ui | 纯逻辑节点编辑器状态机（Command / Undo / 可插拔 HitTestPolicy） |
| `xcal-node-egui` | ui | egui 画布（pan/zoom / 卡片 / 连线 / resize / 磁吸） |
| `xcal-node-egui-ui` | ui | 属性→egui 控件映射（PropertyNodeView / draw_property / spawn_menu） |
| `xcal-app` | app | 组装（无 UI）：Runtime 构建 / Resource 注入 / System 注册 / Pod 装配 |
| `xcal-gui` | app | GUI 可执行壳：`eframe::App` + 窗口参数（依赖 `xcal-app` + `xcal-egui`） |
| `xcal-compile` | app | XSB 独立编译器：脚本 → `.xsb`（复用 `xcal_app::compile_script`） |


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
        value   = Curve.sample(local_t)                      // Const/Sin/Orbit/Phase/Sampled/Morph/Keyframes
        写入目标组件字段 (Circle.radius / Camera3D.eye / Reveal.end / MorphShape.t / ...)
  → SceneRecord → 渲染
```

铁律：
- 动画纯函数：永不累积状态 → seek/倒放免费正确
- Binding 表取代组件耦合：新增动画 = 加一行 Binding，不改 System 源码
- `Timeline` 把「先…再…」编译成顺序 `ClockNode` + `Binding`；`parallel()` 表达同区间多动作
- 时钟树纯数据 + 纯函数：脱离 hecs，可序列化可测试
- 墙钟时间：`Instant::now()` 算 dt，帧率波动不影响时间流速
- UI slider 的 seek 只在 `drag_stopped()` 触发，step_by 会把 global_t 对齐到网格导致卡死

## 帧流水线（2D / 3D / 内容 / 形变 / 书写）

1. **SceneSync**：`BindingApplySystem` 根据 `BindingTable` 把动画值写入
   `Camera3D.eye`（轨道）、`Reveal.end` 等组件，取代旧的 `OrbitAnim` 组件 + `OrbitApplySystem`。
2. **RenderCommit**：五个录制 System 分别填充各自队列：
   - `SceneRecordSystem` → `DrawQueue`（2D）
   - `ContentTextureRecordSystem` → `ContentTextureQueue`
   - `MorphRecordSystem` → `MorphQueue`
   - `RevealRecordSystem` → `RevealQueue`（书写动画）
   - `SceneRecord3DSystem` → `DrawQueue3D`
3. **FrameEnd**：`EguiSceneHost::present` 取中央视口矩形，构造**单个**
   `SceneCompositorCallback`（含全部 `SceneLayer` 声明 + 各队列快照）
   注册为 egui paint callback；egui-wgpu 在主 render pass 内回调：
   - `prepare`：逐层渲染到各自离屏纹理（`WgpuBackend2D` / `WgpuBackend3D::render_layer_offscreen`），
     收集 `CompositeOp` → 一次 `Compositor::prepare` 得到单个 `CompositeFrame`；
   - `paint`：`Compositor::composite_layers` 按声明顺序混合（层序 = `SceneView::layers` 索引序）。

## 脚本装配（Pod / Broker）

Lua / Python 脚本只在**装配期**参与，不参与每帧采样：

```
Lua / Python 脚本
  → Injector 注入的 SceneApi / ContentApi（typed capability）
  → SceneScript（对象 + Binding / ClockForest 描述）
  → xcal-app::assembly（静态插件清单）
  → xc_pod::Broker（需求校验 / 拓扑排序 / 环检测 / 注入）
  → Scene + BindingTable + ClockForest
```

- `xc-pod-lua` / `xc-pod-py` 的生产依赖不含任何 `xcal-*`；domain 能力由 App 注入；
- 入口：`cargo run -p xcal-gui -- --script`（Lua）/ `--python`（Python）；
- 文档导出：`cargo run -p xcal-app --example export_scene_api`
  （`target/scene_api/` 下的 JSON + LuaLS + `.pyi`）。

## 运行

```shell
# 默认：§1.1 六条分镜（硬编码参考实现）
cargo run -p xcal-gui

# 脚本编排同一分镜（装配期）
cargo run -p xcal-gui -- --script          # Lua
cargo run -p xcal-gui -- --python          # Python（需系统 Python）

# 能力测试 demo（Name / Group / Visibility / Opacity）
cargo run -p xcal-gui -- --demo

# headless：脚本直接渲到 mp4（默认 ffmpeg 管道；--frames DIR 走序列帧）
cargo run -p xcal-app --bin xcal-render -- \
    crates/app/xcal-app/scripts/storyboard.lua --out out/storyboard.mp4 --fps 30 --size 640x480

# placeholder 兜底（关闭 Typst；同时关闭 Python）
cargo run -p xcal-gui --no-default-features

# 独立 3D demo（不依赖 xcal-app / scheduler，直接演示 WgpuBackend3D）
cargo run -p xcal-render-backends --example cube_3d

# 节点编辑器 demo（多输入/多输出 pin、卡片 resize、连线磁吸、撤销重做）
cargo run -p xcal-node-egui --example node_editor_demo

# 能力文档 / 序列帧导出
cargo run -p xcal-app --example export_scene_api
cargo run -p xcal-app --example export_frames

# 全量测试 / 文档
cargo test --workspace
cargo doc --no-deps -p xcal-scene -p xcal-render
```
