# Fourier 动画：缺口清单（S1–S7 后更新）

> **总账见 [`GAPS.md`](../../../../GAPS.md)**。本文件保留为 fourier 作品的专属快照；
> 跨作品 / 跨层的状态判定以总账为准。
>
> 作品：`fourier.lua` / `fourier.py`（旋转向量 / epicycles 版）。
> 渲染：`zela-render`，640×480 @ 30fps，**216 帧 / 7.2s**；
> 两版 mp4 SHA256 相同。
>
> 本轮修掉了 P0 阻塞项；下面按「已解 / 仍缺」分组。

---

## 已解（本轮落地）

| 缺口 | 解法 | 验证 |
|---|---|---|
| **G1** 多段绑定段前覆盖 | `BindingApplySystem` 按 `start` 升序应用，**段前不写**（`ClockForest::is_before_start`） | `later_segment_is_not_written_before_start`、`fourier_camera_push_pull_uses_segment_semantics` |
| **G3** 没有旋转通道 | `scene.clip_transform(channel, curve)`（B7 起 `Target.TransformValue` 与 `TransformChannel.Auto` 已删除 —— 显式通道是写 `Transform` 的唯一入口） | `transform_rotation_channel_rotates_about_z`、`transform_channels_are_isolated` |
| **G2** 脚本没有 Orbit / Keyframes | 脚本 `Curve.Orbit` / `Curve.Keyframes`；新增原生 `Curve::Orbit2D` | `orbit2d_is_a_circle_in_xy_plane`、`script_orbit_maps_to_2d_orbit`、`script_keyframes_map_to_scalar_keyframes` |
| **G4** 静态折线 / Reveal 不消费 Transform | 脚本 `Polyline`（可见初态）；`RevealRecordSystem` 折算世界仿射 | `reveal_record_consumes_transform`、`fourier` 曲线/脉冲 |
| **G13** Reveal 初态不可见 | `Polyline` 直接 `start=0,end=1` | `fourier` 静态曲线 |
| **G5** 没有 Opacity 构造器 | `scene.opacity(value)` → 原生 `Opacity` 组件 | `fourier` 曲线淡入 |
| **S5** 没有父子变换累乘 / Text 成组 | `Parent` 脚本构造器（句柄引用，commit 解析）+ `TransformHierarchySystem` + `WorldTransforms` 资源；Text 父对象 + 字形子对象 | `transform_hierarchy` 8 条用例、`fourier` 标题组 / 向量链 |
| **S2 配套** tessellate 静默兜底 | 全部 `Shape2D` 变体白名单测试 | `every_shape2d_variant_produces_geometry_or_is_whitelisted` |

### S1 的边界（已定死）

- 同一段 `parallel` 写同一字段：**后声明者胜**（稳定排序），不依赖 HashMap 顺序。
- 段前不写；段后照常写（`Hold` 求值已钳末值）⇒ 顺序段正确、并行段互不干扰。

### S4 的一处计划修正

计划建议「2D 下固定 `Orbit.axis = +Z`」；实测 3D `Orbit` 的基向量就是 `+Z`，
绕 `+Z` 旋转会投影成常量（XY 平面退化）。因此新增 `Curve::Orbit2D`
（`(r cosθ, r sinθ, 0)`），脚本 `Curve.Orbit` 映射到它；3D `Orbit` 与 3D 相机
demo 逐位不变。

---

## 仍缺（本轮记账，未做）

### P1 — 表达力 / 一致性

1. **脚本层 `Curve.Keyframes` 只有标量**（**内核已支持向量**）：
   `ze_time::AnimValue` 已含 `Vec2` / `Vec3` / `Color` / `Path`，`Curve::Keyframes`
   本身也用它；缺口只在脚本入口 —— `ze_scene_api::types::Curve::Keyframes`
   目前是 `Vec<ScalarKey>`，只能写标量。位置/向量的多关键帧编排仍要靠
   `Ramp2`（仅两端）或多段。
2. ~~**`WorldTransforms` 只有 2D 仿射**~~ **已关闭（S1，2026-10-20）**：
   `WorldTransforms` 已换型存 `Transform3D`，2D 消费方经 `get2d` 投影视图读取；
   3D `SceneRecord3DSystem` 已改读 `(WorldTransforms,)`，3D 父子层级已接线。
   零扰动已实证（S0-1 补证，见 `GAPS.md` G-101）。
3. **`MorphRecordSystem` / `ContentTextureRecordSystem` 不消费世界变换**：
   Text（Path）与向量（Arrow/Reveal/Circle）已覆盖，但形变/内容纹理对象在
   `Parent` 下不会跟随。
4. **`Opacity` 不沿 `Parent` 传播**：文本组整体淡入需要给每个字形各挂
   `Opacity`（本轮标题未做整体淡入）。
5. **父子环路错误只写 `TransformHierarchyError` 资源**：测试可读、debug 可查，
   但宿主尚未把它升级为启动/帧错误（headless 不会因环路中止）。
6. **`Reveal` 世界变换在 CPU 侧逐帧折算路径**：大点表（240+ 点）每帧克隆 +
   变换；若成为热点，应把世界矩阵传进 reveal uniform，而不是改几何。

### P2 — 工效

7. **`scene.clip_transform(channel, curve)` 是独立函数**，不是计划里的
   `Target.Transform { channel }` 形态；语义等价，但 API 表面多了一个入口。
8. **Text 仍缺颜色 / 对齐 / 尺寸回读**（G6），标题位置靠硬编码；
   字形独立着色可行（各自 `PathShape`），但没有「整体改色」的批量入口。
9. **`Axes2D` 仍无刻度标签**（G7）。
10. **无 ZOrder 脚本入口**（G8）；重描副本仍靠创建顺序。
11. **无 `fit_bounds` 脚本入口**（G10）；相机取景手调。
12. **Python `from` 关键字**迫使 `Arrow` / `Ramp` / `Ramp2` 用位置参数（G11）。
13. **无虚线 / 线帽 / 渐变**（G12）。
14. **`Parent` 只接受脚本句柄**：没有「按名字/路径重挂父」的运行期 API；
    父对象必须先创建。

### 关于 `Shape2D::Custom`

本轮没有新增图元：静态折线用 `Reveal`/`Polyline`，旋转/缩放/平移用
`Transform` 通道，父子用 `Parent`。缺口是**动画通道 / 层级语义**，不是静态
形状枚举，因此 Custom 不是合适落点（它没有逐帧几何更新通道）。
`Shape2D::Group` 仍返回空几何；白名单测试把它明确列为「不启用」。

### WASM 跨端对拍（第二阶段记账）

1. **未达成项：跨编译目标逐位相同**。
   - native `fourier` = `fd40329f…690d`；浏览器（Chrome WebGPU）= `bec735…1e279c`。
   - **已知可接受**：结构 216/216 一致；仅 **1/216 帧、1 个像素**不同
     （RGB 绝对差 `(43,26,10)`，MAD 0.000086）。不是缺陷、不是回归。
   - **根因**：仍指向 CPU 求值链路的跨目标末位差异（相机/几何变换）；
     `sin/cos` 的具体实现差异**未被坐实**——`f32/f64` 位模式 64 点探针完全一致。
   - **排除项**：FMA 收缩（主段插值 `from=0.0` ⇒ FMA 安全；全仓 `mul_add` 零命中；
     第 103 帧相机静止）；GPU 回读 / WebGPU 光栅化（绘制指纹在 GPU 之前已不同）。
   - **将来若要攻**：提供自己的 `rotation_z(rad)` 绕开 glam 内部三角函数；
     **代价是 native 基线作废并重录**，需重新评估性价比。
   - **判别实验（留档，未执行）**：opt-level 梯度——debug 也分歧 ⇒ libm；
     仅 opt>0 分歧 ⇒ FMA/向量化。
   - **判据与工具**：`zela-playback/examples/fingerprint.rs`
     （`manifest` 结构清单 / `compare` 结构硬失败 + 数值容差），
     阈值与实测值见 `FRAME_HASH_BASELINES.md`。

> **确定性口径**：本仓库谈“确定性”必须分层——同目标可复现（✅ 最硬）、
> 跨端结构一致（✅ 硬判据）、跨端逐位相同（❌ 已知可接受）。
> 详见 `FRAME_HASH_BASELINES.md` 的「确定性口径」一节；三层混用会误判为核心保证退化。

---

## 当前硬验收结果

- `fourier.lua` / `fourier.py`：**216 帧 / 7.2s**，SHA256 相同；
- `storyboard.lua`：**135 帧 / 4.5s**，SHA256 与 S1–S6 前一致；
- 同目标（`--script` vs `--bundle`）：fourier / storyboard 序列哈希均不变；
- 跨端结构：216/216 一致（聚合哈希 `3cbb0cb65ce50842`，结构串无浮点）；
- 跨端逐位：1/216 帧、1 个像素差异（已知可接受，MAD 0.000086）；
- `cargo check --workspace --all-targets`：0 warning；
- `cargo test --workspace -- --test-threads=1`：全绿（含本清单所有新用例）。
