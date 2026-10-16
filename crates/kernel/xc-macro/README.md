# xc-macro

> **xcal-rs 的过程宏支撑库**。当前提供 `#[derive(Transform2DInto)]`，
> 为 `xcal-scene::transform` 模块提供零样板的 2D 屏幕↔逻辑坐标转换派生。

---

## 在架构中的位置

```
xc-macro (proc-macro)
        │ 被 xcal-scene 通过 extern crate self as xcal_scene 引用
        ▼
xcal-scene（Transform2D / Transform2DInto trait 系统）
        ▲
xcal-render / xcal-egui / xcal-app
```

用户通常**不直接**依赖本 crate，而是通过 `xcal-scene` 间接获得派生宏：

```rust
use xcal_scene::Transform2DInto;   // trait（定义于 xcal_scene::transform）
use xc_macro::Transform2DInto;     // 派生宏
```

注意两者**同名但不同物**：`xcal_scene::Transform2DInto` 是位于
`xcal_scene::transform` 的 trait，而 `xc_macro::Transform2DInto` 是过程宏。
派生时需分别导入（或写全路径 `#[derive(xc_macro::Transform2DInto)]`）。

---

## 可用宏

### `#[derive(Transform2DInto)]`

为结构体自动派生 `xcal_scene::transform::Transform2DInto`（消耗 `self` 的版本）。
非消耗版本的 `Transform2D` trait 由 blanket impl 自动提供（要求类型
`impl Clone`）。

#### 字段属性 `#[transform(...)]`

| 属性 | 效果 |
|------|------|
| 默认（无属性） | 对该字段调用 `Transform2DInto::to_logical_into` / `::to_screen_into` |
| `#[transform(skip)]` | 直接 move 字段，不做任何变换（适合颜色、z-index 等与相机无关字段） |
| `#[transform(iter)]` | `self.field.into_iter().map(|v| v.to_*_into(camera)).collect()`，适合 `Vec`/数组 |

`skip` 与 `iter` 互斥；同一字段同时写会产生编译期错误。

#### 支持的结构体形式

| 形式 | 示例 |
|------|------|
| 具名字段结构体 | `struct Point { x: f32, y: f32 }` ✅ |
| 元组结构体 | `struct Pair(u64, u64)` ✅ |
| 单元结构体 | `struct Marker;` ✅（两方法都返回 `Self`）|
| 泛型结构体 | 自动在 `where` 中为默认字段追加 `T: Transform2DInto` 约束 |
| enum / union | ❌ 编译期 panic（`Transform2DInto` 仅用于 struct）|

---

### 使用示例

```rust
// trait 仍在场景层（`Transform2DInto` 是 `xcal-scene` 的本地 trait，
// 见 M4 计划 §1.6：它已不依赖场景层向量的具体身份，无需搬家）；
// 但方法签名里的 `Camera2D` 归**场景对象层** `xc-object`（M3-2 归位）。
use xcal_scene::{Transform2D, Transform2DInto};
use xc_object::Camera2D;
use xc_macro::Transform2DInto;

#[derive(Clone, Transform2DInto)]
struct Vertex {
    pos:    (f32, f32),                 // 默认：递归变换
    #[transform(skip)]
    color:  [u8; 4],                    // 跳过
    #[transform(iter)]
    extras: Vec<(f32, f32)>,            // 逐个元素变换
}

let cam = Camera2D::default();           // identity
let v = Vertex { pos: (1.0, 2.0), color: [255;4], extras: vec![(3.0, 4.0)] };

// 消耗版本
let logical = v.to_logical_into(&cam);
// 非消耗版本（由 blanket impl 提供，依赖 Clone）
let v_again = Vertex { pos: (1.0, 2.0), color: [255;4], extras: vec![(3.0, 4.0)] };
let logical_ref = v_again.to_logical(&cam);
```

#### 泛型

```rust
#[derive(Clone, Transform2DInto)]
struct DrawCall<T> {
    transform: (f32, f32, f32),         // (x, y, rot) 要求 T=... 实际递归时按字段类型
    #[transform(skip)]
    payload: T,                         // 泛型字段加 skip 可避免约束
}
```

---

### `#[derive(SceneObject)]`（占位）

目前是空壳派生宏，保留给未来 `scene::comp` trait 系统自动装配用。
暂不生成任何代码。

```rust
use xc_macro::SceneObject;

#[derive(SceneObject)]
struct MyComponent { /* ... */ }
```

---

## 内部实现要点

- **proc-macro2 + quote + syn 2.x**：输入 `TokenStream` → `syn::DeriveInput`
  → 生成 `impl Transform2DInto for T` 代码。
- **泛型约束自动推导**：对默认字段（非 skip / 非 iter），在 `where` 子句插入
  `FieldTy: Transform2DInto`；`iter` 字段由编译器在调用 `map(...)` 时约束。
- **单元/元组/具名三分支生成**：构造函数用对应形式 `Self{...}` / `Self(...)`
  / `Self`。
- **测试用 `parse_quote!`**：`cargo test -p xc-macro` 会在内部对每个分支
  做 `TokenStream` 字符串打印（非快照，仅保证展开不 panic）。

---

## 依赖

```toml
[lib]
proc-macro = true

[dependencies]
proc-macro2 = "1"
quote        = "1"
syn          = "2"
```

本 crate 是纯 proc-macro crate，**不含 unsafe**（Rust 要求 proc-macro crate
本身用纯安全代码）。
