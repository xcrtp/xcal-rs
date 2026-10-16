# xc-vertex

> **顶点格式契约** — 只声明「顶点长什么样」，不生产数据。
> 零 wgpu 依赖，可编译到 `wasm32`。

---

## 设计定位

三角化器、AA 策略、Pipeline 三块逻辑共享同一个交汇点：**顶点格式**。
本 crate 把这份共享契约钉死成四份声明，任何一方改动都逃不过类型系统与测试。

```
xc-vertex (布局描述符 / AA 模型 / 缓存策略 / 一致性校验)   ← 本 crate
        ▲
xcal-render-backends::vertex_layout (翻译到 wgpu::VertexBufferLayout)
        ▲
wgpu Pipeline
```

**硬约束（依赖铁律）**：本 crate **不得引用任何 wgpu 类型** ——
翻译到 `wgpu::VertexBufferLayout` 的代码在 `xcal-render-backends::vertex_layout`。

- ❌ `hecs` / `egui` / `wgpu` / 任何 `xcal-*` —— 绝对禁止；
- ✅ `serde` — `VertexLayout` / `VertexAttrib` / `AaModel` / `CacheStrategy` 可序列化；
- ✅ `thiserror` — `VertexLayoutError`。

**契约消费方**：

| 消费方 | 用途 |
|--------|------|
| 三角化器 | 填写 `VertexLayout`，声明「我吐出的顶点长什么样」 |
| Pipeline | 读取布局构造 `vertex_buffers`，并把它作为变体缓存 key 的一部分 |
| 缓存层 | 按 `layout.cache` 选择缓存路径 |

---

## 模块一览

| 模块 | 类型 | 说明 |
|------|------|------|
| `layout` | [`VertexLayout`](#vertexlayout--布局描述符) | 布局描述符（V1）+ 可哈希/可序列化（V4）+ 实例化扩展 |
| `layout` | `VertexAttrib` / `AttribType` / `StepMode` | 单个属性、元素类型、步进模式 |
| `layout` | `VertexInputDesc` | 一组 buffer 的完整顶点输入描述（实例化） |
| `layout` | `VertexLayoutError` | 布局自洽性校验错误（9 个变体） |
| `aa` | [`AaModel`](#aamodel--抗锯齿模型) / `AaSlotSemantics` / `AttribSemantic` | AA 模型与通用槽语义映射表（V2） |
| `cache` | [`CacheStrategy`](#cachestrategy--缓存策略) | 缓存策略声明（V3） |
| `check` | [`StaticConsistencyChecker`](#staticconsistencychecker--跨帧一致性校验) | debug 跨帧一致性校验（V5） |
| `check` | `hash_bytes` / `hash_bytes_sampled` | FNV-1a 64 位哈希工具（无外部依赖） |

crate 根导出：

```rust
pub use aa::{AaModel, AaSlotSemantics, AttribSemantic};
pub use cache::CacheStrategy;
pub use check::{hash_bytes, hash_bytes_sampled, StaticConsistencyChecker};
pub use layout::{AttribType, StepMode, VertexAttrib, VertexInputDesc, VertexLayout, VertexLayoutError};
```

---

## 特性速览

### VertexLayout — 布局描述符

契约词汇表**固定为 4 个语义**：`position` + 三个通用槽 `attrib0` / `attrib1` / `attrib2`，
不再增加固定属性。通用槽的含义由 [`AaModel`](#aamodel--抗锯齿模型) 决定 ——
从 MSAA 切到解析 AA 时，**顶点格式不动，只换 shader 变体和三角化器实现**。

```rust
use xc_vertex::{AaModel, AttribType, CacheStrategy, StepMode, VertexLayout};

// 标准布局（R0）：
// position(F32x2) + attrib0(F32x2) + attrib1(F32) + attrib2(F32) → stride = 24
let layout = VertexLayout::standard(AaModel::Analytic, CacheStrategy::Static);
assert_eq!(layout.stride, 24);
assert_eq!(layout.attribs[0].offset, 0);
assert_eq!(layout.attribs[1].offset, 8);
assert_eq!(layout.attribs[2].offset, 16);
assert_eq!(layout.attribs[3].offset, 20);
layout.validate().unwrap();
```

> **R0（书写动画 reveal）**：第三个通用槽 `attrib2(F32)` 承载**归一化弧长** `s ∈ [0,1]`
> （`aa::AttribSemantic::Advance`）。这是把一条**早已预留**的语义填上，不改前三个槽的承诺。
> 代价：布局哈希再次全变（进程内 pipeline 变体缓存重建一次；顶点缓冲按结构体原样上传，自动跟随）。

- **属性顺序即 shader location 顺序**，必须稳定（顺序变 → 哈希变 → 缓存失效）；
- `compute_layout(&mut [VertexAttrib]) -> u32` 是**纯计算**：offset 从 0 起按顺序累加，
  每个属性对齐到自身大小（`AttribType::alignment`），返回 stride；相同输入必得相同输出。

**元素类型**（后端中立，不含 wgpu 类型）：

| `AttribType` | 字节大小 | 对齐 |
|--------------|---------|------|
| `F32` | 4 | 4 |
| `F32x2` | 8 | 8 |
| `F32x3` | 12 | 12 |
| `F32x4` | 16 | 16 |

**校验分层**（`validate()`）：

- `StepMode::Vertex`（顶点数据 buffer）→ **严格词汇表**：语义必须是
  `position` / `attrib0` / `attrib1` / `attrib2`，否则 `IllegalSemantic`；
- `StepMode::Instance`（实例数据 buffer）→ **宽松**：语义名宿主自定义，只要求非空；
- 两者共同：offset 对齐、无重叠（须按 offset 升序）、stride 覆盖全部属性。

---

### 实例化扩展 — VertexInputDesc

wgpu 的 `VertexBufferLayout` 描述**单个**顶点缓冲，而实例化需要**多个**
（`buffer0` = 顶点数据 + `buffer1` = 实例数据）。因此契约从「一个 layout」演进为
「一组 buffer 描述」（`VertexInputDesc`），同时保持单 buffer 用法不变。

```rust
use xc_vertex::{VertexInputDesc, VertexLayout, AaModel, CacheStrategy, StepMode};

// 单 buffer 升级路径：与旧单 buffer 用法完全等价
let desc = VertexInputDesc::single(VertexLayout::standard(AaModel::None, CacheStrategy::Static));
desc.validate().unwrap();
```

- **buffer 顺序即 wgpu 的 vertex buffer slot（0..n）**；
- shader location 口径：**跨 buffer 全局连续编号**（buffer0 占 `0..n`，buffer1 从 `n` 续编）；
- `validate()` 三条规则：至少一个 buffer（否则 `EmptyInput`）、
  至少一个 `Vertex` 步进 buffer（`Instance` 不能孤立存在 → `InstanceOnly`）、逐 buffer 校验。

---

### VertexLayoutError — 校验错误

```rust
pub enum VertexLayoutError {
    Empty,                                   // 布局无属性
    EmptySemantic { index },                 // 语义为空字符串
    Misaligned { semantic, index, offset, align },
    Overlap { semantic, index, offset },     // 与前属性重叠
    StrideTooSmall { stride, semantic, needed },
    IllegalSemantic { index, semantic },     // Vertex 步进 buffer 语义不在词汇表内
    EmptyInput,                              // VertexInputDesc 无 buffer
    InstanceOnly,                            // 只有 Instance buffer，无顶点数据
}
```

`serde` 反序列化**宽松**（接受任意非空语义名，实例化需要宿主自定义语义），
严格词汇表校验分层到 `validate()`。代价：`&'static str` 语义名在加载期经 `Box::leak`
进入 `'static` 存储，泄漏**有界**（来自宿主有限标识符集合，仅发生在布局加载期）。

---

### AaModel — 抗锯齿模型

`attrib0(F32x2)` / `attrib1(F32)` / `attrib2(F32)` 三个通用槽的**含义由 `AaModel` 决定**。
`aa.rs` 给出 `AaModel -> (attrib0 语义, attrib1 语义, attrib2 语义)` 的**纯数据映射表**，
供 shader 作者与三角化器共享同一份声明（可 headless 测试）。

| `AaModel` | attrib0 | attrib1 | attrib2 | 说明 |
|-----------|---------|---------|---------|------|
| `None` | `Undefined` | `Undefined` | `Undefined` | 硬边，纯靠 MSAA；shader 不得读三槽 |
| `Coverage` | `Undefined` | `Coverage` | `Undefined` | 顶点直接带 `[0,1]` coverage，插值即得 |
| `Analytic` | `Normal` | `Side` | `Advance` | 顶点带单位边法线 + side(±1) + **归一化弧长**；shader 解析到边距离后 smoothstep（lyon 方案），并用 `Advance` 决定书写动画写到哪 |
| `Sdf` | `Uv` | `Undefined` | `Undefined` | 距离场：顶点带图集 UV，场值从纹理采样 |

```rust
use xc_vertex::{AaModel, AaSlotSemantics, AttribSemantic};

let s: AaSlotSemantics = AaModel::Analytic.slots();
assert_eq!(s.attrib0, AttribSemantic::Normal);
assert_eq!(s.attrib1, AttribSemantic::Side);
assert_eq!(s.attrib2, AttribSemantic::Advance); // R0：书写动画（reveal）
assert!(!s.shader_note.is_empty()); // 每个模型都带 shader 解释说明
```

`AttribSemantic` 是 §4.1 槽语义词汇表的枚举化，标注「预留」的变体
（`SdfGradient` / `Advance`(R0 已启用) / `SdfDistance`）当前无模型使用，保留以把词汇表完整编码进类型系统。
`AaModel::ALL` 供穷举测试：映射表必须全覆盖、无未定义分支。

---

### CacheStrategy — 缓存策略

策略挂在**布局描述符**上（而非单个 buffer 实例）—— 格式相同的数据，更新语义必须一致。

| `CacheStrategy` | 行为 | 进内容寻址 LRU | 每帧重建 |
|-----------------|------|:---:|:---:|
| `Static` | 与视图无关的纯几何，上传一次 | ✅ | ❌ |
| `ViewDependent` | **本阶段退化为 `PerFrame`**（枚举位保留） | ❌ | ✅ |
| `PerFrame` | 每帧变，走 arena / 流式上传 | ❌ | ✅ |
| `MorphTarget` | 成对缓存（from/to），各自内容寻址 | ✅ | ❌ |

```rust
use xc_vertex::CacheStrategy;

// 归一化口径：ViewDependent 本阶段行为完全等同 PerFrame
assert_eq!(CacheStrategy::ViewDependent.effective(), CacheStrategy::PerFrame);
assert!(CacheStrategy::Static.is_content_addressed());
assert!(CacheStrategy::PerFrame.bypasses_lru());
```

- `effective()` 是**唯一归一化入口**：缓存层应先调用它再分流，
  保证 `ViewDependent` / `PerFrame` 在当前实现下走同一条路径；
- `ViewDependent` 退化的理由：屏幕空间恒定描边要求三角化时知道当前缩放，
  与「三角化结果可缓存」直接冲突 —— 没有实测数据前，先走简单正确的路径。

---

### StaticConsistencyChecker — 跨帧一致性校验

策略声明最大的风险是**说谎** —— 三角化器声明 `CacheStrategy::Static`，实际每帧都在变。
debug 构建下，对声明为 `Static` 的格式，在连续两帧比对数据哈希，不一致则 panic 并指明是哪个格式。

```rust
use xc_vertex::{AaModel, CacheStrategy, StaticConsistencyChecker, VertexLayout};

let layout = VertexLayout::standard(AaModel::None, CacheStrategy::Static);
let mut checker = StaticConsistencyChecker::new();
checker.begin_frame();
checker.submit_static(/* identity = */ 1, &layout, &[1, 2, 3, 4]);
// 下一帧同一 identity 若数据哈希变化 → panic「顶点格式契约校验失败」
```

- **成本控制**：仅在 `debug_assertions` 下启用；release 下所有方法均为 no-op
  （类型始终存在，消费方无需 `cfg` 门控，release 零开销）；
- **仅对 `Static` 声明执行**：`PerFrame` / `ViewDependent` / `MorphTarget` 本就预期变化，
  提交给校验器会被静默忽略；
- 校验的是「**连续两帧**」而非「历史出现过」：某帧缺席不算撒谎，隔帧后以新数据回归也不算；
- 大 buffer 可改用采样哈希（`submit_static_sampled`）。

**哈希工具**（FNV-1a 64 位，无外部依赖）：

| 函数 | 说明 |
|------|------|
| `hash_bytes(data) -> u64` | 全量 FNV-1a 64 位 |
| `hash_bytes_sampled(data) -> u64` | 头尾各取最多 64 字节 + 长度混入（大数据量退化方案，区分度低于全量） |

| `StaticConsistencyChecker` 方法 | 说明 |
|------|------|
| `new()` | 构造 |
| `begin_frame()` | 把上一帧记录定为比较基准，帧号 +1 |
| `submit_static(identity, &layout, data)` | 提交本帧数据（内部全量哈希） |
| `submit_static_sampled(identity, &layout, data)` | 提交本帧数据（采样哈希） |
| `submit_static_hash(identity, &layout, data_hash)` | 提交调用方已算好的哈希 |
| `frame()` / `tracked_len()` | 诊断 |

---

## 测试

```bash
cargo test -p xc-vertex
cargo doc  -p xc-vertex --no-deps
```

覆盖点：

- **V1** — 标准布局 offset/stride（`0/8/16/20`、stride `24`，四槽）、对齐填充（`F32x3` 对齐到 12）、
  `compute_layout` 确定性、重叠/`stride` 过小/未对齐/空布局被拒
- **V2** — AA 映射表全覆盖（每个模型都有明确语义、无未定义分支）、硬边模型三槽声明不可读、
  `Analytic` 的第三个槽声明为 `Advance`（R0 书写动画依赖它）
- **V3** — `ViewDependent` 与 `PerFrame` 行为逐项一致、`Static`/`MorphTarget` 内容寻址、
  `effective()` 归一后只剩三档行为
- **V4** — serde 往返一致、未知语义可解析但 `validate()` 拒绝、空语义被拒、
  相同布局同哈希、任一字段变更都改变哈希、属性顺序变更改变哈希
- **V5** — 撒谎的 `Static` 第二帧 panic（消息指明格式）、诚实数据多帧多对象不 panic、
  不同 identity 可不同、缺席后新数据回归不算撒谎、非 `Static` 提交被忽略、
  采样哈希小 buffer 等价全量 / 对首尾变化敏感、FNV-1a 已知向量
- **实例化** — 旧 JSON（无 `step_mode`）加载为 `Vertex` 且等价 `standard()`、
  `step_mode` 变更改变哈希、校验按 `step_mode` 分层、`VertexInputDesc` 非空 /
  `Instance` 不能孤立 / 逐 buffer 校验、`VertexInputDesc` 可哈希可序列化

---

## 依赖

```toml
[dependencies]
serde.workspace = true
thiserror.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

仅此而已 —— 无 wgpu / hecs / egui / 任何 `xcal-*`。
