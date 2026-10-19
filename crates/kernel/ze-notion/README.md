# ze-notion

> **理念层（数学元语）** — zela 渲染框架的地基。零 trait object、零后端依赖，
> 仅依赖 [`glam`](https://docs.rs/glam/0.33) 提供高性能 SIMD 友好的线性代数原语。
>
> M1 起本 crate 由 `ze-geometry` 更名而来（四层重构第一刀，见执行计划）：定位从
> "纯几何数据层"提升为**理念层（数学元语）**。
>
> **唯一出口（已收口，2026-10-19）**：本 crate 是全仓唯一的数学元语出口 ——
> glam 仅在此被包装 / 再导出；依赖 `ze-notion` 的 crate **不得再直接声明 glam**
> （CI 门禁 `No direct glam outside ze-notion`）。
>
> 当前仍直接依赖 glam 的只有：
> - 本 crate（定义侧，允许）；
> - 节点图组 `zela-node-editor` / `zela-node-egui` / `zela-node-egui-ui`
>   （**CI 显式豁免**）：它们尚不依赖 `ze-notion`，按 M0 规则不违规；但它们是
>   **第二套数学入口**，合成图（ComposerGraph）接入场景前必须先统一
>   （记账见根目录 `GAPS.md` R-19）。
>
> 根 `Cargo.toml` 的 `glam` 只是 workspace 声明，不是依赖边。

---

## 设计定位

zela 的分层里，本 crate 是最底层：

```
ze-notion (理念层：数学值 + 代数运算)   ← 本 crate
        ▲
zela-scene / zela-render  (场景层 + 渲染抽象 / 队列 / 资源管理)
        ▲
zela-render-backends (wgpu / egui / SVG / 文本后端)
```

**硬约束**：本 crate 绝不引入 `egui` / `wgpu` / `hecs` 或任何项目内部 crate，
保证它可被任何上层工程（WASM、Headless 测试、离线 SVG 录制）安全复用；
且**只依赖 glam（+ 可选 serde）**—— 本 crate 是全仓**唯一允许**直接依赖 glam 的地方
（验收口径：`cargo tree -p ze-notion`）。

---

## 模块一览

| 模块 | 类型 | 说明 |
|------|------|------|
| `vec` | `Vec2` / `Vec3` / `Vec4` / `U8Vec4` / `I8Vec4` | glam 向量类型的再导出（整数向量 M1 从 `zela-scene::types` 收回） |
| `mat` | `Mat3` / `Mat4` / `Quat` | 3×3、4×4 矩阵与四元数 |
| `color` | `Color32` | RGBA8888（`u8` 四通道），`From<(u8,u8,u8,u8)>` / `From<[u8;4]>` |
| `lerp` | `Lerp` | 线性插值 trait（`f32` / `Vec2` / `Vec3` / `Color32` / `Path2D`）；纯函数、零 trait object |
| `space` | `MeasureSpace` / `resolve_width` | 呈现层度量空间：宽度属**世界**（随缩放）还是**屏幕**（不随缩放）；唯一换算入口 |
| `rect` | `Rect2D` | 2D 轴对齐矩形，交/并/包含 |
| `path` | `Path2D` / `PathCommand` / `Path3D` | 2D 路径（MoveTo/LineTo/QuadTo/CubicTo/Arc/Close）与 de Casteljau 折线采样；弧长重采样 / 形变 / 变换；`Path3D` 为 M1 新增的 3D 折线承载类型 |
| `primitives` | `Point3D` / `Line3D` / `Triangle2D` / `Circle2D` / `Ellipse2D` / `Line2D` / `Polyline2D` / `Polygon2D` | 基础图元数据结构 + 纯数学代数（包围盒 / 面积 / 长度 / 包含 / 仿射变换）；2D 值类型为 M1 补齐 |
| `transform` | `Transform2D` / `Transform3D` | 仿射变换包装（数学值），可与 `Mat3`/`Mat4` 互转 |
| `func` | `FunctionX2Y` / `SurfaceFn` / `DynFn` | 一元实函数 `y = f(x)`；参数曲面函数 `z = f(x, y)` + 参数域（纯数学，不含采样步数） |
| `curve` | `ParameterCurveT2XY` / `ParameterCurveT2XYZ` | 参数曲线 `t ↦ 点`（采样动作归场景对象层，不在本 crate） |
| `sample` | `Sampler1D` / `Sampler2D` / `Interpolator` | 采样规格（区间 + 步长）与插值函数包装 |
| `proj` | `directx::perspective` / `orthographic` / `look_at_mat4` | 相机投影 / 视图矩阵构造（右手系 + wgpu/D3D NDC） |

`func` / `curve` / `sample` 三模块的类型 M1 从 `zela-scene::scene::components` 归位 ——
它们本是纯数学，此前却与 `Parent` / `Transform` / `Circle` 等 ECS 组件挤在同一文件里。

> ⚠️ 离散化产物的判据是**动作归属**而非类型归属：`Polyline2D` / `Polygon2D` 的 `points`
> 是定义本身；"对函数采样 100 点得到的折线"才是离散化产物 —— 类型在理念层，
> **采样动作在场景对象层 / 装配层**。


---

## 特性速览

### Color32 — 8 位 RGBA

```rust
use ze_notion::Color32;

let c = Color32::new(255, 128, 0, 255);       // 半透明橙色
let c: Color32 = (100u8, 200, 50, 255).into();
let c: Color32 = [0xABu8; 4].into();

let mixed = Color32::from_hex("#FF6600").unwrap().with_alpha(180);
```

内置常量表：`Color32::BLACK / WHITE / RED / GREEN / BLUE / TRANSPARENT …`

---

### MeasureSpace — 呈现层度量空间

"宽度属于哪个空间"是可声明的属性（仅 2D 描边宽度）：

```rust
use ze_notion::{MeasureSpace, resolve_width};

// Screen：不随缩放变化（默认）
assert_eq!(resolve_width(2.0, MeasureSpace::Screen, 40.0), 2.0);
// World：随缩放线性变化
assert_eq!(resolve_width(2.0, MeasureSpace::World, 40.0), 80.0);
// WorldWithMinPx：世界单位但钳到下限像素
assert_eq!(resolve_width(0.01, MeasureSpace::WorldWithMinPx(1.5), 1.0), 1.5);
```

- 判断标准：**被观测对象的属性 → World；观测工具的属性 → Screen**。
- `resolve_width` 是**唯一换算入口**；真正调用它的地方只有 zela-render 的
  `record_scene`（唯一换算点），其它地方不得乘除 `scale`。
- `scale = 0` 不产生 NaN。
- 纯数据层：`MeasureSpace` 可随 `serde` feature 序列化（`Stroke2D.space` 需要）。

---

### Rect2D — 轴对齐矩形

```rust
use ze_notion::{Rect2D, Vec2};

let r = Rect2D::from_min_max(Vec2::new(0.0, 0.0), Vec2::new(10.0, 5.0));
assert_eq!(r.width(), 10.0);
assert_eq!(r.height(), 5.0);
assert!(r.contains(Vec2::new(5.0, 2.5)));

let r2 = Rect2D::from_center_size(Vec2::new(20.0, 0.0), Vec2::new(4.0, 4.0));
let u: Rect2D = r.union(r2);          // 最小外接矩形
let i: Option<Rect2D> = r.intersect(r2); // None
```

支持 `pad()` / `translate()` / `scale_around()` / `split_left/right/top/bottom`
等常用布局原语。

---

### Path2D — 路径 + 折线采样

```rust
use ze_notion::{Path2D, Vec2};

let mut p = Path2D::new();
p.move_to(Vec2::new(0.0, 0.0))
 .line_to(Vec2::new(1.0, 0.0))
 .cubic_to(Vec2::new(1.0, 0.55), Vec2::new(0.55, 1.0), Vec2::new(0.0, 1.0))
 .arc_to(Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0), 0.0, 0.0, std::f32::consts::FRAC_PI_2)
 .close();

// 按弦高误差 ≤ 0.001 展开成折线点列（扁平：所有子路径首尾相接拼成一条）
let samples: Vec<Vec2> = p.flatten(0.001);

// 需要保留子轮廓边界时（如多轮廓字形 o / B / 8 的描边）：
// flatten_subpaths 按 MoveTo 分组返回，避免在轮廓之间画出横穿的桥接线。
let contours: Vec<Vec<Vec2>> = p.flatten_subpaths(0.001);
```

- Bézier 段使用 **de Casteljau 自适应细分**，在拐点处保持精度、在直线段收敛更快；
- `Arc` 通过弦误差控制生成多段 `LineTo`，避免 `sin`/`cos` 过多；
- 迭代器零拷贝、无堆分配（细分栈用小 `ArrayVec`）；
- `flatten_subpaths` 保留每个子轮廓的独立性：`Close` 会把子路径起点追加到该组尾部
  （组内首尾点相同 = 闭合），消费端据此逐轮廓判定闭合（闭合语义由 `Path2D` 的
  `Close` 命令承载，不再另存 `closed` 标记）。

**弧长 / 形变 / 变换**（S1 插值内核）：

```rust
// 1. 累积弧长表（按 flatten 折线；closed 时补闭合段）
let lens: Vec<f32> = p.arc_lengths(0.05, true);
// 2. 按弧长均匀重采样为 n 点——形变对齐的基础（flatten 的点分布不均）
let pts: Vec<Vec2> = p.resample_arc_length(128, 0.05, true);
// 3. 两条路径按弧长对齐逐点插值：顶点数恒为 samples（与输入复杂度无关）
let q: Path2D = Path2D::morph(&from, &to, /*t=*/0.5, 128);
let pts_mid: Vec<Vec2> = Path2D::morph_points(&from, &to, 0.5, 128);
// 4. 路径整体仿射变换（Arc 会退化为 32 段折线，其余命令保持类型）
let moved: Path2D = p.transformed(&t2d);
```

> 约定：`morph` 输出**闭合折线**，`morph_points` 返回 `samples` 个不同顶点
> （不含重复尾点）。`Lerp` 对 glam 类型需用 UFCS（`<Vec2 as Lerp>::lerp(...)`，
> 因 glam 自带同名固有方法）。

---

### Transform2D / Transform3D

```rust
use ze_notion::{Transform2D, Vec2, Mat3};

let t = Transform2D::identity()
    .then_translate(Vec2::new(10.0, 20.0))
    .then_rotate(std::f32::consts::FRAC_PI_4)
    .then_scale(Vec2::new(2.0, 2.0));

let p = Vec2::new(1.0, 0.0);
let p_world = t.transform_point2(p);
assert!((p_world.x - (10.0 + 2.0 * std::f32::consts::FRAC_1_SQRT_2)).abs() < 1e-4);

let mat3: Mat3 = t.into();
let inv = t.inverse();       // 仿射逆（平移+旋转+正交缩放时解析精确）
```

`Transform3D` 用法同理，内部是 `glam::Affine3A`，支持与 `Mat4`/`Quat`
互转及 `look_at` / `from_scale_rotation_translation`。

---

## 测试

```bash
cargo test -p ze-notion           # 运行 77 个单元测试
cargo doc  -p ze-notion --no-deps # 查看完整 API 文档
```

覆盖点：

- `Color32` — round-trip 转换、`with_alpha`、预设值
- `Lerp` — 各类型端点/中点、区间外线性外插（不 clamp）
- `MeasureSpace` — `Screen` 缩放不变性（核心承诺）、`World` 线性、
  `WorldWithMinPx` 双向钳制、`scale = 0` 不产生 NaN、默认 `Screen`
- `Stroke2D` — 可见性判定、默认构造空间为 `Screen`
- `Rect2D` — 并/交退化、包含边界、split 系列、缩放偏移
- `Path2D` — flatten 闭合/开放路径、`flatten_subpaths` 双轮廓分组
  （外圈 + 内孔各自成组，两组不互相拼线）、圆弧弦误差、Cubic/Quad 端点一致、
  弧长重采样等距（直线 / 圆周误差 < 1e-3）、`morph` 顶点数恒为 samples 且中点不越界、
  `transformed` 端点、空/单点路径不 panic
  （历史：修复过 de Casteljau 切分时 NaN 传播 bug）
- `Transform2D/3D` — 逆矩阵精度、点/方向向量变换、与 `Mat*` 互转
- `Circle2D` / `Ellipse2D` / `Line2D` / `Polyline2D` / `Polygon2D`（M1 新增）—
  包围盒 / 面积 / 长度 / 包含（含边界与退化轴）/ 点到线段最近点与距离 /
  仿射变换（直线与折线保型、多边形面积按行列式缩放、镜像翻转绕序）
- `Path3D`（M1 新增）— 开放 / 闭合长度、包围盒、按弧长均匀重采样（开放含端点、
  闭合绕环均分）、`morph` 顶点数恒为 samples、仿射变换、空 / 单点不 panic
- `FunctionX2Y` / `ParameterCurve*` / `Interpolator`（M1 归位）— 求值、
  `Send + Sync`（可作 ECS 组件的必要条件）、`SurfaceFn` 求值与参数域

---

## 依赖

```toml
[dependencies]
glam = "0.33.6"
```

仅此而已。
