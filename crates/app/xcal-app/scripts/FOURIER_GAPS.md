# Fourier 动画：缺口清单

> 作品：`fourier.lua` / `fourier.py`（傅里叶级数逼近方波 + Gibbs 过冲特写）。
> 渲染：`xcal-render`，640×480 @ 30fps，**243 帧 / 8.1s**，两版 mp4 SHA256 相同。
>
> 结论先说：**旋转向量主题在当前脚本能力下不可表达**。缺的不是图元，而是
> 「旋转 target」与「多段绑定语义」。下面按阻塞程度排列。

---

## P0 — 直接阻塞原定主题

### G1. 同一 target 的多段绑定会从第 0 帧互相覆盖

**现象**：`Timeline` 给每段动画一个 `Hold` 节点（`span=(0,dur)`），段外把局部时间
钳到两端。`BindingApplySystem` 每帧**应用全部绑定**，因此后一段的「段前初值」会从
第 0 帧就写入，覆盖前一段/初始状态。

**实测**：给相机加「55→170（6.5s 起）」再「170→55（8.1s 起）」两段后，第 30 帧
（t=1s）相机已经是 170，全景被裁掉。原因就是第二段的 `from=170` 在段前一直生效。

**后果**：
- 不能做「相机推近 → 停留 → 拉回」；
- 不能用 60 段 `Ramp2` 让点沿曲线走（最后一段的初值从第 0 帧就生效）。

**本轮绕过**：
- 相机只做**一段**推进；
- 曲线游标改为「同几何的亮色重描副本 + 单条 `RevealRange` 0→1」，
  由 `RevealRange` 自己表现"笔尖在走"。

**建议**：给绑定加「生效区间」或按 target 选择当前段（只应用 `start <= t` 的段），
或者在段前**不写**（而不是写初值）。这是复杂编排的前置条件。

参考：[timeline.rs](../../../scene/xcal-scene/src/animation/timeline.rs#L290-L320)。

### G2. 脚本 `Curve` 没有 Orbit，也没有任意关键帧

脚本侧只有 `Const / Sin / Phase / Ramp / Ramp2`（[types.rs](../../../scene/xcal-scene/src/pod/types.rs#L186)）。
原生 `Curve::Orbit` 存在（3D 相机用），但没有暴露给脚本；也没有
`Keyframes(Vec<(t, value)>)` 这类多关键帧入口。

**后果**：圆周运动、旋转向量、任意多段轨迹都无法用一条曲线表达。

**本轮绕过**：主题改为不需要旋转的方波逼近；用 `Reveal` 逐段书写。

**建议**：把 `Orbit` 与通用 `Keyframes` 暴露到脚本 `Curve`，并配
`Vec2` → 位置的 target（配合 G3）。

### G3. 没有 Rotation target；`TransformValue` 的 Scalar 被「缩放」占用

`TargetKind` 只有 8 个（[target.rs](../../../scene/xcal-scene/src/animation/target.rs#L146-L157)）。
`TransformValue` 的映射是：`Vec2/Vec3` → position，`Scalar` → **统一缩放**
（[target.rs](../../../scene/xcal-scene/src/animation/target.rs#L379-L416)）。
`Transform` 组件本身有 `rotation` 字段，但没有 target 写入它。

**后果**：箭头/线段/图形无法旋转。即使有了 `Orbit` 曲线，也没有 target 能消费
"角度"。

**本轮绕过**：不做旋转，只做静态曲线 + 书写。

**建议**：拆出 `RotationValue`（Scalar → `rotation.z`）或让 `TransformValue`
支持 `Vec2` 之外的角度通道。这是"旋转向量"主题的硬前置。

---

## P1 — 表达力缺口（可绕过，但影响作品质量）

### G4. 没有静态折线 / 线段 / 矩形图元；`Reveal` 是唯一点表入口且不读 Transform

脚本层可用的几何只有 `Circle / Arrow / Reveal（折线）/ 内容路径`。
没有"静态线段/矩形/多边形"构造器。`Reveal` 虽然接受点表，但：
- 初态是 `[start=0, end=0]`（[components.rs](../../../scene/xcal-scene/src/scene/components.rs#L120-L128)），
  必须动画才可见；
- `RevealRecordSystem` 只录 `(Reveal, StrokeSolid)`，**不消费 `Transform`**
  （[record_systems.rs](../../../render/xcal-render/src/record_systems.rs#L197-L210)），
  所以 `Reveal` 对象不能被平移/缩放。

**本轮绕过**：曲线全部用 `Reveal` + 逐段书写；曾尝试用竖直 `Arrow` 做扫描线，
但因 G1（多段绑定）最终删除。

**建议**：加一个静态 `Polyline`（或让 `Reveal` 支持 `visible` 初态 + Transform），
并考虑 `Rect` / `Line` 的脚本构造器。`Shape2D::Custom` 可以承载静态新图元，但
**不能**解决 G1/G3（它没有逐帧几何更新通道）。

### G5. `SceneApi` 没有 Opacity 构造器，淡入淡出不可用

`Target.OpacityValue` 存在，但脚本无法给对象挂 `Opacity` 组件
（[api.rs](../../../scene/xcal-scene/src/pod/api.rs#L27-L75) 无 `opacity`）。

**本轮绕过**：标题/Gibbs 标注从第 0 帧就在场（全景里很小，推进后才可读）。

**建议**：补 `scene.opacity(value) -> Handle`。

### G6. 文本只有 `source / font_size / x / y`，没有颜色、对齐、尺寸回读

`Text`（[types.rs](../../../scene/xcal-scene/src/pod/types.rs#L173)）没有颜色/对齐字段；
颜色由内容服务的描边样式决定，脚本只能在 `Transform` 里平移，无法知道文本宽度
做居中/右对齐。

**本轮绕过**：标题/标注统一白色，位置硬编码（`-3.3, 2.75` 等）。

**建议**：`Text` 增加 `color` / `align`，或 `content.text` 返回 `(paths, bounds)`。

### G7. `Axes2D` 没有刻度标签

只有网格/轴线样式（[types.rs](../../../scene/xcal-scene/src/pod/types.rs#L74)），
没有 tick label。傅里叶图缺少 x/y 刻度说明。

**本轮绕过**：用标题文字代替图例；刻度由网格自行表达。

### G8. 没有 ZOrder / 图层控制

脚本不能设置 `ZOrder`；同 z 的叠放顺序由创建顺序决定。重描副本必须后创建才能
压在基础曲线之上。

**本轮绕过**：依赖创建顺序（先基础曲线，后亮色副本）。

### G9. 脚本没有"带时间的运行时创建 / 销毁"

`scene.despawn` 是声明期命令，`commit` 时执行；没有"第 N 秒 spawn/despawn"。
无法做逐帧重建的动态内容（例如动态重算曲线、动态标注）。

**本轮绕过**：所有对象声明期一次建好，只用目标动画改变可见部分。

### G10. 相机没有 fit-to-content 的脚本入口

Rust 侧有 `Camera2D::fit_bounds`，脚本只有 `Camera2D { center, scale }`。
本轮 scale/center 是手调常量。

**建议**：暴露 `fit_bounds` 或在脚本层提供 `camera.fit(bounds)`。

---

## P2 — 工效/一致性

### G11. Python 的 `from` 是关键字，`Arrow` / `Ramp` / `Ramp2` 只能位置传参

与 Lua 的具名参数不一致；两版脚本必须小心保持字段顺序。

**本轮绕过**：Python 侧统一用位置参数，Lua 侧用具名参数（已有 storyboard.py 先例）。

### G12. 没有虚线 / 线帽 / 渐变

`Stroke` 只有 `width + color`。目标方波和参考曲线只能用颜色/线宽区分。

**本轮绕过**：目标方波用灰色粗线，部分和用四种颜色。

### G13. `Reveal` 初态不可见

任何静态折线都必须配一段动画才出现；没有"直接可见"的构造器。

---

## 本轮对 app 装配层的一处必要改动（请确认是否保留）

`build_script_runtime`（headless 脚本入口）原本会先装配 app 自带的 3D demo
（立方体 + 参数曲面），2D 图层在它下面 ⇒ 脚本画面被 3D 物体盖住，且脚本没有
`UiConfig` 写入口可以关掉 3D。

**改动**：`build_script_runtime` 调用 `build_runtime(..., with_demo3d=false)`；
GUI 路径仍为 `true`。未改渲染层 / 调度器 / pod 层。
副作用：`xcal-render storyboard.lua` 的 headless 输出**不再带 3D demo**
（帧数仍为 135 / 4.5s）。若要保持旧画面，可改为 CLI/环境变量开关，或给脚本
增加 `stage.show_3d` 一类的正式入口。

参考：[lib.rs](../../../app/xcal-app/src/lib.rs#L124-L140)。

---

## 最小下一步建议

1. **G1（多段绑定语义）**：所有复杂编排的第一前置。
2. **G2 + G3（Orbit/Keyframes + Rotation target）**：解锁"傅里叶旋转向量"主题。
3. **G4（静态折线 + Transform）**：解锁扫描线、参考线、任意轨迹残留。
4. **G5（Opacity）**：解锁淡入淡出与镜头内的图层切换。
5. **G13 / G6 / G7**：作品级排版与可读性。
