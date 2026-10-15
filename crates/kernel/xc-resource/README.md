# xc-resource

> **GPU 资源契约与缓存** — 只声明「资源怎么被标识、怎么被缓存、怎么被回收」，
> **零依赖**（仅 `std`），不依赖任何后端。

---

## 设计定位

三层渲染架构里，本 crate 位于 kernel 层，与 `xc-vertex`（顶点格式契约）并列：

```
xc-resource (资源标识 / 资源键 / 世代感知缓存)   ← 本 crate
        ▲
xcal-render  (RenderBackend trait / 队列 / hecs 兼容层)
        ▲
xcal-render-backends (wgpu / egui 实现)
```

**硬约束（依赖铁律）**：本 crate 必须能编译到 `wasm32`，因此

- ❌ 禁止 `hecs` / `wgpu` / `egui` / 任何 `xcal-*` / 任何外部依赖；
- ✅ 需要 wgpu 类型的后端 trait（`RenderBackend` / `RenderBackend3D`）留在 `xcal-render`；
- ✅ 需要 `hecs::Entity` 的旧 `CacheKey` 兼容层（`From<CacheKey> for ResourceKey`）留在 `xcal-render`。

> ⚠️ **同名不同物**：本 crate 的 [`ResourceCache`](src/resource_cache.rs) 与
> `xcal-schedule` 里 ECS 系统的 `Resources`（系统间共享数据的容器）**没有任何关系** ——
> 前者是 GPU 资源的世代感知缓存，后者是调度期的数据容器。两者不在同一依赖链上，也不会互相引用。

---

## 模块一览

| 模块 | 类型 | 说明 |
|------|------|------|
| `render_backend` | [`GpuResourceId`](#gpuresourceid--世代感知资源标识) | 世代感知的 `index + generation` 资源标识符 + 哨兵值 |
| `render_backend` | `AccessError` | 句柄访问失败原因（`Stale` / `TypeMismatch` / `Dropped`） |
| `render_backend` | [`GpuHandle`](#gpuhandle--类型擦除句柄) | 类型擦除的 GPU 资源句柄（`Box<dyn Any + Send + Sync>`） |
| `resource_cache` | [`ResourceKey`](#resourcekey--资源寻址键) | 资源寻址键：内容寻址（`Content`）/ 版本寻址（`Versioned`） |
| `resource_cache` | [`DeferredDestroyer`](#deferreddestroyer--延迟销毁) | graveyard 3 帧延迟销毁器 |
| `resource_cache` | [`ResourceCache<T>`](#resourcecache--世代感知缓存) | 世代感知 slot + `key → id` 索引的缓存 |
| `resource_cache` | `GpuResourceCache` | `ResourceCache<()>` 的类型别名（最常用形态） |

crate 根导出：

```rust
pub use render_backend::{AccessError, GpuHandle, GpuResourceId};
pub use resource_cache::{DeferredDestroyer, GpuResourceCache, ResourceCache, ResourceKey};
```

---

## 特性速览

### GpuResourceId — 世代感知资源标识

`index + generation` 二元组。`generation` 是**防悬垂**的关键：slot 被回收再分配给新资源时
`generation` 递增，旧句柄拿旧 `generation` 访问就会被判 `Stale`，而不是静默命中新资源。

```rust
use xc_resource::GpuResourceId;

// 未分配 / 空值的哨兵
assert!(GpuResourceId::SENTINEL.is_sentinel());

// 只读访问
let id = GpuResourceId::SENTINEL;
let _ = (id.index(), id.generation());
```

- `SENTINEL` = `{ index: u32::MAX, generation: 0 }`，`is_sentinel()` 只判 `index == u32::MAX`。
- slot 复用时分到的 `generation` 从 0 跳到 1（**永不产出 `generation = 0`**），避免与哨兵混淆。

---

### AccessError — 访问失败原因

```rust
pub enum AccessError {
    Stale { id: GpuResourceId }, // slot 已释放或 generation 不匹配
    TypeMismatch,                // 类型擦除后 downcast 失败
    Dropped,                     // 哨兵 id / slot 越界 / entries 中不存在
}
```

实现 `Display` + `std::error::Error`，可直接 `?` 上抛。

---

### GpuHandle — 类型擦除句柄

内部是 `Box<dyn Any + Send + Sync>`，附带一个 `GpuResourceId`：
创建时为 `SENTINEL`，由 [`ResourceCache`](#resourcecache--世代感知缓存) 插入条目时
通过 **crate 内可见**的 `assign_id` 赋予实际 id。

```rust
use xc_resource::GpuHandle;

let mut handle = GpuHandle::new(42_u32);
assert!(handle.is::<u32>());
assert_eq!(handle.downcast_ref::<u32>(), Some(&42));
assert!(handle.downcast_ref::<i64>().is_none());

if let Some(v) = handle.downcast_mut::<u32>() {
    *v = 100;
}
assert_eq!(handle.into_inner::<u32>(), Some(100)); // 剥离所有权
```

| 方法 | 说明 |
|------|------|
| `new::<T>(value)` | 构造（id = `SENTINEL`） |
| `id()` | 读取资源 id |
| `take()` | 取出内部 `Box<dyn Any + Send + Sync>`（延迟销毁时用） |
| `downcast_ref::<T>()` / `downcast_mut::<T>()` | 借用内部值 |
| `into_inner::<T>()` | 按值剥离内部值（类型不符返回 `None`） |
| `type_id()` / `is::<T>()` | 类型自省 |

`PartialEq` / `Eq` 按 **`id` + 内部 `TypeId`** 比较 —— 类型擦除后无法比较内部值。

---

### ResourceKey — 资源寻址键

两种寻址模式，共用 `TypeId` 防止跨类型误命中：

```rust
use xc_resource::ResourceKey;

struct MeshGeom; // 资源类型（仅用于区分命名空间）

// ① 内容寻址：哈希相同即同一份资源（纯几何上传一次）
let k1 = ResourceKey::content::<MeshGeom>(0xDEAD_BEEF);

// ② 版本寻址：id 定位资源 + version 表示其当前版本（版本变 → 缓存失效）
let k2 = ResourceKey::versioned::<MeshGeom>(/* id = */ 7, /* version = */ 3);
```

| 变体 | 寻址依据 | 典型场景 |
|------|----------|----------|
| `Content { type_id, hash }` | 内容哈希 | 与视图无关的纯几何，内容相同即复用 |
| `Versioned { type_id, id, version }` | 资源 id + 版本号 | 资源内容会被修改，靠版本号触发重建 |

`Copy + Eq + Hash`，可直接做 `HashMap` 的 key。`type_id()` 取二者的类型标签。

---

### DeferredDestroyer — 延迟销毁

GPU 资源不能在被引用的帧内直接 drop。本销毁器用 **graveyard（墓地）机制**：
`schedule_destroy` 的资源先进 pending，需再经过 `graveyard_period` 次
`on_submit` 计数推进（即 GPU 已完成若干次提交，确定不再引用）才真正 drop。

```rust
use xc_resource::{DeferredDestroyer, GpuHandle};

let mut d = DeferredDestroyer::new();           // 默认 graveyard = 3 帧
d.schedule_destroy(GpuHandle::new(1_u32));      // submitted_frame = 0

// 提交帧每推进一次，就推进一次 on_submit
while d.pending_count() > 0 {
    d.begin_frame(); // 收割已安全的 pending（返回本次 drop 数）
    d.on_submit();   // 计数推进
}
```

| 方法 / 常量 | 说明 |
|------|------|
| `DEFAULT_GRAVEYARD_PERIOD: u64 = 3` | 保守默认值 |
| `new()` / `with_period(period)` | 构造 |
| `schedule_destroy(handle)` | 投入待销毁队列（记录当前 `submitted_frame`） |
| `on_submit()` | 提交计数 +1 |
| `begin_frame() -> usize` | 收割 `submitted_at + period <= submitted_frame` 的 pending，返回 drop 数 |
| `pending_count()` | 队列中待销毁资源数 |
| `submitted_frame()` / `graveyard_period()` | 诊断读取 |

---

### ResourceCache — 世代感知缓存

`ResourceCache<T>` 由四部分组成：**slot 表**（含 generation）、**空闲 index 栈**、
**`key → id` 索引**、**`id → CacheEntry` 表**。所有释放都走 `DeferredDestroyer`，
**绝不直接 drop** `GpuHandle`。

```rust
pub struct CacheEntry<T> {
    pub handle: GpuHandle,
    pub meta: T,
    last_used_frame: u64,
}
```

**关键 API：**

| 方法 | 说明 |
|------|------|
| `begin_frame()` | 帧时钟 +1，并收割 graveyard 中已安全的资源 |
| `notify_submit()` | 通知「GPU 已提交一次」（推进延迟销毁计数） |
| `current_frame()` | 当前帧号 |
| `get(key)` / `contains(key)` | 按 key 查询（读） |
| `key_id_of(&key)` | 查 key 对应的 `GpuResourceId` |
| `get_by_id(id)` | **世代校验**入口：id 失效返回 `AccessError` |
| `get_or_insert(key, meta, upload, backend)` | 命中即返回，未命中则调 `upload` 重建并插入 |
| `insert_handle(key, meta, handle)` | 直接插入已构造的 `GpuHandle`（不依赖后端 trait） |
| `mark_dirty(key)` | 标记某个已有条目为脏 |
| `upload_dirty(backend, rebuild)` | 逐条重建脏条目（旧句柄投入延迟销毁），返回重建数 |
| `mark_used(&key)` | 刷新某条目的 `last_used_frame` |
| `evict_unused(max_idle_frames)` | 驱逐空闲超阈值的条目（走延迟销毁），返回驱逐数 |
| `len()` / `is_empty()` / `dirty_len()` / `pending_destroy_count()` | 诊断 |

**泛型与后端解耦**：`get_or_insert` / `upload_dirty` 的后端参数是 `B: ?Sized`
（通常传 `&mut dyn RenderBackend`），因此 kernel 层**不需要知道 `RenderBackend` 的存在**。

> `GpuResourceCache = ResourceCache<()>`：无元数据的最常用形态。

**上界承诺**（动画内容逐帧全 miss / 全重建 / 全驱逐时）：缓存条目数恒等于「每帧条目数」，
pending 队列**不随时间增长**，峰值上界 = `(graveyard_period + 1) × 每帧条目数`。
（单测 `per_frame_churn_keeps_pending_and_entries_bounded` 锁定该承诺。）

---

## 测试

```bash
cargo test -p xc-resource
cargo doc  -p xc-resource --no-deps
```

覆盖点：

- `GpuHandle` — `Send + Sync`、`new`/`downcast`/`into_inner` 往返、类型不符返回 `None`
- `GpuResourceId` — 哨兵判定、与普通 id 区分
- `DeferredDestroyer` — graveyard 3 帧语义（第 3 次 `on_submit` 后开始回收）
- `ResourceCache` — 每帧 churn 下条目数与 pending 队列有界（峰值 = `(period+1) × 每帧条数`）

---

## 依赖

```toml
# 无依赖（仅 std）
```

仅此而已 —— 本 crate 的 `Cargo.toml` 没有 `[dependencies]` 段。
