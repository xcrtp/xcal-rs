# 缺口总账（GAPS.md）

> **唯一记账入口**。散落在 `FOURIER_GAPS.md`、`WASM_PORT_SURVEY.md` 与各 crate
> README / `ARCHITECTURE.md` 的「仍缺 / 已知未接线」统一汇总到这里。
>
> 更新：2026-10-19（融合前盘点与收尾）；2026-10-20（S1：G-101 关闭 + R-21 记账）。
> `FOURIER_GAPS.md` 保留为 **fourier 作品快照**；`WASM_PORT_SURVEY.md` 保留为
> **当时的探测证据**，两者正文都注明总账在本文件。
>
> **确定性口径**（详见
> [`FRAME_HASH_BASELINES.md`](crates/app/zela-app/scripts/FRAME_HASH_BASELINES.md#L117-L128)）：
> 同目标可复现 ✅（最硬）· 跨端结构一致 ✅ · 跨端逐位相同 ❌（已知可接受）。
> 本文件谈「确定性」时沿用这三层，不混用。

## 状态定义（四类 + 两个补充）

| 状态 | 含义 | 判定要求 |
|---|---|---|
| **可清** | 代码里已经做了，或范围已消失 | 必须给出具体代码位置 |
| **描述不准，需改写** | 事实已变，条目本身误导 | 给出新事实与证据 |
| **仍成立** | 缺口仍存在 | 写清影响层与证据 |
| **升格 / 降格** | 成为前置阻塞（P0）或降为工效项 | 写清理由 |
| 范围外 | 本仓库/浏览器路线明确不做 | 写清替代路线 |
| 待定 | 证据不足或依赖未定决策 | 不得为了清而清 |

---

## P0 — 2D/3D 融合前置

| 编号 | 缺口 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| **G-101** | `WorldTransforms` 只有 2D 仿射；3D 录制只读 local `Transform`，3D 父子层级未接线 | 场景层 → 渲染层 | **升格（原 F-02）→ S1 已关闭** | 关闭于 S1：结构层提交 `84cabeb`（[`transform_hierarchy.rs`](crates/scene/zela-scene/src/transform_hierarchy.rs#L39-L65) 改存 `Transform3D` + `get2d` 投影）、消费方接线提交 `3ed493f`（[`record_systems.rs`](crates/render/zela-render/src/record_systems.rs) 3D 录制 `Reads = (WorldTransforms,)`）；像素对拍 fourier / storyboard 与基线逐位一致（冻结态通过） |
| **G-101 补证（S0-1，2026-10-20）** | S1 零扰动已实证：h_after（HEAD `ba75687`，S1 后）== h_before（`ff2751d`，S1 前）== 同一对哈希 fourier `b6eaa864933847ff…` / storyboard `6de42d6b15943c…`（均用当前 ZELA 口径重跑）。注：计划原定 h_before 取 `84cabeb^`，但该中间提交 `3ed493f` 不可编译（record_systems 已接线而 draw.rs 仍传 Mat3），故取真正可编译的 S1 前 `ff2751d`（`3ed493f^`）—— 以代码为准，见执行报告 §六.4 |

> **消息误标注明（S0-4）**：`3ed493f` 的提交消息写「WorldTransforms 换型为 3D（结构层）」，实际内容只是 `record_systems.rs` 的消费方接线（结构层本体在 `84cabeb`）。该提交已在 `origin/feat/2d3d-fusion` 上（共享分支），故不改写历史，仅在此记账。

> **为什么是 P0**：只要融合要求 2D 与 3D 对象进入**同一父子层级**（或 3D 对象跟随
> 父对象变换），当前 3D 录制不消费 `WorldTransforms` 就直接失效。若融合只做
> 「同屏按层叠加」，则现有层合成已够用 —— 这一点需要融合形态确认，但
> **3D 层级本身是确定性缺口**，不依赖融合也存在。

**待定（不作为 P0）**：跨 2D/3D 的统一相机 / 深度合成模型。当前 2D 与 3D
各自持有相机、按 `SceneView::layers` 顺序合成（无跨层深度）。是否需要统一，
取决于融合是「同屏叠加」还是「同一深度空间互遮挡」—— 后者才构成前置。

---

## A. 作品快照（原 `FOURIER_GAPS.md` 14 条）

| 编号 | 一句话描述 | 影响层 | 状态 | 证据 / 说明 |
|---|---|---|---|---|
| F-01 | 脚本 `Curve.Keyframes` 只有标量 | 脚本层 | **描述不准，已改写** | 内核 [`AnimValue`](crates/kernel/ze-time/src/curve.rs#L16-L23) 已支持 `Scalar/Vec2/Vec3/Color/Path`，`Curve::Keyframes` 也用 `AnimValue`；缺口只在脚本入口 [`pod/types.rs`](crates/scene/zela-scene/src/pod/types.rs#L259-L275) 的 `Vec<ScalarKey>` |
| F-02 | `WorldTransforms` 只有 2D 仿射 | 场景/渲染 | **升格 P0（G-101）** | 同上 |
| F-03 | `MorphRecordSystem` / `ContentTextureRecordSystem` 不消费世界变换 | 渲染 | 仍成立 | [`record_systems.rs`](crates/render/zela-render/src/record_systems.rs#L133-L175)、[`L249-L272`](crates/render/zela-render/src/record_systems.rs#L249-L272) 的 `Reads` 均无 `WorldTransforms` |
| F-04 | `Opacity` 不沿 `Parent` 传播 | 渲染 | 仍成立 | [`draw.rs`](crates/render/zela-render/src/draw.rs#L482-L487) 只读对象自身组件；无祖先链查询 |
| F-05 | 父子环路只写 `TransformHierarchyError` 资源 | 场景 | 仍成立 | 生产侧仅 [`zela-playback`](crates/app/zela-playback/src/lib.rs#L196-L198) 插入默认值；无宿主升级为帧错误 |
| F-06 | `Reveal` 世界变换在 CPU 侧逐帧折算路径 | 渲染性能 | 仍成立 | [`record_systems.rs`](crates/render/zela-render/src/record_systems.rs#L216-L235) 每帧 `path.transformed(...)`（克隆 + 变换） |
| F-07 | `clip_transform` 是独立函数，不是 `Target.Transform` 形态 | 脚本 API | **可清** | 能力已由 [`pod/api.rs`](crates/scene/zela-scene/src/pod/api.rs#L79-L83) 提供；计划中的枚举形态从未落地，语义等价且可并行写不同通道 |
| F-08 | Text 缺颜色 / 对齐 / 尺寸回读 | 内容/脚本 | 仍成立 | [`Text`](crates/scene/zela-scene/src/pod/types.rs#L227-L232) 只有 `source/font_size/x/y`；[`ContentApi`](crates/scene/zela-scene/src/pod/api.rs#L105-L111) 返回 `PathList`，无排版度量回读 |
| F-09 | `Axes2D` 无刻度标签 | 对象层 | 仍成立 | [`axes.rs`](crates/object/ze-object/src/axes.rs#L200-L221) 只产 `x_ticks/y_ticks/grid_lines/axis_lines`，无文字 |
| F-10 | 无 ZOrder 脚本入口 | 脚本层 | 仍成立 | `ZOrder` 组件存在（[`ze-object`](crates/object/ze-object/src)），[`SceneApi`](crates/scene/zela-scene/src/pod/api.rs#L27-L93) 无构造器 |
| F-11 | 无 `fit_bounds` 脚本入口 | 脚本层 | 仍成立 | [`Camera2D::fit_bounds`](crates/object/ze-object/src/camera.rs#L117) 已存在；脚本 API 无对应函数 |
| F-12 | Python `from` 关键字迫使位置参数 | 语言适配器 | 仍成立 | 脚本内注明：[`fourier.py`](crates/app/zela-app/scripts/fourier.py#L18) |
| F-13 | 无虚线 / 线帽 / 渐变 | 渲染 IR | 仍成立 | [`Style2D`](crates/kernel/ze-render-ir/src/shape.rs#L181-L194) 只有 fill/stroke/width/space |
| F-14 | `Parent` 只接受脚本句柄 | 脚本层 | 仍成立 | [`Parent`](crates/scene/zela-scene/src/pod/types.rs#L75-L80) 为 `Handle`；无按名字/路径重挂 API |

**A 段小计**：可清 1 · 描述不准 1 · 升格 P0 1 · 仍成立 11。

---

## B. WASM 移植探测（`WASM_PORT_SURVEY.md`）

| 编号 | 原项 | 状态 | 证据 / 说明 |
|---|---|---|---|
| W-01 | P1-1 字体按路径读盘 | **可清** | 浏览器只回放 ZSB；字形在 native 已烘焙为路径，`zela-web` 无字体代码 |
| W-02 | P1-2 同步 GPU 回读 | **可清** | [`read_texture_async`](crates/render/zela-render-backends/src/render_context.rs#L171) |
| W-03 | P1-3 上下文创建 `block_on` | **可清** | [`request_adapter_and_device`](crates/render/zela-render-backends/src/render_context.rs#L63)；`blocking` feature 隔离 |
| W-04 | P1-4 导出 fs / ffmpeg | 范围外 | 浏览器不导出；ZSB 路线绕开 |
| W-05 | P1-5 `zela-gui` 关不掉下层默认 features | **降格** | `zela-gui` 不进浏览器 ⇒ 与浏览器无关，仅工效项 |
| W-06 | P2-1 `gpu_test` 的 libc / sleep / spawn | **可清** | 归入 `blocking` feature；wasm 构建不编译 |
| W-07 | P2-2 `Receipt::wait` 忙等 | **可清** | [`DriverPod::submit`](crates/scene/zela-scene/src/driver.rs#L200-L207) 同步跑完 `run_frames` 后 `complete()`，`yield_now` 走不到 |
| W-08 | P2-3 CI wasm gate 未覆盖 | **可清** | [`.github/workflows/architecture.yml`](.github/workflows/architecture.yml) 的 Playback runtime gate |
| W-09 | P2-4 `ze-pod-py` wasm | 范围外 | 浏览器不启用 Python Pod |

**B 段小计**：可清 6 · 降格 1 · 范围外 2。

---

## C. README / ARCHITECTURE 散落项

> 盘点结果：真正带「仍缺 / 已知未接线」正文的 README 是 **6 个**
> （`ze-time`、`ze-scene-contract`、`zela-render`、`ze-pod-lua`、`ze-pod-py`），
> 其余 README 没有缺口小节；`ARCHITECTURE.md` 另有一张「已知未接线项」表。

| 编号 | 缺口 | 来源 | 状态 | 证据 / 说明 |
|---|---|---|---|---|
| R-01 | `Curve::PathMorph` 已就绪未接线 | `ze-time` README | 仍成立 | 只有定义与单测（[`curve.rs`](crates/kernel/ze-time/src/curve.rs#L151)），无 `render/`、`app/` 引用 |
| R-02 | `ObjectId::is_stale` 无生产消费方 | `ze-scene-contract` README | 仍成立 | 仅单测；销毁-重建流程未引入 |
| R-03 | `record_scene` 不消费 `Descriptors` | `ze-scene-contract` README | 仍成立 | 消费方是构造期 `apply`；录制层直接吃 `Style2D` |
| R-04 | `Shape2D` 无 `Text/Image` 变体 | `zela-render` README | 仍成立 | 内容以路径形式进入 `Path/Polyline` |
| R-05 | `Parent` 零生产消费方 | `ze-scene-contract` README / `ARCHITECTURE.md` | **可清（文档已修）** | S5 已接线：[`transform_hierarchy.rs`](crates/scene/zela-scene/src/transform_hierarchy.rs)、[`record_systems.rs`](crates/render/zela-render/src/record_systems.rs#L86) |
| R-06 | `Path3D` 零消费方 | `ARCHITECTURE.md` | 仍成立 | 与 `Path2D` 不同构，接入是破坏性变更 |
| R-07 | `glam::` 未完全收敛到 `ze-notion` | `ARCHITECTURE.md` | 仍成立 | 2026-10-19 实测 **25 次**（口径：`*.rs` 中 `glam::` 出现次数，含注释、含 3 个豁免 crate；不含 `.md` 与根 `Cargo.toml`）；`zela-scene` 代码引用已归零 |
| R-08 | 节点编辑器冻结（已排期） | `ARCHITECTURE.md` | 仍成立（排期项） | 零生产消费方；等 UI 接口稳定 |
| R-09 | 2D / 3D 录制 System 分居 | `ARCHITECTURE.md` | 仍成立 | 现已同住 `zela-render::record_systems`，但仍是两个 System / 两个队列 |
| R-10 | CPU 回放路径不支持真合成 | `ARCHITECTURE.md` | **可清（文档已修）** | 旧 `egui_2d` 已在 S4.1 删除（[`zela-egui` README](crates/ui/zela-egui/README.md#L140-L145)） |
| R-11 | `formula.rs` 归位未定 | `ARCHITECTURE.md` | 仍成立 | 归位目标 crate 有争议 |
| R-12 | `tessellate` 下沉未做 | `ARCHITECTURE.md` | 仍成立 | 目前与 2D 后端同层 |
| R-13 | 非原生 Pod 的 `FunctionX2Y` 适配器 | `ARCHITECTURE.md` | 仍成立 | 脚本只承担装配期，不参与每帧采样 |
| R-14 | 每层多一次全屏 blit | `ARCHITECTURE.md` | 仍成立 | 层数少时可接受 |
| R-15 | 3D 离屏无 MSAA（1x） | `ARCHITECTURE.md` | 仍成立 | `wgpu_3d` / `compositor` 的 `sample_count: 1` |
| R-16 | `opacity × blend` 未穷举验证 | `ARCHITECTURE.md` | 仍成立 | 测试覆盖缺口 |
| R-17 | 3D 无深度缓冲 | `ARCHITECTURE.md` | **可清（文档已修）** | [`DEPTH_FORMAT = Depth32Float`](crates/render/zela-render-backends/src/wgpu_3d.rs#L98) 且两个 pass 都挂深度附件 |
| R-18 | `ze-pod-lua` / `ze-pod-py` wasm 未做 | 两个 README | 范围外 | 浏览器只回放 ZSB，不运行脚本；不进 wasm gate |
| R-19 | 节点图组直接依赖 glam（数学入口第二套）：`zela-node-editor`(10) / `zela-node-egui`(6) / `zela-node-egui-ui`(1) | 节点图组 | 仍成立（不违规，记账） | 三者不依赖 `ze-notion`，按 M0 不违规；CI 已显式豁免并注明理由。**触发条件：合成图（ComposerGraph）接入场景前，必须先统一到 `ze-notion`**；此前不改代码（当前零消费方，避免为未发生的需求付代价） |
| R-20 | `Shape2D::Custom` 在结构指纹 topo 中统一输出 `"custom"`，不同 `SpecId` 的自定义图元在结构层不可区分 | 结构对拍 | 仍成立（记账，不修代码） | 现状可接受：spec 是具名字符串（非注册序号），且当前仓库**没有 Custom 的生产者**（只有测试），不影响对拍。**触发条件：Custom 出现真实生产者时，须把 spec 名写进 topo** |
| R-21 | `Transform3D::to_affine2d` 是**投影**：`z` 平移与 xy↔z 耦合被丢弃（D4） | 理念层 `ze-notion` | 仍成立（记账，行为如此） | [`transform.rs`](crates/kernel/ze-notion/src/transform.rs#L115-L134) doc 已写明"投影，非同构"；2D 对象挂 3D 父下是 S6 的目标形态，本轮不报错也不兜底。**消亡条件：S6 之后复核**（2D 进 3D 后该投影是否还有消费方） |

**C 段小计**：可清 3（含 2 条纯文档漂移 + `Parent`）· 仍成立 16 · 排期 1（计入仍成立）·
范围外 1。

---

## D. B1 批次记账（光照数据化与 PBR，2026-10-21）

> 计划《S1收尾与B1_光照数据化与PBR执行计划.md》B1-A~F 已全部落地（逐笔提交）。
> 本段：**已关闭**（硬编码 / 静默兜底类缺口）与**新增能力**。像素为演进态
> （`baselines.txt` 的 reason + 提交号见 §统计后的维护约定与录制段）。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| B-01 | `cube.wgsl` 硬编码光源（`LIGHT_DIR` / `AMBIENT`）→ 场景级 light uniform buffer 驱动 | 着色器 | **已关闭（B1-A）** | 提交 `5743460`；`Light` 组件不带 position（单一真源，位置走 `Transform` → `WorldTransforms`） |
| B-02 | 点光缺失：新增 Point light（`range` 窗口衰减避免 1/d² 爆炸）；数量超编译期上限 ⇒ **显式报错** | 渲染 | 新增能力（B1-B） | 提交 `8efc515`；位置走 `Transform` → `WorldTransforms`（S1 已通） |
| B-03 | PBR 缺失：新增 `ShaderVariant::Pbr`（metallic-roughness，线性空间）；法线贴图走**屏幕空间 TBN**（Mikkelsen） | 渲染 | 新增能力（B1-C） | 提交 `1970c2d`；铁律 4 未改顶点格式（无 tangent 属性槽） |
| B-04 | 脚本层无 3D 入口（3D 场景硬编码在 `authored/demo3d.rs`）→ `camera3d` / `cube` / `parametric_surface` / `material3d` / `light` **参数化构造器** | 脚本 → 场景 | **已关闭（B1-D）** | 提交 `6d4a1c9`；不传顶点数组（`MeshGeometry` 无 `Serialize`，撞"图元可序列化"硬限制）⇒ 3D 首入 `--script == --bundle` 对拍网；demo 入册提交 `6c3376e` |
| B-05 | pipeline `(topology, variant)` **笛卡尔积预建**（3×2=6 条，随 variant 数爆炸）→ `get_or_create_pipeline` **按需构建 + 缓存** | 渲染后端 | **已关闭（B1-E）** | 提交 `797c914`；key 四维定齐 `(topology, variant, cull_mode, blending)` —— B2 只增取值不再动机制；结构判据 `test_b1e_pipelines_built_on_demand`（0→1→1→2→3） |
| B-06 | pipeline 未命中**静默回退 Lambert**（仅 debug 断言）+ 点列表**静默丢弃** | 渲染后端 | **已关闭（B1-E/F）** | `797c914` 结构性删除回退（get_or_create 无"漏建"状态）；`7a9299f` 点列表改 `panic!` 显式报错（不静默铁律 2），`test_b1f_points_topology_panics_explicitly` |

**D 段小计**：已关闭 4 · 新增能力 2（不计入缺口类别）。

> **澄清（B2-0e）**：B1 验收里「像素未变」仅指 **2D 基线集**（`fourier` / `storyboard` /
> `smoke` 均无 3D 内容）；3D 回归由 **23 条 GPU 像素单测**保障，**3D 尚未进
> frame hash 对拍网**（`baselines.txt` 无 3D 条目）。3D 的逐位对拍在 S6/后续批次
> 接入基线窗口前，缺口保持可见。

---

## E. B2 批次记账（管线状态 / 队列统一 / 2D 进 3D，2026-10-21）

> 计划《B2执行计划_管线状态队列统一与2D进3D.md》B2-0 ~ B2-5 已落地（逐笔提交）。
> 本段记录**已关闭项**与**新增/仍成立的缺口**；B2-6 ~ B2-9 完成后继续追加。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| E-01 | `PipelineKey` 含 wgpu 类型（`Option<wgpu::Face>` / `BlendState`）→ 改为 IR `CullMode` / `LayerBlend`，并补第五维 `VertexLayout`（D15） | 渲染后端 | **已关闭（B2-2）** | `grep -n "struct PipelineKey" -A 8` 零 wgpu 类型；`test_b2_2_pipeline_state_dimensions`（1→2→3 条） |
| E-02 | `cull_mode` 由拓扑推导、`blend` 硬编码 `REPLACE`（#2/#3）→ 意图层 `Material3D.double_sided` / `transparent` 驱动 | 渲染 / 场景组件 | **已关闭（B2-2）** | `test_b2_2_double_sided_visible_from_back`（背面可见） |
| E-03 | 2D / 3D 两个队列、3D 无层序（#11/D2）→ **一个 `DrawQueue`**（2D 形状 + 3D 网格/线段共存）+ 统一 `ZOrder`（IR 唯一声明，`ze_scene_core::ZOrder` re-export） | 渲染管线 | **已关闭（B2-1）** | `test_queue_stable_within_equal_zorder`（R-F 显式锁定）、`test_unified_queue_mixes_2d_and_3d_by_zorder` |
| E-04 | 2D 图元无法参与 3D（融合缺口）→ `derive_mesh`（Rect/Circle/Ellipse/Polygon/Line/**Path**/**Group** → 平面网格，Flat + double_sided 材质默认化） | 渲染管线 | **新增能力（B2-3；B3 Step 1 扩到 7 种）** | `test_b2_3_derived_2d_rect_renders_in_3d`；B3 Step 1 起统一走 `tessellate_with`（lyon），手写三角化已删；`Polyline`/`Custom` 仍显式报错（不静默） |
| E-05 | 透明物体无排序（无透明桶）→ 不透明先画、透明按 `ZOrder` 升序后画；透明 pipeline **不写深度**（R-A） | 渲染后端 | **已关闭（B2-3）** | `test_b2_3_transparent_bucket_orders_by_zorder`（对调 ZOrder 后主导色对调） |
| E-06 | 共面 z-fighting → `ZOrder × Z_ORDER_EPSILON` 的 **normal offset**（不用常量 depth bias，D4） | 渲染后端 | **已关闭（B2-4）** | `test_b2_4_epsilon_from_zorder_resolves_coplanar`；`DepthBiasState::default()` 未改 |
| E-07 | 3D 无 AA（1x）vs 2D 4x（#6/#8）→ 3D MSAA 4x（`DEFAULT_MSAA_SAMPLES`），采样数经 `new_with_samples` 由能力侧给定 | 渲染后端 | **已关闭（B2-5）** | `test_b2_5_msaa_4x_produces_partial_coverage`（4x 部分覆盖像素更多） |
| E-08 | **纹理走样**：MSAA 治不了纹理采样走样（`Material3D.texture` / `ContentTexture` 进 3D 后会有） | 渲染后端 | **仍成立（已知缺口）** | 需要 **mipmap + 各向异性过滤**；本轮不做（计划 B2-5 记账要求）。B3 Step 4b 起 `SurfaceTexture` 显式声明 `filter` / `mipmap`（Fixed `texel_size`），但**mipmap 生成未实现** ⇒ 2D 内容进 3D 同样受本条约束 |
| E-09 | 硬件 MSAA **resolve 的跨端确定性**是否为 `[browser]` 段"1 帧 1 像素"差异的贡献者 | 对拍 | **待验证（不是断言）** | 本轮无浏览器环境（D5：跨端重录推迟）；`[browser]` 段仍仅存档 |
| E-10 | 2D 派生网格仅覆盖可直接三角化的图元（`Path` 需要 lyon；`Group` 需要递归） | 渲染管线 | **已关闭（B3 Step 1）** | 复用 `tessellate_with`：7 种图元可派生；剩余 `Polyline` / `Custom` 仍 `ShapeMeshError::Unsupported` 显式报错 |
| E-11 | 双槽基线**没有差分视图**（两个孤立值，无法看出"当前值落在哪一槽"） | 对拍工具 | **已关闭（B2-6）** | xtask `render_diff_report`（诊断旁路，不参与门禁）；`diff_report_tests` 3 条 |
| E-12 | 帧级归因缺**分层标签**（无法定位首帧分歧） | 对拍 / 可观测性 | **已关闭（B2-6）** | `ZELA_FRAME_DIGEST_SAMPLE=N` 逐帧 digest 采样；**不进序列哈希链**（D7） |
| E-13 | 脚本看不到 B2 的意图层（`double_sided` / `transparent`） | 脚本 API | **已关闭（B2-7）** | pod `Material3D` 增两字段（D19：不暴露 GPU 机制）；`zela.scene` version 5 → **6** |
| E-14 | 新能力缺展示脚本 | Demo | **已关闭（B2-7）** | `fusion.lua`（双面 + 透明相交体）入册 `demo/manifest.txt`，payload-hash `-` |
| E-15 | 3D **未进 frame hash 对拍网**（B1 遗留） | 对拍 | **已关闭（B2-8）** | `demo3d` 升级为 fixture（Q4 决议）；hashes 覆盖 fourier / storyboard / demo3d；`[browser]` 仍仅存档 |
| E-16 | 能力 API 改了可能**忘记递增 version** | 脚本 API | **已关闭（B2-9）** | `capability_fingerprint`（FNV-1a，跨 Rust 稳定）+ 钉住常量；`cargo verify --step structure` 内跑护栏（变而未递增 ⇒ **fail**） |
| E-17 | 逐帧 digest 旁路**仅 native**（`zela-render` 二进制）；浏览器路径未接 | 可观测性 | **仍成立（范围限制）** | wasm 无环境变量；浏览器归因留待跨端重录（D5） |

**E 段小计（B2-0 ~ B2-9）**：已关闭 **13**（含 E-10，B3 Step 1）· 新增能力 1 · 仍成立/待验证 **3**（E-08 mipmap、E-09 跨端 resolve、E-17 旁路仅 native）。

---

## F. B3 批次记账（顶点生产与后端解耦，2026-10-21）

> 计划《B3-0执行计划_顶点生产与后端解耦.md》：纯结构调整，**像素冻结**。
> 目标是把"生产 + 打包"从 backend 移到渲染无关层，使组合从 N×M 降为 N+M。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| F-01 | 3D 顶点生产（`interleaved_pos_uv_normal` / `fallback_uvs` / line 内联）**全在后端私有函数** | 渲染后端 | **已关闭（B3-0b）** | 迁至 `zela-render::vertex_record`；`grep fn interleaved_pos_uv_normal\|fn fallback_uvs crates/render/zela-render-backends/` = 0；line 走 `line_vertex_buffer`（B7） |
| F-02 | 2D 三角化调用与顶点打包在后端（`tessellate` + `bytes_of(&vertices)`） | 渲染后端 | **已关闭（B3-0c）** | `tessellate_to_buffers` / `morph_endpoint_buffers` / `tessellate_reveal_to_buffer`；后端只拼接字节（`CachedGeometry { data: VertexBufferData }`） |
| F-03 | 三角化失败 `eprintln + continue`（静默跳过） | 渲染后端 | **已关闭（B3-0d）** | 改显式 panic；`gpu_unregistered_custom_shape_panics_explicitly`（`should_panic`） |
| F-04 | 缓存结构 2D 两级 / 3D 一级**未统一** | 渲染后端 | **仍成立（B4：本批不动）** | 2D 仍 `geom_cache → batch_cache`；3D 仍一级 `CachedMeshBucket` |
| F-05 | 顶点生产**未提前**到场景录制（2D 容差依赖相机 scale） | 渲染管线 | **仍成立（有意）** | 生产仍在渲染阶段执行，只是挪出 backend crate（计划 §一 的明确约束） |

**F 段小计（B3-0）**：已关闭 3 · 仍成立 2（均为计划明确范围外/有意保留）。

---

## G. B3-0.5 批次记账（Backend 抽象与 Pod 化，2026-10-21）

> 计划《B3-0.5执行计划_Backend抽象与Pod化.md》：把"跨后端契约"从 wgpu 类型里剥出来，
> 并纠正像素口径（**同后端 + 同机器 = 测试夹具**，不再承诺跨端逐像素一致）。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| G-01 | `PodValue` 无法承载 bulk 字节（只能 `Array(U8)`，与 `Vec<u8>` 语义撞车） | Pod 层 | **已关闭（B3-0.5a）** | `PodBytes` newtype + `PodValue::Bytes`；`describe()` / `mismatch()` 补分支；`vec_u8_still_uses_array` 防回归；脚本绑定层显式报错（不静默） |
| G-02 | 渲染目标描述（尺寸 / 格式 / 采样数）留在后端，IR 侧无契约 | 渲染 IR | **已关闭（B3-0.5b）** | `ze_render_ir::target::RenderTargetDesc`（serde + Hash + `validate()`）；`ColorFormat` 补 Bgra 两型；旧 `RenderTarget` 仍在 backend 持 GPU 句柄 |
| G-03 | 四个 render capability（2d / 3d / mixed / shapes）没有签名集与指纹 | 渲染 IR | **已关闭（B3-0.5c）** | `ze_render_ir::caps`：`SIGNATURES` / `signatures_of` / `fingerprints` / `PINNED_FINGERPRINTS` / `negotiate`；`structure` 步跑 `render_fingerprints_are_pinned` |
| G-04 | 后端"实现了哪些 capability"没有显式声明，部分实现会静默失败 | 渲染后端 | **已关闭（B3-0.5d）** | `zela_render_backends::render_backend::{IMPLEMENTED_NAMESPACES, implemented_capabilities, negotiate_capabilities}`；`negotiation_fails_explicitly_for_shapes` |
| G-05 | `zela-render` 带 `feature = "wgpu"`，GPU 侧模块与渲染无关层混居 | 分层 | **已关闭（B3-0.5e）** | 四个模块迁入 backends；`default = []` 且无 wgpu 依赖；`cargo tree -p zela-render \| grep -c wgpu`：**12 → 0** |
| G-06 | `zela.render.shapes` **wgpu 未实现**（需要直接吃 `Path2D` 控制点的解析抗锯齿 / 矢量化渲染） | 渲染后端 | **仍成立（范围限制）** | `implemented_capabilities().shapes == false`；请求它 ⇒ 显式 `CapabilityNegotiationError`（不假装实现） |
| G-07 | `Capability` trait 的 `from_negotiated` 与 **typed 快路径**（`Pod::resolve()`）未落地；当前契约是签名集 + 后端自述集合，跨边界调用仍走 typed trait | Pod / 后端 | **仍成立（D4.4 后续）** | 本轮只做契约与协商；`PodBytes` 是契约层表示，typed 快路径是可选优化（不改变语义） |
| G-08 | `Compositor` 未 pod 化、`COMPOSITE_QUAD` 未外迁 | 渲染后端 | **明确不做（§九）** | 随 Compositor 单独批次处理 |
| G-09 | `FRAME_HASH_BASELINES.md` 旧标题/首段把像素一致性写成了跨运行时承诺 | 文档 | **已纠正（B3-0.5e）** | 标题改「同后端 + 同机器」；写明像素 = 测试夹具、跨端不追求一致、`[browser]` 仅存档；`baselines.txt` 注释同步 |

**G 段小计（B3-0.5）**：已关闭 6 · 仍成立 2（G-06 / G-07）· 明确不做 1（G-08）。

---

## H. B3-1 批次记账（场景层分层重构，2026-10-21）

> 计划《B3-1执行计划_场景层分层重构.md》：把 12k 行的 `zela-scene` 分成
> **概念（core）/ 实现（ecs）/ 能力（api）**，作品移出。**纯结构搬迁，像素必须不变**。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| H-01 | `pod/types.rs` / `scene_service.rs` 反向依赖 ECS 内部（`crate::animation` / `crate::scene::components` / `crate::scene_script`） | 场景层 | **已关闭（B3-1a/b）** | `grep "^use crate::" ze-scene-api/src` = 0（仅剩 crate 内 `crate::types` / `crate::content_service`） |
| H-02 | `ze-object` 与 `zela-scene` 组件/纯逻辑混居 | 分层 | **已关闭（B3-1b/f）** | `ze-scene-core::object` 承接全部对象；`ze-object` crate 已删除（`cargo tree` 中无该包） |
| H-03 | 动画目标 `Target::apply(&mut WorldScene)` 把 hecs 适配器钉进"数据层" | 核心层 | **已关闭（B3-1b）** | 新增 `ze_scene_core::scene_write::{SceneWrite, with_component}`（object-safe 擦除写）；ECS 用组件登记表实现分发 |
| H-04 | 组件登记表 / persist / math_pod 写在 ECS 实现里，脚本数据（core）却要依赖它 | 核心层 | **已关闭（B3-1b）** | `registry` / `persist` / `math_pod` 下沉 core；ECS 的 `add_known` 与 `SceneWrite` 共用导出的 `known_component_table!` |
| H-05 | `pod/` 与实现同 crate，语言边界不独立 | 场景层 | **已关闭（B3-1c）** | 新 crate `ze-scene-api`；`cargo tree` 中 `zela-` / `hecs` = **0**；能力指纹未变 |
| H-06 | `zela-scene` 自称"契约实现"却有 11k 行 | 场景层 | **已关闭（B3-1d）** | 实现改名 `zela-scene-ecs`；`zela-scene` 变壳层（re-export） |
| H-07 | `authored/`（分镜 / demo）住在场景层 | 场景层 | **已关闭（B3-1e）** | 独立 `zela-authored`；`zela-scene-ecs` 内无 `authored` |
| H-08 | `ze-scene-core` / `ze-scene-api` 的"不能依赖实现"只是约定 | CI | **已关闭（B3-1f）** | `structure` 步新增 jq 判据（`JQ_SCENE_LAYERS`） |
| H-09 | `Target` 的 `apply` 签名变化（`&mut dyn SceneWrite`）会波及下游实现者 | 公开 API | **仍成立（有意）** | 仓内只有 `authored` 测试实现；这是分层必要的破坏性变更，未留兼容 shim |
| H-10 | `scene_script.rs` 的 `commit` 用例需要真实 hecs 场景 | 测试布局 | **已关闭（B3-1b）** | 用例迁至 `zela-scene-ecs/src/scene_script_tests.rs`（实现与用例分离，语义不变） |
| H-11 | `zela-scene` 壳层与 `ze-object` 壳层的删除时机 | 兼容层 | **已关闭（B3-1 收尾）** | `ze-object` 与 `zela-scene` 壳层均已删除；237 处旧 `zela_scene` 路径改为 `zela_scene_ecs::`；ze-macro 派生代码指向 `ze_scene_core::transform`（不再需要任何 `zela_scene` 路径） |
| H-12 | `WGPUDevice` / `CPUDevice` 仍在组件层（D7） | 场景核心 | **已关闭（B3-1 收尾）** | 全仓零消费方 ⇒ 删除 `components::marker` 整块；`zela-render::components` 的转发同步移除 |
| H-13 | `zela-render` 生产依赖场景 **ECS 实现**（`zela-scene-ecs`，45 处引用；含 `WorldView` / `WorldTransforms`） | 分层 | **已关闭（B3-1 收尾）** | `WorldTransforms` 下沉 core；新增 `ze-scene-hecs`（`SceneRead` 只读适配器 + `ObjectId↔Entity`）；`cargo tree -p zela-render \| grep zela-scene-ecs` = **0** |

**H 段小计（B3-1）**：已关闭 12 · 仍成立 1（H-09）。

---

## H-bis. B3-P 批次记账（JS Pod，2026-10-21）

> 计划《B3-P执行计划_JS_Pod.md》：新增 `ze-pod-js`（boa，纯 Rust），
> 与 Lua / Python 同形地复用 `ze-pod-binding`；**只接装配期**，不碰回放。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| P-01 | `host-js`（浏览器原生引擎）**未接线** | JS 适配器 | **明确不做（B3-Pe 可选）** | `ze-pod-js/src/lib.rs` 两条 `compile_error!`：非 wasm 启用 ⇒ 编译期报错（J3）；wasm 启用 ⇒ 显式"尚未接线"（不静默降级） |
| P-02 | wasm 体积尚未基线化 | 构建 | 仍成立 | `js` feature 默认不开；`zela-web` 未引用 `ze-pod-js` ⇒ 浏览器产物体积不变（未变即无需记录前后值） |
| P-03 | JS 只有 `number`（f64） | JS 适配器 | **已处理** | `exact_integer()` 显式校验整值 / 范围（i32 / u32 / u8 / u64 各自边界）；u64 上限取 `2^53-1`（精确整数）；越界报错不截断 |
| P-04 | 语言适配器不在 wasm gate 名单 | CI | **已关闭（A1）** | xtask 新增 `OPTIONAL_WASM_CRATES = ["ze-pod-js"]`，`step_wasm` 对其实跑 `cargo check --target wasm32-unknown-unknown`；lua / py 仍不进（mlua 不适配 wasm 内核路径、pyo3 编不到） |
| P-05 | **wasm 运行时对拍未做** | JS 适配器 | 仍成立 | 本机无浏览器宿主：只做了 `cargo check --target wasm32`（编译期）与 native 运行时用例；"同一脚本在浏览器里行为一致"尚未被任何用例覆盖 |

**H-bis 段小计（B3-P）**：已关闭 1（P-04）· 已处理 1（P-03）· 明确不做 1（P-01）· 仍成立 2（P-02 / P-05）。

---

## I. B3-2 批次记账（Render Pod 契约探针，2026-10-21）

> 计划《Pod 线：JS Pod 收尾 + Render Pod 探针》B 组：`zela.render2d` 的四条签名
> （`ze_render_ir/src/caps.rs:24-29`）从未被实现，却已被 `PINNED_FINGERPRINTS`
> 钉住。本段记录**真实实现**（`zela-render-backends/src/render_pod_probe.rs`，
> 仅 `#[cfg(test)]`）得到的结论。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| R-01 | 四条签名从未被实现却被指纹门禁冻结 | 渲染 IR | **已闭合（B3-2）** | `RenderProbePod` 真实现 `capabilities` / `create_target` / `upload` / `draw_2d`；typed 直调与 Pod 边界两条路径**逐字节相同**（`pod_boundary_matches_typed_path`） |
| R-02 | `upload(PodBytes) -> Handle` **每次调用新开槽**；契约无 release / 原地更新 | 渲染 IR | **仍成立（有意保留）** | `upload_allocates_a_new_slot_per_call` 取证：两次 upload 得到不同槽位、旧句柄仍存活；生产接线前必须补 `release(Handle)`（或 `upload_into(Handle, PodBytes)`）。本轮**不改签名**：没有第二个消费方时冻结错误形状的风险高于泄漏风险 |
| R-03 | `RenderTargetDesc.format` / `sample_count` 被 2D 后端忽略 | 渲染 IR / 后端 | **仍成立（探针显式收窄）** | `wgpu_2d.rs:630-651`：离屏固定 `OFFSCREEN_FORMAT` + `MSAA_SAMPLES`。探针对非 `Rgba8Unorm` / 非 4x 的 desc **显式报错**（`target_desc_is_narrowed_explicitly`）；契约侧应改用 `capabilities()` 查询（返回类型目前未定义） |
| R-04 | `draw_2d` 无法表达**视口 / 相机 / 清屏** | 渲染 IR | **仍成立** | `create_target` 只有尺寸 / 格式（无视口 rect、无 pixel ratio）；2D 离屏固定清透明（`wgpu_2d.rs:70-75`），背景归合成器；相机变换只能预烘焙进载荷 ⇒ 探针约定"整目标 = 视口、1pt = 1px、坐标已是屏幕点" |
| R-05 | `Handle` 无法同时表达"目标 + 批次" | 渲染 IR | **仍成立** | `draw_2d(Handle, PodBytes)` 只有一个 Handle；探针把批次句柄编码进 `ProbeFrame { batch_slot, batch_generation }` —— 即 `PodBytes` 缺 `Handle` 的表示规则（只能自定义编码） |
| R-06 | `Handle ↔ GpuHandle` 映射 | 渲染 IR / 资源 | **可行（有前提）** | `ze_pod_core::HandleTable<Slot>` 承载映射（世代防 ABA）；`GpuHandle` 是 `Box<dyn Any>`（`ze-resource/src/render_backend.rs:78-84`，`!Send + !Sync`）⇒ 只能留在 Pod 内部；`Handle.slot` 可与 `GpuResourceId.index()` 对齐，`generation` 对齐 |
| R-07 | `Send + Sync` 是否硬阻塞 | Pod / 后端 | **平台相关：native 不阻塞，wasm 阻塞** | `wgpu_2d.rs:352-362` 有 `#[cfg(not(target_arch = "wasm32"))] unsafe impl Send/Sync for WgpuBackend2D`（S1 徽章，`GpuHandle` 本身是 `Box<dyn Any>` ⇒ `!Send + !Sync`）。native：`ProbeState: Send + Sync` ⇒ `RefCell` 换 `Mutex` 即可登记 `SyncPod`（`probe_state_is_send_and_sync_on_native`）；wasm：徽章不存在 ⇒ `examples/send_probe.rs` 在 `--target wasm32` 下必须失败 |
| R-08 | `DrawCommand` / `Style2D` 无 serde | 渲染 IR | **仍成立（独立决策）** | 探针用自定义可序列化载荷（`ProbeBatch` / `ProbeFrame`）跑通边界 ⇒ serde **不是**"边界可行"的前提；但生产接线需要稳定编码（补 serde 或定义显式二进制格式）。`Shape2D` 已有 serde，补齐的收益集中在 `Style2D` |

### B3-2d 结论（逐条）

1. **`draw_2d` 四条够不够**：**单帧够用，生命周期不够**。四条能跑通"建目标 → 上传 → 画一帧"；
   但缺 `release` / 原地更新（R-02）、缺视口与相机语义（R-04）、缺清屏（R-04）。
   ⇒ **不改签名**（触发条件：出现第一个真消费方且需要 >1 帧或 buffer 更新）。
2. **`upload` 返回 Handle vs 写入已有 Handle**：**返回 Handle 是对的** —— 它与本仓
   "句柄 = 不可变资源标识 + 世代"的模型一致；"写入已有"会要求 Handle 参数 + 可变语义，
   与 `HandleTable::get_mut` 的借用模型冲突。**但必须配套 release**（契约缺，R-02）。
3. **`Handle ↔ GpuHandle` 映射**：**可行**（R-06）。规则：`Handle.slot` ↔ `GpuResourceId.index`、
   `Handle.generation` ↔ `GpuResourceId.generation`；`GpuHandle` 永不出 Pod。
4. **"Send+Sync 是硬阻塞"**：**分平台**（R-07）。`Pod` 自身不要求 `Send + Sync`
   （`PodNode::new` 合法）；native 上 2D 后端带 S1 徽章（`unsafe impl Send/Sync`）
   ⇒ `ProbeState: Send + Sync`，把 `RefCell` 换 `Mutex` 就能登记 `SyncPod`；
   **wasm 上徽章不存在**（wgpu 资源含 `Rc<RefCell<..>>`）⇒ 跨平台统一的 SyncPod 不成立。
5. **`DrawCommand` / `Style2D` 要不要补 serde**：**要，但不是本批**（R-08）。探针证明边界
   可以靠自定义编码跑通；生产接线前需要稳定编码，届时补齐 `Style2D`（`Shape2D` 已有）。

**I 段小计（B3-2）**：已闭合 1（R-01）· 可行 1（R-06）· 成立 1（R-08）· 平台相关 1（R-07）· 仍成立 4（R-02 / R-03 / R-04 / R-05）。
签名**未改** ⇒ `PINNED_FINGERPRINTS` 不变。

---

## J. B3 批次记账（Surface 双路径，2026-10-21）

> 计划《B3执行计划_Surface双路径》。本段记录**实际落地**部分；Step 4 / Step 5 未执行，单列说明。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| S0-01 | `zela-render` 断 ECS 依赖后缺 CI 门禁 | CI | **已关闭（B3 Step 0）** | 新增 `JQ_RENDER_LAYER`；临时加违规依赖验证能捕获后撤销 |
| S0-02 | 四条基线 `@ f2c8778` 是**回填值**，易被误读为"产生时提交" | 对拍 | **已关闭（B3 Step 0）** | `FRAME_HASH_BASELINES.md` 增加回填值警告（跨批次不得采信） |
| S0-03 | `B3-R` / `B4` / `B3-2d` 三名并存 | 文档 | **已关闭（B3 Step 0）** | GAPS 段标题 / 结论标题 / 代码注释统一为 B3-2 / B3-2d；`grep B3-R` = 0 |
| S0-04 | 可选的 hecs 隔离判据（"hecs 只在 `ze-scene-hecs` / `zela-scene-ecs`"） | CI | **不成立（未采纳）** | 实测 `zela-schedule` / `zela-egui` / `zela-app` / `zela-playback` 的生产边仍直接依赖 hecs；判据前提与代码不符 |
| S1-01 | 派生桥手写 ellipse / polygon / line_ribbon（第二份三角化真源） | 渲染管线 | **已关闭（B3 Step 1）** | 统一走 `tessellate_with`（lyon）；7 种图元（含 Path / Group）；三个手写函数已删 |
| S1-02 | 2D 的边法线（attrib0）/ advance（attrib2）在 3D 布局里无对应槽 | 渲染管线 | **仍成立（显式限制）** | 转换时丢弃边法线，着色法线恒 +Z；Direct 对 `Reveal` 显式报错 |
| S1-03 | `Polyline` / `Custom` 仍不可派生 | 渲染管线 | **仍成立（显式报错）** | `ShapeMeshError::Unsupported`；Direct 路径未声明支持，不静默 |
| S1-04 | Direct 单材质：`fill` + `stroke` 同时存在时只派生填充 | 渲染管线 | **已关闭（B3 Step 4a-A）** | `derive_mesh → Vec<DerivedMesh>`：fill / stroke 各一份，各自材质颜色；不再丢描边 |
| S2-01 | Direct 路径的触发条件不能靠内容类型推断 | 渲染管线 | **已关闭（B3 Step 2）** | 只由 `SurfaceRenderer::Renderer3D` 决定（`surface_route.rs`）；无 Surface ⇒ 默认 2D |
| S2-02 | Direct 失败无处表达（`System::run` 无返回值） | 渲染管线 | **已关闭（B3 Step 2）** | `DrawQueue::surface_errors()` 收集显式错误；不影响其它对象录制 |
| S2-03 | Direct + `SurfaceMapping::Screen` 的 3D 语义未定义 | 渲染管线 | **仍成立（显式拒绝）** | `ScreenMappingUnsupported`；不猜映射 |
| S3-01 | 无 Surface 组件 / 归属判定 | 场景核心 | **已关闭（B3 Step 3）** | `Surface`（无 position）+ `SurfaceRenderer`；沿 `Parent` 链解析、visited 环检测；缺 renderer 显式错误 |
| S3-02 | 默认 renderer 策略 | 场景核心 | **未拍（按计划延后）** | 计划要求用 Step 1 实测覆盖度反推；当前 7/9 图元可 Direct，但 Step 4/5 未做，暂不设默认 |
| S4-01 | Texture 路径多实例（2D 离屏 → 纹理 → 代理平面） | 渲染后端 | **未执行（4c/4d）** | 需把 `ContentTextureQueue` 单份泛化为 `HashMap<LayerId, _>` 并改 `SceneCompositorCallback` 遍历 |
| S4-02 | `Renderer2D` 没有可表达的固定纹理契约 | 场景核心 | **已关闭（B3 Step 4b）** | `SurfaceTexture { texel_size, filter, mipmap }` + `camera_for`；缺声明 / 尺寸非法 / 宽高比不一致 ⇒ 显式错误 |
| S5-01 | fourier / storyboard 迁移 + 重录 | 作品 | **未执行** | 依赖 S4-01；且必须先完成 Step 0-2（已做） |

**J 段小计（B3，已执行部分）**：已关闭 9（S0-01/02/03、S1-01、S1-04、S2-01/02、S3-01、S4-02）· 仍成立/显式拒绝 3（S1-02/03、S2-03）· 不成立未采纳 1（S0-04）· 未拍 1（S3-02）· 未执行 2（S4-01 / S5-01）。

---

## 统计与维护约定

| 类别 | 数量 |
|---|---|
| 升格为 P0（融合前置） | 1（F-02 / G-101，**G-101 已关闭**） |
| 可清 | 11（A 1 + B 6 + C 3 + G-101） |
| 描述不准，已改写 | 1（F-01） |
| 降格 | 1（W-05） |
| 仍成立 | 28（A 11 + C 16 + 排期 1） |
| 范围外 | 3（B 2 + C 1） |
| 待定 | 1（统一相机 / 深度合成模型） |

共 **44 条**：A 段 14（作品）+ B 段 9（WASM 探测）+ C 段 21（README/ARCHITECTURE）；G-101 是 F-02 的升格形态，不重复计数。
D 段（B1 批次记账）6 条：已关闭 4 · 新增能力 2 —— 不计入缺口类别（见上表下方 D 段小计）。

维护约定：

- 新增缺口必须先落到本文件，再在 crate README 留一行指针；不得只写 README。
- 「可清」必须附代码位置；无法给出证据的标「待定」，不得为了清而清。
- 每次清理后同步更新 `WASM_PORT_SURVEY.md` 的状态列与本文件统计。
