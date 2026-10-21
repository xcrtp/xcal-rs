# ze-scene-contract

> **契约层** — 场景抽象：**身份 / 关系 / 样式描述符 / 场景读写契约**。
> 上层（渲染管线、宿主 UI、场景对象层）只依赖本层；**具体 ECS（hecs）只出现在实现方 `zela-scene`**。

---

## 设计定位

```
ze-notion          理念层      数学值 + 代数运算
ze-render-ir       渲染 IR     Shape2D / Style2D / ToShape2D / ResolveStyle
ze-scene-contract  契约层      Scene / ObjectId / Parent / Describable   ← 本 crate
ze-object          场景对象层  Circle / Camera2D / Transform / Axes2D / …
zela-scene         ECS 实现    Scene trait 的 hecs 实现（WorldView / WorldScene）
zela-render        渲染管线    三角化 / 合批 / 队列
```

**硬约束（依赖铁律）**：

- ❌ `hecs` / `egui` / `wgpu` / 任何 `zela-*` / `ze-object` —— 禁止；
  验收：`cargo tree -p ze-scene-contract | grep -E "zela-|ze-object"` = 0；
- ✅ `ze-notion`（数学值）、`ze-render-ir`（`Describable::apply` 要产出 `Style2D`）
  —— 这是"能被引用的**最小公共集**"：`#[derive(Describable)]` 在 `ze-object`/下游展开，
  其引用的 trait 与 `*Desc` 必须在这里才能被它们引用。

**为什么 `ObjectId` 不用 `hecs::Entity`**：契约层的价值正是"不认识 ECS"。
`ObjectId { index, generation }` 借用了 hecs 的**模型**（见 `id.rs`），但不借用它的**类型**；
实现方负责两者无损互转（`ze_scene_hecs::{object_id_of, entity_of}`）。

**契约消费方（FanIn = 3，另有经转发的间接消费者）**：

| 消费方 | 用法 |
|--------|------|
| `ze-object` | 图元/装配对象实现 `Describable`；`Axes2D`/`FunctionGraph` 经 `Scene` 装配 |
| `zela-scene` | 本层 trait 的 **hecs 实现**（`WorldView` / `WorldScene` / `Scene`） |
| `zela-render` | `record_scene<S: SceneRead>` 的唯一入参抽象 |
| `zela-egui` | 经 `ze_scene_contract::SceneRead` 与 `ze_scene_hecs::WorldView`（`WorldScene` 在 `zela_scene_ecs::contract`） 转发使用（不直连本层）|

---

## 模块一览

| 模块 | 类型 | 说明 |
|------|------|------|
| `id` | `ObjectId` | 稳定句柄：`index`（可复用槽位）+ `generation`（代）；`DANGLING` 哨兵；`is_stale(current)` |
| `relation` | `Parent` / `Children` | 父子关系（组件形态：`Parent` 挂子对象、`Children` 挂父对象）|
| `organize` | `Name` / `Group` / `GroupId` | 组织语义：声明式引用（按名字找对象）+ 分组标签（**只含标识、不含变换**）|
| `organize` | `Visibility` / `VisibilityTag` / `GroupVisibility` | 「**对谁可见**」（关系而非布尔）：tag 集合（相机 / 组 / 全局）+ 组级开关 |
| `descriptor` | `Describable` / `StrokeDesc` / `FillDesc` / `MaterialDesc` | 样式描述符体系（增量覆盖）|
| `descriptor` | `SLOT_STROKE` / `SLOT_FILL` / `theme_style` / `apply_stroke_desc` / `apply_fill_desc` | 供 `#[derive(Describable)]` 展开调用的合并辅助 |
| `scene` | `Components` / `SceneRead` / `Scene` | 场景读写契约（只读能力与写能力**分成两个 trait**）|

---

## 特性速览

### `ObjectId` — 身份与失效检测

```rust
use ze_scene_contract::ObjectId;

let old = ObjectId::new(7, 1);
let after_reuse = ObjectId::new(7, 2);          // 槽位 7 被复用 ⇒ generation +1
assert!(old.is_stale(after_reuse));             // 旧句柄失效（不会误指新对象）
assert!(!after_reuse.is_stale(after_reuse));    // 当前句柄对自身有效
assert_eq!(ObjectId::DANGLING, ObjectId::default()); // 永不匹配的哨兵
```

> ⚠️ **记账**：`is_stale` 目前**无生产消费方**（只有单测）。用途明确（销毁-重建场景时防悬挂句柄），
> 属「已知未接线项」，见 `ARCHITECTURE.md`。

### `SceneRead` / `Scene` — 只读与写分成两个 trait

```rust
pub trait SceneRead {
    fn contains(&self, id: ObjectId) -> bool;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool { self.len() == 0 }
    fn for_each<T: 'static + Send + Sync>(&self, f: impl FnMut(ObjectId, &T));
    fn for_each_pair<A, B>(&self, f: impl FnMut(ObjectId, &A, &B));
    fn for_each_pair_opt<A, B, C>(&self, f: impl FnMut(ObjectId, &A, &B, Option<&C>));
    /// 只读**点查询**（P1-3）：按 `id` 取一个组件（访客形态，见下方设计点 3）。
    fn with<T: 'static + Send + Sync, R>(&self, id: ObjectId, f: impl FnOnce(&T) -> R)
        -> Option<R>;
}

pub trait Scene: SceneRead {                    // 写路径
    fn spawn_dyn(&mut self, components: Components) -> Option<ObjectId>;
    fn despawn(&mut self, id: ObjectId) -> bool;
    fn attach(&mut self, child: ObjectId, parent: Parent) -> bool;
    fn get_mut<T: 'static + Send + Sync>(&mut self, id: ObjectId) -> Option<&mut T>;
    fn set<T: 'static + Send + Sync>(&mut self, id: ObjectId, component: T) -> bool;
}
```

三个设计点，都不是口味问题：

1. **只读/写拆开**：渲染录制（`record_scene`）与大多数查询**只需要读**，而只读能力可由
   `&World` 满足（实现方的 `WorldView`）；把只读拆出来 ⇒ 调用点能用**不可变**借用接入
   （`SceneRecordSystem` 拿到的就是 `&hecs::World`）。

2. **访客形态（`for_each*`）而非返回迭代器**：hecs 安全 API 把迭代项生命周期绑在
   `QueryBorrow` 的 `&mut` 借用上（`QueryBorrow::iter(&mut self) -> QueryIter<'_, Q>`），
   且没有按值 `IntoIterator` —— 把借用守卫放在方法体里就无法把迭代器返回出去。
   访客形态让守卫的生命周期收在方法体内；这是「借用守卫」这一事实的直接后果，
   **不是为迁就实现而降低表达力**（`for_each` 同样能表达全部查询）。

3. **`for_each_pair_opt`（可选第三组件）**：对象层的**可缺席摆放信息**（`ZOrder` 绘制层级）
   要求"有则用、无则默认"——若要求 `C` 必须存在，就没法用一次查询同时覆盖两类对象。
   hecs 原生支持 `query::<(Entity, &A, &B, Option<&C>)>()`，故不是额外负担。

4. **`with` 是访客形态而非 `Option<&T>`（P1-3，实测修正）**：hecs 0.11 的
   `World::get::<&T>(entity)` 返回的是**借用守卫** `Ref<'_, T>`（生命周期绑在 `&World`
   的借用上，守卫本身必须存活），**`&T` 交不出去** —— 与 `for_each*` 是同一约束。
   消费方：相机**句柄**（`ze_scene_core::Camera2DHandle::resolve`）按 `ObjectId` 取场景对象上的
   取景参数（`Framing2D`），而渲染录制只拿得到 `&S`（`SceneRead`）、没有 `&mut`。

`spawn_dyn` 的**组件登记表**在实现方（hecs 无类型擦除 bundle 入口）：未登记的组件类型 ⇒
返回 `None`（**不静默丢组件** —— 宁可失败也不造"少了一半组件"的对象）。

### `descriptor` — 描述符体系（增量覆盖 + 类型层约束）

```rust
use ze_scene_contract::{Describable, FillDesc, StrokeDesc};

// 由 #[derive(Describable)] + #[describable(Stroke, Fill)] 生成：
//   type Descriptors = (Option<StrokeDesc>, Option<FillDesc>);
// 线段 #[describable(Stroke)] ⇒ Descriptors = (Option<StrokeDesc>,)
//   —— "线段不能填色"因此是**编译期**约束，不是运行期忽略。
let style = circle.apply(&(
    Some(StrokeDesc::new(Color32::RED, 2.0)),
    None,                                  // 缺失槽位取主题默认（增量覆盖）
));
```

- **多个独立 descriptor 类型组合成元组**，而不是一个"总样式结构体" —— 槽位清单由
  `#[describable(..)]` 参数逐项决定；
- `theme_style(slot_mask)` **按声明槽位裁剪**主题默认：`Style2D::default()` 是"有填充、无描边"，
  若原样用于 `Line`（无 Fill 槽位）就会得到 `fill = Some(WHITE)` ——
  "线段不能填色"的类型层保证会被运行期主题默认偷偷破坏。故未声明槽位在此显式关闭。
- ⚠️ `apply` **只做声明合并，不做换算**：产出**未换算**的 `Style2D`
  （`stroke_width` / `space` 原样带过去）。世界 → 屏幕换算仍只发生在
  `zela-render::record_scene` 的**唯一换算点**。
- ⚠️ **记账**：`record_scene` 目前**不消费** `Descriptors`（它直接吃 `Style2D`/样式组件）
  —— 与 `Parent` 同类，属「已知未接线项」。

### `relation` — 父子关系

`Parent(pub ObjectId)` 挂子对象、`Children(Vec<ObjectId>)` 挂父对象；`Scene::attach`
只写子侧的 `Parent`，`Children` 的维护策略由装配层决定（契约层不强制双向同步）。

> **S5 已接线**：`TransformHierarchySystem`（`zela-scene`）每帧按拓扑序累乘出
> `WorldTransforms`（S1 起存 **3D 世界矩阵 `Transform3D`**，2D 消费方经 `get2d`
> 读投影视图），`SceneRecordSystem` / `RevealRecordSystem` 消费，`SceneRecord3DSystem`
> 同消费（3D 录制读 `Reads = (WorldTransforms,)`）；
> 环路写入 `TransformHierarchyError` 资源。相关缺口见根目录 `GAPS.md`。
> `push` 不查重 —— 去重是装配层策略。

### `organize` — 组织与可见性（N1–N3）

```rust
// 声明式引用 + 分组（纯标签，不含变换 —— 与 `Parent` 的根本区别）
let comps: Components = vec![Box::new(Name::new("curve")), Box::new(Group::new(GroupId::new(3)))];
let id = scene.spawn_dyn(comps);
let found = zela_scene_ecs::contract::find_by_name(&scene, "curve");  // Option<ObjectId>（第一个命中）

// 「对谁可见」：非空 tag 集合，**任一命中即可见**（并，非交）
let v = Visibility::for_camera(camera_id).with_tag(VisibilityTag::Group(g));
v.visible_for(Some(camera_id), &GroupVisibility::new());   // 任一命中 ⇒ true

// 组级开关（集中一处，不做 N 次逐对象写入）
let mut groups = GroupVisibility::new();
groups.hide(g);                                            // 组内"随组走"的对象随之不可见
```

- **可缺席组件 = 默认值**（照 `ZOrder` 先例）：`Name` 缺席 = 匿名、`Group` 缺席 = 不属于任何组、
  `Visibility` 缺席 = **全局可见**；`Visibility::default()`（空 tag 集）与缺席**同口径** ——
  避免"挂了组件反而隐形"的反直觉陷阱。
- **tag 只支持一层、不做嵌套组**；tag 只认**相机与组**（不做按名字的 tag）。
- `GroupId(u32)` 而**不是** `ObjectId`：组是纯标签、不建组实体 ⇒ 无生命周期、不需要 generation 失效检测。
- `Name` **不强制唯一**：`find_by_name` 返回**第一个**命中（场景规模几十个对象，O(n) 足够）。
- 与图层总闸的关系：`UiConfig::show_2d` / `show_3d` 是**图层级总闸**（关掉整层不画，不看对象 tag）；
  **总闸优先**，之后才轮到本层的对象级 `Visibility`（判定点在 `zela-render` 的录制路径）。

---

## 测试与验收

```bash
cargo test  -p ze-scene-contract
cargo doc   -p ze-scene-contract --no-deps

# 分层验收（= 0）
cargo tree -p ze-scene-contract | grep -E "zela-|ze-object"
# wasm 可编译性（kernel 层承诺）
cargo check -p ze-scene-contract --target wasm32-unknown-unknown
```

覆盖：`ObjectId` 失效检测（销毁-复用后旧句柄必失效）、`DANGLING` 永不匹配、
`Children` push/remove、`StrokeDesc` 默认空间、**声明合并不换算**（`World` 宽度原样保留）、
**`theme_style` 按槽位裁剪**（只声明 Stroke 的对象不得有填充 —— 证伪点）。

---

## 依赖

```toml
[dependencies]
ze-notion      = { path = "../ze-notion" }        # 数学值（Color32 / MeasureSpace）
ze-render-ir   = { path = "../ze-render-ir" }     # Style2D / StrokeSolid（apply 的产出目标）
```

仅此两个，且都是 kernel 内部 —— 无 hecs / egui / wgpu / 任何 `zela-*`。
