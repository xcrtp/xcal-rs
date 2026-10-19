# ze-render-ir

> **渲染 IR 层** — 2D 图元的**中间表示**：`Shape2D`（逻辑图元）+ `Style2D`（绘制样式）
> + 样式组件 + 「对象 → IR」的降级 trait（`ToShape2D` / `ResolveStyle`）。
> 只依赖理念层，可编译到 `wasm32`。

---

## 设计定位

```
ze-notion          理念层      数学值 + 代数运算（Circle2D / Path2D / Transform2D / …）
ze-render-ir       渲染 IR     Shape2D / Style2D / ToShape2D / ResolveStyle   ← 本 crate
ze-scene-contract  契约层      Scene / ObjectId / Parent / Describable
ze-object          场景对象层  Circle / Camera2D / Axes2D / …（实现 ToShape2D）
zela-render        渲染管线    三角化（lyon）/ 合批 / DrawQueue
```

**硬约束（与理念层同级）**：

- ❌ `hecs` / `egui` / `wgpu` / 任何 `zela-*` / `ze-object` —— 禁止；
  验收：`cargo tree -p ze-render-ir | grep -E "zela-|ze-object"` = 0；
- ✅ `ze-notion` —— 唯一依赖。

**为什么三角化不在这里**：`Shape2D → 顶点` 是**后端侧动作**（lyon 属实现细节，
会拖入重依赖），故留在 `zela-render::tessellate`。IR 层只声明"画什么"，不声明"怎么变成顶点"。

---

## 模块一览

| 模块 | 类型 | 说明 |
|------|------|------|
| `shape` | `Shape2D` | 逻辑图元枚举（`Empty` / `Line` / `Polyline` / `Path` / `Circle` / `Ellipse` / `Rect` / `Polygon` / `Group` / `Custom`）+ `bounds()` |
| `shape` | `Style2D` | 绘制样式：`fill` / `stroke` / `stroke_width` / `space: MeasureSpace` |
| `shape` | `CustomShapeData` / `SpecId` / `CustomSpec` / `CustomSpecs` | 自定义图元：`CustomShapeData = SpecId + bounds`（可序列化）；几何函数在**具名规格表** `CustomSpecs` 里按名字查 |
| `style` | `StrokeSolid` / `FillSolid` | **样式组件**（放进场景的载荷：颜色 + 宽度 + 宽度所属空间）|
| `style` | `StrokeStyle` / `FillStyle` | 样式枚举（当前只有 `Solid`）|
| `style` | `ResolveStyle` | **唯一换算点**：`resolve_style(scale) -> Style2D` |
| `style` | `fill_style` / `stroke_style_raw` | 队列便捷构造：**不做**换算（调用方给的已是屏幕空间量）|
| `degrade` | `ToShape2D` | 对象 → **屏幕空间** `Shape2D`（吃相机降级出的**仿射矩阵**）|
| 根 | `MeasureSpace` | 转发 `ze-notion`：宽度属**世界**（随缩放）还是**屏幕**（不随缩放）|

---

## 特性速览

### `ToShape2D` 吃仿射、不吃相机 —— 这是"解开成环的钥匙"

```rust
pub trait ToShape2D {
    fn to_shape2d(&self, t: &Transform2D) -> Shape2D;   // Transform2D = 仿射矩阵（理念层）
}
```

- `Camera2D` 在**场景对象层**（`ze-object`），而 `ze-object` 依赖本层 ⇒ IR 层认识相机会**成环**；
- 于是接口降维到理念层：**相机在调用点降级成仿射矩阵**
  （`Camera2D::to_transform2d()`），IR 只吃矩阵：

```
screen.x = rect.center.x + (world.x - center.x) * scale
screen.y = rect.center.y - (world.y - center.y) * scale      // Y 翻转
  ⇒ col0 = [ s,  0, 0 ] ; col1 = [ 0, -s, 0 ] ; col2 = [ rc.x - c.x·s, rc.y + c.y·s, 1 ]
```

- 矩阵线性部分是 `diag(s, -s)`（`s = Camera2D.scale`），故实现方要从它取回**统一缩放因子**
  （圆的半径、`Path2D` 椭圆弧的 radii 需要它）时，读 `t.matrix.x_axis.x` 即可；
- **`impl ToShape2D for Circle/Line/Path/Rect` 在 `ze-object`**：trait 在本层、图元在对象层，
  只有"实现里出现本地类型"才合法（**孤儿规则**）。

### `ResolveStyle` — 唯一换算点

```rust
pub trait ResolveStyle { fn resolve_style(&self, scale: f32) -> Style2D; }

// 世界空间宽度随缩放，屏幕空间恒定：
StrokeSolid::new_in(2.0, RED, MeasureSpace::World).resolve_style(10.0).stroke_width == 20.0;
StrokeSolid::new(2.0, RED).resolve_style(10.0).stroke_width == 2.0;   // 默认 Screen
```

⚠️ **纪律**：描边宽度**只在这里**按 `scale` 解析为屏幕像素。项目中其它任何地方都**不得**
再乘除 `scale` —— 否则会出现"某些描边乘过、某些没乘"的不一致。
`fill_style` / `stroke_style_raw` 是给"已经是屏幕空间量"的队列便捷方法，**不换算**。

### `Shape2D` — 变体形态**冻结**，且**全部可序列化**

`Circle { center, radius }` 这类**扁平字段形态**是后端（`wgpu_2d` / `tessellate`）
**源级依赖**的契约：改名 / 改形会连带改后端源码，**禁止**。
新增能力走 `#[non_exhaustive]` 之外的路径：要么加**新变体**（后端 `_ =>` 落到兜底），
要么用 `Shape2D::Custom(CustomShapeData)`。

`Shape2D` 是 ZSB 装配产物的硬前提：**所有变体都 `Serialize + Deserialize`**。
`Custom` 不再携带函数指针，只带**可寻址的规格名**：

```rust
// 函数指针只住在具名规格表里（编译期常量）：
static SPECS: &[CustomSpec] = &[
    CustomSpec::new("my.glyph").with_path(my_glyph_path),
];
let specs = CustomSpecs::new(SPECS);

// 图元只记名字 + 包围盒（可跨进程 / 跨运行时）：
let data = CustomShapeData::new(SpecId::new("my.glyph"), bounds);
let shape = Shape2D::Custom(data);

// 三角化：按名字查表；未注册 ⇒ 显式 `TessellateError`（不是静默空几何）
let items = tessellate_with(&shape, &t, &style, tolerance, &specs)?;
```

> **为什么不是注册序号**：编译器与回放端是两个独立二进制；序号一旦错位，
> 同一个 ID 会指向不同函数 ⇒ 画面错乱且不报错。名字是唯一稳定身份。

### 与 `Describable`（契约层）的分工

- **本层**给的是**样式实例**（`StrokeSolid` / `FillSolid`）与**唯一换算点**；
- **契约层** `ze-scene-contract::Describable` 给的是**声明**（`StrokeDesc` / `FillDesc`
  + 主题默认 + 槽位裁剪），`apply` 产出的 `Style2D` **未换算**，仍交由本层的换算点解析。

---

## 测试与验收

```bash
cargo test  -p ze-render-ir
cargo doc   -p ze-render-ir --no-deps

cargo tree -p ze-render-ir | grep -E "zela-|ze-object"          # = 0
cargo check -p ze-render-ir --target wasm32-unknown-unknown     # kernel 门禁
```

覆盖：`Shape2D::bounds`（线 / 圆 / 空图元 / 分组取并 / 自定义图元的声明 bounds）、
`Shape2D` 全变体的 serde 往返与 `Custom` 只写规格名、`CustomSpecs` 的精确名字查找、
`StrokeSolid` 两参构造默认 `Screen`、
`ResolveStyle` 的"World 按 scale 放大 / Screen 不变"（唯一换算点）、`FillSolid` 解析后无描边。

---

## 依赖

```toml
[dependencies]
ze-notion = { path = "../ze-notion" }
```

仅此一个 —— 无 hecs / egui / wgpu / 任何 `zela-*`。
