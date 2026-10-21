# zela

基于 ECS（hecs）与 wgpu 的 2D/3D 渲染实验框架：分阶段的 System 调度器、
Painter 抽象、egui UI overlay、3D 渲染层，以及语言无关的 Pod 边界
（Lua / Python 在装配期编排场景）。同一套装配尾也能在浏览器（WebGPU / WASM）
回放 ZSB。

> **改名说明（仓库中唯一保留旧名的位置）**：本项目原名 `xcal-rs`，crate
> 前缀为 `xc-` / `xcal-`；2026-10 重命名为 `zela`，前缀改为 `ze-` /
> `zela-`。代码、字符串、脚本与配置中不再出现旧前缀。

## Quick Start

> 目标：**5 分钟内看到第一个 mp4**。命令都在仓库根目录执行。
> 前置：Rust stable；路径 1 / 2 需要 `ffmpeg` 在 PATH。
> 不想装系统 Python 时，统一加 `--no-default-features --features typst`
> （Lua + Typst，不含 Python Pod）。

### 1) 渲染一个作品（Lua → mp4）

```shell
cargo run -p zela-app --no-default-features --features typst --bin zela-render -- \
  --script crates/app/zela-app/scripts/storyboard.lua \
  --out out/storyboard.mp4 --fps 30 --size 640x480
```

- 产物：`out/storyboard.mp4`（135 帧 / 4.5s）。
- 没有 ffmpeg：把 `--out out/storyboard.mp4` 换成 `--frames out/storyboard`，
  得到 PNG 序列。
- 要真公式：去掉 `--no-default-features --features typst`（默认含 `typst`）；
  `--python` 另需系统 Python 与 `python` feature。

### 2) 编译 + 回放 ZSB

```shell
cargo run -p zela-compile --no-default-features --features typst -- \
  crates/app/zela-app/scripts/fourier.lua --out out/fourier.zsb

cargo run -p zela-app --no-default-features --features typst --bin zela-render -- \
  --bundle out/fourier.zsb --out out/fourier.mp4 --fps 30 --size 640x480
```

- `.zsb` 是装配产物；native 与浏览器共用 `zela-playback` 的回放路径。
- 同一脚本的 `--script` 与 `--bundle` 逐帧像素一致，见
  [`FRAME_HASH_BASELINES.md`](crates/app/zela-app/scripts/FRAME_HASH_BASELINES.md)。

### 3)（可选）浏览器预览

```shell
cargo install wasm-pack
rustup target add wasm32-unknown-unknown
powershell -ExecutionPolicy Bypass -File crates/web/zela-web/build.ps1
python -m http.server 8080 -d crates/web/zela-web/www
```

打开 <http://localhost:8080/>（Chrome / Edge，需要 WebGPU）。
加 `?compare=1` 会额外打印帧序列哈希与结构指纹，用于跨端对拍。

- 前置：`wasm-pack`（首次会自动下载匹配的 `wasm-bindgen`）与静态文件服务器；
  不需要系统 Python 参与编译。

## 架构总览

workspace 采用分层目录结构 `crates/<layer>/`，通用基础 crate 用 `ze-` 前缀，
领域/应用层保留 `zela-` 前缀。分层顺序为
**kernel → object / graph / script → schedule → scene → render → ui → app → web**
（允许上层依赖下层，禁止反向；`web` 只是 wasm 薄壳）。

```text
crates/
├── kernel/          # 纯数据层（ze-prerender 为例外：kernel 层唯一重 crate，重型依赖 feature-gated）
│   ├── ze-notion           理念层/数学元语（Color32 / Rect2D / Path2D / Path3D / 图元 / Transform2D）
│   ├── ze-render-ir        渲染 IR（Shape2D / Style2D / ToShape2D / ResolveStyle；不认识 hecs）
│   ├── ze-scene-contract   场景契约（Scene / ObjectId / Parent / Describable；hecs 只在实现方）
│   ├── ze-time             时钟树 + 曲线 + 导演（Director 唯一时间源，纯数据纯函数）
│   ├── ze-content          内容中间表示（ContentIR + 源码归一化 + 确定性 SVG + 占位排版）
│   ├── ze-prerender        内容预渲染引擎接入（Typst / placeholder + 哈希缓存），默认零重依赖
│   ├── ze-vertex           顶点格式契约（VertexLayout / AaModel / CacheStrategy，无 wgpu 依赖）
│   ├── ze-resource         GPU 资源契约与缓存（ResourceKey / ResourceCache / GpuHandle，纯 std）
│   ├── ze-macro            Transform2DInto 派生宏
│
├── ze/              # 基础层（pod + graph；可独立发布）
│   ├── pod/
│   │   ├── ze-pod-core         Pod 机制（capability 协商 / 不透明句柄 / Injector / Broker）
│   │   ├── ze-pod-binding      语言无关绑定驱动（BindingPlan / ValueCodec / JSON 文档）
│   │   ├── ze-pod-macro        #[capability] 与 #[derive(PodType)] 过程宏
│   │   ├── ze-pod-lua          Lua 适配器（mlua，解释器 vendored）
│   │   ├── ze-pod-py           Python 适配器（pyo3，feature 门控）
│   │   ├── ze-pod-tests        集成测试 + 基准（publish = false）
│   │   └── ze-pod              门面：core + binding + macro（不含 lua / py，保持 wasm32）
│   └── graph/
│       ├── ze-node-core        节点图内核（Node / Graph / Socket trait + NodeInteraction）
│       ├── ze-graph-algo       图算法（TopoSort / SCC / 脏标记传播）
│       ├── ze-graph-eval       惰性拉取求值（LazyPullEval）
│       ├── ze-node-schema      属性模型（Property / NodeSpec / DynamicObject / NodeRegistry）
│       ├── ze-node-schema-macros  #[derive(NodeSpec)] 过程宏
│       └── ze-node             门面：core + schema + algo + eval（不含 UI）
│
├── schedule/        # 通用 ECS 调度框架（不含任何场景/渲染概念）
│   └── zela-schedule       Resource / System trait / 六阶段 Scheduler + Telemetry（依赖仅 hecs）
│
├── scene/           # 场景层（B3-1：概念 / 实现 / 能力三分）
│   ├── ze-scene-core       对象 + 组件 + 纯逻辑（动画数据 / 脚本数据 / 持久化；零 hecs、零 zela-*）
│   ├── ze-scene-api        Pod 能力声明（SceneApi / ContentApi + 两个 Service；零 hecs、零 zela-*）
│   ├── zela-scene-ecs      ECS 实现（Scene / 资源 / 生命周期 / 全部 System）
│   └── zela-scene-bundle   ZSB 装配产物（格式 / payload 哈希 / 可还原性检查；无脚本依赖）
│
├── render/          # 渲染抽象与具体后端
│   ├── zela-render         2D/3D 渲染抽象（Painter / DrawQueue / Shape2D / RenderLayer）
│   └── zela-render-backends  wgpu / egui-wgpu 具体后端（WgpuBackend3D / WgpuBackend2D / Compositor）
│
├── ui/              # UI 与编辑器层
│   ├── zela-egui           egui 适配层（输入翻译 / 面板 chrome / 内容提交；不含场景 System）
│   ├── zela-node-editor    纯逻辑节点编辑器状态机（Command / Undo / HitTestPolicy）
│   ├── zela-node-egui      egui 画布（pan/zoom / 卡片 / 连线 / resize / 磁吸）
│   └── zela-node-egui-ui   属性→egui 控件映射（PropertyNodeView / draw_property）
│
├── app/             # 应用外壳层
│   ├── zela-app          组装（无 UI）：Runtime / Pod 装配 / Resource 注入 / System 注册
│   ├── zela-playback     回放运行时：ZSB → Scene → 调度器 → 帧回调（native / wasm 共用）
│   ├── zela-gui          GUI 可执行壳（eframe::App）
│   └── zela-compile      ZSB 独立编译器（脚本 → .zsb）
│
├── zela/            # 应用层门面
│   └── zela               Pod + 节点图统一入口（re-export ze-pod / ze-node）
│
└── web/             # 浏览器层
    └── zela-web          浏览器薄壳（wasm32）：WebGPU canvas / fetch ZSB / rAF 固定 dt
```

依赖铁律：
- `ze-*` crate 不得依赖任何 `zela-*` crate 或 egui/wgpu
- `zela-*` crate 可依赖 `ze-*`，但不得依赖 `zela-app`
- 装配尾只有 `zela-playback` 一份；`zela-app` / `zela-web` 调用它，
  `zela-gui` / `zela-compile` 只是壳（**`zela-app` 不依赖 `zela-egui`**）
- 语言适配器（`ze-pod-lua` / `ze-pod-py`）属于 `crates/ze/pod/` 的语言适配器组，**生产依赖不含任何 `zela-*`**；
  domain 能力由 App 通过 `register_capability::<SceneApi>(..)` 注入
- `crates/ze/pod/` 的核心组（`ze-pod-core` / `ze-pod-binding` / `ze-pod-macro`）平台无关，必须过 wasm gate；
  语言适配器组（mlua / pyo3）**不在** wasm gate 名单内（CI 注释已注明原因）

## Crate 一览

| Crate | 层 | 职责 |
|-------|----|------|
| `ze-notion` | kernel | 理念层（数学元语）：Vec/Mat 再导出 / Color32 / Rect2D / Path2D·Path3D / 2D 图元 / Transform2D·3D / Lerp / 函数·曲线·采样器 / 投影辅助（全仓唯一 glam 出口） |
| `ze-render-ir` | kernel | 渲染 IR：`Shape2D` / `Style2D` / 样式组件 / `ToShape2D` + `ResolveStyle`（唯一换算点） |
| `ze-scene-contract` | kernel | 场景契约：`Scene` / `ObjectId` / `Parent` / `Describable`；具体 ECS 只出现在 `zela-scene-ecs` |
| `ze-time` | kernel | 纯逻辑时间内核：ClockForest（时钟树+环检测+eval_local）/ Curve（纯函数 sample）/ Director（唯一时间源） |
| `ze-content` | kernel | 内容中间表示：ContentIR / StyledPath / Span 锚点 + 源码归一化 + 确定性 SVG 量具 + 零依赖占位排版 |
| `ze-prerender` | kernel | 内容预渲染：ContentEngine 异步形状 API + InMemoryEngine + 哈希缓存 + Typst 接入（feature 全 optional） |
| `ze-vertex` | kernel | 顶点格式契约（VertexLayout / VertexAttrib / AaModel / CacheStrategy；无 wgpu 依赖，可序列化） |
| `ze-resource` | kernel | GPU 资源契约与缓存（GpuResourceId / GpuHandle / ResourceKey / ResourceCache / DeferredDestroyer） |
| `ze-macro` | kernel | `Transform2DInto` 派生过程宏 |
| `ze-pod-core` | pod | Pod 机制：capability 协商（typed 句柄）/ 不透明句柄 / Injector / Broker（需求校验、拓扑、环检测、注入） |
| `ze-pod-binding` | pod | 语言无关绑定驱动：`BindingPlan` / `ValueCodec` + `invoke` / JSON 文档真源；只依赖 `ze-pod-core` |
| `ze-pod-macro` | pod | `#[capability]` 与 `#[derive(PodType)]` 过程宏（`ze-pod-core` 保持零依赖） |
| `ze-scene-core` | scene | 场景核心：对象（原 `ze-object`）/ 组件 / 纯逻辑（动画数据 · 脚本数据 · 持久化 · 登记表）；**零 hecs、零 `zela-*`** |
| `ze-scene-api` | scene | 语言无关能力：`SceneApi` / `ContentApi` + `SceneServicePod` / `ContentServicePod`；**零 hecs、零 `zela-*`** |
| `ze-node-core` | graph | 节点图内核（Node / Graph / Socket trait + NodeInteraction 交互策略） |
| `ze-graph-algo` | graph | 图算法（TopoSort / SCC / 脏标记传播） |
| `ze-graph-eval` | graph | 惰性拉取求值（LazyPullEval） |
| `ze-node-schema` | graph | 属性模型（Property / NodeSpec / DynamicObject / NodeRegistry），不依赖 egui |
| `ze-node-schema-macros` | graph | `#[derive(NodeSpec)]` 过程宏：`#[node(...)]`（含 `resizable/draggable/selectable`）+ `#[property(...)]` |
| `zela-schedule` | schedule | 通用 ECS 调度框架（Resource / System trait / 六阶段 Scheduler + Telemetry）；仅依赖 hecs |
| `zela-scene-ecs` | scene | ECS 实现：`Scene`（hecs 适配）/ 资源 / 生命周期 / 全部 System + `register`；依赖 `ze-scene-core` / `ze-scene-api` / `zela-schedule` |
| `zela-authored` | app | 场景作品：分镜 / 能力 demo / 3D demo（不再是框架代码） |
| `zela-scene-bundle` | scene | ZSB 装配产物：容器 / payload SHA-256 / Meta（cap+pod 版本）/ 可还原性检查；**无脚本依赖** |
| `ze-pod-lua` | pod（语言适配器组） | Lua 适配器（mlua vendored）：Lua codec + 能力表 + run 入口；不依赖任何 `zela-*` |
| `ze-pod-py` | pod（语言适配器组） | Python 适配器（pyo3，`python` feature 门控）：批量能力 / run 入口 / `.pyi` 导出 |
| `ze-pod-tests` | pod（测试组） | 集成测试 + 基准（`publish = false`）：mini pod、topic 扇出、逻辑时序、flush、Lua/Python 对称性 |
| `ze-pod` | pod（门面） | 聚合 `ze-pod-core` + `ze-pod-binding` + `ze-pod-macro`；不含 lua / py，保持 wasm32 |
| `ze-node` | graph（门面） | 聚合 `ze-node-core` + `ze-node-schema` + `ze-graph-algo` + `ze-graph-eval`；不含 UI |
| `zela` | zela（门面） | 聚合 `ze-pod` + `ze-node`，作为基础层统一入口 |
| `zela-render` | render | 2D/3D 渲染抽象（Painter / DrawQueue / Shape2D / RenderLayer / Viewport）+ 录制 System |
| `zela-render-backends` | render | wgpu / egui-wgpu 具体后端（WgpuBackend3D / WgpuBackend2D / Compositor / SceneCompositorCallback） |
| `zela-egui` | ui | egui 适配层：输入翻译 / 面板 chrome / 写 `ViewRect` / 内容提交（**不含任何场景 System**） |
| `zela-node-editor` | ui | 纯逻辑节点编辑器状态机（Command / Undo / 可插拔 HitTestPolicy） |
| `zela-node-egui` | ui | egui 画布（pan/zoom / 卡片 / 连线 / resize / 磁吸） |
| `zela-node-egui-ui` | ui | 属性→egui 控件映射（PropertyNodeView / draw_property / spawn_menu） |
| `zela-app` | app | 组装（无 UI）：Runtime 构建 / Resource 注入 / System 注册 / Pod 装配 |
| `zela-playback` | app | 回放运行时：`build_bundle_runtime` / `assemble_runtime` / 帧推进与渲染；native 与 wasm 共用 |
| `zela-gui` | app | GUI 可执行壳：`eframe::App` + 窗口参数（依赖 `zela-app` + `zela-egui`） |
| `zela-compile` | app | ZSB 独立编译器：脚本 → `.zsb`（复用 `zela_app::compile_script`） |
| `zela-web` | web | 浏览器薄壳（仅 wasm32）：WebGPU canvas、fetch ZSB、rAF 固定 `dt`、离屏对拍 |


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
  → zela-app::assembly（静态插件清单）
  → ze_pod_core::Broker（需求校验 / 拓扑排序 / 环检测 / 注入）
  → Scene + BindingTable + ClockForest
```

- `ze-pod-lua` / `ze-pod-py` 的生产依赖不含任何 `zela-*`；domain 能力由 App 注入；
- 入口：`cargo run -p zela-gui -- --script`（Lua）/ `--python`（Python）；
- 文档导出：`cargo run -p zela-app --example export_scene_api`
  （`target/scene_api/` 下的 JSON + LuaLS + `.pyi`）。

## 运行

```shell
# 默认：§1.1 六条分镜（硬编码参考实现）
cargo run -p zela-gui

# 脚本编排同一分镜（装配期）
cargo run -p zela-gui -- --script          # Lua
cargo run -p zela-gui -- --python          # Python（需系统 Python）

# 能力测试 demo（Name / Group / Visibility / Opacity）
cargo run -p zela-gui -- --demo

# headless：脚本直接渲到 mp4（默认 ffmpeg 管道；--frames DIR 走序列帧）
cargo run -p zela-app --bin zela-render -- \
    crates/app/zela-app/scripts/storyboard.lua --out out/storyboard.mp4 --fps 30 --size 640x480

# placeholder 兜底（关闭 Typst；同时关闭 Python）
cargo run -p zela-gui --no-default-features

# 独立 3D demo（不依赖 zela-app / scheduler，直接演示 WgpuBackend3D）
cargo run -p zela-render-backends --example cube_3d

# 节点编辑器 demo（多输入/多输出 pin、卡片 resize、连线磁吸、撤销重做）
cargo run -p zela-node-egui --example node_editor_demo

# 能力文档 / 序列帧导出
cargo run -p zela-app --example export_scene_api
cargo run -p zela-app --example export_frames

# 全量测试 / 文档
cargo test --workspace
cargo doc --no-deps -p zela-scene-ecs -p zela-render
```

## 许可

本项目以 **MIT OR Apache-2.0** 双许可发布；全文见
[LICENSE-MIT](LICENSE-MIT) 与 [LICENSE-APACHE](LICENSE-APACHE)。

可独立发布的 crate：`ze-pod-core`、`ze-pod-binding`、`ze-pod-macro`、
`ze-pod-lua`、`ze-pod-py`、`ze-pod`、`ze-node-core`、`ze-node-schema`、
`ze-node-schema-macros`、`ze-graph-algo`、`ze-graph-eval`、`ze-node`、`zela`。
`ze-pod-tests` 为内部测试 crate（`publish = false`）。
