# xc-prerender

> **内容预渲染（排版引擎接入）层** — 把公式 / 文本源码编译为引擎无关的
> [ContentIR](../xc-content)（含源码锚点）。kernel 层唯一的「重 crate」：
> 所有重型引擎依赖（Typst 等）必须放在 feature 后，**默认零重依赖可独立编译**。

---

## 设计定位

两阶段内容管线的引擎侧，与 [xc-content](../xc-content) 的契约关系：

```
xc-notion (理念层/数学元语) ← xc-content (ContentIR + 归一化 + SVG + 占位)
        ▲
xc-prerender（本 crate：引擎抽象 + 缓存 + Typst 接入）
        ▲
上层渲染（xcal-render：ir_to_draw_cmds → Shape2D 直接光栅化）
```

**依赖方向**：`xc-notion ← xc-content ← xc-prerender`。
**铁律**：禁止依赖任何 graph / render / ui / app（`xc-*` / `xcal-*` 上层）；
重引擎依赖全部 `optional`，未启用 feature 时零重依赖。

---

## 异步形状的引擎 API

```rust
pub trait ContentEngine: Send + Sync {
    fn name(&self) -> &str;
    fn request(&self, source: &str, config: &ContentConfig) -> ContentHandle; // 不阻塞
    fn poll(&self, handle: ContentHandle) -> ContentStatus;                   // Pending/Ready/Gone
    fn take(&self, handle: ContentHandle) -> Option<ContentResult>;           // Gone 语义
}
```

`request()` 立即返回不透明句柄 → `poll()` 查询 → `take()` 消费结果
（取走后句柄失效，第二次 `take` 返回 `None`）。
当前实现内部同步完成（首次 `poll` 即 Ready），将来换真正异步调度
**无需改动任何调用点**——这就是先定异步形状的原因（未来 LaTeX/SVG 必然异步）。

---

## 模块一览

| 模块 | 关键类型 / 函数 | 说明 |
|------|----------------|------|
| `request` | `ContentHandle` / `ContentConfig` / `ContentStatus` / `CacheKey` / `ContentError` | 请求、句柄、状态、错误、缓存键 |
| `engine` | `ContentEngine` trait / `InMemoryEngine` / `CompileFn` | 函数驱动的内存引擎：统一管理句柄、槽位、缓存 |
| `cache` | `ContentCache` | 内容哈希缓存（线程安全读写锁） |
| `placeholder` | `engine()` | 零重依赖占位引擎：API 基线 / 管线测试 / 无字体降级 |
| `typst` *(feature)* | `typst_engine()` / `compile` / `TypstWorld` / `convert` | Typst 数学排版引擎接入 |

---

## 快速上手

### Placeholder 引擎（默认可用，零重依赖）

```rust
use xc_prerender::{ContentConfig, ContentEngine, ContentStatus, placeholder_engine};

let engine = placeholder_engine();
let h = engine.request("x^2 + 1", &ContentConfig::default());
assert!(matches!(engine.poll(h), ContentStatus::Ready));

let ir = engine.take(h).unwrap().unwrap();   // ContentIR：每条 path 自带 fill/anchor
```

### Typst 引擎（feature = "typst"）

```toml
# Cargo.toml
xc-prerender = { path = "../xc-prerender", features = ["typst"] }
```

```rust
let engine = xc_prerender::typst_engine()?;
let cfg = ContentConfig { font_size: 20.0, ..Default::default() };
let h = engine.request("sum_(i=1)^n i = (n (n + 1)) / 2", &cfg);
let ir = engine.take(h).unwrap().unwrap();
```

- 源码不以 `$` 开头时自动包装为 inline math `$ … $`；
- 编译前注入 `#set page(width: auto, height: auto, margin: 0pt)` +
  `#set text(size: …pt, fill: rgb(…))` 前缀，字号/颜色由 `ContentConfig` 驱动；
- `TypstWorld` 实现 `typst::World`（字体与 Library 随引擎长期持有复用，
  comemo 记忆化使相同源码重编译近乎免费）；
- `convert.rs` 把 Typst `Frame`（Text/Shape/Group 三类 FrameItem）递归转换为
  `ContentIR`，glyph path 携带回切归一化源码的**字节区间锚点**。

---

## 扩展新引擎

实现一个 `CompileFn` 并注入 `InMemoryEngine` 即可，句柄 / 槽位 / 缓存全部复用：

```rust
let compile: Arc<CompileFn> = Arc::new(|src: &ContentSource, cfg| {
    // src 已归一化；返回 Result<ContentIR, ContentError>
    my_latex_compile(&src.text, cfg)
});
let engine = InMemoryEngine::new("latex", compile);
```

---

## 缓存与归一化

- 缓存键 `CacheKey = 归一化源码哈希 + 配置哈希`（字号 / fill / scale_factor）；
- **所有引擎共用同一套归一化**（[xc-content](../xc-content) 的
  `normalize_source`）——锚点匹配是字节区间求交，不先归一化则跨引擎锚点必然失效；
- `InMemoryEngine::with_cache` 可挂外部缓存实例（Typst 侧与 World 共享同一缓存）。

---

## 调试

设置环境变量 `XC_PRERENDER_DUMP_SRC` 会把包表前缀后的完整 wrapped source
打印到 stderr，便于定位 Typst 诊断信息。

---

## 测试

```bash
cargo test -p xc-prerender                        # 5 个测试（placeholder 基线）
cargo test -p xc-prerender --features typst       # +5 个 Typst 集成测试
```

覆盖点：

- 引擎管线 — request→poll→take 全流程、`take` Gone 语义、未知句柄
- 缓存 — 相同 (源码, 配置) 命中缓存不重复编译、不同源码各自编译
- Placeholder — IR 非空、锚点能回切归一化原文
- Typst — 简单/复杂公式编译、锚点回切唯一 token 数、请求去重、
  compile 幂等 + SVG 快照输出

---

## Features 与依赖

```toml
[features]
default = []
typst = ["dep:typst", "dep:typst-library", "dep:ecow", "dep:fontdb", "dep:ttf-parser"]
placeholder = []   # 占位引擎，默认可用，无额外依赖

[dependencies]
xc-notion = { path = "../xc-notion" }
xc-content  = { path = "../xc-content" }
thiserror.workspace = true
derive_more.workspace = true
# typst = "0.14" 等 → 全部 optional
```

打开 `typst` feature 会引入 typst workspace 共 ~15 个 crate——
这就是重型依赖必须留在 feature 后的原因。
