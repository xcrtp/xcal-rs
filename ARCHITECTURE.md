# Architecture

> **本文件的『依赖图 / 分层 / 合规校验』由 `cargo-workspace-analyzer` 自动生成。**
> 修改 `Cargo.toml` 后重新运行 `cargo-workspace-analyzer -d .` 即可刷新。
> **不要手动编辑下方自动生成部分**——手写内容（设计原则、待决问题）请放在末尾『## 设计说明』。

## 分层总览

| 层 | 路径 | 前缀 | 职责 |
|---|---|---|---|
| kernel | `crates/kernel/` | `ze-` | 纯计算/数据内核（无 egui、无渲染；含通用能力领域，如几何、排版）|
| pod    | `crates/ze/pod/`    | `ze-` | **领域无关的 Pod 层**：核心组 `ze-pod-core` / `ze-pod-binding` / `ze-pod-macro`（平台无关，必须过 wasm gate）+ 门面 `ze-pod`（只聚合核心组，不含 lua / py）+ 语言适配器组 `ze-pod-lua` / `ze-pod-py`（按 mlua / pyo3 属平台相关，**不进** wasm gate）+ 测试组 `ze-pod-tests`（`publish = false`，只服务集成测试 / 基准）。全部生产依赖**均不含任何 `zela-*`**；domain 能力由 App 注入 |
| object | `crates/object/` | `ze-` | **场景对象层**（M3 新增）：用户可见的场景对象（图元 newtype / 相机 / 变换 / 绘制层级 / 装配对象 `Axes2D`·`FunctionGraph`），只依赖 kernel |
| graph  | `crates/ze/graph/`  | `ze-` | 图论 + 节点结构契约（NodeSpec/Property/DynamicObject）+ 门面 `ze-node`（聚合 core + schema + algo + eval，不含 UI）|
| schedule | `crates/schedule/` | `zela-` | 通用 ECS 调度框架（Resource / System trait / 六阶段 Scheduler + Telemetry）；不含任何场景/渲染概念 |
| scene  | `crates/scene/` | `zela-` | **场景层**（R1.6 更名、本轮归位）：ECS 场景数据（`Scene` / 组件）+ 场景 System + UI ↔ 场景 `domain` 契约 + Pod domain（`SceneApi` / `ContentApi`）|
| script | ~~`crates/script/`~~ | — | 已并入 `crates/ze/pod/` 的语言适配器组（本轮迁移，`crates/script/` 目录消失） |
| render | `crates/render/` | `zela-` | 渲染抽象（trait / 队列 / 三角化）+ 具体后端 |
| ui     | `crates/ui/`     | `zela-` | egui 前端 + 编辑器 |
| app    | `crates/app/`    | `zela-` | 组装 / 可执行 |
| zela   | `crates/zela/`   | `zela` | 应用层门面：聚合 `ze-pod` + `ze-node`（真实 re-export，不是空壳） |

## 依赖图

依赖方向自上而下：`kernel / pod → object / graph → schedule → scene → render → ui → app`，反向禁止。
（`pod` 是独立底座：只依赖自身核心组，不依赖 kernel / scene；`schedule` 无内部依赖，是通用底座；
`render` / `ui` / `app` 亦可直接依赖 `kernel` / `pod` / `object` / `graph`。）

```mermaid
graph TD
    subgraph kernel[crates/kernel]
        ze_notion["ze-notion"]
        ze_render_ir["ze-render-ir"]
        ze_scene_contract["ze-scene-contract"]
        ze_content["ze-content"]
        ze_prerender["ze-prerender"]
        ze_time["ze-time"]
        ze_vertex["ze-vertex"]
        ze_resource["ze-resource"]
        ze_macro["ze-macro"]
    end

    subgraph pod[crates/ze/pod]
        ze_pod_core["ze-pod-core（核心组）"]
        ze_pod_binding["ze-pod-binding（核心组）"]
        ze_pod_macro["ze-pod-macro（核心组）"]
        ze_pod_lua["ze-pod-lua（语言适配器组）"]
        ze_pod_py["ze-pod-py（语言适配器组）"]
        ze_pod_js["ze-pod-js（语言适配器组，boa）"]
        ze_pod_tests["ze-pod-tests（测试组，publish=false）"]
    end

    %% B3-1：`crates/object/ze-object` 已并入 ze-scene-core::object（crate 删除）

    subgraph graph[crates/ze/graph]
        ze_node_core["ze-node-core"]
        ze_graph_algo["ze-graph-algo"]
        ze_graph_eval["ze-graph-eval"]
        ze_node_schema["ze-node-schema"]
        ze_node_schema_macros["ze-node-schema-macros"]
    end

    subgraph schedule[crates/schedule]
        zela_schedule["zela-schedule"]
    end

    subgraph scene[crates/scene]
        ze_scene_core["ze-scene-core"]
        ze_scene_hecs["ze-scene-hecs"]
        ze_scene_api["ze-scene-api"]
        zela_scene_ecs["zela-scene-ecs"]
    end

    subgraph render[crates/render]
        zela_render["zela-render"]
        zela_render_backends["zela-render-backends"]
    end

    subgraph ui[crates/ui]
        zela_egui["zela-egui"]
        zela_node_editor["zela-node-editor"]
        zela_node_egui["zela-node-egui"]
        zela_node_egui_ui["zela-node-egui-ui"]
    end

    subgraph app[crates/app]
        zela_app["zela-app"]
    end

    %% kernel
    ze_notion
    ze_vertex
    ze_resource
    ze_macro
    ze_render_ir --> ze_notion
    ze_scene_contract --> ze_notion
    ze_scene_contract --> ze_render_ir
    ze_content --> ze_notion
    ze_prerender --> ze_content
    ze_prerender --> ze_notion
    ze_time --> ze_notion

    %% pod：核心组 + 语言适配器组（生产边不含任何 zela-*）
    ze_pod_binding --> ze_pod_core
    ze_pod_py --> ze_pod_core
    ze_pod_py --> ze_pod_binding
    ze_pod_py --> ze_pod_macro
    ze_pod_lua --> ze_pod_core
    ze_pod_lua --> ze_pod_binding
    ze_pod_js --> ze_pod_core
    ze_pod_js --> ze_pod_binding
    ze_pod_tests --> ze_pod_core
    ze_pod_tests --> ze_pod_binding
    ze_pod_tests --> ze_pod_macro
    ze_pod_tests --> ze_pod_lua
    ze_pod_tests --> ze_pod_py
    ze_pod_tests --> ze_pod_js

    %% 原 ze-object 的依赖边已归入 ze_scene_core（见上）

    %% graph → kernel
    ze_graph_algo --> ze_node_core
    ze_graph_eval --> ze_node_core
    ze_node_schema --> ze_node_core
    ze_node_schema --> ze_node_schema_macros

    %% schedule（无内部依赖，仅依赖 hecs）

    %% render → kernel + object + schedule
    %% 场景分层（B3-1）：ECS 实现 → 核心 / 能力层（壳层已删除）
    zela_scene_ecs --> ze_scene_core
    zela_scene_ecs --> ze_scene_api
    zela_scene_ecs --> ze_scene_hecs
    ze_scene_hecs --> ze_scene_contract
    zela_scene_ecs --> zela_schedule
    zela_scene_ecs --> ze_pod_core
    zela_scene_ecs --> ze_pod_macro
    ze_scene_api --> ze_scene_core
    ze_scene_api --> ze_pod_core
    ze_scene_api --> ze_pod_macro
    ze_scene_core --> ze_notion
    ze_scene_core --> ze_time
    ze_scene_core --> ze_macro
    ze_scene_core --> ze_content
    ze_scene_core --> ze_prerender
    ze_scene_core --> ze_pod_core
    ze_scene_core --> ze_pod_macro
    ze_scene_core --> ze_render_ir
    ze_scene_core --> ze_scene_contract
    zela_render --> ze_content
    zela_render --> ze_notion
    zela_render --> ze_render_ir
    zela_render --> ze_scene_core
    zela_render --> ze_vertex
    zela_render --> ze_resource
    zela_render --> ze_scene_hecs
    zela_render --> zela_schedule
    zela_render --> ze_scene_contract
    zela_render_backends --> ze_notion
    zela_render_backends --> ze_render_ir
    zela_render_backends --> zela_render
    zela_render_backends --> ze_scene_core

    %% ZSB 装配产物 → scene（不含任何语言适配器）
    zela_scene_bundle --> zela_scene_ecs

    %% ui → kernel + graph + render + schedule
    zela_egui --> ze_notion
    zela_egui --> ze_macro
    zela_egui --> ze_scene_core
    zela_egui --> ze_pod_core
    zela_egui --> zela_scene_ecs
    zela_egui --> zela_schedule
    zela_egui --> zela_render
    zela_egui --> zela_render_backends
    zela_node_editor --> ze_node_core
    zela_node_editor --> ze_graph_algo
    zela_node_egui --> ze_node_core
    zela_node_egui --> zela_node_editor
    zela_node_egui_ui --> ze_node_core
    zela_node_egui_ui --> ze_node_schema
    zela_node_egui_ui --> zela_node_egui

    %% GUI 壳 → app + ui（S0：UI 不再由 app 依赖）
    zela_gui --> ze_pod_core
    zela_gui --> zela_app
    zela_gui --> zela_egui
    zela_gui --> zela_render_backends
    zela_gui --> zela_scene_ecs
    zela_gui --> zela_schedule

    %% app → everything（**不依赖 zela-egui**）
    zela_app --> zela_scene_ecs
    zela_app --> zela_scene_bundle
    zela_app --> zela_schedule
    zela_app --> ze_macro
    zela_app --> zela_render
    zela_app --> zela_render_backends
    zela_app --> ze_content
    zela_app --> ze_prerender
    zela_app --> ze_notion
    zela_app --> ze_scene_core
    zela_app --> ze_pod_core
    zela_app --> ze_pod_binding
    zela_app --> ze_pod_lua
    zela_app --> ze_pod_py

    %% 独立编译器 → app + bundle
    zela_compile --> zela_app
    zela_compile --> zela_scene_bundle
```

## 合规校验

| 规则 | 状态 | 说明 |
|---|---|---|
| NoReverseDependency | ✅ 通过 | 无 ze-* → zela-* / egui / wgpu 生产依赖 |
| NoAppDependency | ✅ 通过 | 无 zela-* crate 依赖 zela-app |
| NoGraphDomainTypes | ✅ 通过 | crates/ze/graph 无领域类型引用 |
| NoPodDomainDependency | ✅ 通过 | `crates/ze/pod` 全部 5 个 crate 的生产依赖不含任何 `zela-*`（含两个语言适配器）；domain 由 App 注入 |
| NoCycles | ✅ 通过 | 生产依赖为 DAG（dev-deps 不计入） |

**依赖分析（2026-10-18 按 `cargo metadata` 等价重算，仅生产依赖、dev-deps 不计；见文末说明）：**

> 本次复核把脚本层、`ze-pod-core` 家族与 ZSB 两个新 crate 一并纳入：mermaid **88 条 ⇔ Cargo.toml 88 条**，
> 零缺零余；FanIn/FanOut 共 **33 行**。
> 此前的 W2 复核（8 条漏边修复）仍有效，本表是在其基础上的全量重算。
>
> **2026-10-19 更新**：新增 `zela-playback` / `zela-web` 后，`cargo metadata` 口径的
> 内部生产边为 **100 条**，见 [`workspace-analyzer.mmd`](workspace-analyzer.mmd)。
> 下方 FanIn/FanOut 表仍是 2026-10-18 快照，**未包含这两个 crate**。

| Package | FanIn | FanOut | Instability |
|---------|-------|--------|-------------|
| ze-graph-eval | 0 | 1 | 1.00 |
| ze-pod-tests | 0 | 5 | 1.00 |
| zela-compile | 0 | 2 | 1.00 |
| zela-gui | 0 | 6 | 1.00 |
| zela-node-egui-ui | 0 | 3 | 1.00 |
| zela-egui | 1 | 8 | 0.89 |
| zela-app | 2 | 14 | 0.88 |
| zela-render | 3 | 9 | 0.75 |
| zela-scene | 5 | 11 | 0.69 |
| ze-node-schema | 1 | 2 | 0.67 |
| zela-node-editor | 1 | 2 | 0.67 |
| zela-node-egui | 1 | 2 | 0.67 |
| ze-pod-py | 2 | 3 | 0.60 |
| zela-render-backends | 3 | 4 | 0.57 |
| ze-graph-algo | 1 | 1 | 0.50 |
| ze-pod-lua | 2 | 2 | 0.50 |
| ze-prerender | 2 | 2 | 0.50 |
| ze-time | 1 | 1 | 0.50 |
| ze-object | 5 | 4 | 0.44 |
| ze-scene-contract | 3 | 2 | 0.40 |
| zela-scene-bundle | 2 | 1 | 0.33 |
| ze-content | 4 | 1 | 0.20 |
| ze-pod-binding | 4 | 1 | 0.20 |
| ze-render-ir | 5 | 1 | 0.17 |
| ze-macro | 4 | 0 | 0.00 |
| ze-node-core | 6 | 0 | 0.00 |
| ze-node-schema-macros | 1 | 0 | 0.00 |
| ze-notion | 11 | 0 | 0.00 |
| ze-pod-core | 8 | 0 | 0.00 |
| ze-pod-macro | 3 | 0 | 0.00 |
| ze-resource | 1 | 0 | 0.00 |
| ze-vertex | 1 | 0 | 0.00 |
| zela-schedule | 5 | 0 | 0.00 |

Instability = FanOut / (FanIn + FanOut)；0 = 底层稳定，1 = 高层易变。

---

## 设计说明（手写区，不被脚本覆盖）

### 命令 / 事件机制（Driver pod）

「谁推进了时间」不再藏在调用栈里，而是一条显式命令：

```
App / zela-render ──submit(Advance { dt })──► DriverPod（持有 Scheduler + 相机）
                                                  │
                                                  ├─ publish_topic("zela.driver.events")
                                                  │    FrameFinished / Advanced / Error
                                                  │        └─ 按 PodId 升序 typed 调用 EventSink<E>::on_event
                                                  └─ 每帧 run_frame(dt)
```

| | 命令队列 `CommandQueue<C>` | 事件 topic（`publish_topic`） |
|---|---|---|
| 方向 | 控制 | 通知 |
| 语义 | FIFO、顺序确定 | 异步、可丢 |
| 参与帧推进 | **是** | **否** |
| 回执 | `Receipt::wait()` | — |

铁律：**任何影响场景状态的，必须走命令队列；事件只能用于观测。**
命令 / 事件是**不同泛型类型**（`C` vs `E`），把事件当命令提交是**编译错误**；
若退化成 `PodValue` 承载，两者签名会完全相同，约束就只剩约定。

GUI 与 headless（`zela-render` / `export_frames`）**共用同一个 Driver pod**：
GUI 用墙钟算 `dt`，导出用固定 `dt = 1/fps`；场景、系统图、渲染回调完全相同。
导出路径因此不存在"另写一套简化帧循环"的分叉。

### topic / 逻辑时序 / flush（P6）

- **topic**：广播 = `Broker::topic_members(topic)`（按 `PodId` 升序）+ 逐个
  `negotiate_on_handle::<EventSink<E>>` typed 调用；`Broadcast<E>` 保留为兼容入口，
  本身不做扇出。
- **Ord**：`SeqKey = (frame, source, seq)`，不含 `Instant` / 时间戳；
  `Sequenced<T>` 由框架实现 `Ord`（只比较键，不要求 `T: Ord`）。
- **flush**：`FlushBuffer` 只在 `FlushBoundary::FrameEnd / BatchEnd` 排序；
  `push_immediate` 是逃生舱，不保证顺序。
- **寻址**：`PodHandle`（`PodType` = `u64`）标识 Pod 本身；资源 `Handle` 带 owner，
  不得用于寻址。
- **自证**：`crates/ze/pod/ze-pod-tests`（`publish = false`）提供 mini pod、
  集成断言与基准；不被任何生产 crate 依赖。

### 调度流水线

**每个 Phase 的系统**（`zela-app` 的 `register_systems`）

```
Input:         DirectorAdvance · CameraDrive · CameraInput
LogicUpdate:   TimeControl · ParamPanel
SceneSync:     BindingApply
Prepare:       TransformHierarchy · AxesRefresh · FunctionGraphRefresh
RenderCommit:  SceneRecord (2D) · SceneRecord3D (3D) · ContentTextureRecord · MorphRecord · RevealRecord
FrameEnd:      —（内容提交由 host.present 承担；
                 面板 chrome 与 ViewRect 由 host.begin_frame 在 run_frame 前写入）
```

**M4 起，几何不再出现在 UI 层**：坐标轴 / 网格（相机裁剪、步长自适应、刻度线）
归 `ze_scene_core::Axes2D`，函数采样归 `ze_scene_core::FunctionGraph`；`Prepare` 阶段的两个
System 只做"取相机 / 取相位 → 调对象层 `refresh`"的接线。原先的 `GridBuildSystem`
已退役（其几何四段全部归位）。

3D 合成方式：`EguiSceneHost::present` 先铺 `SceneView::background`，再**遍历**
`SceneView::layers`（索引序 = 合成序，先 = 下），把**全部**图层（S4.1 后 2D / 3D /
内容纹理都走 GPU）交给**单个** `SceneCompositorCallback`（`Shape::Callback` 注册在
有效序**起点**），由它一次性离屏渲染并合成全部图层。
不使用 `Order::Background`，clip rect 自动限制到中央视口。

### 技术约束

- Rust edition 2024
- hecs 0.11 为唯一 ECS 后端，不替换
- wgpu 30 / egui 0.36 / eframe 0.36
- glam 0.33：NDC Z∈[0,1]；上层经 `ze_notion::proj` 使用 `directx::` 系列（不直接引 glam）
- Transform2D traits 由 `ze_macro::Transform2DInto` 过程宏派生
- System trait 关联类型 `Reads` / `Writes` 无默认值（stable Rust 不支持 associated type defaults）
- `ErasedSystem` 为 object-safe 运行时 trait，`Box<dyn ErasedSystem>` 存储
- Scheduler 每阶段独立 CommandBuffer，阶段结束 flush
- ExecutorContext `resources` 字段私有，仅通过 `get<T>()` / `get_mut<T>()` 访问
- DrawQueue 使用稳定排序保留同 ZIndex 的插入顺序

### 两阶段内容管线（ze-content / ze-prerender）

```
源码字符串 → ze-prerender（engine: typst [feature] / placeholder）
           → ze-content::ContentIR（StyledPath + fill/stroke + Span 锚点，pt，Y-up）
           → zela-render::ir_to_draw_cmds（scale = 目标字号 / nominal_font_size，
                                          Shape2D::Path + Style2D + Transform2D）
           → zela-app::formula（接线桥：flatten_subpaths + apply_point
                               → Path 组件（Path2D）+ StrokeSolid，每个子轮廓一条实体）
```

- **铁律**：`typst` 仅允许出现在 `ze-prerender` 且 feature-gated；`zela-render`
  只依赖 `ze-content`（验收：`cargo tree -p zela-render` 无 typst）。
- 锚点两级映射：`Glyph.span` → `WorldExt::range`（main source 字节区间）
  → 原始归一化源码 `find`；归一化/哈希统一由 `ze-content` 实现，保证跨引擎一致。
- **消费侧接线**（`crates/app/zela-app/src/formula.rs`）：复用 `ir_to_draw_cmds` 的
  缩放/平移，不另写变换；用 `Path2D::flatten_subpaths` 按子轮廓分组，每个子轮廓
  单独 spawn 一条 `(Path, StrokeSolid)`（多轮廓字形 `o`/`B`/`8` 因此不会出现横穿
  字形内部的桥接线）。零 System 改动——复用现有 `record_scene` 的 `(Path, StrokeSolid)`
  分支。
- **字号解耦**：`FormulaSpec.nominal_font_size`（Typst 编译标称字号，默认 12.0）决定
  曲线提取精度与 IR 本地点尺度；`FormulaSpec.font_size` 是目标显示大小，仅经
  `ir_to_draw_cmds` 的 `scale = target / nominal` 生效。展平容差是相对标称尺度的绝对值，
  故显示尺寸缩小不会放大折线误差。
- **空心字口径（当前）**：旧管线 Fill 分支用 `convex_polygon`，非凸字形会被画成凸包，
  因此公式暂走 Stroke（轮廓）路径；实心填充待 S3 引入三角化后接管
  （挂在 `CustomSpec::to_triangles`；图元只带 `SpecId`，函数指针在具名规格表里）。
- feature：`zela-app` 默认 `typst`；`--no-default-features` 时回退 `placeholder` 引擎
  （显示方块，链路仍可跑通）。

### S1 插值内核与形变（路线甲）

```
ze-notion：Lerp / arc_lengths / resample_arc_length / Path2D::morph(+morph_points) / transformed
ze-time    ：Easing + AnimValue::Path + Curve::{Morph, PathMorph, Keyframes}
ze-content ：pair_by_anchor（语义配对，按源码锚点）
zela-scene  ：MorphShape 组件 + target::MorphT（`Arc<dyn Target>`，P0）+ MorphApplySystem（SceneSync）
```

- **路线甲**：`ze-time` 依赖 `ze-notion`（插值原语），但**不得**依赖 `ze-content`
  （语义配对知识留外层）。验收：`cargo tree -p ze-time | grep -c ze-content` = 0。
- 对齐按**弧长**（非索引）：`flatten` 输出点分布不均，按索引配对会让直线段被拉伸、
  曲线段被压缩，形变过程"抽动"。
- 输出顶点数**恒为 `samples`**（默认 128）→ 稳定开销，与输入复杂度无关。
- **场景接缝**：`MorphShape{from, to, t}` 只写 `t`（经 `ze_scene_core::animation::MorphT`），
  生产路径由 `zela-render::record_systems::MorphRecordSystem` 录成 `MorphItem`
  （from/to 常量 + 逐帧 `t`）交 GPU 形变管线；`MorphApplySystem` 是 CPU 回退
  （实体同时带 `Path` 时生效）。from/to 不每帧重建。
- **语义配对（S1b）**：`ze_content::pair_by_anchor` 按源码 `Span` 锚点配对
  （`x^2 → x^3` 的 `x` 配到 `x`，而非按位置配到 `3`）；单侧存在 → 淡入/淡出
  （退化为包围盒中心的一个点）；无锚点 → 退化为索引配对。
- 已知限制：旋转对齐（起点不一致会"绕圈"，原型期约定起点对齐）、
  Color 线性空间非感知均匀、当前为 CPU 插值。

### 呈现层度量空间（MeasureSpace）

让"宽度属于哪个空间"成为**可声明**的属性（仅 2D 描边宽度；不含 3D）。

**判断标准**：这个度量是**被观测对象的属性**，还是**观测工具的属性**？
- 被观测对象的属性（有数学含义的粗线段、公式描边）→ `MeasureSpace::World`：随缩放变化；
- 观测工具的属性（网格线、坐标轴）→ `MeasureSpace::Screen`：不随缩放变化；
- 需要保底时 → `MeasureSpace::WorldWithMinPx(min)`：世界单位但钳到最小像素。

属性分空间，不是对象分空间——同一个对象的不同属性可属不同空间
（向量箭头的**长度**是世界，**粗细**是屏幕）。

```
Stroke2D.space / StrokeSolid.space / Style2D.space（声明）
        │
        ▼
ze_notion::resolve_width(width, space, scale)   ← 唯一换算入口
        │
        ▼
zela-render::record_scene（从 camera.scale 取值）  ← 唯一换算点
        │
        ▼
DrawQueue（Style::Stroke.width 已是屏幕像素）
```

- **换算点必须唯一**：世界宽度需在**几何变换之前**乘一次 `scale`；若某些地方乘过、
  某些没乘，会出现"网格线在 2D 里对、3D 里错"这类不一致。项目中 `resolve_width`
  的非测试调用只有 `crate::draw` 的 `ResolveStyle for StrokeSolid` 一处。
- 默认 `MeasureSpace::Screen`（`StrokeSolid::new` / `Stroke2D::new` 两参构造），
  既有调用点行为不变；显式声明用 `new_in(...)`。
- 公式描边：`ir_to_draw_cmds` 把宽度乘 `target / nominal_font_size`（与几何同系数）
  换算为世界单位并标 `World` —— 否则 1.0pt 会被当成 1.0 世界单位，在 scale=40 下
  渲染成 40px。
- 与 S3 无关：`width / space / 当前 scale` 是 **per-draw uniform**，不碰顶点格式；
  只有到 S3 需要缓存几何时才把换算挪进 shader（接口不变）。

### S3 2D 渲染统一（M0 / S3-0 / S3-1）

**M0 图元体系迁移**：2D 载荷统一为 `Shape2D` / `Style2D`（旧 `Shape` / `Style` 枚举删除）。
`Painter2D::paint(&self, &Shape2D, &Style2D)`；`DrawQueue::push(DrawCommand { z, shape, style })`
+ `ensure_sorted()`（`Vec::sort`，稳定）+ `execute(&painter)`。迁移后画面不变。

**S3-0 三角化适配层**（`zela-render::tessellate`，纯 CPU / headless，无 feature 门控）：
`Shape2D + Transform2D + Style2D` → `Vec<Tessellation2D>`，顶点**严格**符合顶点契约
（`VertexLayout::standard`，**R0 后 stride 24、四槽** position/attrib0/attrib1/attrib2；
`Vertex2D { position, normal, side, advance }`，`size == 24` 编译期断言）。三角化用
`lyon_tessellation` 1.0，不自研；`tolerance_for_scale(scale) = (0.25/scale).clamp(1e-4, 1.0)`。
**描边几何参数化（R1）**：`position` 槽 = lyon `position_on_path()`（路径点，不含宽度），
轮廓点由 shader 现场补偿 `position + normal * (stroke_width * 0.5)`（`normal` 保持 lyon
原样、含 miter 缩放，**不得归一化**），故几何与宽度 / `camera.scale` 无关，填充顶点
`normal = [0,0]` → 偏移恒 0。宽度仍由 CPU 侧唯一换算点 `draw.rs::resolve_style` 解析。
`tessellation_layout()` 声明 `CacheStrategy::PerFrame`；2D 后端只读内容哈希、
从不读该标签，故标签**仅作声明**。

**S3-1 wGPU 2D 后端**（`zela-render-backends::wgpu_2d`）：离屏 `Rgba8Unorm`（非 sRGB，
存 gamma）+ MSAA 4x。`finish_frame` 流程 = 稳定排序 → 逐项三角化（**CPU 侧结果缓存，
命中 `mark_used` 防误驱逐**）→ **按 `(color, stroke_width)` 合批**（成员顶点拼接 +
索引 rebase 成一套缓冲，整批一次 `draw_indexed`；`draw_count` 仍是几何条数，
新增 `batch_count` = 实际 draw call 数）→ **形变项**（`from`/`to` 等点重采样后各三角化
一次并按内容哈希缓存，缓存键不含 `t`，逐帧只写 `t` uniform；`shape_2d_morph.wgsl`
双 vertex buffer 做 `mix`）→ `UniformArena`（**复用 `wgpu_3d`**，256B 对齐 dynamic offset，
`UNIFORM_SIZE = 96` = mat4 + color + **params**，`params.x = stroke_width`、`params.y = t`）
→ 离屏绘制。`SceneCompositorCallback`（egui-wgpu）经 `prepare` / `paint` 与之对接。
**S3-2 起本文件不再自带合成管线**（`pipeline_composite` / `composite_bgl` / `sampler` /
`composite()` / `COMPOSITE_QUAD` 已移除），离屏清屏改为**透明**，合成统一交给
`zela-render-backends::compositor::Compositor`。
**S4.1 起本文件是唯一的 2D 渲染路径**：旧 CPU 回放后端（`egui_2d`）与其
**编译期** feature、运行期管线开关已一并删除——2D 不再有"两条并存管线"，
全部 `Shape2D` 都经 lyon 三角化 + 离屏 MSAA + 合成。

**egui-wgpu 回调 viewport 契约（踩坑记录）**：egui-wgpu 处理 `Primitive::Callback` 时会把
render pass viewport 设为**回调自身矩形**（`set_viewport`，到下一个 primitive 才复位）。
即回调内 NDC `[-1,1]²` 对应回调矩形本身——合成四边形必须写死全 NDC 盒，不得用整屏尺寸
把视口像素矩形换算成裁剪坐标（否则视口被**二次应用** → 2D 视图在 X 方向被压扁）。
回归测试 `composite_lands_inside_scoped_viewport_on_real_device` 在旧实现下必然失败。

### S3-2 图层合成器

**目标**：每个图层渲染到自己的**离屏纹理**，由合成器按 `SceneView::layers` 的
**索引顺序**（先 = 下）混合进主 pass —— 2D / 3D 可任意叠放（此前 3D 硬编码在 2D 之上），
并为 S3-3（2D / 3D 互为纹理）铺路。

**声明契约**（`zela-scene::domain::view`）：
- `LayerBlend { Opaque, Alpha, Additive }`，`Default = Opaque`；
- `LayerDescriptor.opacity: f32` + `clamped_opacity()`（合成器唯一取值口径，clamp `[0,1]`）；
- `SceneView.background: Color32`，默认 `DEFAULT_BACKGROUND = (20,20,20,255)`
  —— 即改动前 2D 图层内那个全屏背景矩形；背景**不属于任何图层**，铺在所有图层之下。

**背景外移（C2）**：`GridBuildSystem` 不再产出 `ZIndex::new(-1000)` 的全屏矩形；
2D 离屏清屏 `CLEAR_COLOR` alpha 由 1 → 0（`Rgba8Unorm` 透明底）；背景改由
`EguiSceneHost::present` 在回放/合成前铺一次（CPU / GPU 两条路径都保留背景）。

**3D 离屏化（C3）**：`WgpuBackend3D::render_layer_offscreen` 把一层画进按视口尺寸缓存的
离屏纹理，清屏 **alpha = 0**；产出 `Layer3DFrame { offscreen, format, _arena_buffer }`。
`depth_stencil` 仍为 `None`（深度属 S4）。

**合成器（C4）**：`zela-render-backends::compositor`
- `CompositeOp { texture, src_format, blend, opacity, rect_px }` →
  `prepare(device, &[..]) -> CompositeFrame`（建 bind group + opacity uniform，
  **不重排**，层序 = 输入序）→ `composite_layers(pass, &frame)`；
- `COMPOSITE_QUAD` = 写死覆盖 NDC `[-1,1]²` 的 TriangleStrip（左上→右上→左下→右下），
  2D / 3D 共用；顶点布局仍来自契约 `VertexLayout::standard(AaModel::None, CacheStrategy::Static)`；
- **无动态 blend / 无动态入口点** → 预建 `HashMap<(LayerBlend, Conversion), RenderPipeline>`
  （3 blend × 3 conversion = 9 条，`label` 统一 `"zela-compositor-pipeline"`）；
- 混合（预乘 alpha）：`Opaque` = REPLACE；`Alpha` = 色/alpha 均 `One / OneMinusSrcAlpha`；
  `Additive` = `One / One`。`opacity` 由 uniform（x 分量）乘进源色，不走 blend constant；
- **sRGB 域转换**：`textureSample` 对 sRGB 格式纹理**自动解码**，故按
  (源 sRGB-ness, 目标 sRGB-ness) 选 fragment 入口：同域 `fs_identity`、
  源非 sRGB → sRGB 目标 `fs_gamma_to_linear`、源 sRGB → 非 sRGB 目标 `fs_linear_to_gamma`
  （`Conversion::for_formats`）。3D 离屏沿用主目标格式（源=目标 → Identity，逐字节保持现状），
  2D 离屏为 `Rgba8Unorm`（存 gamma）。

**接线（C5）**：`EguiSceneHost::present` 删除"`.find()` 取第一个 3D 层"的写法，
改为**遍历 `SceneView::layers`**（索引序 = 合成序）。
`UiConfig::flip_layer_order`（控制面板复选框）仅用于验收时翻转一次顺序。
两个图层都声明 `LayerBlend::Alpha` —— 离屏是透明底，用 `Opaque`(REPLACE) 会把透明像素
写进主 pass、抹掉背景与下层。

**单一回调（S3-2 修复）**：`egui_wgpu::CallbackResources` 是**全局共享**的
`HashMap<TypeId, _>`，**不是 per-callback 的**。早期"每个图层各注册一个回调"的设计里，
`Layer2DCallback` 与 `Layer3DCallback` 各自 `insert::<CompositeFrame>()` —— 后插入覆盖
先插入，两个 `paint()` 取到**同一帧**，把同一张离屏纹理画两遍（实机现象：顺序 A 只见 3D、
顺序 B 只见 2D）。故改为**单个** `SceneCompositorCallback`（`zela-render-backends::scene_callback`）：
- 持有本帧全部图层声明 `Vec<SceneLayer { id, kind, blend, opacity }>` + 两个队列快照；
- `prepare`：逐层渲染离屏 → 按声明序收集全部 `CompositeOp` → **一次** `Compositor::prepare`
  → 单个 `CompositeFrame`，与各层 `_arena_buffer`（保活）一起存入 TypeMap 的 `SceneCompositorFrame`；
- `paint`：设 scissor → `composite_layers`；
- `present` 只注册**一个**回调，放在有效序中**首个 GPU 图层**的位置（CPU 2D 层就地回放）。
- `LayerId` 改为**按层唯一**（`build_scene_view` 按索引派生），并有单测断言两两不相等。

**已知限制**：3D 离屏 MSAA 已与 2D 对齐（4x，B2-5；`DEFAULT_MSAA_SAMPLES`）；每层多一次全屏 blit（层数少时可接受，层多时优化）；
`opacity` × `blend` 组合未穷举验证；纹理采样走样（mipmap / 各向异性）仍是缺口（GAPS E-08）。

> 两条已退出限制：3D 两个 pass 均挂 `Depth32Float` 深度附件（`wgpu_3d::DEPTH_FORMAT`）；
> 旧 CPU 2D 回放（`egui_2d`）已在 S4.1 删除，2D 统一走 `wgpu_2d` 离屏 + 合成。

> **B1 批次（2026-10-21）已落地**（详见 `GAPS.md` D 段）：
> - **光照数据化**：`cube.wgsl` 的 `LIGHT_DIR` / `AMBIENT` 硬编码已删，改场景级 light
>   uniform buffer 驱动；`Light` 不带 position（位置走 `Transform` → `WorldTransforms`）。
> - **点光 / PBR**：Point light（range 窗口衰减，超限显式报错）；`ShaderVariant::Pbr`
>   （metallic-roughness，线性空间，法线贴图走**屏幕空间 TBN**，铁律 4 未改顶点格式）。
> - **脚本层 3D 入口**：`camera3d` / `cube` / `parametric_surface` / `material3d` / `light`
>   参数化构造器（不传顶点数组）⇒ 3D 首入 `--script == --bundle` 对拍网。
> - **pipeline 按需构建（B1-E）**：废弃 `(topology, variant)` 笛卡尔积预建，改
>   `get_or_create_pipeline` + 缓存，key 四维定齐 `(topology, variant, cull_mode, blending)`；
>   **不静默（B1-F）**：静默回退 Lambert 已结构性删除，点列表改 `panic!` 显式报错。

> **B2 批次（2026-10-21）已落地**（详见 `GAPS.md` E 段）：
> - **队列与层序（B2-1）**：2D / 3D 合并为**一个 `DrawQueue`**（`DrawItem::{Shape,Mesh,Line}`），
>   统一 `ZOrder`（IR 唯一声明；`ZOrder` 稳定排序，同层内保持插入序 —— 显式测试锁定 R-F）；
>   `DrawQueue3D` 成为类型别名；录制合并为 `SceneRecordAllSystem` 单写者。
> - **管线状态 IR 化（B2-2）**：`PipelineKey` 去 wgpu（`CullMode` / `LayerBlend`）+ 第五维
>   `VertexLayout`；作者只声明 `double_sided` / `transparent`（`Material3D`，D19）。
> - **2D 进 3D（B2-3）**：`derive_mesh` 把 2D 图元降级为平面网格（Flat + 双面 + 透明推导）；
>   透明桶按 `ZOrder` 排序、透明 pipeline 不写深度；epsilon 由 `ZOrder` 派生为 normal offset
>   （B2-4，不用常量 depth bias）。
> - **AA 统一（B2-5）**：3D MSAA 4x，采样数由能力侧给出（`new_with_samples` + `capabilities`）。
> - **可观测性（B2-6）**：xtask 双槽差分报告器（诊断旁路）；帧级 digest 采样旁路
>   （`ZELA_FRAME_DIGEST_SAMPLE=N`，不进序列哈希链）。
> - **脚本意图层 + demo（B2-7）**：`Material3D` 暴露 `double_sided` / `transparent`（version 6）；
>   新增 `fusion.lua` 入册。
> - **对拍与记账（B2-8）**：`demo3d` 升级为 frame hash fixture（3D 首入对拍网）；
>   `cargo verify --step hashes` 现覆盖 fourier / storyboard / demo3d。
> - **能力指纹（B2-9）**：`hash(signatures())` × `version` 配对校验，挂 xtask `structure`。

> **B3-0 前置批次（2026-10-21）已落地**（详见 `GAPS.md` F 段，**像素冻结**）：
> - **生产与上传解耦**：新增 `ze_vertex::VertexBufferData{ layout, vertices: Vec<u8>, indices, vertex_count }`，
>   生产端（`zela-render::vertex_record`）负责"几何 → 交错字节"，后端只做
>   `create_buffer_init`（组合由 N×M 降为 N+M）。
> - **3D 生产迁出**：`interleaved_pos_uv_normal` / `fallback_uvs` / line 顶点构造全部移到
>   `zela-render::vertex_record`（后端零命中），内联构造删除（B7 单一真源）。
> - **2D 打包迁出**：三角化调用与打包走 `tessellate_to_buffers` / `morph_endpoint_buffers` /
>   `tessellate_reveal_to_buffer`；`CachedGeometry` 缓存 `VertexBufferData`（两级缓存不变，B4）。
> - **不静默（B6）**：2D 三角化失败由 `eprintln + continue` 改为**显式错误**（有 `should_panic` 测试）。
> - 2D stride 24 / 3D stride 32 未变（D1）；像素与 B2 结束时**逐位相同**。

> **B3-0.5 批次（2026-10-21）已落地**（详见 `GAPS.md` G 段：Backend 抽象与 Pod 化）：
> - **Pod bulk 字节（B3-0.5a）**：`PodBytes` newtype + `PodValue::Bytes`；`Vec<u8>` 仍走
>   `Array(U8)`（语义不撞车）；脚本绑定层对 `Bytes` **显式报错**（bulk 数据不走脚本边界）。
> - **渲染目标契约进 IR（B3-0.5b）**：`ze_render_ir::target::RenderTargetDesc`
>   （尺寸 / `ColorFormat` / 采样数，serde + Hash + `validate()`）；GPU 句柄仍留后端。
> - **四个 render capability（B3-0.5c）**：`ze_render_ir::caps` 定义
>   `zela.render2d` / `zela.render3d` / `zela.render.mixed` / `zela.render.shapes` 的
>   签名集、能力指纹与 `negotiate`（**全有或全无**）；`structure` 步校验钉住指纹。
> - **后端自述（B3-0.5d）**：`zela-render-backends::render_backend::implemented_capabilities()`
>   声明 wgpu 实现 2d / 3d / mixed；**`shapes` 未实现** ⇒ 请求即显式协商失败。
> - **分层收口（B3-0.5e）**：`RenderTarget` / `RenderLayer` / `RenderBackend` /
>   `ResourceCache` shim 迁入 backends；`zela-render` 删除 `feature = "wgpu"`
>   （`cargo tree` 中 wgpu：12 → 0），成为真正的渲染无关层。
> - **像素口径纠正**：基线是**同后端 + 同机器**的测试夹具，不是跨端产品承诺
>   （`FRAME_HASH_BASELINES.md` / `baselines.txt` 同步）。

> **B3-1 批次（2026-10-21）已落地**（详见 `GAPS.md` H 段：场景层分层重构）：
> 原 `zela-scene` 12k 行按"概念 / 实现 / 能力"三分：
> - **`ze-scene-core`**（新）：场景对象（原 `ze-object`）+ 组件 + 纯逻辑
>   （animation 数据 / scene_script / persist / registry / domain / camera_input 纯函数 /
>   composer 数据）。**零 hecs、零 zela-***（CI 判据）。
> - **`ze-scene-api`**（新）：`SceneApi` / `ContentApi` + 两个 Service Pod（原 `pod/`）。
>   **零 hecs、零 zela-***；能力 namespace / version 不变（`zela.scene@6` / `zela.content@1`）。
> - **`zela-scene-ecs`**：只留 ECS 实现（world / resources / System 接线 / 生命周期）。
>   兼容壳层 `zela-scene` **已在同批删除**：237 处旧 `zela_scene` 路径全部改为
>   `zela_scene_ecs::`，ze-macro 派生代码改指向 `ze_scene_core::transform`。
> - **`ze-scene-hecs`**（收尾新增）：契约层 `SceneRead` 的 hecs 只读适配器
>   （`WorldView` / `object_id_of` / `entity_of`）。渲染层因此不再依赖 ECS 实现，
>   `cargo tree -p zela-render` 中 `zela-scene-ecs` 归零。
> - **`WorldTransforms` 下沉**：容器移入 `ze_scene_core::world_transforms`
>   （`from_map` / `insert` / `get` / `get2d`），生产仍由 ECS 侧
>   `TransformHierarchySystem` 负责。
> - **`zela-authored`**（新）：分镜 / demo 等**作品**移出场景层，app 按需依赖。
> - **`SceneWrite` 擦除写入口**：动画目标（`Target`）不再接受 `WorldScene`，
>   核心层因此完全不知道 hecs；ECS 用组件登记表实现具体分发。
> - **单一真源不变**：`ZOrder` 仍只在 `ze-render-ir` 声明；`ze-object` crate 已删除。

> **B3 批次（2026-10-21，进行中）**：Surface 双路径。
> - **派生桥复用三角化器**：`derive_mesh` 统一走 `tessellate_with`（lyon），覆盖
>   Rect / Circle / Ellipse / Polygon / Line / Path / Group；手写三角化已删除。
>   fill / stroke **各一份网格与材质**（`Vec<DerivedMesh>`，不再丢描边）；
>   2D 边法线（attrib0）丢弃，着色法线恒 +Z；`Polyline` / `Custom` 显式报错。
> - **Surface 归属**（`ze_scene_core::surface`）：`Surface`（bounds / mapping / anchor，
>   **无 position**）+ `SurfaceRenderer`（Renderer2D / Renderer3D，必填）。
>   归属沿 `Parent` 链解析（不存 `surface_id`），visited 环检测；缺 renderer ⇒ 显式错误。
> - **纹理契约**（B3 Step 4b）：`SurfaceTexture { texel_size(Fixed), filter, mipmap }`
>   + `camera_for`（bounds → 整张纹理）；缺声明 / 尺寸非法 / 宽高比不一致 ⇒ 显式错误。
>   `filter` / `mipmap` 不取后端默认值；mipmap **生成**仍未实现（E-08 仍成立）。
> - **Direct 路由**（`zela_render::surface_route`）：仅 `Renderer3D` 走 Direct；
>   失败（缺 renderer / 环 / Screen 映射 / Reveal·Morph·ContentTexture / 派生失败）
>   写进 `DrawQueue::surface_errors()`，不静默。
> - **Texture 路径（Step 4）与作品迁移（Step 5）尚未执行**：需要每帧编排
>   （`SceneCompositorCallback`）+ 纹理分辨率/过滤进契约（E-08 放大）。


### 已知未接线项（Known unwired，记账非缺陷）

以下各项是**有意保留的"已就绪未接线"**：用途明确、机制已就位，但当前不接线。
按本项目原则「有明确用途而非凭空设计 → 不是问题，但不能忘记」，集中记账于此。
（其中节点编辑器与其余各项性质不同：它**已排期**，只是触发条件未到 —— 见下表。）

> **S5 已把 `Parent` 移出本表**：`TransformHierarchySystem` 每帧产出 `WorldTransforms`，
> `SceneRecordSystem` / `RevealRecordSystem` 已消费（详见
> [`transform_hierarchy.rs`](crates/scene/zela-scene-ecs/src/transform_hierarchy.rs)）。
> **S1 换型为 3D**：`WorldTransforms` 只存 `Transform3D`（Mat4 单一真源），2D 消费方经
> `get2d` 读投影视图（丢 `z`，见 `GAPS.md` R-21）；`SceneRecord3DSystem` 同消费
> （`Reads = (WorldTransforms,)`），3D 父子层级已接线。

| 项 | 位置 | 现状 | 何时接 |
|---|---|---|---|
| `ObjectId` generation 失效检测 | 契约层 `ze_scene_contract::ObjectId::is_stale` | 有单元测试；**无生产消费方** | 引入对象销毁-重建流程时 |
| `Path3D` | 理念层 `ze-notion` | 已建、**零消费方**；与 `Path2D` **不同构**（不是同一类型的 3D 版本） | 需要真 3D 曲线时；因不同构，届时是**破坏性变更**，需单独立项 |
| `ze-notion`「全仓唯一数学元语出口」 | 全仓 | **2026-10-19 已收口**：`zela-render-backends` / `zela-app` / `zela-scene` / `zela-render` / `ze-time` / `zela-egui` / `zela-playback` 已改经 `ze-notion` 并删除 glam 依赖，CI 加门禁。剩余直接引用共 **25 次**（`*.rs` 出现次数口径）：`ze-notion` 定义侧 6 处、节点图组 17 处（`GAPS.md` R-19）、`zela-scene` 2 行说明性注释 | 节点图组在合成图（ComposerGraph）接入场景前统一；其余不再新增 |
| 节点编辑器（`zela-node-editor` / `zela-node-egui` / `zela-node-egui-ui`） | `crates/ui/*` + `crates/ze/graph/*` | **已排期（当前冻结）**：类型/算法/UI 齐备但**零生产消费方**（graph 6364 行 + UI 4502 行 ≈ 10866 行）。用户决策：**肯定要做，但要等 UI 接口与交互层稳定之后**，且**后期可能引入破坏性变更** | UI 接口与交互层稳定后单独立项 |
| 2D / 3D 录制 System 仍是两个 | `zela-render::record_systems`（`SceneRecordSystem` / `SceneRecord3DSystem`） | 两者已同住渲染层（A6），但仍是两个 System / 两个队列；是否合并取决于真实 3D 动画的录制结构 | 有真实 3D 动画时 |
| `formula.rs` 归位（632 行，约一半测试） | `crates/app/zela-app/src/formula.rs` | 承载"空心字"这一**既定视觉**；归位目标 crate 亦有争议（`ze-content` / `zela-render` / 新桥接层） | 真实动画确定它的形态之后 |
| `tessellate` 下沉 | `crates/render/zela-render/src/tessellate.rs` | 纯 CPU 三角化适配层，目前与 2D 后端同层 | 需要非 wgpu 三角化消费方时 |
| 非原生 Pod 的 `FunctionX2Y` 适配器 | `zela-scene::math_pod` | **C3 已消费（2026-10-17）**：第一个非原生 Pod（Lua / `ze-pod-lua`）已落地并接线 —— 方向是**脚本在装配期产出 `SceneScript`**，不参与每帧 `FunctionX2Y` 采样。走慢路径的 `FunctionX2Y` 适配器仍**无生产消费方**（`ze-pod-lua` 不消费它），故该子项继续记账 | 用户想用 Lua 定义**每帧采样**的数学函数时 |

> M4 起另有两项从"未接线"转为**已接线**：`Scene` trait 的**第二实现**
> （`zela-render` 测试里的非 ECS `TestScene`，与 hecs 侧 `WorldView` / `WorldScene` /
> `zela_scene_ecs::Scene` 并存），以及契约层的 `Scene::set` 写路径
> （消费方 `FunctionGraph::refresh`）。
>
> **W3 记账约束**：节点编辑器**后期可能引入破坏性变更**（用户决策），因此
> **现在任何跨层接口设计都不得以"将来节点编辑器要用"为理由** —— 这是
> 「不为假想需求设计」原则的直接应用。（它的 graph/UI 接口届时自会重画，
> 现在为它预留接口只会得到一份要拆掉的约束。）
>
> **W1 记账（非 UI 路径已被验证）**：`StandardBackends` + `render_frame_headless`
> 让渲染有了一条**不依赖 egui** 的入口，且与 egui 宿主路径**逐字节一致**
> （GPU 用例 `headless_path_matches_egui_path_bit_for_bit`）—— "Scene 可被任意消费"
> 从**类型层面的声明**变成**被验证的事实**。
>
> **B 线（序列帧导出，本轮已接线）**：`zela_app::export` 在 headless 入口上做
> "逐帧 → 抓像素 → PNG"，入口为 `cargo run -p zela-app --example export_frames`。
> **没有 seek**（Scene 不保证纯）⇒ 只能边跑边抓；视频编码仍交给外部工具。
> 每帧自建目标纹理的开销**只测不优化**（导出示例会打印 ms/帧）。
>
> **T0-② 判据口径已收窄（本轮修正）**：`new_target_requires_no_central_list_change`
> 只证明**构造期**（`SceneEncoder::animate::<T>` 走 `TargetCtor::make`）无需集中清单。
> **持久化期**仍需在 `target_ctor_by_name` 加一行（`serde::Deserialize` 拿不到外部
> registry 的必要取舍）—— 新增 target 若要可持久化，必须同步那张表。
> 函数侧走的是另一套（两步走：序列化写 name，反序列化由 Pod / registry 反查），
> 两套口径的差异是真实取舍，记账于此。
>
> **脚本 Pod（2026-10-17，C3 消费；D 线后归位到 `crates/ze/pod/ze-pod-lua`）**：
> 它是 `ze-pod-core` 的第一个**非原生 Pod** 生产消费方 —— 方向是**脚本在装配期产出
> `SceneScript`**（脚本 → 宿主经 `Injector` 注入的 `SceneApi` typed 函数；宿主 → 脚本
> 经 erased `Pod::call("zela.script::run")`），**不参与每帧采样**。
> 依赖图 / FanIn-FanOut 表**已包含** `ze-pod-core` / `ze-pod-macro` / `ze-pod-binding` /
> `ze-pod-lua` / `ze-pod-py`（2026-10-18 按 `cargo metadata` 口径重算，见文末备注）。
> 实际边：`zela-scene → ze-pod-core / ze-pod-macro`（`math_pod`）、
> `ze-pod-lua → ze-pod-core / ze-pod-binding / mlua`、`ze-pod-py → ze-pod-core / ze-pod-binding /
> ze-pod-macro / pyo3`；`zela-app → ze-pod-core / ze-pod-binding / ze-pod-lua / ze-pod-py`；
> **两条语言边都不含 `zela-*`**。
>
> **Pod 类型系统与绑定生成（2026-10-17，V 线）**：`ze-pod-core` 从"字符串签名"升级为
> **类型描述 IDL**：
> - `typespec.rs`：`TypeSpec`（Scalar / Struct / Enum / Array / Opaque）+ `ParamSpec`
>   （**形参名进 schema**）+ `ErasedFn`（绑定入口）；仍然零依赖、过 wasm gate；
> - `value.rs`：`PodValue` / `PodType` 只用于**跨语言边界**；`FunctionX2Y` 热路径
>   不引用（`math_pod` / `ze-time` 中 `PodValue|TypeSpec` 命中数为 0）；
> - `#[derive(PodType)]`：一处定义生成 schema + 双向转换 + `IntoPodTypeName`；
> - `#[capability(bindings)]`：生成 `FnIo` + 类型擦除 trampoline（`calls()`）；
> - `ze-pod-lua`（D 线前为 `zela-script`）：Lua 绑定与文档由 `calls()` + `TypeSpec` **自动生成**
>   （`Circle { ... }` / `Curve.Phase { ... }` 构造器），手写胶水只剩宿主落点转换；
> - `SceneApi` 升至 v2：4 个曲线函数 → `Curve` 枚举、`rgba:u32` → `Color` 结构体，
>   共 10 个函数。**未开启 `bindings` 的 capability 不受影响**（`calls()` 默认空）。
>
> **脚本接口补全（2026-10-17，W 线）**：
> - **target 变枚举**：`TargetKind`（`zela-scene`，`#[derive(PodType)]`）成为唯一真源；
>   `target_ctor_by_name` 退化为 `TargetKind::from_name` 的薄查询，字符串表已删除。
>   拼错变体在 Lua 绑定层即报错（不再静默不排期）；
> - **接口补齐**：`Axes2D` / `Arrow` / `Fill` / `Transform` / `RevealPath` / `PathShape`；
>   组件句柄可对应**组件组**（`RevealPath` = `Reveal` + `StrokeSolid`），内容样式不必在脚本重述；
> - **Injector 第二次真实使用**：`ContentApi`（`formula` / `text` → `Array<Path>`）独立成
>   Service，与场景服务共享 `PathStore`；排版引擎仍由 App 建一次后传入；
> - `storyboard.lua` 覆盖**全部 6 个镜头**；与硬编码参考实现逐对象 / 逐绑定等价
>   （占位引擎：25 对象 / 9 绑定）。`storyboard.rs` 保留为**参考实现**（默认启动路径仍在用），
>   是否删除由用户决定；
> - **W4**：删除 `spawn` 的 `ComponentList` 字符串 override（`Vec<Handle>` 自带 schema 名
>   `Array<ze_pod_core.Handle>`）；非法组件句柄从"静默跳过"改为 `run` 的明确失败。
>
> **Pod 解耦与 Broker（2026-10-17，B 线）**：
> - **B1 语言无关绑定层**：新增 kernel crate `ze-pod-binding`（只依赖 `ze-pod-core`，可进 wasm gate）。
>   它持有 `BindingPlan` / `ValueCodec` / `invoke` / JSON 文档；Lua 适配器只实现 `LuaCodec`。
>   `ze-pod-binding` 中不出现任何语言运行时类型；
> - **B2 domain 与解释器分居**：`SceneApi` / `ContentApi` / 类型词汇 / `SceneServicePod` /
>   `ContentServicePod` / `PathStore` 全部搬到 `zela-scene::script`（**D 线后改名
>   `zela-scene::pod`**）；Lua 适配器只留 Lua codec（**D 线后 `zela-script` 整个删除**）。
>   `grep SceneServicePod crates/ze/pod/ze-pod-lua/src/` = 0；
> - **B3 Broker**：`ze-pod-core::Broker` 收拢"需求校验 / 拓扑排序 / 环检测 / 注入 / 生命周期"。
>   依赖边 = `requires.service → provides`；`A → B → A` 在注入前即报 `BrokerError::Cycle`
>   （含参与链），不会构造 `Arc` 互持。每次装配打印一次报告（顺序 / 边 / 注入 / optional 缺失）；
> - **B4 Python（feature 门控）**：`ze-pod-py`（B4 时名 `zela-py`）用 pyo3 实现 `PythonApi`（`linspace` / `make_dot`）。
>   Python 侧经注入的 `scene.*` 建对象（通用绑定驱动 + `PyCodec`）；Lua 侧 `python.*` 表由
>   同一套绑定生成。feature 关闭时 `cargo test --workspace` 不引入 pyo3；
> - **optional 首次有消费方**：`python` 能力以 optional 需求注册 —— 无 Python 时装配通过、
>   `python` 表不存在；有提供者时自动接线。
>
> **Pod 层彻底解耦（2026-10-17，D 线）**：
> - **`zela-script` 删除**：`ScriptHost` 整个消失（`grep -r ScriptHost crates/` = 0）；
>   语言适配器拆为 `crates/ze/pod/ze-pod-lua` 与 `crates/ze/pod/ze-pod-py`，
>   两者**互不依赖**，生产边**不含任何 `zela-*`**（`cargo tree -p ...` 实测）；
> - **domain 归位**：`zela-scene::script` → **`zela-scene::pod`**。内容服务带来的
>   `zela-scene → ze-prerender` 依赖仍在（记账：若将来 scene 要进 wasm，这条会先挡路）；
> - **Broker 补 `run` / `run_with`**：在已装配的图上取节点 → `Pod::call` → downcast，
>   承担原 `ScriptHost::run` 的职责；错误通道为 `UnknownNode` / `BadEntry` /
>   `UnknownEntry` / `Run` / `BadOutput`；
> - **D3 描述层 / 执行层分离**：`ze-pod-core::AssemblyPlan` / `NodeSpec` 是**纯数据**
>   （拓扑 / 环 / optional 缺失可在不建 Pod 时单测）；App 的 `src/assembly.rs`
>   声明静态插件清单，Broker 消费它完成校验 / 排序 / 注入，并在 debug 下断言
>   "描述层 == 执行层"；
> - **资产归 App**：`storyboard.lua` 在 `crates/app/zela-app/scripts/`，
>   `export_scene_api` example 在 `crates/app/zela-app/examples/`；
> - **测试归属**：`ze-pod-lua` 用 dev-dependency 自建 mini domain Pod 验双向；
>   完整 6 镜头等价性仍在 `zela-app`（Lua / Python 两版各一条）。
>   两个语言适配器**不进 wasm gate**（mlua 需 C 工具链、pyo3 需系统 Python）。
>
> **Python 脚本与文档（2026-10-17，D+ 线）**：
> - `ze-pod-py` 补 `run` 入口（`zela.python::run(String) -> Any`），协议与 Lua 版同形
>   （入口服务 `__reset` / `__finish`）：Python 脚本在注入后的模块 globals 里执行；
> - App 新增 `scripts/storyboard.py` 与 `--python`（`SceneChoice::PythonStoryboard`），
>   与硬编码分镜逐对象 / 逐绑定等价（25 对象 / 9 绑定）；
> - **Python Pod 默认开启**：`zela-app` 的 `default` features 含 `python`
>   （需要系统 Python；关闭后 Lua 路径仍可用）；
> - 文档导出新增按 capability 命名空间落盘的 `.pyi`
>   （`zela/__init__.pyi` / `zela/scene.pyi` / `zela/content.pyi` / `zela/python.pyi`）；
>   `ze-pod-py` 同时把 capability 注册为真实的 `zela.*` 模块并安装到 `sys.modules`，
>   脚本可 `import zela.scene` / `from zela import scene, Circle`；
> - 多 `PyPod` 实例使用**唯一模块名**（`PyImport_ExecCodeModuleEx` 会复用同名模块，
>   否则实例间会共享 globals），并对 `sys.modules` 的安装 + 执行加锁串行化。
>
> **备注（2026-10-18，脚本层与 Pod 入图）**：按 `cargo metadata`（仅生产依赖，
> `kind != dev`）全量重算，内部依赖边 **59 → 73（+14）**：
> - 新增 `ze-pod-binding → ze-pod-core`；`ze-pod-lua → ze-pod-core / ze-pod-binding`；
>   `ze-pod-py → ze-pod-core / ze-pod-binding / ze-pod-macro`；
> - 补齐 `zela-scene → ze-content / ze-prerender / ze-pod-core / ze-pod-macro`；
> - 补齐 `zela-app → ze-pod-core / ze-pod-binding / ze-pod-lua / ze-pod-py`；
> - FanIn/FanOut 表由 24 行扩为 **29 行**（新增 `ze-pod-core` 5/0、`ze-pod-macro` 2/0、
>   `ze-pod-binding` 3/1、`ze-pod-lua` 1/2、`ze-pod-py` 1/3；`ze-notion` FanIn 11、
>   `zela-scene` FanOut 11、`zela-app` FanOut 14）；
> - 程序化比对结果：mermaid **73 条 ⇔ Cargo.toml 73 条**，零缺零余。
>   工具恢复后重跑 `cargo-workspace-analyzer -d .` 即可覆盖回自动产物。

### cargo-archtest 适配状态

`cargo-archtest v0.2.3` 的架构模型是「单 crate 内按 `src/` 子目录分层」，
不适用于本 workspace 的「workspace 级 crate 分层」。当前 CI 流程：
- `cargo check --workspace --all-targets`（编译自然拦截反向依赖）
- `cargo-workspace-analyzer -d .`（依赖分析 + 循环检测）

> 备注（2026-09-03）：本次刷新时 `cargo-workspace-analyzer` 在本机连续
> 挂起（CPU 空转、无文件输出；正常时会生成 `workspace-analyzer.mmd` /
> `architecture.json`），故图与 FanIn/FanOut 表改用
> `cargo metadata --format-version 1`（仅生产依赖，`kind != dev`）等价重算。
> 工具恢复后重跑 `cargo-workspace-analyzer -d .` 即可覆盖回自动产物。
>
> 备注（2026-10-13）：S1 引入 `ze-time → ze-notion` 边后按同一口径再次重算
> FanIn/FanOut（`ze-time` FanOut 0→1、`ze-notion` FanIn 5→7、`zela-app` FanOut 5→8）。
>
> 备注（2026-10-15）：依赖图与 `workspace-analyzer.mmd` 按同一 `cargo metadata` 口径
> 手工补齐（工具仍不可用），合计补 9 条边：
> - 本文件图区：修正 `zela_node_egui → ze_node_core`、`zela_node_egui_ui → ze_node_schema`
>   两处前缀笔误，并补 `ze-node-schema → ze-node-core` / `→ ze-node-schema-macros`（2 条）；
> - `workspace-analyzer.mmd`：补 `ze-prerender → ze-content` / `→ ze-notion`、
>   `ze-time → ze-notion`、`zela-app → ze-content` / `→ ze-notion` / `→ ze-prerender`、
>   `zela-render → ze-content`（7 条，29 → 36）。
>
> FanIn/FanOut 表经复核无需改动。工具恢复后重跑 `cargo-workspace-analyzer -d .`
> 即可覆盖回自动产物。
>
> 备注（2026-10-15，R1.6）：场景层 crate 由 core 更名为 `zela-scene`、ui 层的过程宏
> crate 归位 kernel 并更名为 `ze-macro`、ECS 调度内核抽为 `zela-schedule`
> （置于 `crates/schedule/`），依赖图按同一 `cargo metadata` 口径同步：
> - 新增 `schedule` 层 / 节点 `zela-schedule`；kernel 层补 `ze-vertex` / `ze-resource` /
>   `ze-macro`（R1 下沉 + R1.5 归位）；render 层描述改为「场景层 + 渲染抽象 + 具体后端」，
>   ui 层移除 proc-macro；
> - 补边（相对 R1 基线）：`zela-scene → zela-schedule`、`zela-app → zela-schedule`、
>   `zela-egui → zela-schedule`、`zela-render → ze-vertex`、`zela-render → ze-resource`、
>   `zela-egui → ze-notion`；
> - 图与 `workspace-analyzer.mmd` 保持「仅含 workspace 内部 crate 节点」的既有约定，
>   `zela-schedule → hecs`（外部 crate）不入图。
>
> FanIn/FanOut 表按 `cargo metadata` 全量重算（18 → 21 行）。工具恢复后重跑
> `cargo-workspace-analyzer -d .` 即可覆盖回自动产物。
>
> 备注（2026-10-16，M2/M3）：新增层 `object`（`crates/object/ze-object`）与 kernel
> 节点 `ze-render-ir` / `ze-scene-contract`，依赖图与 FanIn/FanOut 按同一
> `cargo metadata` 口径同步：
> - 新增边：`ze-render-ir → ze-notion`、`ze-scene-contract → ze-notion / ze-render-ir`、
>   `ze-object → ze-notion / ze-render-ir`、`zela-scene → ze-object`、
>   `zela-render → ze-render-ir / ze-object`、`zela-egui → ze-object`（9 条）；
> - 表由 21 → 24 行：`ze-notion` FanIn 8→11、`zela-scene` FanOut 4→5、
>   `zela-render` FanOut 5→7、`zela-egui` FanOut 6→7；新增 `ze-scene-contract`（0/2）、
>   `ze-object`（3/2）、`ze-render-ir`（3/1）。
> - **本轮为部分落地**：`ze-object` 暂不依赖 `ze-scene-contract`（`Describable`
>   派生宏延后），`record_scene` 暂保留 `&hecs::World`（见 M2/M3 交付说明）。
> 工具恢复后重跑 `cargo-workspace-analyzer -d .` 即可覆盖回自动产物。
>
> 备注（2026-10-16，M4）：M 阶段收尾。**M4 本身未新增依赖边**（它只搬逻辑，不加 crate 依赖），
> 但 M3 余项引入的边（`ze-object → ze-scene-contract`、`zela-scene → ze-object`、
> `zela-render → ze-object / ze-scene-contract`、`zela-render-backends → ze-object`、
> `zela-app → ze-object`）在上一轮的表中尚未计入，故本轮把 FanIn/FanOut 与
> `workspace-analyzer.mmd` 一并按同一口径全量重算：
> - 关键变化：`ze-scene-contract` FanIn **0 → 3**（消费者 `ze-object` / `zela-scene` /
>   `zela-render`，Instability 1.00 → 0.40 —— **契约层不再是零消费方空层**）；
>   `ze-object` 3/2 → **5/4**、`ze-render-ir` 3/1 → **4/1**、`ze-macro` 3/0 → **4/0**、
>   `zela-render` 3/7 → 3/8、`zela-scene` 4/5 → 4/7、`zela-render-backends` 2/3 → 2/4、
>   `zela-app` 0/9 → 0/10；
> - 结构判据（本轮）：`Parent` 定义 1 处、`record_scene` 不认识 hecs、
>   UI 层无几何计算（`GridBuildSystem` 退役）、装配对象 `Axes2D` / `FunctionGraph`
>   在对象层。已知未接线项（`Parent` 累乘 System、`Scene` 的第二实现等）见上文
>   「已知未接线项」章节。
> 工具恢复后重跑 `cargo-workspace-analyzer -d .` 即可覆盖回自动产物。
>
> 备注（2026-10-16，M4 收尾）：本轮**无行为变更**（只改注释 / 文档 + 一处换算归位）。
> - UI 层最后一处几何换算（指针拖拽像素 → 世界位移）归位到对象层
>   `Camera2D::screen_delta_to_world`，`host.rs` 只保留"取负"这一**控制器**语义；
> - `SceneRecordSystem` / `ContentTextureRecordSystem` / `MorphRecordSystem` 补写
>   "与 hecs 的接触面"标注（替换 `Scene` 实现时各自要改哪里）；
> - `Axes2D::refresh` 补写性能记账（每帧约 39 个实体 + 图元下沉判据 / 触发条件）；
> - 新增上文「已知未接线项」章节。
>
> 备注（2026-10-16，App 简化与渲染层清理）：本轮做**减法**（补下层能力 → 上层代码自然消失）。
> - **P1 能力补完**：`Circle2D::sample(n, start_angle)`（理念层补上与其他代数同类的"离散化"）、
>   `Camera2D::fit_bounds(bbox, margin)`（对象层补上"取景适配"）。补完之后 app 里的
>   手写 64 点三角函数循环与 `path.commands` 包围盒遍历**消失**（改为 `Circle2D::sample`
>   与 `Path2D::bounds` + `Rect2D::union`）；`grep -n "commands" crates/app/.../lib.rs` = 0。
> - **P2 渲染层清理**：删除 `draw_queue_2d.rs`（`DrawQueue2D` / `DrawCommand2D`，123 行，
>   零生产消费方 —— 保留只会成为"纸面能力"）；修正 `zela-render/src/lib.rs` 里
>   自相矛盾的模块注释（`shape.rs` 并非真实文件，是路径稳定的转发模块）；
>   `components.rs` 加"兼容 facade，非死代码"标注；**新增 `scene` 层**：
>   `crates/render/zela-scene` → `crates/scene/zela-scene`（层序改为
>   `kernel → object / graph → schedule → scene → render → ui → app`）。
> - **P3 app 归位**：后端安装 → `zela_render_backends::install_standard_backends`（app 一行）；
>   CJK 字体 → `EguiSceneHost::install_cjk_font`（跨平台候选 + `ZELA_CJK_FONT`，不再硬编码
>   Windows 路径）；内容纹理常量 → `ze_scene_core::{CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH,
>   CONTENT_TEXTURE_HEIGHT}`（值不变 1 / 512 / 512）；可观测与帧管理 →
>   `EguiSceneHost::draw_call_count` / `end_frame`。
> - **依赖图无变化**：本轮只移动目录与搬内部实现，**没有新增/删除任何依赖边**，
>   package 名亦未变 ⇒ FanIn/FanOut 表与 `workspace-analyzer.mmd` 无需重算（工具不可用也不影响）。
>
> 备注（2026-10-16，首个数学动画原型 P0–P5）：本轮第一次**为"内容"改造引擎**
> （判据：脚本里写不出来的东西才补；脚本用不到的一个都不补）。依赖图新增两条边：
> `zela-render → zela-schedule`（三个录制 System 需要 `System`，P4）与
> `zela-render → hecs`（**仅 dev-dependencies**，测录制 System 用）。
> - **P0 目标 trait 化**：`BindingTarget` 枚举 → `Arc<dyn Target>`
>   （`zela-scene::animation::target`）。五个目标：`CircleRadius` / `Phase` / `CameraEye` /
>   `MorphT` / `Camera2DFraming`（新增）。新增驱动目标**不改枚举、不改 System 源码**。
>   代价（记账）：失去 enum exhaustiveness；`dyn Trait` 不能 serde。为 UI 分类当时另加了
>   无载荷的类别判别式（**T0-② 已删除**：改由 target 自报 `name()` / `target_id()`）。
> - **P1 相机归位 + 解绑**：`AppCamera` 删除 → 相机是**场景对象**（`ze_scene_core::Framing2D`，
>   无 `rect`）；`Camera2D` 与渲染目标 `rect` 解绑（录制时经 `ViewRect` 绑定，方案 a）；
>   控制器纯函数移到 `zela-scene::camera_input::apply_pan_zoom`；
>   `SceneRead` 新增只读**点查询** —— 实现为访客形态 `with<T, R>(id, f)`：
>   hecs 0.11 的 `World::get::<&T>` 返回**借用守卫** `Ref<'_, T>`，`&T` 交不出去
>   （与 `for_each` 同一约束；计划原文的 `Option<&T>` 断言与代码事实不符，以代码为准）。
> - **P2 相机句柄**：`ze_scene_core::Camera2DHandle(pub ObjectId)` + `resolve(scene, rect)`
>   + `spawn_camera2d`；本轮**只做引用型**，自由相机内联参数按 §5.1 延后（分镜用不到）。
> - **P3 Timeline**：`zela-scene::animation::timeline`，顺序游标 → 每段一个
>   `ClockNode{local_offset, span:(0,dur), mode:Hold}` + `Binding`；`parallel()` 表达
>   **同区间多动作**（镜头推进同时改 scale/center）。`ClockForest` / `eval_local` **零改动**。
> - **P4 录制 System 归位**：三个 System → `zela-render::record_systems`；`ViewRect`
>   下沉到 `zela-render`；`MorphRecordSystem` / `ContentTextureRecordSystem` 的
>   **内联 hecs 查询**改为契约层 `for_each*`（渲染层仍不认识 hecs）。
> - **P5 分镜脚本**：`zela-app::storyboard` 声明 §1.1 六条分镜（网格常驻 / 相机对象 /
>   0–2s 曲线生长 / wait / 2.5–3.5s 镜头推进 / 3.5–4.5s 公式浮现）；
>   原 2D demo（振动圆 / sin 曲线相位 / 圆↔方 / `x²`↔`x³`）被分镜取代，3D demo 不动。
> - **P5 实测修（引擎缺陷，非分镜问题）**：**零长度路径 lyon 不产出几何** ——
>   "退化点 → 完整折线"的形变（分镜 3/6）会被 2D 后端**整条跳过**，表现为
>   2D 视口只剩网格、**看不到任何动画**，而 `MorphQueue` 层完全看不出异常。
>   修法：`wgpu_2d::prepare` 在仅一侧可三角化时，用其拓扑合成另一侧
>   （顶点位置塌到该侧的点、法线归零 ⇒ `t=0` 不可见、随 `t` 平滑长出，
>   索引/顶点数按构造相等）。证据：`morph_from_degenerate_point_grows_into_target_line`
>   （GPU 像素级：`batch_count == 1`、覆盖像素随 `t` 严格递增、`t=0` 为 0）。
> - **P5 实测修（二）**：形变端点原先**恒按闭合环**重采样 ⇒ 开口曲线（`y = sin x`）
>   首尾之间多出一条**可见的弦**（画在 x 轴上）。改为**闭合性由两端共同决定**
>   （任一端带 `Close` 即闭合，两端都开口则保持开口）。
>   用例：`open_morph_endpoints_stay_open_without_chord` / `closed_morph_endpoints_stay_closed`。

### 相机控制与合成器滤镜（K 线 / C 线）

> 两条**无交集**的线：K 线在 `zela-scene` / `zela-egui` / `zela-app`，C 线在
> `zela-render` / `zela-render-backends`。执行中**未发生文件冲突**。

**K 线（相机控制）**

> - **K0 模式接线**：`CameraMode` 从"零消费方预留变体"变为**真正被消费** ——
>   模式是**相机对象的组件**（支持多相机各自不同模式；未挂 ⇒ `Default` = `PanZoom`）。
>   新增 `Fps` 变体；因枚举是 `#[non_exhaustive]`，另给 `CameraMode::{PAN_ZOOM,ORBIT,FPS,PATH}`
>   关联常量作为**下游构造路径**。
> - **命令路由（实测收敛的结论）**：app 中央视口里 2D / 3D 图层**占据同一矩形**，
>   "在哪层拖拽"无法从坐标区分 ⇒ 路由按**模式能力**：各模式只消费属于自己的命令，
>   其余 no-op（`PanZoom`：Pan/Zoom；`Orbit`：Orbit/Dolly；`Fps`：Look/Move；`Path`：全部忽略）。
>   `SceneEvent::Camera` 的 `layer` 字段因此**未被消费**（单视口 + 模式互不重叠）。
> - **K1 FPS**：新增命令 `Look{delta_yaw,delta_pitch}` / `Move{delta:Vec3}`（x=右/y=上/z=前）
>   + 组件 `ze_scene_core::FpsView{yaw,pitch}`（**显式存朝向**：增量旋转会漂移且 pitch 无法 clamp）
>   + 纯函数 `apply_fps`；pitch 钳 `±FPS_PITCH_LIMIT = π/2 − 0.01`。
> - **K2 轨迹相机**：`Path` 模式**忽略全部交互命令**（与既有语义注释一致）；
>   轨迹**未新增 Curve 变体** —— `Keyframes`/`Sampled` 已能驱动 `Framing2D.center` /
>   `CameraEye`（分镜的镜头推进即生产中的 2D 轨迹样例）。
> - **K3 3D 轨道**：纯函数 `apply_orbit`（Orbit 绕 target 保持半径 + 俯仰钳制；Dolly 改
>   eye↔target 距离）。**与自动轨道动画的冲突：会打架**（`CameraEye` 绑定每帧无条件写 `eye`）；
>   处置 = **模式互斥 + 显式让位**：`CameraDriveSystem`（app 层）按「视口操作目标」把
>   3D 相机模式切到 `Orbit`/`Fps` 时，把该轨道绑定**移出 `BindingTable`**（切回 2D 时放回）。
> - **K3 补（2D 也让位）**：同一条实测结论对 2D 相机同样成立 —— 分镜的 `Camera2DFraming` 绑定
>   （`center` + `scale`）也每帧无条件写、把用户的 Pan/Zoom **同帧抹掉**（相位序
>   `Input` → `SceneSync`）。处置 = 新增 `UiConfig::free_2d_camera` 开关：
>   打开 ⇒ 这 2 条分镜绑定**移出**、2D 相机模式改 `PanZoom`（接受交互）；关闭（默认）⇒ 放回、
>   模式 `Path`（分镜驱动）。两台相机的让位判定由**同一个** `CameraDriveSystem` 原子完成 ——
>   它们让位的是同一张 `BindingTable`（见下条「实测坑」）。
> - **GPU 像素级验收**：`test_k_line_camera_controllers_change_pixels`（同一场景用
>   控制器产出的位姿渲染 4 次：Look 后覆盖塌缩 / Move 后覆盖增大 / Orbit 后画面改变 /
>   轨迹三采样帧互不相同）。
> - **实测坑（app 启动即 panic）**：3D 模式 System 起初注册在 `LogicUpdate`，
>   与 `ParamPanelSystem` **同相位写 `BindingTable`** ⇒ `Scheduler::add_system` 的
>   debug 静态冲突检查在启动时 panic。**分别安装单个 System 的单测测不出来**
>   （`App::new` 需要 `eframe::CreationContext`，跑不到）。
>   处置：相机驱动 System 移到 **`Input` 相位**（那里没有别的 `BindingTable` 写者），
>   并在 `CameraInputSystem` **之前**注册（同帧改模式 ⇒ 同帧按新模式解释输入）；
>   同时把注册序列抽成**唯一注册点** `register_systems`，由
>   `full_system_graph_assembles_without_borrow_conflicts` 覆盖整张图
>   （已用"放回 LogicUpdate"证伪：该用例能复现同一条 conflict panic）。
>   **相位约束记账**：`BindingTable` 的写者 —— `ParamPanelSystem`(LogicUpdate) 与
>   `CameraDriveSystem`(Input)；`BindingApplySystem`(SceneSync) 只读。

**C 线（合成器滤镜）**

> - **C0 post 基础设施**：新增**中间全屏纹理**（格式 = 主 pass 目标格式 ⇒ 不再多一次域转换）
>   + 一次全屏 pass。**空滤镜链走直通路径（零改动、逐位不变）**；非空才建中间纹理。
>   证据：`gpu_post_route_with_identity_filter_matches_direct_bit_exactly`
>   （恒等滤镜槽走完整 post 路径 ⇔ 直通，**逐位一致**）。
> - **C1 uber shader + 4 固定槽位**：`shaders/post_filter.wgsl` 按 `kind` 分支（uniform 分支），
>   滤镜**不再是管线维度** ⇒ 管线数仍 = 3 blend × 3 域转换；`FilterKind::code()` 与 WGSL
>   常量表一一对应（单测锁定）。uniform 160 B（头 + 纹素步长 + 4×32 B 槽）。
>   证据：`gpu_filters_add_no_pipelines_and_pipelines_are_lazy`（三种滤镜跑完整链路后
>   `pipeline_count() == 1`）。
> - **C2 三类滤镜各一**：点运算 `Gain`（每通道增益）、几何重映射 `Distortion`
>   （**逆向映射** `uv' = c + (uv−c)·scale·(1+k1·r²n+k2·r²n²)`）、邻域 `Blur`（3×3 高斯，
>   权重 4/2/1、和 16）。**预乘 alpha 硬约束**：增益对 rgb 与 a 同乘（或 rgb 增益 ≤ 1）、
>   模糊在**预乘空间**做 —— 证据：`gpu_post_filters_point_geometry_neighborhood`
>   断言边界半透明像素 `r ≈ a`（非预乘空间会出现黑边 ⇒ 该断言会失败）。
> - **C3 惰性管线缓存**：`Compositor::new` **不再预建** 9 条；`ensure_for(&mut self, ..)`
>   按本帧 `(blend, 域转换)` 组合现场建、建后入表。**签名核查结论**：`paint` 拿到的是
>   `&CallbackResources`（不可变）⇒ 不能把 `composite_layers` 改 `&mut self`；落点是
>   **在 `prepare`（有 `&mut`）里 `ensure_for`** ⇒ `composite_layers` 借用形式**零改动**，
>   无需 `RefCell`/`Mutex`。实测（debug）：首建 **2.58 ms**、命中 **2.0 µs**。

**R 线（书写动画 reveal）**

> - **R0 契约扩容（解锁项）**：`VertexAttrib::VOCABULARY` 三槽 → 四槽（`+ "attrib2"`），
>   `VertexLayout::standard()` 加 `attrib2(F32)@20` ⇒ **stride 20 → 24**；`AaSlotSemantics`
>   加 `attrib2` 字段，`Analytic → AttribSemantic::Advance`（填上契约里**早已预留**的语义）。
>   连带改动（逐条都在计划 §11 清单里）：`Vertex2D` 加 `advance`（`size == 24` 编译期断言）、
>   `vertex_layout` 翻译测试的 offset/stride/location、`COMPOSITE_QUAD` 字面量、
>   `quad_bytes_are_twenty_per_vertex` → `..._twenty_four_...`、`shape_2d_morph.wgsl` 的
>   `to_*` location **顺延 +1**（from 占 0..3 / to 占 4..7）。
>   **哈希漂移**：布局哈希全变一次（进程内 pipeline 变体缓存重建；无持久化 ⇒ 无迁移成本）。
> - **R1 三角化填 `s`**：`StrokeCtor` 取 lyon `advancement()`，`tessellate_stroke` 末尾
>   `normalize_advance` 除以**实测最大值** ⇒ `s ∈ [0,1]`。除以实测值而非自算弧长，是为了
>   与 lyon 的两条内部口径（丢近点、闭合收尾边）**同源**，不依赖对它的推断。
>   **预细分**（`REVEAL_STEP_PX = 2.0` 输出空间 px）：lyon **完全不细分直线段**，
>   不补点则切口精度 = 顶点间距（`sin` 曲线放大后几十像素的阶梯）。
>   **多子路径 = 累加**（实测，`lyon_tessellation/src/stroke.rs:1359`：新子路径起点沿用
>   上一条末点的推进量，中间不补"抬笔"距离）⇒ 书写顺序是"轮廓依次写出"。
> - **R2 shader 消费**：`shape_2d.wgsl` 加 `@location(3) advance`（顶点）+ 同名 varying
>   （片元），并在 `fs_main` **discard** 掉 `s ∉ [params.z, params.w]` 的片元。
>   **做法从 (a) 顶点塌缩改为 (b) 片元丢弃（实测证伪 (a)）**：塌缩到同一个视口外点只对
>   "整个三角形都在区间外"成立；实测描边三角化里存在**恰有一个顶点越界**的三角形
>   （miter join 邻域），它会从塌缩点拉出覆盖屏幕的楔形 —— `b = 0` 时仍有 **747** 个
>   不透明像素、且覆盖数**不单调**（`b = 0.75` 的 1162 > `b = 1` 的 1080）。切 (b) 后
>   三条像素判据全绿。`REVEAL_EPS = 1e-6` 吸收插值末位误差（否则 `b = 1` 会误丢末端片元，
>   `[0,1]` 无法与既有画面逐位一致）。`params.z/w` 是**空闲位**（原本 `w` 恒 0 ⇒
>   现在普通绘制写 `w = 1.0` 表示"区间全开"，这是**必须**改的一处：漏了会把整条几何裁空）。
> - **R3 动画接线**：组件 `Reveal { path, start, end }`（与 `MorphShape` 同级；独立组件
>   ⇒ 不进 `record_scene` 的组件组合，天然避免"整条路径被画两遍"）+ target
>   `RevealRange`（`Scalar → end`、`Vec2 → (start, end)`，口径对齐 `Camera2DFraming`）+
>   目标类别变体（**T0-② 已删除**）。**不改枚举以外任何 System**（P0 trait 化的红利）。
> - **R4 队列与录制**：`RevealItem { path, start, end, style }` + `RevealQueue` +
>   `push_reveal_item`（屏幕空间 + 唯一换算点）+ `RevealRecordSystem`（RenderCommit，
>   `R=(ViewRect, Camera2DHandle)`、`W=(RevealQueue,)`）。后端 `wgpu_2d` 的**趟 4**：
>   `tessellate_reveal` → 缓存键 = 内容哈希（**不含区间**）⇒ 稳态零上传；区间逐项不同
>   ⇒ **不进合批**，每项一次 `draw_indexed`；**复用同一个 `pipeline_2d`**
>   （即"新增书写动画不改 shader 之外的渲染结构"，管线数不变）。
>   `SceneCompositorCallback` 多一个 `reveals` 快照（`host::present` 从 `RevealQueue` 取）。
> - **R5 像素级验收（4 条，全部实测通过）**：`reveal_grows_monotonically`
>   （`b = 0/0.25/0.5/0.75/1` 覆盖像素 `0 → 严格递增`）、`reveal_is_ordered_along_path`
>   （`b = 0.25` 时 `max_x` 不越过四分之一处）、`reveal_full_range_matches_no_reveal`
>   （`[0,1]` 与不带 reveal **逐位一致**）、`reveal_interval_covers_middle_only`
>   （`[0.3,0.7]` 中段可见、两端无像素）。
> - **端到端**：整图注册回归（`full_system_graph_assembles_without_borrow_conflicts`
>   加 `RevealQueue`，13 个 System 无冲突）、真实 `cargo run -p zela-app` 跑 1700+ 帧无 panic、
>   强制重编后零新增 warning。
> - **分镜表达同步改变**：分镜 3 / 6 从 `MorphShape`（整条曲线从一点"绽开"）改为
>   `Reveal`（笔尖沿路径推进）—— 这才是 §0.1 要的 Write 类效果；形变管线仍在（GPU 用例覆盖）。
>   **公式必须逐轮廓"依次"写**（第一版用了 `.parallel` ⇒ 全部轮廓共享同一 `ClockNode`，
>   实测表现是"每个字符同时被画出来"，不是写字）：改为连续 `play`（顺序游标首尾相接），
>   把分镜 6 的 1s 平摊给 N 个轮廓；判据 = `storyboard_declares_six_shots_with_declared_timing`
>   断言"≥2 段且严格首尾相接、合计 1s" + 运行期断言"t=4s 时仍有 end==0 的轮廓"
>   （并行动作时该断言必然失败，因为那时所有轮廓都是 0.5）。
>   **书写方向显式定义**：轮廓按包围盒 `min_x` 升序排期（`order_by_reading_order`，
>   **稳定排序**、无包围盒者排最后）—— 引擎输出顺序**不是** API 承诺，不能拿它当书写顺序。
>   排序放在"编译产物 → spawn"之间（`compile_to_stroke_paths` 是通用工具，不知道书写语义）。
>   **局限**：单键 `min_x` ⇒ 单行公式正确；多行 / 上下标需换键 `(min_y, min_x)` 或行聚类，本轮不做。

**W 线（补词汇前收尾：W1–W4）**

> - **W1 渲染入口解耦 + headless 入口**：新增 `standard_backends` 模块 ——
>   `StandardBackends::new(&RenderContext)`（**零 egui 依赖**，三个 `new` 只吃
>   device/queue/format，已核查）+ `install_into(&mut CallbackResources)`（egui 宿主）。
>   `install_standard_backends(rs)` **对外签名不变**，内部改为这两步（插入顺序不变）。
>   关键抽象是 `BackendStore`（取用口的两个实现：TypeMap / 自持有）——
>   **帧体只有一份**（`SceneCompositorCallback::prepare_frame` / `paint_frame`），
>   故"两条宿主路径逐位一致"是结构保证而非巧合；`scene_callback` 也随之**不再整体**
>   门控在 `egui-wgpu`（字段全是纯数据，它同时是 headless 的"一帧渲染请求"）。
>   `RenderContext::read_texture` 提升为公开 API（三份测试副本消除）。
>   证据：`headless_path_matches_egui_path_bit_for_bit`（内容层 + 2D + 3D 三层齐全、
>   空链与非空链各跑一遍，两条路径的目标纹理**逐字节相同**）。
>   **导出未做**（记账）：只到"抓一帧 → RGBA8"；且无 seek ⇒ 只能实时录制。
> - **W2 依赖图与文档修正**：8 条**漏画的** mermaid 边补齐（`zela_render → zela_schedule`
>   / `zela_render → ze_scene_contract` / `zela_render_backends → ze_object` /
>   `zela_scene → ze_render_ir` / `zela_scene → ze_scene_contract` /
>   `ze_object → ze_scene_contract` / `ze_object → ze_macro` / `zela_app → ze_object`），
>   并**程序化比对**（解析每个 `[dependencies]`，dev-deps 不计入）：mermaid **59 条 ⇔
>   Cargo.toml 59 条，零缺零余**。顺带扫出两处 FanIn/FanOut 漂移（`zela-render` 8→9、
>   `zela-schedule` 3→4，同源于漏边）与两处 README 自相矛盾（`ze-vertex` 的
>   stride 20 / `zela-render-backends` 的"预建 9 条管线"）——均已修。
> - **W3 节点编辑器排期记账**：见上文「已知未接线项」新增行 —— **已排期（当前冻结）**、
>   触发条件是**UI 接口与交互层稳定之后**、**预告破坏性变更**；并记一条约束：
>   任何跨层接口设计**不得**以"将来节点编辑器要用"为理由。
> - **W4 `zela-scene` 的 `glam::` 收敛**：43 处 → **0 处代码引用**（改用 `ze_notion`
>   或场景层出口 `crate::types`），仅剩 **2 行说明性注释**（`types.rs` 历史沿革 /
>   `transform.rs` 的"Y 翻转只有一份"约定）—— 按纪律**不为凑 grep 删注释**。
>   `types.rs` **未**补 `Mat3/Mat4/Quat` 转发：`transform.rs` 的 11 处只有 Vec2/Vec3，
>   不需要新增类型（不留纸面能力）。
>   2026-10-19 收口后全仓 `*.rs` 中 `glam::` **出现 25 次**
>   （口径：含注释、含 3 个豁免 crate；不含 `.md` 文档提及；不含根 `Cargo.toml`
>   workspace 声明）：`ze-notion` 定义侧 6 + 节点图组 17 + `zela-scene` 注释 2。
>   `zela-scene` 的 `glam` 依赖已在同批从 Cargo.toml 删除。

**N 线（准备阶段二：对象组织与可见性 N1–N6）**

> 解决"对象怎么被组织、怎么被引用、怎么被看见"——补词汇（Transform 通道 / 淡入淡出 /
> 出场退场 / 箭头 / 文字）的地基。四者全部落在**契约层 `ze-scene-contract`**（+ `Opacity`
> 在 `ze-render-ir`），与 `Parent` 同一判据："描述场景中的关系，不是数学概念"。
>
> - **N1 `Name`**：`Name(pub String)`，缺席 = 匿名。查找走
>   `zela_scene_ecs::contract::find_by_name(&impl SceneRead, &str) -> Option<ObjectId>`
>   （`for_each::<Name>` 遍历、返回**第一个**命中）—— **契约层不新增 trait 方法**
>   （加方法要动 4 个实现，成本不匹配收益；几十个对象的 O(n) 足够）。
> - **N2 `Group` / `GroupId`**：纯标签（**只含标识、不含变换**，这是与 `Parent` 的根本
>   区别 —— 绕开"父子变换累乘"这个未接线项）。`GroupId(u32)` 而非 `ObjectId`：
>   不建组实体 ⇒ 无生命周期、不需要 generation 失效检测。不做嵌套组。
> - **N3 `Visibility` / `VisibilityTag` / `GroupVisibility`**：可见性是**关系而非布尔**
>   （同一对象在不同相机下可不同）。一个组件含**非空 tag 集合**（而非挂多个组件 ——
>   多组件的并/交语义没有自然答案），**任一命中即可见（并，非交）**；空集与缺席**同口径**
>   （= 全局可见，避免"挂了组件反而隐形"）。tag 只认 `Global` / `Camera(ObjectId)` /
>   `Group(GroupId)`（复用相机 `ObjectId`，不另建标识体系）。`GroupVisibility` 是
>   **组级开关的另一半**（集中一处，不做 N 次逐对象写入）、注册为 app 资源。
>   **与图层总闸的优先级**：`UiConfig::show_2d` / `show_3d` 是**图层级总闸**（关掉整层
>   不画、不看对象 tag），**总闸优先**；总闸开后才轮到对象级 `Visibility`
>   （两处注释各写一句：`build_scene_view` 与 `record_scene_tagged`）。
> - **N4 `Opacity`**：`Opacity(pub f32)` 在 `ze-render-ir`（样式侧），缺席 = 1.0，
>   **显式 `impl Default` = `FULL`**（不 derive，以免得到 `0.0` 全透明这种危险缺省）。
>   通道 = 组件 + target `OpacityValue`（`Scalar → Opacity.0`）+ 目标类别变体
>   （**T0-② 已删除**，照 `Camera2DFraming` 的"单 target 按变体分派"口径）。
>   ⚠️ **预乘 alpha 硬约束（实测纠正计划初稿）**：`Style2D` 存**直色**，预乘发生在 GPU 侧
>   （`uniform_bytes` 写 `rgb * a`）⇒ **施加不透明度 = 只缩直色的 alpha**（`rgb` 不动）；
>   计划 §6.2.4 写的"`rgb` 与 `a` 同乘"对直色是**错的** —— 同乘会被 GPU 再乘一次 `a`，
>   预乘后 `rgb` 变成 `o²` 档（`o = 0.5` 时应得 `r ≈ a ≈ 128`，那样只得 `r ≈ 64`）。
>   "同乘"法则只适用于**已预乘**的数据（合成器 per-layer opacity）。
>   施加点**唯一**：`zela_render::draw::apply_opacity`（`o == 1.0` 逐位早退 ⇒ 未挂组件画面不变）。
> - **N5 录制过滤接线**：`record_group` 加 `camera_tag: Option<ObjectId>` + `GroupVisibility`，
>   闭包内先判可见性（不可见者连降级都不做）、再降级、最后施加不透明度。
>   新增 `record_scene_tagged(scene, camera, camera_tag, groups, queue)` ——
>   **`record_scene` 的冻结签名不动**（它是"`camera_tag = None`、无隐藏组"的不过滤形态，
>   内部转调新函数 ⇒ 行为逐位不变）。**为什么拆函数**：`record_scene` 只拿得到 `&Camera2D`、
>   **拿不到相机的 `ObjectId`**，而 `VisibilityTag::Camera(..)` 的判定恰恰需要它。
>   **闭包内二次借用 `scene`（计划 §1.2.4 的核查项）实测通过** —— `with` 与 `for_each*`
>   同为 `&self`，闭包首参即 `ObjectId` ⇒ 点查询即可，**契约层不必新增四元组访客**
>   （`for_each_pair_opt` 的三个位置已被 `(形状, 样式, ZOrder)` 占满）。
>   `SceneRecordSystem` / `MorphRecordSystem` / `RevealRecordSystem` 的 `Reads` 各加
>   `GroupVisibility`（可缺席资源 ⇒ 缺席按默认空处理），`camera_tag` 取自 `Camera2DHandle`。
> - **N6 GPU 像素级验收（5 条新用例，全部实测通过）**：`visibility_and_opacity_absent_match_global_full`
>   （`Visibility::global` + `Opacity::FULL` 与两者都缺席**逐位一致**）、
>   `visibility_camera_tag_filters_pixels`（相机 A 有像素 / 相机 B **零覆盖像素**）、
>   `visibility_group_switch_filters_pixels`（组开有 / 组关零）、
>   `opacity_half_premultiplies_and_has_no_dark_halo`（`o = 0.5` ⇒ 预乘后 `r ≈ a ≈ 128`、
>   全图 `|r − a| ≤ 2` 无黑边）、`opacity_zero_yields_no_pixels`（`o = 0` ⇒ 零像素）。
>   这些用例**走完整录制路径**（`record_scene_tagged` 消费真实 ECS 场景 → 队列 → 后端 →
>   回读），为此 `zela-render-backends` 增 `hecs` **dev-dependency**（库本体仍不认识 hecs）。
> - **端到端**：整图注册回归（`full_system_graph_assembles_without_borrow_conflicts`，
>   13 个 System 无冲突）、真实 `cargo run -p zela-app` 跑 2700+ 帧无 panic、
>   清缓存重编后 `cargo check --workspace --all-targets` **零新增 warning**。
> - **记账（本轮明确不做）**：运行时创建 / 销毁对象（`ctx.commands` 仍零消费方 ——
>   一旦引入，`⟲ 重新播放` = `seek(0)` 的"状态由 `t` 唯一决定"前提会坏，**这是引入
>   运行时 spawn 前必须先解决的约束**）；`Parent` 父子变换累乘；嵌套组；按名字的 tag；
>   `Name` 全局唯一性强制；**`SceneRecord3DSystem`（app 层）本轮不做** N5 过滤
>   （其 `Reads = ()` 无 `Camera2DHandle`，且 3D 侧无真实消费方）；
>   `ContentTextureRecordSystem` **不参与过滤**（它是**纹理源**而非显示层，其"相机"是
>   `spec.camera`、不是当前 2D 相机，按相机 tag 过滤没有明确语义）。**不新建 crate**。
> - **一处登记缺口顺带补上**：`Reveal` 此前**未进 `add_known` 白名单**（R 线遗留）——
>   漏登记会让 `spawn_dyn` 静默返回 `None`；本轮与 `Name`/`Group`/`Visibility`/`Opacity`
>   一并 `arm!`。
> - **能力测试 demo**（N 线之后追加）：`zela-app` 增 `capability_demo.rs`，用**最小场景**
>   （两条曲线 + 一个对照组）把四项能力各跑一遍；`--demo` / `ZELA_DEMO=1` 切换
>   （`SceneChoice`）。**它不是作品，是能力检验** —— 每条能力落在一处、GPU 像素判据
>   两两互斥（实测：组全开 `sin`=714 / `cos`=643 / 对照组=0；隐藏 `GROUP_COS` ⇒
>   `cos`=**0** 而 `sin` **仍为 714**；`Opacity = 0` ⇒ `cos`=0 / `sin` 仍 714）。
>   三条实测教训见该模块头注与 `zela-app/README.md`：① 光挂 `Group` **不会**随组开关走
>   （还要 `Visibility::of_group`，两组件正交）；② 两条曲线相交 ⇒ 像素读数互相污染
>   （demo 里竖直错开）；③ `Opacity` 组件缺席时 `OpacityValue` **静默跳过**（动画静默失效）。

待上游提供 workspace 模式后补充 `cargo archtest` 步骤。