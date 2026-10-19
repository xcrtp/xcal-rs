# 缺口总账（GAPS.md）

> **唯一记账入口**。散落在 `FOURIER_GAPS.md`、`WASM_PORT_SURVEY.md` 与各 crate
> README / `ARCHITECTURE.md` 的「仍缺 / 已知未接线」统一汇总到这里。
>
> 更新：2026-10-19（融合前盘点与收尾）。
> `FOURIER_GAPS.md` 保留为 **fourier 作品快照**；`WASM_PORT_SURVEY.md` 保留为
> **当时的探测证据**，两者正文都注明总账在本文件。
>
> **确定性口径**（详见
> [`FRAME_HASH_BASELINES.md`](crates/app/xcal-app/scripts/FRAME_HASH_BASELINES.md#L117-L128)）：
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
| **G-101** | `WorldTransforms` 只有 2D 仿射；3D 录制只读 local `Transform`，3D 父子层级未接线 | 场景层 → 渲染层 | **升格（原 F-02）** | [`transform_hierarchy.rs`](crates/scene/xcal-scene/src/transform_hierarchy.rs#L33-L52) 仅 `Transform2D`；[`record_systems.rs`](crates/render/xcal-render/src/record_systems.rs#L280-L299) 的 3D 录制 `Reads = ()`，只取 `Transform::matrix()` |

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
| F-01 | 脚本 `Curve.Keyframes` 只有标量 | 脚本层 | **描述不准，已改写** | 内核 [`AnimValue`](crates/kernel/xc-time/src/curve.rs#L16-L23) 已支持 `Scalar/Vec2/Vec3/Color/Path`，`Curve::Keyframes` 也用 `AnimValue`；缺口只在脚本入口 [`pod/types.rs`](crates/scene/xcal-scene/src/pod/types.rs#L259-L275) 的 `Vec<ScalarKey>` |
| F-02 | `WorldTransforms` 只有 2D 仿射 | 场景/渲染 | **升格 P0（G-101）** | 同上 |
| F-03 | `MorphRecordSystem` / `ContentTextureRecordSystem` 不消费世界变换 | 渲染 | 仍成立 | [`record_systems.rs`](crates/render/xcal-render/src/record_systems.rs#L133-L175)、[`L249-L272`](crates/render/xcal-render/src/record_systems.rs#L249-L272) 的 `Reads` 均无 `WorldTransforms` |
| F-04 | `Opacity` 不沿 `Parent` 传播 | 渲染 | 仍成立 | [`draw.rs`](crates/render/xcal-render/src/draw.rs#L482-L487) 只读对象自身组件；无祖先链查询 |
| F-05 | 父子环路只写 `TransformHierarchyError` 资源 | 场景 | 仍成立 | 生产侧仅 [`xcal-playback`](crates/app/xcal-playback/src/lib.rs#L196-L198) 插入默认值；无宿主升级为帧错误 |
| F-06 | `Reveal` 世界变换在 CPU 侧逐帧折算路径 | 渲染性能 | 仍成立 | [`record_systems.rs`](crates/render/xcal-render/src/record_systems.rs#L216-L235) 每帧 `path.transformed(...)`（克隆 + 变换） |
| F-07 | `clip_transform` 是独立函数，不是 `Target.Transform` 形态 | 脚本 API | **可清** | 能力已由 [`pod/api.rs`](crates/scene/xcal-scene/src/pod/api.rs#L79-L83) 提供；计划中的枚举形态从未落地，语义等价且可并行写不同通道 |
| F-08 | Text 缺颜色 / 对齐 / 尺寸回读 | 内容/脚本 | 仍成立 | [`Text`](crates/scene/xcal-scene/src/pod/types.rs#L227-L232) 只有 `source/font_size/x/y`；[`ContentApi`](crates/scene/xcal-scene/src/pod/api.rs#L105-L111) 返回 `PathList`，无排版度量回读 |
| F-09 | `Axes2D` 无刻度标签 | 对象层 | 仍成立 | [`axes.rs`](crates/object/xc-object/src/axes.rs#L200-L221) 只产 `x_ticks/y_ticks/grid_lines/axis_lines`，无文字 |
| F-10 | 无 ZOrder 脚本入口 | 脚本层 | 仍成立 | `ZOrder` 组件存在（[`xc-object`](crates/object/xc-object/src)），[`SceneApi`](crates/scene/xcal-scene/src/pod/api.rs#L27-L93) 无构造器 |
| F-11 | 无 `fit_bounds` 脚本入口 | 脚本层 | 仍成立 | [`Camera2D::fit_bounds`](crates/object/xc-object/src/camera.rs#L117) 已存在；脚本 API 无对应函数 |
| F-12 | Python `from` 关键字迫使位置参数 | 语言适配器 | 仍成立 | 脚本内注明：[`fourier.py`](crates/app/xcal-app/scripts/fourier.py#L18) |
| F-13 | 无虚线 / 线帽 / 渐变 | 渲染 IR | 仍成立 | [`Style2D`](crates/kernel/xc-render-ir/src/shape.rs#L181-L194) 只有 fill/stroke/width/space |
| F-14 | `Parent` 只接受脚本句柄 | 脚本层 | 仍成立 | [`Parent`](crates/scene/xcal-scene/src/pod/types.rs#L75-L80) 为 `Handle`；无按名字/路径重挂 API |

**A 段小计**：可清 1 · 描述不准 1 · 升格 P0 1 · 仍成立 11。

---

## B. WASM 移植探测（`WASM_PORT_SURVEY.md`）

| 编号 | 原项 | 状态 | 证据 / 说明 |
|---|---|---|---|
| W-01 | P1-1 字体按路径读盘 | **可清** | 浏览器只回放 XSB；字形在 native 已烘焙为路径，`xcal-web` 无字体代码 |
| W-02 | P1-2 同步 GPU 回读 | **可清** | [`read_texture_async`](crates/render/xcal-render-backends/src/render_context.rs#L171) |
| W-03 | P1-3 上下文创建 `block_on` | **可清** | [`request_adapter_and_device`](crates/render/xcal-render-backends/src/render_context.rs#L63)；`blocking` feature 隔离 |
| W-04 | P1-4 导出 fs / ffmpeg | 范围外 | 浏览器不导出；XSB 路线绕开 |
| W-05 | P1-5 `xcal-gui` 关不掉下层默认 features | **降格** | `xcal-gui` 不进浏览器 ⇒ 与浏览器无关，仅工效项 |
| W-06 | P2-1 `gpu_test` 的 libc / sleep / spawn | **可清** | 归入 `blocking` feature；wasm 构建不编译 |
| W-07 | P2-2 `Receipt::wait` 忙等 | **可清** | [`DriverPod::submit`](crates/scene/xcal-scene/src/driver.rs#L200-L207) 同步跑完 `run_frames` 后 `complete()`，`yield_now` 走不到 |
| W-08 | P2-3 CI wasm gate 未覆盖 | **可清** | [`.github/workflows/architecture.yml`](.github/workflows/architecture.yml) 的 Playback runtime gate |
| W-09 | P2-4 `xc-pod-py` wasm | 范围外 | 浏览器不启用 Python Pod |

**B 段小计**：可清 6 · 降格 1 · 范围外 2。

---

## C. README / ARCHITECTURE 散落项

> 盘点结果：真正带「仍缺 / 已知未接线」正文的 README 是 **6 个**
> （`xc-time`、`xc-scene-contract`、`xcal-render`、`xc-pod-lua`、`xc-pod-py`），
> 其余 README 没有缺口小节；`ARCHITECTURE.md` 另有一张「已知未接线项」表。

| 编号 | 缺口 | 来源 | 状态 | 证据 / 说明 |
|---|---|---|---|---|
| R-01 | `Curve::PathMorph` 已就绪未接线 | `xc-time` README | 仍成立 | 只有定义与单测（[`curve.rs`](crates/kernel/xc-time/src/curve.rs#L151)），无 `render/`、`app/` 引用 |
| R-02 | `ObjectId::is_stale` 无生产消费方 | `xc-scene-contract` README | 仍成立 | 仅单测；销毁-重建流程未引入 |
| R-03 | `record_scene` 不消费 `Descriptors` | `xc-scene-contract` README | 仍成立 | 消费方是构造期 `apply`；录制层直接吃 `Style2D` |
| R-04 | `Shape2D` 无 `Text/Image` 变体 | `xcal-render` README | 仍成立 | 内容以路径形式进入 `Path/Polyline` |
| R-05 | `Parent` 零生产消费方 | `xc-scene-contract` README / `ARCHITECTURE.md` | **可清（文档已修）** | S5 已接线：[`transform_hierarchy.rs`](crates/scene/xcal-scene/src/transform_hierarchy.rs)、[`record_systems.rs`](crates/render/xcal-render/src/record_systems.rs#L86) |
| R-06 | `Path3D` 零消费方 | `ARCHITECTURE.md` | 仍成立 | 与 `Path2D` 不同构，接入是破坏性变更 |
| R-07 | `glam::` 未完全收敛到 `xc-notion` | `ARCHITECTURE.md` | 仍成立 | 2026-10-18 实测 189 处；`xcal-scene` 已归零 |
| R-08 | 节点编辑器冻结（已排期） | `ARCHITECTURE.md` | 仍成立（排期项） | 零生产消费方；等 UI 接口稳定 |
| R-09 | 2D / 3D 录制 System 分居 | `ARCHITECTURE.md` | 仍成立 | 现已同住 `xcal-render::record_systems`，但仍是两个 System / 两个队列 |
| R-10 | CPU 回放路径不支持真合成 | `ARCHITECTURE.md` | **可清（文档已修）** | 旧 `egui_2d` 已在 S4.1 删除（[`xcal-egui` README](crates/ui/xcal-egui/README.md#L140-L145)） |
| R-11 | `formula.rs` 归位未定 | `ARCHITECTURE.md` | 仍成立 | 归位目标 crate 有争议 |
| R-12 | `tessellate` 下沉未做 | `ARCHITECTURE.md` | 仍成立 | 目前与 2D 后端同层 |
| R-13 | 非原生 Pod 的 `FunctionX2Y` 适配器 | `ARCHITECTURE.md` | 仍成立 | 脚本只承担装配期，不参与每帧采样 |
| R-14 | 每层多一次全屏 blit | `ARCHITECTURE.md` | 仍成立 | 层数少时可接受 |
| R-15 | 3D 离屏无 MSAA（1x） | `ARCHITECTURE.md` | 仍成立 | `wgpu_3d` / `compositor` 的 `sample_count: 1` |
| R-16 | `opacity × blend` 未穷举验证 | `ARCHITECTURE.md` | 仍成立 | 测试覆盖缺口 |
| R-17 | 3D 无深度缓冲 | `ARCHITECTURE.md` | **可清（文档已修）** | [`DEPTH_FORMAT = Depth32Float`](crates/render/xcal-render-backends/src/wgpu_3d.rs#L98) 且两个 pass 都挂深度附件 |
| R-18 | `xc-pod-lua` / `xc-pod-py` wasm 未做 | 两个 README | 范围外 | 浏览器只回放 XSB，不运行脚本；不进 wasm gate |
| R-19 | 节点图组直接依赖 glam（数学入口第二套）：`xcal-node-editor`(10) / `xcal-node-egui`(6) / `xcal-node-egui-ui`(1) | 节点图组 | 仍成立（不违规，记账） | 三者不依赖 `xc-notion`，按 M0 不违规；CI 已显式豁免并注明理由。**触发条件：合成图（ComposerGraph）接入场景前，必须先统一到 `xc-notion`**；此前不改代码（当前零消费方，避免为未发生的需求付代价） |

**C 段小计**：可清 3（含 2 条纯文档漂移 + `Parent`）· 仍成立 14 · 范围外 1 ·
排期 1（计入仍成立）。

---

## 统计与维护约定

| 类别 | 数量 |
|---|---|
| 升格为 P0（融合前置） | 1（F-02 / G-101） |
| 可清 | 10（A 1 + B 6 + C 3） |
| 描述不准，已改写 | 1（F-01） |
| 降格 | 1（W-05） |
| 仍成立 | 26（A 11 + C 14 + 排期 1） |
| 范围外 | 3（B 2 + C 1） |
| 待定 | 1（统一相机 / 深度合成模型） |

共 **42 条**：A 段 14（作品）+ B 段 9（WASM 探测）+ C 段 19（README/ARCHITECTURE）；G-101 是 F-02 的升格形态，不重复计数。

维护约定：

- 新增缺口必须先落到本文件，再在 crate README 留一行指针；不得只写 README。
- 「可清」必须附代码位置；无法给出证据的标「待定」，不得为了清而清。
- 每次清理后同步更新 `WASM_PORT_SURVEY.md` 的状态列与本文件统计。
