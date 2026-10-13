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
| `rect` | [`Rect2D`](#rect2d-轴对齐矩形) | 2D 轴对齐矩形，交/并/包含/扩展 |
| `path` | `Path2D` / `PathCommand` | 2D 路径（MoveTo/LineTo/QuadTo/CubicTo/Arc/Close）与 de Casteljau 折线采样（`flatten` 扁平 / `flatten_subpaths` 按子轮廓分组） |
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
cargo test -p xc-geometry           # 运行 29 个单元测试
cargo doc  -p xc-geometry --no-deps # 查看完整 API 文档
```

覆盖点：

- `Color32` — round-trip 转换、`with_alpha`、预设值
- `Rect2D` — 并/交退化、包含边界、split 系列、缩放偏移
- `Path2D` — flatten 闭合/开放路径、`flatten_subpaths` 双轮廓分组
  （外圈 + 内孔各自成组，两组不互相拼线）、圆弧弦误差、Cubic/Quad 端点一致
  （历史：修复过 de Casteljau 切分时 NaN 传播 bug）
- `Transform2D/3D` — 逆矩阵精度、点/方向向量变换、与 `Mat*` 互转

---

## 依赖

```toml
[dependencies]
glam = "0.33.6"
```

仅此而已。
