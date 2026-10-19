# ze-node-schema

> **属性模型层**。位于 `ze-node-core`（纯节点内核）与 UI 层之间，提供：
>
> - `Property` / `PropertyValue` — UI 中立的参数字典（12 种常用变体）；
> - `NodeSpec` trait — 在 `Node` 之上的声明式属性接口；
> - `NodeRegistry` — 类型擦除的节点目录，可按 `type_name` 动态实例化、
>   拿 schema、拿 `&dyn NodeSpec`，**不需要调用方任何 downcast**
>   （弥补 Rust 缺失的 trait upcasting）；
> - `DynamicObject` — 运行时可扩展的"命名属性包"，允许用户在不重新编译的情况下添加字段。
>
> **硬约束**：本 crate 只依赖 `ze-node-core + serde`，**绝不依赖 egui / zela-node-editor / zela-node-egui(-ui)**
> 或任何 xcal 项目内部 crate——兑现"后端可换 / WASM / headless"的架构承诺。

```
ze-node-core  ───► ze-node-schema-macros
                      │
                      ▼
               ze-node-schema  ← 本 crate
                      │
          ┌───────────┴───────────┐
          ▼                       ▼
   zela-node-egui-ui (egui 控件映射)   其它后端（CLI 参数面板 / WASM / Web）
```

---

## 12 种 Property 变体

`Property` / `PropertyValue` 是同构枚举：每个 `Property::Variant(Config)`
都对应同构的 `PropertyValue::Variant(inner)`，保证 schema / value 完全一一映射、
没有"配置了 Vector、value 里却拿 Scalar"的非法状态。

| Property 变体 | Config 字段 | 对应 Rust 类型 (derive) | 说明 |
|---|---|---|---|
| `Scalar`  | `{ label, value, min, max, step, unit }` | `f32` | 单精度浮点 |
| `Double`  | `{ label, value, min, max, step, unit }` | `f64` | 双精度浮点 |
| `Integer` | `{ label, value, min, max, step, unit }` | `i32/i64/u32/u64/isize/usize` | 整数 |
| `Bool`    | `{ label, value }`                      | `bool`  | 布尔 |
| `String`  | `{ label, value, kind: Text/Path/Code }` | `String`| 文本 |
| `VecN`    | `{ label, size: 2..=4, components, min, max, step }` | `[f32; 2/3/4]` | 浮点向量（上限 `VEC_MAX_COMPONENTS = 4`）|
| `Color`   | `{ label, value, with_alpha }`           | `[u8; 4]` | RGBA |
| `Enum`    | `{ label, current, variants: Vec<(tag,display)> }` | —（宏暂未自动生成，手动模式）| 枚举 |
| `Range`   | `{ label, lo, hi, min, max, step }`      | — 同上 | 标量区间选择 |
| `Function`| `{ label, handle: FunctionHandle }`      | — | 外带句柄（指针函数 / 编译过的着色器 ……）|
| `Dynamic` | `{ label, fields: Vec<DynamicField> }`   | `DynamicObject` | 运行时字段扩展 |
| `List`    | `{ label, element: Box<Property>, elements: Vec<PropertyValue> }` | — | 可变长度数组 |

对应错误：`PropertyError::{UnknownProperty, TypeMismatch, ValueOutOfRange, UnexpectedDynamicField, DynamicFieldTypeMismatch}`。

---

## NodeSpec trait

```rust
pub trait NodeSpec {
    /// 返回本节点所有属性的 `(name, Property)` 列表。
    /// 每个 Property 携带 label/范围等 schema + 当前 value。
    fn properties(&self) -> Vec<(String, Property)>;

    /// 按名字写入一个属性；类型不匹配时返回 Err。
    fn apply_property(&mut self, name: &str, value: PropertyValue)
        -> Result<(), PropertyError>;
}
```

**手动写**这套 impl 很繁琐；请使用 `#[derive(NodeSpec)]`（见
ze-node-schema-macros），它为 struct 的每个 `#[property]` 字段自动生成
无样板的 `properties()` 与**穷举 match** 的 `apply_property()`（**不调用任何
downcast**）。

### 示例（手动）

```rust
use ze_node_schema::{NodeSpec, Property, PropertyValue, ScalarConfig, PropertyError};

struct MyNode { gain: f32 }

impl NodeSpec for MyNode {
    fn properties(&self) -> Vec<(String, Property)> {
        vec![("gain".into(), Property::Scalar(ScalarConfig {
            label: "增益".into(),
            value: self.gain,
            min: 0.0, max: 10.0, step: Some(0.01), unit: Some("x".into()),
        }))]
    }
    fn apply_property(&mut self, n: &str, v: PropertyValue) -> Result<(), PropertyError> {
        match (n, v) {
            ("gain", PropertyValue::Scalar(x)) => { self.gain = x; Ok(()) }
            ("gain", _) => Err(PropertyError::TypeMismatch {
                expected: "scalar".into(), got: v.kind_name().into(),
            }),
            (o, _) => Err(PropertyError::UnknownProperty(o.into())),
        }
    }
}
```

---

## NodeRegistry：无 downcast 的类型擦除目录

问题：如果用户只拿到 `&dyn Node`，如何访问它的 `NodeSpec::properties()`？
Rust 目前不支持 `trait NodeSpec: Node` 的 trait upcasting（`&dyn Node as
&dyn NodeSpec` 不可行）。`NodeRegistry` 是这个 gap 的官方解决方案。

```rust
let mut reg = NodeRegistry::new();

// 注册（NodeSpec + Node 派生过的类型一行搞定）
reg.register::<ConstantF32>("Source", ConstantF32::default_node);
reg.register::<SinF32>     ("Math",   SinF32::default_node);

// ── 1) 类别列表（给 egui 右键菜单用） ──
for (category, nodes) in reg.categories() {
    for info in nodes {
        println!("[{category}] {} (type={})", info.display, info.type_name);
    }
}

// ── 2) 动态实例化 ──
let node = reg.spawn("demo.sin.f32").unwrap();   // Box<dyn Node>

// ── 3) 拿 schema / 写属性（都不需要知道具体类型） ──
let schema: Vec<(String, Property)> = reg.properties(node.as_ref()).unwrap();
let events = reg.apply_property(node.as_mut(), "phase", PropertyValue::Scalar(0.5));

// ── 4) 类别信息（category / display / type_name） ──
let info: &RegistryNodeInfo = reg.info_of_type("demo.const.f32").unwrap();
```

内部使用 `type_name → (ConstructorFn, SpecAdapterVTable)` 两张表。
`SpecAdapterVTable` 通过 `trait SpecBridge: Node + NodeSpec` + 泛型闭包捕获
具体类型实现，不依赖任何 `downcast_ref::<T>()`（**除非 T 本身注册时是正确
的类型**——由 `register::<T>` 保证）。

---

## DynamicObject：运行时字段扩展

`DynamicObject` 是一个 `Vec<DynamicField { name, type_tag, value: PropertyValue }>`
包装，自带类型校验：

```rust
let mut obj = DynamicObject::new();
obj.add_field("power", Property::Integer(IntegerConfig {
    label: "Power".into(), value: 10,
    min: i64::MIN, max: i64::MAX, step: None, unit: None,
}))?;
obj.set("power", PropertyValue::Integer(20))?;       // 类型正确 ✔️
obj.set("power", PropertyValue::Bool(false))          // TypeMismatch ✋
   .expect_err("should reject");
```

序列化友好（`#[derive(Serialize, Deserialize)]`），可持久化到项目文件里。
配合 `Property::Dynamic(config)` / `PropertyValue::Dynamic(obj)`，一个节点可
同时拥有"静态属性（编译期 struct 字段）+ 动态属性（运行时字段）"。
详见 `tests/dynamic_coexist.rs` 测试。

---

## 宏：`#[derive(NodeSpec)]` 一键化

```rust
use ze_node_schema_macros::NodeSpec;
use ze_node_schema::NodeSpec as _;

#[derive(NodeSpec, Clone)]
#[node(
    type_name = "demo.constant.f32",
    display   = "Constant (f32)",
    category  = "Source",
    outputs   = ["f32"],
    dirty     = "on_input_change",
    state     = "()",
)]
struct ConstantF32 {
    #[property(label = "value", range = 0.0..=10.0, step = 0.01, default = 3.14)]
    value: f32,
}

// 自动生成：
//   impl Node for ConstantF32         — meta/inputs/outputs/dirty_policy/state_type
//   impl NodeSpec for ConstantF32     — properties/apply_property (无 downcast!)
//   impl Default for ConstantF32      — default = #[property(default=...)]
//   ConstantF32::default_node()       — Box<dyn Node>
//   ConstantF32::CATEGORY: &str       — 配合 NodeRegistry
```

支持字段类型：`f32, f64, i32/i64/u32/u64/isize/usize, bool, String, [f32;2|3|4], [u8;4]`。
其余 Enum / Range / Function / Dynamic / List 请手动 impl（宏保持前向兼容）。

---

## 依赖

```toml
ze-node-core            = { path = "../ze-node-core" }
ze-node-schema-macros  = { path = "../ze-node-schema-macros" }
serde                = { features = ["derive"] }
serde_json           = "1.0"
thiserror            = "2.0"
```

## 测试

```bash
cargo test -p ze-node-schema
cargo test --test headless_eval -p ze-node-schema      # 10 个 headless 单测
cargo test --test dynamic_coexist -p ze-node-schema    # 动态/静态节点共图 + 共用 Evaluator
```

共 **18** 个属性集成测试：
- 12 种 `Property` 变体 serde round-trip；
- `DynamicObject` 非法字段类型不 panic；
- 静态 `NodeSpec` + `DynamicObject` 在同一图中共用 `LazyPullEval`。
