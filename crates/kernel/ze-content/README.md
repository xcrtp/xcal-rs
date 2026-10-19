# ze-content

> **内容中间表示（Content IR）内核** — 公式/文本排版结果的引擎无关数据模型。
> 纯数据 + 纯函数：只依赖 [ze-notion](../ze-notion)（及可选 `serde`），
> 无 typst / egui / wgpu / hecs，可被任何上层 crate 安全复用。

---

## 设计定位

两阶段内容管线的第一阶段产物落在这里：

```
ze-prerender（排版引擎接入：typst / placeholder）
        │  产出
        ▼
ze-content（本 crate：ContentIR + 归一化 + SVG + 占位生成）  ← kernel 层
        │  消费
        ▼
zela-render（ir_to_draw_cmds → Shape2D + Style2D + Transform2D）
```

**为什么需要独立 IR**：不同排版引擎（Typst、未来 LaTeX/纯 SVG）输出的几何与
锚点语义必须归一到同一数据模型，渲染层才能无视后端直接光栅化；而源码锚点
（点击公式中某个字符 → 回溯源码区间）必须在引擎之间行为一致——**锚点匹配是
字节区间求交，不先做源码归一化则跨引擎锚点必然失效**。

---

## 模块一览

| 模块 | 关键类型 / 函数 | 说明 |
|------|----------------|------|
| `ir` | `ContentIR` / `ContentSource` / `StyledPath` / `Span` | IR 数据模型：每条 path 自带 fill/stroke/anchor |
| `normalize` | `normalize_source` / `hash_source` | 去 `//`、`/* */` 注释 + 折叠空白 + trim；归一化哈希做缓存键 |
| `svg` | `to_svg` / `SvgOptions` | IR → 确定性 SVG（固定浮点精度 + `data-span` 锚点属性），快照/人工核验量具 |
| `placeholder` | `generate_placeholder` / `PlaceholderOptions` | 零依赖占位排版：每个 UTF-8 字符一个矩形，锚点字节边界准确 |
| `pair` | `pair_by_anchor` / `degenerate_at` | 语义配对：按源码锚点把两个 IR 的路径配成 `(from, to)` 对（形变用） |

---

## 核心数据模型

```rust
use ze_content::{ContentSource, ContentIR, StyledPath, Span};

// 源码入口：构造时自动归一化 + 计算哈希
let src = ContentSource::from_raw("x^2 + 1");
assert_eq!(src.text, "x^2 + 1");   // 归一化文本
let _hash: u64 = src.hash;         // 缓存键用

// IR：单位 pt；bounds = 墨迹包围盒；baseline 近似 y=0 对齐；
// nominal_font_size = 排版时字号，消费端 scale = 目标字号 / nominal_font_size
let mut ir = ContentIR::empty(src.clone(), 16.0);
ir.items.push(
    StyledPath::filled(path, ze_notion::Color32::BLACK)
        .with_anchor(Span::new(0, 1)),   // 锚点：归一化源码字节区间
);
ir.recompute_bounds();
```

- `StyledPath { path: Path2D, fill: Option<Color32>, stroke: Option<Stroke2D>, anchor: Option<Span> }`
  —— **每条路径自描述样式与锚点**，渲染层不再需要全局样式表。
- 坐标系约定：Y-up，`(0,0)` 近似在 baseline 左下；`bounds` 为墨迹包围盒。
- `Span` 可空（`Option`）：引擎解析不出锚点时不阻塞渲染。

---

## 确定性 SVG 快照

`to_svg` 是管线的「量具」：固定浮点精度（默认 3 位）保证字节级确定性输出，
锚点以 `data-span="start:end"` 写入，方便人工/回归核验。

```rust
use ze_content::svg::{to_svg, SvgOptions};

let svg = to_svg(&ir, &SvgOptions::default());
std::fs::write("snapshot.svg", svg).unwrap();
```

---

## 占位引擎（零依赖基线）

无字体、无排版引擎也能建立可运行基线：每个非空白字符一个矩形，
空白字符只推进宽度；多字节字符（如 `β`）锚点按 UTF-8 字节边界准确切分。

```rust
use ze_content::placeholder::{generate_placeholder, PlaceholderOptions};
let ir = generate_placeholder("x^2 + 1", &PlaceholderOptions::default());
```

---

## 语义配对（形变用）

`x^2 → x^3` 的 `x` 应该变到 `x^3` 的 `x`，而不是按位置变到 `3`。
「对应关系」的知识留在本层（路线甲：**不进 `ze-time`**）：

```rust
use ze_content::pair_by_anchor;
// 优先级：锚点相同 → 配对；仅单侧存在 → 淡入/淡出（退化为包围盒中心点）；
// 无锚点 / 剩余项 → 按索引顺序配对。
let pairs: Vec<(Path2D, Path2D)> = pair_by_anchor(&from_ir, &to_ir);
```

产出的路径对交给下层 `Path2D::morph` / `Curve::PathMorph` 逐对插值。

---

## 测试

```bash
cargo test -p ze-content          # 25 个单元测试
cargo tree -p ze-content          # 依赖卫生：不含 typst / egui / wgpu / hecs
```

覆盖点：IR bounds 合并、归一化（注释/空白/哈希幂等）、SVG 确定性与锚点
属性、占位生成器（空白布局、多字节字符、bounds）、锚点配对
（乱序仍按锚点配对 / 淡入淡出 / 无锚点退化索引）。

---

## 依赖

```toml
[dependencies]
ze-notion = { path = "../ze-notion" }
serde = { version = "1", optional = true }   # feature = "serde" 时启用
```
