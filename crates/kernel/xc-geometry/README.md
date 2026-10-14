# xc-geometry

> **纯几何数据层** — xcal-rs 渲染框架的地基。零 trait object、零后端依赖，
> 仅依赖 [`glam`](https://docs.rs/glam/0.33) 提供高性能 SIMD 友好的线性代数原语。

---

## 设计定位

xcal-rs 的三层渲染架构里，本 crate 是最底层：

```
xc-geometry (纯数据 + 纯函数算法)  ← 本 crate
        ▲
xcal-render  (trait / 队列 / 资源管理)
        ▲
xcal-render-backends (wgpu / egui / SVG / 文本后端)
```

**硬约束**：本 crate 绝不引入 `egui` / `wgpu` / `hecs` 或任何项目内部 crate，
保证它可被任何上层工程（WASM、Headless 测试、离线 SVG 录制）安全复用。

---

## 模块一览

| 模块 | 类型 | 说明 |
|------|------|------|
| `vec` | `Vec2` / `Vec3` / `Vec4` | glam `Vec2/Vec3A/Vec4` 的再导出 |
| `mat` | `Mat3` / `Mat4` / `Quat` | 3×3、4×4 矩阵与四元数 |
| `color` | `Color32` | RGBA8888（`u8` 四通道），`From<(u8,u8,u8,u8)>` / `From<[u8;4]>` / 透明度混色 |
| `lerp` | `Lerp` | 线性插值 trait（`f32` / `Vec2` / `Vec3` / `Color32` / `Path2D`）；纯函数、零 trait object |
| `space` | `MeasureSpace` / `resolve_width` | 呈现层度量空间：宽度属**世界**（随缩放）还是**屏幕**（不随缩放）；唯一换算入口 |
| `rect` | [`Rect2D`](#rect2d-轴对齐矩形) | 2D 轴对齐矩形，交/并/包含/扩展 |
| `path` | `Path2D` / `PathCommand` | 2D 路径（MoveTo/LineTo/QuadTo/CubicTo/Arc/Close）与 de Casteljau 折线采样；弧长重采样 / 形变 / 变换 |
| `transform` | `Transform2D` / `Transform3D` | 仿射变换包装（平移 × 旋转 × 缩放），可与 `Mat3`/`Mat4` 互转 |
| `primitives` | `Point3D` / `Line3D` / `Triangle2D` | 基础图元数据结构 |

---

## 特性速览

### Color32 — 8 位 RGBA

```rust
use xc_geometry::Color32;

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
use xc_geometry::{MeasureSpace, resolve_width};

// Screen：不随缩放变化（默认）
assert_eq!(resolve_width(2.0, MeasureSpace::Screen, 40.0), 2.0);
// World：随缩放线性变化
assert_eq!(resolve_width(2.0, MeasureSpace::World, 40.0), 80.0);
// WorldWithMinPx：世界单位但钳到下限像素
assert_eq!(resolve_width(0.01, MeasureSpace::WorldWithMinPx(1.5), 1.0), 1.5);
```

- 判断标准：**被观测对象的属性 → World；观测工具的属性 → Screen**。
- `resolve_width` 是**唯一换算入口**；真正调用它的地方只有 xcal-render 的
  `record_scene`（唯一换算点），其它地方不得乘除 `scale`。
- `scale = 0` 不产生 NaN。
- 纯数据层：`MeasureSpace` 可随 `serde` feature 序列化（`Stroke2D.space` 需要）。

---

### Rect2D — 轴对齐矩形

```rust
use xc_geometry::{Rect2D, Vec2};

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
use xc_geometry::{Path2D, Vec2};

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
  （组内首尾点相同 = 闭合），消费端据此逐轮廓 `closed` 标记。

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
use xc_geometry::{Transform2D, Vec2, Mat3};

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
cargo test -p xc-geometry           # 运行 46 个单元测试
cargo doc  -p xc-geometry --no-deps # 查看完整 API 文档
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

---

## 依赖

```toml
[dependencies]
glam = "0.33.6"
```

仅此而已。
