# xcal-script

> **xcal-rs 的第一个非原生 VM**：Lua 5.4 脚本以**装配期**方式编排场景。
> 脚本只产出 `SceneScript`（对象 + 时序），不直接碰 ECS / 渲染 / GPU。
>
> 本 crate 同时是 **`xc-vm` 注入机制的第二次真实使用**：
> `scene`（场景装配）与 `content`（公式 / 文字编译）是两组独立能力，
> 经同一个 `Injector` 注入。

---

## 在架构中的位置

```
kernel:  xc-vm / xc-vm-macro / xc-content / xc-prerender
                    ▲
                    │
scene:        xcal-scene  ◄──  xcal-script   ← 本 crate
                    ▲                ▲
                    │                │
render:        xcal-render      （不依赖）
                    ▲
app:            xcal-app  ──► ScriptHost::with_engine(...) / run / commit
```

依赖方向是**单向**的：

- `xcal-script` → `xcal-scene`：执行 `SceneEncoder`、复用 `TargetKind` / `Curve`；
- `xcal-script` → `xc-content` / `xc-prerender`：内容编译（kernel 层，轻依赖）；
- `xcal-script` **不依赖** `xcal-render`：scene 层不得反向依赖 render 层；
- `xcal-app` → `xcal-script`：装配 `ScriptHost`，把 App 建好的排版引擎传进来。

---

## 两个方向

```text
脚本 → 宿主：装配期。Lua 里调 scene.* / content.*；
             宿主侧是原生 Rust Service，经 Injector 注入。
宿主 → 脚本：Vm::call("xcal.script::run", [source])（erased 慢路径）。
```

| 入口 | 形式 | 用途 |
|------|------|------|
| `ScriptHost::with_engine(engine)` | 原生 | 装配两个 Service、校验需求、注入 |
| `ScriptHost::run(source)` | 原生 | 执行 Lua，取回 `SceneScript` |
| `Vm::call("xcal.script::run")` | erased | 不依赖 `xcal-script` 类型的宿主 |
| `xcal.script::run` | Lua 可见 | 仅作为 VM 的对外函数签名 |

内部交接函数 `xcal.scene::__reset` / `__finish` 不是脚本可见 API，只走 `Vm::call`。

---

## 能力面

### `scene`（`SceneApi`，16 个函数）

| 函数 | 签名 | 说明 |
|------|------|------|
| `circle` | `(Circle) -> Handle` | 圆组件 |
| `stroke` | `(Stroke) -> Handle` | 描边组件（屏幕空间） |
| `camera2d` | `(Camera2D) -> Handle` | 2D 取景组件 |
| `reveal` | `(Reveal) -> Handle` | 书写组件（点表） |
| `axes2d` | `(Axes2D) -> Handle` | 坐标网格容器（`AxesRefreshSystem` 每帧生成几何） |
| `arrow` | `(Arrow) -> Handle` | 箭头图元 |
| `fill` | `(Fill) -> Handle` | 实心填充样式 |
| `transform` | `(Transform) -> Handle` | 逐对象摆放（当前只暴露位置） |
| `reveal_path` | `(RevealPath) -> Handle` | **组件组**：`Reveal` + `StrokeSolid` |
| `path_shape` | `(PathShape) -> Handle` | **组件组**：`Path` + `StrokeSolid` |
| `spawn` | `(Array<Handle>) -> Handle` | 创建对象（组件组在此展开） |
| `despawn` | `(Handle) -> ()` | 销毁对象 |
| `clip` | `(Target, Curve) -> Handle` | 动画动作：**目标枚举** + **曲线枚举** |
| `animate` | `(Handle, Handle, f64) -> ()` | 顺序播放一段动画 |
| `parallel` | `(Handle, Handle) -> ()` | 与上一段同区间的额外动作 |
| `wait` | `(f64) -> ()` | 推进时序游标 |

### `content`（`ContentApi`，2 个函数）

| 函数 | 签名 | 说明 |
|------|------|------|
| `formula` | `(Formula) -> Array<Path>` | Typst 公式 → 描边路径，**按阅读顺序** |
| `text` | `(Text) -> Array<Path>` | 纯文本 → 描边路径（正文外观） |

返回的每条 `Path` 携带内容服务解析出的 `color` / `width`，所以
`scene.spawn({ RevealPath { path = p } })` 不需要在脚本里重述样式。

---

## 类型词汇（`#[derive(VmType)]` 一处定义）

| 类型 | 字段 |
|------|------|
| `Color` | `r, g, b, a: u8` |
| `Vec2` | `x, y: f32` |
| `Circle` | `x, y, r` |
| `Stroke` | `width, color` |
| `Camera2D` | `center: Vec2, scale` |
| `Reveal` | `points: Array<Vec2>, closed: bool` |
| `LineStyle` | `width, color`（**纯值**，与组件 `Stroke` 区分） |
| `Axes2D` | `min_px, grid: LineStyle, axis: LineStyle` |
| `Arrow` | `from, to: Vec2, head_length, head_width` |
| `Fill` | `color` |
| `Transform` | `x, y, z` |
| `Path` | `path: Handle, color, width` |
| `RevealPath` / `PathShape` | `path: Path` |
| `Formula` / `Text` | `source: String, font_size, x, y` |
| `Curve` | 枚举：`Const / Sin / Phase / Ramp / Ramp2` |
| `Target` | 枚举：8 个 unit 变体（见下） |

`Curve` 是一个枚举的五个变体，取代了早期的 `sin` / `phase` / `constant` /
`ramp` / `ramp2` 五个函数；`Ramp` / `Ramp2` 的 `dur` 是 `f64`，
与 `Timeline` 的时长同精度（`1/N` 不会在 f32 上掉一个 ulp）。

`Target` 的真源在 `xcal-scene` 的 `TargetKind`（同一枚举既是持久化名字表，
也是脚本绑定名）。Lua 侧 8 个 unit 变体是**值**：

```lua
Target.CircleRadius  Target.Phase         Target.CameraEye  Target.MorphT
Target.Camera2DFraming  Target.RevealRange  Target.OpacityValue  Target.TransformValue
```

拼错变体（如 `Target.RevealRage`）在绑定层立即报错，不会静默不排期。

---

## 绑定生成

```rust
#[capability(namespace = "xcal.scene", version = "3", bindings)]
pub struct SceneApi<'a> {
    pub circle: fn(&'a dyn Vm, shape: Circle) -> Handle,
    // ...
}
```

- `#[capability(bindings)]` 生成 `calls()`（`FnIo` + 类型擦除 trampoline）；
- `#[derive(VmType)]` 生成 `TypeSpec` + 双向转换 + `IntoVmTypeName`；
- `binding.rs` 按 `calls()` 自动生成 `scene.*` / `content.*` 与类型构造器：
  - 单 `Struct` 参数 + `Opaque` 返回 ⇒ **组件构造器**（`Circle { ... }`）；
  - `Enum` 中无字段的变体 ⇒ **值**（`Target.RevealRange`）；
  - 有字段的变体 ⇒ 构造器函数（`Curve.Phase { omega = 0.5 }`）；
  - `Array<Struct>` ⇒ 值构造器 + 数组。
- 新增一个 capability 函数**不需要改 `binding.rs`**；注册名与 `signatures()`
  由用例强制一致。

`spawn` 的参数是 `Array<xc_vm.Handle>`（异构组件），协商名由
`impl IntoVmTypeName for Vec<Handle>` 给出，与 schema 的 `Array<Opaque>` 一致——
不再需要 `#[vm_type(param0 = "ComponentList")]` 这类字符串 override。

---

## 组件组与句柄种类

- 一个组件句柄可以对应**多个组件**（`RevealPath` = `Reveal` + `StrokeSolid`），
  `spawn` 时展开；
- 组件 / 动作 / 对象三张句柄表各有独立的 owner `VmId`：
  把对象句柄当组件传给 `spawn` 会得到明确错误（`run` 失败），
  而不是撞上同槽位的组件；
- 每一轮 `run` 前重置三张句柄表与内容路径表（旧句柄全部失效）。

---

## 内容服务

- 公式 / 文字编译与 App 的 `xcal-app::formula` **逐式一致**（同一份
  `xc-content` / `xc-notion` 原语）；
- 排版引擎由 App 建一次后经 `ScriptHost::with_engine(Arc<InMemoryEngine>)` 传入；
- `PathStore` 是两个 Service 共享的路径表（owner 是表自己的 `VmId`），
  场景服务读它来构造 `Reveal` / `Path` 组件；
- 公式按包围盒 `min_x` 升序（阅读顺序），文字保持引擎顺序。

---

## 分镜脚本

[`scripts/storyboard.lua`](scripts/storyboard.lua) 覆盖现有分镜的**全部 6 个镜头**
（坐标轴、相机、曲线书写、wait、镜头推进、公式逐轮廓书写）以及 T2 箭头 / T3 标注文字。

`xcal-app` 的默认启动路径仍用硬编码 `storyboard.rs`（参考实现）；
`--script` 走本 crate。两者的等价性由
`xcal-app` 的 `script_storyboard_matches_hardcoded` 逐对象、逐绑定锁定
（占位引擎下 25 对象 / 9 绑定）。

```lua
local camera = scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 40.0 },
})
local curve = scene.spawn({
    Reveal { points = points, closed = false },
    Stroke { width = 2.0, color = Color { r = 0, g = 200, b = 200, a = 255 } },
})
scene.animate(curve, scene.clip(Target.RevealRange, Curve.Phase { omega = 0.5 }), 2.0)

local paths = content.formula(Formula { source = "y = sin x", font_size = 0.8, x = 1.0, y = 2.0 })
for i = 1, #paths do
    scene.spawn({ RevealPath { path = paths[i] } })
end
```

---

## 文档导出

```bash
cargo run -p xcal-script --example export_scene_api
```

产物在 `target/scene_api/`：

- `scene_api.json`：**唯一真源**，结构为
  `{ "api": "xcal.script", "capabilities": { "scene": {...}, "content": {...} }, "types": [...] }`；
- `scene_api.lua`：LuaLS 注解，从同一份 `calls()` 生成（形参名来自 schema，
  不再是 `p1..pN`；unit 枚举变体是值）。

---

## 测试

```bash
cargo test -p xcal-script
cargo test -p xcal-app script_storyboard_matches_hardcoded -- --nocapture
cargo run -p xcal-app -- --script
```

`tests/lua_scene.rs` 覆盖：

- 脚本子集 ⇔ 手写 `SceneEncoder`（结构 / 时序 / 组件值 / 路径包围盒）；
- 双向对接（宿主调 `run`、脚本调 `scene.*`）；
- `scene` / `content` 注册名 == `signatures()`；
- 文档完整性（每个函数都有 `///`）与导出结构（结构体 / 枚举 / unit 变体）；
- 拼错 target 变体、非法组件句柄、未注入 —— 三条明确错误通道。

---

## 已知边界

| 项 | 现状 |
|----|------|
| `Transform` | 只暴露位置；旋转 / 缩放沿用默认 |
| `Axes2D` | 暴露 `min_px` + 两个线样式；定义域固定为 `unbounded()` |
| `Path` 句柄 | 仅在本轮 `run` 内有效（路径表每轮重置） |
| `xcal-script` 的 wasm | 未做（`mlua` 的解释器不适合 wasm 内核路径） |
| Script → 节点图读回 | 未做（脚本产出的是 `SceneScript`，不是可编辑节点） |
| 内容桥接 | 与 `xcal-app::formula` 各有一份（分层限制，见上） |
