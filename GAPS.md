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
| E-03 | 2D / 3D 两个队列、3D 无层序（#11/D2）→ **一个 `DrawQueue`**（2D 形状 + 3D 网格/线段共存）+ 统一 `ZOrder`（IR 唯一声明，`ze_object::ZOrder` re-export） | 渲染管线 | **已关闭（B2-1）** | `test_queue_stable_within_equal_zorder`（R-F 显式锁定）、`test_unified_queue_mixes_2d_and_3d_by_zorder` |
| E-04 | 2D 图元无法参与 3D（融合缺口）→ `derive_mesh`（Rect/Circle/Ellipse/Polygon/Line → 平面网格，Flat + double_sided 材质默认化） | 渲染管线 | **新增能力（B2-3）** | `test_b2_3_derived_2d_rect_renders_in_3d`；`Path`/`Group`/`Custom` 显式报错（不静默） |
| E-05 | 透明物体无排序（无透明桶）→ 不透明先画、透明按 `ZOrder` 升序后画；透明 pipeline **不写深度**（R-A） | 渲染后端 | **已关闭（B2-3）** | `test_b2_3_transparent_bucket_orders_by_zorder`（对调 ZOrder 后主导色对调） |
| E-06 | 共面 z-fighting → `ZOrder × Z_ORDER_EPSILON` 的 **normal offset**（不用常量 depth bias，D4） | 渲染后端 | **已关闭（B2-4）** | `test_b2_4_epsilon_from_zorder_resolves_coplanar`；`DepthBiasState::default()` 未改 |
| E-07 | 3D 无 AA（1x）vs 2D 4x（#6/#8）→ 3D MSAA 4x（`DEFAULT_MSAA_SAMPLES`），采样数经 `new_with_samples` 由能力侧给定 | 渲染后端 | **已关闭（B2-5）** | `test_b2_5_msaa_4x_produces_partial_coverage`（4x 部分覆盖像素更多） |
| E-08 | **纹理走样**：MSAA 治不了纹理采样走样（`Material3D.texture` / `ContentTexture` 进 3D 后会有） | 渲染后端 | **仍成立（已知缺口）** | 需要 **mipmap + 各向异性过滤**；本轮不做（计划 B2-5 记账要求） |
| E-09 | 硬件 MSAA **resolve 的跨端确定性**是否为 `[browser]` 段"1 帧 1 像素"差异的贡献者 | 对拍 | **待验证（不是断言）** | 本轮无浏览器环境（D5：跨端重录推迟）；`[browser]` 段仍仅存档 |
| E-10 | 2D 派生网格仅覆盖可直接三角化的图元（`Path` 需要 lyon；`Group` 需要递归） | 渲染管线 | **仍成立（范围限制）** | `ShapeMeshError::Unsupported` 显式报错；后续批次扩展 |
| E-11 | 双槽基线**没有差分视图**（两个孤立值，无法看出"当前值落在哪一槽"） | 对拍工具 | **已关闭（B2-6）** | xtask `render_diff_report`（诊断旁路，不参与门禁）；`diff_report_tests` 3 条 |
| E-12 | 帧级归因缺**分层标签**（无法定位首帧分歧） | 对拍 / 可观测性 | **已关闭（B2-6）** | `ZELA_FRAME_DIGEST_SAMPLE=N` 逐帧 digest 采样；**不进序列哈希链**（D7） |
| E-13 | 脚本看不到 B2 的意图层（`double_sided` / `transparent`） | 脚本 API | **已关闭（B2-7）** | pod `Material3D` 增两字段（D19：不暴露 GPU 机制）；`zela.scene` version 5 → **6** |
| E-14 | 新能力缺展示脚本 | Demo | **已关闭（B2-7）** | `fusion.lua`（双面 + 透明相交体）入册 `demo/manifest.txt`，payload-hash `-` |
| E-15 | 3D **未进 frame hash 对拍网**（B1 遗留） | 对拍 | **已关闭（B2-8）** | `demo3d` 升级为 fixture（Q4 决议）；hashes 覆盖 fourier / storyboard / demo3d；`[browser]` 仍仅存档 |
| E-16 | 能力 API 改了可能**忘记递增 version** | 脚本 API | **已关闭（B2-9）** | `capability_fingerprint`（FNV-1a，跨 Rust 稳定）+ 钉住常量；`cargo verify --step structure` 内跑护栏（变而未递增 ⇒ **fail**） |
| E-17 | 逐帧 digest 旁路**仅 native**（`zela-render` 二进制）；浏览器路径未接 | 可观测性 | **仍成立（范围限制）** | wasm 无环境变量；浏览器归因留待跨端重录（D5） |

**E 段小计（B2-0 ~ B2-9）**：已关闭 **12** · 新增能力 1 · 仍成立/待验证 **4**（E-08 mipmap、E-09 跨端 resolve、E-10 派生桥范围、E-17 旁路仅 native）。

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
