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

> 计划《B3执行计划_Surface双路径》及后续 Step 4 / Step 5-6 计划。本段记录**实际落地**部分：
> Step 0-4c 已全部落地；Step 5-6 落地为「两个脚本作品 + 对拍注册」。fourier / storyboard
> 迁移纹理路径（S5-01）已**否决**（决策留痕，不是待执行缺口）。

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
| S2-03 | Direct / Texture + `SurfaceMapping::Screen` 的 3D 语义未定义 | 渲染管线 | **仍成立（显式拒绝）** | Direct：`ScreenMappingUnsupported`；Texture：`ScreenTextureUnsupported`（`content_texture_specs` 只接受 `World`）。触发条件：先定义 HUD 近平面 / 代理平面世界位置语义（见 B3 Step 4d，明确"不做"） |
| S3-01 | 无 Surface 组件 / 归属判定 | 场景核心 | **已关闭（B3 Step 3）** | `Surface`（无 position）+ `SurfaceRenderer`；沿 `Parent` 链解析、visited 环检测；缺 renderer 显式错误 |
| S3-02 | 默认 renderer 策略 | 场景核心 | **未拍（按计划延后）** | 计划要求用 Step 1 实测覆盖度反推；当前 7/9 图元可 Direct，但 Step 4/5 未做，暂不设默认 |
| S4-01 | Texture 路径多实例（2D 离屏 → 纹理 → 代理平面） | 渲染后端 | **已关闭（B3 Step 4c-1..4c-5）** | 容器 / id 链 / `fit_margin` / demo3d 迁移 / 双 Surface 互不串扰验证全部落地 |
| S4-02 | `Renderer2D` 没有可表达的固定纹理契约 | 场景核心 | **已关闭（B3 Step 4b）** | `SurfaceTexture { texel_size, filter, mipmap }` + `camera_for`；缺声明 / 尺寸非法 / 宽高比不一致 ⇒ 显式错误 |
| S4-03 | `surface_errors` 只收集、无生产消费方（验收 O-2） | 宿主 / 帧管理 | **已关闭（B3 Step 4c-1）** | `zela_render::frame::end_frame` 非空 ⇒ `panic!`；egui / playback / web 三个宿主共用该帧末路径 |
| S4-04 | `ContentTextureSpec` / `ContentTextureQueue` 单份；`LayerId` 用图层位置分配 | 渲染 IR / 帧管理 | **已关闭（B3 Step 4c-2）** | 容器按 `LayerId` 索引；`ObjectId → LayerId → TextureHandle` 单一映射（`content_layer_id` / `content_texture_handle`）；视口图层用固定 id；`grep "LayerId(view.layers.len()" = 0` |
| S4-05 | Surface 纹理相机只能 1:1，无法表达 demo3d 的 0.8 留边 | 场景核心 | **已关闭（B3 Step 4c-3）** | `SurfaceTexture.fit_margin`；`Some(m)` 直接调用 `Camera2D::fit_bounds`（逐位相同）；非法 margin ⇒ 显式错误 |
| S4-06 | demo3d 走硬编码 `CONTENT_TEXTURE_ID` / 手算内容相机（与 Surface 路径**两套真源**） | 作品 / 装配 | **已关闭（B3 Step 4c-4）** | demo3d 改为 `Surface(Renderer2D) + SurfaceTexture(fit_margin=0.8)`，内容实体挂 `Parent(surface)`；材质纹理由 `content_texture_handle` 派生；**四基线逐位不变（零差异）** |
| S4-07 | 多 Surface 只是"签名存在"，无验证 | 测试 | **已关闭（B3 Step 4c-5）** | `two_surfaces_record_into_independent_layers`：两个 Surface 各自一层、纹理句柄不同、内容跨度互不串扰 |
| S4-08 | `SurfaceTexture.filter` / `mipmap` 已进契约，但后端采样仍用全局默认 | 渲染后端 | **仍成立（未接线）** | 为保持 demo3d 零差异，4c 未改后端 sampler；mipmap 生成本就未实现（E-08）。触发条件：出现对过滤敏感的 Surface 内容 / 引入 mipmap 生成 |
| S4-09 | 内容层录制只支持 `(Path, StrokeSolid)`，其它组合走 Surface 会静默丢内容 | 渲染管线 | **已关闭（B3 Step 5-1）** | 新增 `record_scene_for_surface`：复用 9 组全组合 + Parent 归属过滤；`ContentTextureItem` 标记**不再必需**（组件保留登记以兼容旧数据）；demo3d 零差异 |
| S4-10 | Surface 归属的 2D 内容被**屏幕视口重复绘制**（同一对象出现两次且带偏移） | 渲染管线 | **已关闭（B3 Step 5 修正）** | `record_group` 引入 `SurfaceScope::{Viewport, Content}`：视口层跳过归属任一 `Surface` 的对象（内容只进该 Surface 的纹理）；test `renderer2d_content_is_not_drawn_in_viewport`；`surface_matrix` 重录 `baafe1aa… → 7bed8a31…`，既有四基线不变 |
| S4-11 | `composite = true` 把整张纹理**铺满视口**（无 letterbox / 无目标矩形）⇒ 纹素宽高比 ≠ 输出宽高比时画面被拉伸（圆变椭圆） | 渲染后端 | **已关闭（B4 验收）** | `CompositeOp.fit: Option<(u32,u32)>`：`Some` ⇒ 按源纹理宽高比 **contain + 居中**（`fit_rect` + `rect_to_ndc`，顶点阶段按 per-layer uniform 的 `rect` 重映射四边形）；`None` ⇒ 铺满（视口层，离屏尺寸 == 视口尺寸 ⇒ 恒等）。验收：`gpu_fit_letterboxes_instead_of_stretching`（2:1 源进 4×4 ⇒ 上下各 1 行黑边）+ 3 条纯函数用例；既有 5 条 native 基线**逐位不变**（作品 A/B 本就是 4:3 = 4:3，rect 换算为精确的 `(-1,1,-1,1)`） |
| S4-12 | `Planar` 兜底 UV 的 V 轴方向反了：贴图在无 UV 几何上**上下倒转** | 渲染管线 | **已关闭（B3 Step 5 修正）** | `fallback_uvs` Planar 改为 `v = 1 - y`（纹理行首在模型 +y，与 `parametric_surface` 手写的 `1.0 - v` 同口径）；test `planar_fallback_uv_puts_image_top_at_positive_y` |
| S4-13 | `solid_cube` 只有 8 个共享顶点、**无 UV / 无法线**：Planar 兜底在侧面（x=常量）退化成 0 面积 ⇒ PBR 法线 TBN 除以 `det≈0` 得 NaN（右脸随机噪点）；面法线恒 +Z ⇒ 三个面同亮度 | 渲染管线 | **已关闭（B3 Step 5 修正）** | 改 24 顶点（每面 4 个）：显式 UV（图像行首在 `+y`）+ 面法线；`cube.wgsl` 的 TBN 在 `abs(det) ≤ 1e-6` 时跳过法线贴图；tests `test_solid_cube_faces_have_non_degenerate_uv`；demo3d 重录 `dc8b1388… → ddd69b3e…`（噪点消失 + 面法线正确着色，逐条可解释） |
| S5-01 | 既有 2D 作品迁移纹理路径 | 作品 | **已否决（不改写）** | 既有 2D 作品**不迁**纹理路径：无 3D 消费者，强行 `composite` 会多一次全屏重采样且无收益。纹理路径的验证改由新增作品承担（`surface_matrix` / `atlas`）。本条不是"待执行的缺口"，是决策留痕 |
| S6-01 | 脚本无法声明 Surface / 无法把内容纹理挂到材质 | 脚本 API | **已关闭（B3 Step 5-0）** | `scene.surface(zela.Surface)` + `Material3D.texture = Texture.Surface{..}`；`SurfaceTextureRef` 在 commit 解析；`zela.scene@7`（指纹 `15_573_081_187_359_822_637`） |
| S6-02 | `zela.Material3D` 加 `texture` 字段而指纹不变（字段结构不进指纹） | 脚本 API | **仍成立（已知）** | 加**方法**会变指纹、加**字段**不会 —— 看守力度不一致；本轮靠版本递增 + 手工记账 |
| S6-03 | Rust 版 `authored/demo3d.rs` 不进任何对拍 / 测试（验证盲区） | 作品 / 测试 | **仍成立（记账）** | `build_script_runtime` 用 `with_demo3d = false`，`--script` / `--bundle` 都不构建它；`tests/` 只覆盖能力 demo 与分镜 ⇒ 该模块的改动无人验（4c-4 的"零差异"对它是平凡真）。触发条件：给它加对拍入口，或显式降级为"仅 GUI 示例" |
| S6-04 | 脚本可表达的图元是 9 组录制表的**真子集** | 脚本 API | **仍成立（已知）** | `ze-scene-api` 只有 `Circle` / `Polyline` / `Arrow` / `PathShape`（无 Rect / Line / Ellipse / Polygon）；作品 A 因此只能覆盖 Circle / Arrow / Path × {Fill, Stroke}，Rect / Line 组合**无法用脚本表达** |

**J 段小计（B3，已执行部分）**：已关闭 20（S0-01/02/03、S1-01、S1-04、S2-01/02、S3-01、S4-01/02/03/04/05/06/07/09/10/11/12/13、S6-01）· 仍成立/显式拒绝 7（S1-02/03、S2-03、S4-08、S6-02/03/04）· 不成立未采纳 1（S0-04）· 未拍 1（S3-02）· 已否决 1（S5-01）。

---

## K. B4 批次记账（矢量后端移植与渲染参数契约，2026-10-22）

> 计划《B4执行计划_矢量后端移植与渲染参数契约》。上游 `manimgx @ dcfe01a`（本地快照在仓库内 `manimgx/`，已 gitignore）。
> B4 Step 1 / 2 / 3 / 4 / 5 / 6 与 B6 Step 1 / 2.1 / 3 / 4 已落地；矢量后端**能画、能对拍、能出作品基线**，
> 但 `shapes` 契约与后端可注册仍未做（V2-12 / V2-18），渐变 / 纹理 / dash 仍缺概念（V2-13）。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| V1-01 | 渲染参数（分辨率 / 采样 / 过滤 / 后处理）不在任何契约里 | 渲染 IR | **已关闭（B4 Step 1）** | `ze_render_ir::RenderParams`（size / supersample / msaa / filter / tone / bloom / ao / env）；`Default` = 当前行为；`validate()` / `is_supported_by(&Capabilities)`；`target_desc()` 是 `RenderTargetDesc` 的唯一上游投影 |
| V1-02 | 后端能力没有"后处理 / IBL"维度，无法表达"不支持" | 渲染 IR | **已关闭（B4 Step 1）** | `PostCaps`（tone / bloom / ao / env）进 `Capabilities`；wgpu 后端声明 `none()` ⇒ 一旦 Meta 要求后处理就**显式失败** |
| V1-03 | ZSB 不承载渲染参数，回放端各写各的默认值 | 装配产物 / 回放 | **已关闭（B4 Step 2）** | `Meta.render_params`（`#[serde(default)]`，不进内容 hash）；`zela-playback::bundle_render_params` + `check_render_params_supported`；`zela-render` 开渲前校验 |
| V1-04 | 许可证基线未立：上游多数文件**自身没有 SPDX 头** | 依赖 / 合规 | **已关闭（B4 Step 3）** | 新 crate `zela-render-vector`：`LICENSES/{MIT,Apache-2.0}.txt` 逐字复制；`UPSTREAM.md` 记录 SHA + 逐文件许可（light/occlusion/tone/bloom + dfg.bin 为 Apache-2.0）；只开 `render` feature |
| V1-05 | `RenderCapabilitySet::ALL` 把"只有一个后端"写进类型；无后端注册/选择点 | 渲染后端 | **已关闭（B4 Step 4）** | 删除 `ALL`，新增 `union()`；`BackendKind::{Wgpu, Vector}` 各自自述；`BackendRegistry` 注册 / 选择 / 协商（未实现或未注册 ⇒ 显式错误）；测试锁住"wgpu-only 协商 shapes 必须失败" |
| V2-01 | 矢量后端本身尚未移植（`vector.*` / `render.rs` 公共类型 / 着色栈 / 光栅后端，约 9.9k 行） | 渲染后端 | **已关闭（B4 Step 5）** | `zela-render-vector` 收录上游源码（`vector.*` + `vector_buffers/compute` + `render.rs` 公共类型 + `blend*` / `depth` / `paint` / `mesh` / `shadow` / `bloom` / `occlusion` / `environment` + 着色栈 wgsl + `dfg.bin`）；逐个补 SPDX 头；宿主适配层 `api::FilledPath` / `render_2d`。验证：`cargo test -p zela-render-vector` 26 passed（含上游自带用例 + `filled_square_draws`） |
| V2-02 | 双后端共存验证 + 矢量作品新基线 | 测试 / 对拍 | **已关闭（B4 Step 6，新基线拆到 V2-07）** | `zela-render-backends/tests/vector_vs_wgpu.rs`：同一进程内 wgpu 与矢量各渲一帧，判据 = 非空 + 覆盖率相对差 ≤ 10% + 逐像素 RGB MAE ≤ 24/255。实测：轴对齐矩形 4096/4096 像素、MAE 0.00；非 45° 斜边三角形 MAE 12.18/255、覆盖率相对差 0.005。**⚠️ 本条 12.18 / 24 是夹具 bug 修正前（B4 验收 P1-1）的旧读数，已被 V2-09 取代（全图 0.09 / 阈值 8，内部区 ≤ 2）；此处仅作血缘留痕** |
| V2-03 | 上游是 `m1/m2` 双变换 + **GPU 插值**；我们是 CPU 每帧烘单矩阵 | 渲染后端 | **记账（移植时已知非等价）** | 移植时令 `m1 = m2` 可跑通，但**丢掉"一次上传、GPU 插值"的性能模型**；确定性分镜不受影响（逐帧渲染） |
| V2-04 | 三角化资产（`tessellate.rs` / `VertexBufferData` / `shape_to_mesh`）在矢量路线下不是唯一真源 | 文档 / 分层 | **记账（性质变更）** | 不删除、不破坏分层；在 ARCHITECTURE 写明"wgpu 后端专用路径"，避免后人误以为全局唯一 |
| V2-05 | 旧 ZSB 没有 `render_params` 字段 | 装配产物 | **已按兼容策略关闭（V1-03）** | `#[serde(default)]` ⇒ 缺字段按 `RenderParams::default()`（= 当前行为）；不升容器版本（该字段不进 hash） |
| V2-06 | 矢量后端**未接进作品管线**：`DrawCommand` / `Path2D` → `Record` 的全量映射（描边 / 渐变 / 图片 / 圆角）未做；`zela.render.shapes` 未实现；`BackendKind::Vector` 仍 `is_available() == false` | 渲染后端 / 装配 | **部分关闭（B6：填充 + 描边已映射，且已进作品基线）** | `zela-render-vector::scene` 把 `Shape2D + Transform2D + Style2D` 映射成填充记录（圆/椭圆 4 段三次曲线、矩形/多边形、路径含 Quad/Cubic/Arc）；B6 Step 2.1 补上**描边记录**（`Record::stroked`）。真实 `DrawQueue` 帧对拍：填充 4568 vs 4580 像素、MAE 0.05/255；描边三角形 934 vs 954、MAE 0.20/255。B6 V2-07 起 `--backend vector` 用同一份快照出作品帧并进 `[vector]` 基线。**仍缺**：渐变笔刷行 / 纹理图片 / dash（V2-13）、`shapes` 签名（V2-12）、可注册（V2-18） |
| V2-07 | 矢量作品新基线（计划 §6.3）依赖 V2-06 | 验收 / 基线 | **已关闭（B6 Step 3）** | `zela-render --backend vector`：同一份 `DrawQueue` 快照 → `render_scene_2d`（纯 2D 帧；含 3D / 内容图层的帧**显式报错**，不静默跳过）。作品 `vector_paths.lua`（Circle / Arrow × Fill / Stroke、`Reveal` 折线描边、`content.text` → 路径填充，静态 1 帧）实测 14 对象、1 帧、`d19962e7…`。`baselines.txt` 新增 `[vector]` 段；`xtask` 的 `hashes` / `--record` / `--rebind` / `+dirty` 门禁经 `is_managed_section` 覆盖该段，并有 3 条解析器用例 |
| V2-08 | 基线归因：`--record` 在**脏工作树**上把值标成 `@ <HEAD>`，但该提交并不包含这份状态（6 条值全标 `@ da9ea93`，实际由 `642b9d4` 携带）；slotA → slotB 的差异也无逐条说明 | 验收 / 基线 | **已关闭（B4 验收 P1-4，口径 D）** | `--record` 检测脏树 ⇒ 写 `@ <sha>+dirty`；新 `cargo verify --rebind`（要求干净树）把 `<sha>+dirty` **整段**换成携带录制的提交并自校验；`smoke` / `hashes` 在见到未归因 `+dirty` 时**直接失败**；`# delta:` 逐条留痕（slotA→slotB 前缀 + 原因指针）；存量 `baselines.txt` 已按真实血缘重写（demo3d `dc8b1388→ddd69b3e`、atlas `efe5bf39→93632049`，@ `642b9d4`）。文档：`FRAME_HASH_BASELINES.md` 口径 D；测试：`xtask/tests/parser_tests.rs` 5 条 |
| V2-09 | 双后端对拍的**口径**问题（B4 验收 P1-1/2/3）：① MAE 12.18 无法区分"AA 方式不同"与"色彩管线不同"；② 矢量自测在起不来 GPU 时静默 `return`（仍然 passed）；③ 对拍注释声称"验证注册机制"，实际绕过 `BackendRegistry` | 测试 / 对拍 | **已关闭（B4 验收）** | ① MAE 拆「内部区 / 边缘带」并输出计数，内部区门禁 ≤ 2/255；**诊断出真 bug**：斜边夹具把屏幕坐标（y 向下）当成世界坐标（y 向上）⇒ 比的是镜像形状。修正后实测全图 MAE 0.09（内部区 0.00、边缘带 3.44），全局阈值由 24 收紧到 8。② `api.rs` 起不来即 `panic!`（与对拍同口径）。③ 注释改为"本对拍不经过 `BackendRegistry`，注册机制由 `registry.rs` 单测覆盖" |

| V2-10 | 描边 `cap` / `join` 曾是 **lyon 的隐默默认**（不在任何契约里）：换后端 / 换 lyon 版本都会静默改画面 | 渲染 IR | **已关闭（B5 Step 1）** | `StrokeCap` / `StrokeJoin` / `StrokeShape` 进 `ze-render-ir::style`；`StrokeSolid` 带 `#[serde(default)]`（默认 butt + miter ⇒ 旧 ZSB 逐位不变）；`Style2D.stroke_shape` 是**后端唯一消费点**；`tessellate.rs` 的 `stroke_options()` 显式驱动 lyon（`StrokeOptions::default()` 不再裸用）。实测钉住 `lyon_tessellation 1.0.22` 默认 `Butt / Miter / 4.0`，测试断言"默认选项逐字段相等 + 改 cap/join 真的改变顶点数" |
| V2-11 | miter limit 两边不一致：lyon 默认 **4.0**，上游矢量**硬编码 10**（`vector.wgsl:497` / `render.rs:330`） | 渲染后端 | **记账（B5 §1.3）** | 即使两边都声明 `Miter`，极端尖角表现仍不同。改它要动上游 wgsl 语义 ⇒ 本批不改。**触发条件：矢量后端成为默认 2D 路径之前必须关闭** |
| V2-12 | `zela.render.shapes` 签名只有 `capabilities` / `submit_shapes`：**没有** `create_target` / `upload`；且 `PathList` 表达不了 `Circle` / `Rect` / `Group` | 能力契约 / 装配 | **未执行（B6 复核后仍不改）** | 改签名必须同步 `PINNED_FINGERPRINTS`（`shapes = 1_097_982_529_554_556_752`）。**为什么不改**：`submit_shapes` 在仓库里**没有任何实现也没有任何调用点**（`IMPLEMENTED_NAMESPACES` 不含它、wgpu 协商 shapes 本就显式失败），脚本层也没有 `create_target` / `upload` 的绑定表；此时把入参改成尚不存在的 `ShapeList` 只会得到一个**没有实现可校验的 ABI 字符串**（违反"不得为凑可注册而填未验证的编码"）。触发：真正要接 `shapes` 时，先定 target 生命周期与 `Shape2D` 列表的宿主载体，再一次性改签名 + 指纹 |
| V2-13 | 渐变笔刷行 / 纹理图片 / dash 未映射（**缺概念**，不是缺映射） | 渲染后端 | **记账（B5 §七）** | 渐变需先定义色标 + 轴向与 `Style2D` 的关系；图片需给 `Shape2D` 加 `Image` 变体（R-04）；dash 需先有 dash 概念（上游是 `dash[3]` 周期/占空/相位）。触发：作品需要时 |
| V2-14 | 矢量后端的**描边记录**未接 | 渲染后端 | **已关闭（B6 Step 2.1）** | ⚠️ B5 的阻塞理由**不成立**：`Curves.miter` 由 `render.rs:476 Store::add_path` 自动计算，与 `vector.rs` 无关；真正缺的只是 `Record` 的描边构造。现在 `Record::stroked`（`stroke` + `params.z` = **宽度** + `flags >> STYLE` = `cap \| joint << 2`）已落地，`scene::scene_records` 按"先填充后描边"产出。**事实更正**：`params.w` 是背景描边的宽度，不是 miter；miter limit 硬编码在 wgsl（见 V2-11）。宽度语义由实测钉住（传半宽会画出一半宽）。对拍：描边三角形 934 vs 954 像素、MAE 0.20/255、内部区 0.29 |
| V2-15 | 着色栈的**结构事实**：`tone` 由 bloom 的合成 pass 应用，而那条 pass 只在**存在 3D layered 计划**时运行（`render.rs:2419`）；`occlusion_of()` 还要求存在环境光（类型 4）或全向光（类型 0），否则 AO 静默不生效 | 渲染后端 | **已关闭（B6 Step 1 验收用例）** | 结论不变（B6 §2.5 说 "bloom/tone 独立于光照" **不成立**），但现在有 3 条 3D 用例正面钉住：有 3D 计划时 `AgX ≠ clip`（2D-only 帧仍相等，见 `tone_mapping_changes_pixels_and_defaults_are_identical`）；AO 有环境光时让内角变暗、只有方向光时**逐位不变**。另记：受光/材质编码来自上游 `feed.py:1134`（`shade_in_3d` → `LIT`）与 `:1150`（`material[3] = 1`），`plan.lit` 按 `material[3] > 0` 判定 |
| V2-16 | `ToneCurve` 语义错配（B4 建契约时按常见命名猜的）：`Reinhard` / `Aces`，而 `tone.wgsl` 只有 `0 = clip` / `1 = AgX` | 渲染 IR | **已关闭（B6 Step 1.1）** | 改为 `ToneCurve { Clip, #[default] AgX }`；`default_for(caps)` 按后端能力取默认（wgpu 全关 / 矢量全开），显式请求不支持项仍报错 —— 与"静默降级"的区别写进 `RenderParams::default_for` 文档 |
| V2-17 | 校验口径：静态单帧测不出**闪烁**（时域不稳定） | 测试 / 对拍 | **已关闭（B6 Step 4）** | `per_frame_tolerance_holds_across_an_animation`：8 帧旋转 + 平移矩形，逐帧走**同一套**门禁（非空 + 覆盖率 ≤10% + 全图 MAE ≤ 8 + 内部区 ≤ 2），再对帧间 MAE 序列取"最大 ≤ 3×均值"。实测：逐帧 MAE 0.03–0.08，帧间均值 1.31 / 最大 1.54 ⇒ 无异常跳变。阈值来自实测数据（不是先设一个宽到永远绿的数） |
| V2-18 | `BackendKind::Vector` 仍 `is_available() == false`（B6 Step 2.3 未转 true） | 渲染后端 / 装配 | **未执行（B6 复核：无可自述 namespace）** | 注册的意义是"给系统能力加 namespace"。矢量后端**没有**可自述的：`zela.render2d` / `render3d` 的 `upload(PodBytes)` 吃三角化顶点（矢量后端按路径解析渲染，不吃），`zela.render.shapes` 未定稿（V2-12）。⇒ 注册它不会带来任何能力，翻开关就是凑数（违反 §四.4）。`--backend vector` 是 `zela-render` 的**进程级开关**（同一份 `DrawQueue` 快照），不经注册表；`post_caps()` 已能自述（`Wgpu = none()` / `Vector = all()`） |
| V2-19 | "开着色栈（tone + bloom + AO + env）的作品"无法用脚本表达 | 作品 / 脚本 API | **记账（B6 Step 3 第二件作品改为像素用例）** | 脚本层没有"2D 光源 / 曝光 / tone / bloom"的表达（`zela.scene@7` 本批不变），而矢量后端**不吃 3D 图层**（`--backend vector` 对含 3D 的帧显式报错）⇒ 脚本作品开不了着色栈。验收改为**3D 固定场景像素用例**：`tone_mapping_changes_a_lit_3d_frame`（AgX ≠ clip，clip 死白 / AgX 压回）、`bloom_spreads_bright_pixels_outward`（亮区外溢）、`ambient_occlusion_requires_ambient_light`（墙 + 地板内角变暗；只有方向光时无效）。环境图（env）本次未接：`View3` 没有环境图入口，`Environment` 上传仍是 `pub(crate)` |
| S4-14 | 孤儿 Surface：`Renderer2D + composite=false` 且无 `Material3D.texture` 引用 ⇒ 内容既不在视口画（S4-10 起跳过）、也不参与合成、也没有代理平面 ⇒ **完全不可见且不报错** | 渲染管线 | **已关闭（B5 A-2）** | `content_texture_specs` 在同一趟里 `for_each::<Material3D>` 查引用；无人引用 ⇒ `SurfaceError::UnconsumedSurface`（报错文案给出修法：设 `composite=true` 或让 3D 材质引用）。测试 `unconsumed_content_surface_errors_explicitly`（无人引用报错 / 有引用通过 / composite=true 通过） |

**K 段小计（B4 + B5 + B6）**：已关闭 16（V1-01..05、V2-01/02、V2-07、V2-08/09、V2-10、V2-14、V2-15、V2-16/17、S4-14）· 部分关闭 1（V2-06）· 记账 5（V2-03/04、V2-11、V2-13、V2-19）· 未执行 2（V2-12、V2-18）。

---

## L. B8 批次记账（性能优化与可观测性，2026-10-23）

> 计划《B8执行计划_性能优化与可观测性》+ 更正件《B8补充更正_GPU校验Pass与tile指纹》
> + 验收报告《B8_Step1执行验收报告_性能实测与方向修正》。
> 本批是**纯性能改动**：像素逐位不变是唯一正确性证明（V3-01 原口径）。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| V3-01 | 手写 `sha256` 被用在不匹配的负载上：它自己的文档写"只服务 payload hash"（几十 KB、每次装配一次），实际**每帧**哈希 1.2 MB 帧缓冲；朴素 compress + 每次调用 1.2 MB `Vec` 拷贝 ⇒ 实测 22 MB/s、占 45% 帧时间 | 性能 / 装配 | **已关闭（B8 Step 3′0）** | 新增 `sha256::sha256_fast`：非 wasm 走 `sha2`（**target-specific 依赖**，wasm 侧仍用手写实现、格式 crate 保持极轻）；`frame_hash` 只在大输入上走快路径。对拍 `fast_path_matches_the_hand_written_one`（空 / 63 / 64 / 65 B / 1.2 MB + FIPS 向量）。实测（dev）54.3 → 26.5 ms/帧；**release 0.64 ms/帧**；六条基线值不变 |
| V3-02 | PNG 用 crate 默认压缩档（zlib level 6 + 逐行 `Adaptive` 滤波），1.2 MB ≈ 57 ms/帧 | 性能 / 导出 | **已关闭（B8 Step 3′00）** | `Compression::Fast` + `FilterType::Sub`。**`NoFilter` 实测更差**（46 ms 且文件大 4.3 倍：419 KB vs 97 KB）⇒ 选 `Sub`。只改 `.png` 文件字节；序列哈希吃像素缓冲（`frame_hash` 模块说明早已写明这点），六条基线不变。dev 57.3 → 41.5 ms/帧；release 1.8 ms/帧 |
| V3-03 | §0.3「实际渲染 110 ms/帧」的推算被实测推翻 | 计划 / 决策 | **已关闭（B8 Step 1 实测）** | fourier 渲染核心 **8.9 ms/帧**（dev）、demo3d 4.1 ms；93% 是哈希 + 编码。⇒ 由"每帧贵"推出的「新作品 ≤60 帧」「纳管成本 3.3×」「分级阈值」全部失据（影响 B7 的成本模型） |
| V3-04 | 双缓冲流水线（原 Step 4） | 性能 / 流水线 | **已关闭（取消）** | `t_read`（含 GPU 等待）实测 dev 1.3–4.4%、release 17–32%，但整帧已降到 3.7–5.2 ms（release）⇒ 绝对值 ≤1.7 ms。按更正件 §3.2 的 30% 线直接取消；理由记账：夹具路径的毫秒级回读不值得一套双缓冲复杂度 |
| V3-05 | GPU 校验 pass / tile 指纹（原 Step 3′B） | 性能 / 校验 | **记账（推迟，不是否决）** | GPU 校验的真实收益是省 CPU 哈希（54 ms dev），而该代价已被 Step 3′0 的库级修复覆盖（release 0.64 ms）⇒ "为加速而换基线格式 + 全量重录"的理由不成立。触发条件：release 下哈希重新成为主瓶颈，或需要"差异定位到 tile"的诊断能力（那种场景 tile 指纹确实比 32B 全局摘要强） |
| V3-06 | sink 抽象（原 Step 2）：主流程被迫为测试夹具付哈希代价 | 装配 / 导出 | **已关闭（B8 Step 2）** | `FrameSink`（`PngSink` / `FfmpegSink` / `HashSink`）；`--hash` 显式挂、`--hash-out` 隐含，**默认不挂**；xtask 对拍显式传 `--hash`。V3-12：同帧 PNG **字节**在挂/不挂哈希时逐位相同（fourier `frame_00100` 与 `vector_paths` 各验一次） |
| V3-07 | dev 与 release 的 20–40× 差距没有任何记账，容易被当成"优化没生效" | 性能 / 构建 | **已关闭（B9 Step A + Step 1）** | 同机同码：hash 26.5 → 0.64 ms、PNG 41.5 → 1.8 ms、整帧 77.0 → 5.2 ms。B9 用实验证明两者**像素逐位一致**（release 下 fourier `b6eaa864…`、demo3d `ddd69b3e…`、vector_paths `98d61c1a…` 全部等于基线）⇒ 门禁可以改用 `[profile.verify]`（opt-level 3 + debug=false，仍继承 dev 的检查） |
| V3-08 | §0.3「实际渲染 110 ms/帧」的推算被实测推翻后，没有回答"切 profile 会不会改变像素" | 性能 / 构建 | **已关闭（B9 Step A）** | 三个作品（2D 路径 / 3D PBR / 矢量后端）的 release 序列哈希与基线逐位相同；无需 `fp-contract=off`，也没有分歧帧。 |
| V3-09 | `cargo verify` 全程跑在未优化构建上（全仓无 `[profile]`，xtask 的 `cargo run` 没有 `--release`） | 性能 / 构建 | **已关闭（B9 Step 1）** | 根 `Cargo.toml` 新增 `[profile.verify]`（`inherits = "dev"` + opt-level 3 + debug=false + incremental=false）；xtask 抽 `run_args()` 统一给 `zela-compile` / `zela-render` 加 `--profile verify`，`check` / `test` 仍走 dev。实测：`hashes` 52.48s（profile 切换后、仍是 19 次进程）；编译：增量 9.03s。 |
| V3-10 | 每次渲染进程约 4s 的固定开销没有拆分（"不知道贵在哪"） | 性能 / 构建 | **已关闭（B9 Step 2 探针 + B7 Step 0.2 复测修正）** | verify profile 下实测（fourier / vector_paths）：GPU 初始化 **531 / 502 ms**、脚本运行时构建（含 typst）**718 / 668 ms**、`StandardBackends::new` **5 ms**、首帧 18 ms / 439 ms（后者含矢量管线创建）、其余帧 4.5 ms/帧。**B7 修正**：批量进程内逐作品复测 runtime 构建 = **0–1 ms** ⇒ 那 718 ms 是 **typst 世界 / 字体加载，进程级一次**，已天然跨作品复用；B9 报告结论②「runtime 构建每作品仍要付」**不成立**（这是好消息：不需要 hoist）。 |
| V3-11 | 19 次进程调用 ≈ 46s 固定开销（占 profile 切换后 `hashes` 的 ~88%） | 性能 / 装配 | **已关闭（B9 Step 3）** | 新 bin `zela-verify`：**一次** GPU 初始化，逐作品新建 Runtime 与 `StandardBackends`（`wgpu_3d` 的离屏/深度目标按 `LayerId` 索引，复用会串味 ⇒ 选安全方案），编译也在同进程（§4.5）。帧循环抽到 `zela_app::frame_loop`，与 `zela-render` **共用一份**（不是第二条渲染路径）。实测：批量 **6.46s** vs 独立进程 **53.16s**，6 个作品哈希逐位相同；`--independent` 保留为对照组。 |
| V3-12 | 「挂 / 不挂 HashSink 像素逐位相同」（B8 的 V3-12）只有人工比对，会腐烂 | 测试 / 门禁 | **已关闭（B9 Step 5）** | `xtask::assert_sink_bypass` 并入 `--step hashes`：`vector_paths`（1 帧）分别挂/不挂哈希各渲一次，比 **PNG 字节**（不比序列哈希 —— 不挂时没有哈希值，比哈希是自证）。实测 39504 B 完全相同。 |
| V3-13 | `opt-level = 3` 会让编译变慢、与"验证要快"存在张力 | 性能 / 构建 | **记账（本批末测量，未引入 sccache / lld）** | 首次 verify-profile 全量构建 + 六步全跑 = **530.57s**（含 test/wasm/structure/smoke/hashes，编译时间未单独隔离）；改一个 zela-app 源文件后的增量重建 = **9.03s**。未引入 sccache（外部依赖 + CI 配置）与换链接器（需 `.cargo/config.toml` 分 target），理由见 B9 §5.2 的风险栏。触发：若首次构建时间成为日常瓶颈，再按 §5.3/§5.4 逐项评估。 |

**L 段小计（B8 + B9）**：已关闭 11（V3-01/02/03/04/06/07/08/09/10/11/12）· 记账 2（V3-05、V3-13）。

---

## M. B7 批次记账（内容丰满期，2026-10-23）

> 计划《B7执行计划_内容丰满期》。Step 0 / 1 / 2 已落地（Step 3–5 见后续提交）。

| 编号 | 缺口 / 变更 | 影响层 | 类别 | 证据 |
|---|---|---|---|---|
| W1-01 | `MeshGeometry.id` 取自**进程内**全局计数器（`ze-scene-core/components/3d.rs` 的 `next_geometry_id`），且随 serde 序列化进 ZSB payload ⇒ 同一脚本的 payload hash 取决于"本进程此前造过多少几何体"，ZSB 不是源码的纯函数 | 装配产物 / 可复现性 | **记账（B7 Step 2 发现，本批不改）** | `zela-compile`（一进程一作品，build.ps1 的路径）与 `zela-verify`（一批作品共用进程）对 fusion / pbr_materials / pbr_lights / pbr_scene / atlas 算出不同 payload（如 fusion `5db2aecb…` vs `64c00b63…`）；解包对比：**唯一差异**是 `creates/*/*/data/id`（1,2,3 vs 7,8,9），其余字节相同。因此 `demo/manifest.txt` 断言的是"一进程一作品"的值（build.ps1 的真实路径）。触发：需要编译产物可复现（构建缓存 / 跨机比对 / 内容寻址）时，把 id 排除出 payload（serde skip，解码时重分配） |
| W1-02 | manifest 的 payload 断言长期空转：13 件里 12 件写 `-`（等于没断言），唯一有值的 fourier 也已不可复现 | 装配产物 / 门禁 | **已关闭（B7 Step 2）** | fourier 的 `f8ed3eae…` 停在 45b0f85（改名 + ZSB 容器 v3）；当前代码下**任一字体配置都复现不出**（系统字体 `dad48069…`、`ZELA_TYPST_FONT_DIR` 确定字体 `a5bdfbe7…`）⇒ 该断言今日必然失败（不断言它从哪次提交开始坏）。Step 2 用 build.ps1 的同一命令逐件重算 13 条并逐条断言通过；含文本的 5 件（fourier / storyboard / surface_matrix / atlas / vector_paths）必须带确定字体才可复现，已写进 manifest 表头 |

| W1-03 | JS 适配器（boa）实现完整但**零作品**：没有一件脚本走 JS，等于没有兑现证据 | 脚本 / Pod 装配 | **已关闭（B7 Step 3）** | `surface_matrix.js` 是 `surface_matrix.lua` 的逐语句翻译（同组件、同字段、同枚举、同顺序）。判据与实测：① 分镜 JSON **逐字节相同** ⇒ `payload hash` 两件都是 `e0951fd4…`（带确定字体）；② 帧序列哈希两件都是 `928cd5f9…`（640×480，1 帧，`script == bundle`）⇒「同一场景、两种语言、同一像素」成立。落地配套：`js` 进 xtask 对拍 features（`typst,js`，纯 Rust，不让对拍依赖本机 Python）、`build.ps1` 的编译命令同开 `js`（不进 wasm —— 网页只回放 ZSB）、`zela-verify` 支持"作品名自带扩展名"（同名 `.lua` / `.js` 并存时探测顺序选不出来） |
| W1-04 | `[browser]` 段定位待改：JS 是唯一能进 wasm 的脚本语言，但真值仍未录制 | 基线 / 浏览器 | **记账（B7 Step 3 探测）** | `surface_matrix.js` 的 ZSB 与其它作品**同一容器格式**（回放端只解释分镜，不关心源语言）⇒ 浏览器侧第一次具备"可比对"的前提；`build.ps1` 已带 `?compare=1` 对拍入口。本机跑不出浏览器产物（需要 `wasm-pack` + Chrome WebGPU）⇒ 本批不录值，`[browser]` 段维持"仅存档、不参与自动比对"。触发：在 Chrome WebGPU 打开 `index.html?compare=1` 跑一次即可补录（`[browser]` 只增不改语义） |

| W1-05 | 6 项"零使用能力"里只有 3 项在脚本层**可达**（`scene.despawn` / `Curve.Const` / `Target.CameraEye`）；`Group` / `Target.Phase` / `Target.MorphT` 是**没有入口**，不是没人用：`Group` 不在 `SceneApi` 签名里、`PhaseAnim` 只由函数图像对象内部产生、`MorphShape` 只在渲染/测试里构造 | 脚本 API / 能力兑现 | **已关闭（B7 Step 4，`zela.scene@8→@9`）** | 新增三个入口：`scene.group(zela.Group{id})` → `Group` 标签、`scene.phase_anim(zela.PhaseAnim{phase,omega})` → `PhaseAnim`、`scene.morph(zela.Morph{from,to,closed,t,samples})` → `MorphShape`（折线点表走 `Path2D::from_polyline`，与 `reveal`/`polyline` 同口径）；指纹 `15_573_081_187_359_822_637 → 10_059_647_094_731_395_853`。**`Group` 判断结论**：语义 = 纯组合标签、不含变换、不嵌套，与 `Parent`（父子变换累乘）分工清晰 ⇒ **补，不删**。作品 `capability_gaps.lua`（90 帧）一次覆盖 6 项，`script == bundle`；`despawn` 有效性用"去掉那一行"的对照脚本实测：哈希 `d66b333a…` ≠ `29713c3d…`（白圆出现在视口内） |
| W1-06 | `Target.Phase` 写入的 `phase` **没有消费者**：脚本层没有创建函数图像对象的入口（`FunctionX2Y` + `Sampler1D` 不暴露，且函数是 Rust 闭包，ZSB 无法序列化） | 脚本 API / 动画目标 | **记账（B7 Step 4）** | `capability_gaps.lua` 里 `phase` 每帧被 `BindingApplySystem` 写入（组件值确实变了），但 `SceneRecordSystem` 只在函数图像对象上取 `x + phase` ⇒ **像素不变**。触发：要暴露函数图像对象时必须先解决"函数怎么持久化"（命名函数表 vs 表达式子集），不是加一个组件构造器就够 |
| W1-07 | `scene.despawn` **没有时间参数**：在 commit 期执行 ⇒ 只能表达"创建后取消"，无法表达"动画中途移除" | 脚本 API / 生命周期 | **记账（B7 Step 4）** | `scene_service::scene_despawn` → `SceneEncoder::despawn`，写入 SceneScript 的 `despawns`，commit 时一次性应用。作品用它做"条件性不生成"（对照实测见 W1-05）。触发：需要中途销毁时，得给 `despawn` 加时间轴参数（属动画语义扩展，不是补一个字段） |
| W1-08 | A6「`zela.scene` 指纹只变一次」被 Step 4 打破（`@7→@8` 后又 `@8→@9`） | 验收口径 | **记账（用户明确豁免）** | 根因：计划 §6.1 把 `Group` / `Target.Phase` / `Target.MorphT` 当成"可达但零使用"，实际按代码是"不可达"（W1-05）；Step 1 已提交，无法合并。用户在 Step 4 明确选择"开 @8→@9，六项全补" ⇒ 本条只留痕，不作为违规 |

**M 段小计（B7）**：已关闭 3（W1-02、W1-03、W1-05）· 记账 4（W1-01、W1-04、W1-06、W1-07）。

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
