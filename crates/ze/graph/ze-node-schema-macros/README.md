# ze-node-schema-macros

> **`ze-node-schema` 的过程宏支撑**。
> 提供 `#[derive(NodeSpec)]`（配合 `#[node(...)]` 与 `#[property(...)]`
> 属性），零样板地生成 `Node + NodeSpec + Default` 三个 impl 以及
> `default_node()` 构造器。

```
#[derive(NodeSpec)]
        │
        ▼
ze-node-schema-macros  ← 本 crate
        │
        ▼
ze-node-schema (消费宏：trait NodeSpec + 类型)
        │
        ▼
zela-node-egui-ui + ze-graph-eval（最终用户）
```

---

## `#[derive(NodeSpec)]` 用法

```rust
use ze_node_schema_macros::NodeSpec;

#[derive(NodeSpec, Clone)]
#[node(
    type_name = "demo.add.f32",        // 跨序列化稳定标识；必填
    display   = "Add (f32)",           // 标题栏显示名；缺省=结构体名
    category  = "Math",                // NodeRegistry 分类；缺省="Misc"
    inputs    = ["f32", "f32"],        // 输入 socket 类型标签
    outputs   = ["f32"],               // 输出 socket 类型标签
    dirty     = "on_input_change",     // 脏策略：always | never | on_input_change
    state     = "()",                  // 状态类型字符串标识
)]
struct AddF32 {
    // ── 每个字段对应一个属性；缺省字段直接走 Default ──
    #[property(label = "倍率", range = 0.0..=10.0, step = 0.01, default = 1.0, unit = "x")]
    gain: f32,

    #[property(label = "颜色", default = [255, 255, 0, 255])]
    tint: [u8; 4],
}
```

### 生成的 impl 总览

```rust
// 1) impl Node for AddF32  —— OnceLock 静态 meta/inputs/outputs
impl ::ze_node::Node for AddF32 {
    fn meta(&self) -> &'static NodeMeta {
        static M: OnceLock<NodeMeta> = OnceLock::new();
        M.get_or_init(|| NodeMeta::new("demo.add.f32", "Add (f32)", 1))
    }
    fn inputs(&self)  -> &'static [InputSocket]  { /* "f32" + "f32" */ }
    fn outputs(&self) -> &'static [OutputSocket] { /* "f32" */ }
    fn dirty_policy(&self) -> DirtyPolicy { DirtyPolicy::OnInputChange }
    fn state_type(&self) -> Box<dyn TypeTag>     { Box::new(NamedTypeTag::new("()")) }
    fn as_any(&self)    -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}

// 2) impl NodeSpec for AddF32  —— **零 downcast，全穷尽 match**
impl ::ze_node_schema::NodeSpec for AddF32 {
    fn properties(&self) -> Vec<(String, Property)> {
        vec![
            ("gain".into(), Property::Scalar(ScalarConfig {
                label: "倍率".into(), value: self.gain,
                min: 0.0, max: 10.0, step: Some(0.01), unit: Some("x".into()),
            })),
            ("tint".into(), Property::Color(ColorConfig {
                label: "颜色".into(), value: self.tint, with_alpha: true,
            })),
        ]
    }
    fn apply_property(&mut self, name: &str, value: PropertyValue) -> Result<(), PropertyError> {
        match (name, value) {
            ("gain", PropertyValue::Scalar(v)) => { self.gain = v; Ok(()) }
            ("gain", _) => Err(PropertyError::TypeMismatch { expected: "scalar".into(), got: v.kind_name().into() }),
            ("tint", PropertyValue::Color(v)) => { self.tint = v; Ok(()) }
            ("tint", _) => Err(PropertyError::TypeMismatch { ... }),
            (_, _) => Err(PropertyError::UnknownProperty(name.into())),
        }
    }
}

// 3) impl Default （用每个 #[property(default = ...)] 初始化）
impl Default for AddF32 { fn default() -> Self { Self { gain: 1.0, tint: [255,255,0,255] } } }

// 4) 关联函数 + 常量
impl AddF32 {
    pub const CATEGORY: &'static str = "Math";
    pub fn default_node() -> Box<dyn ::ze_node::Node> { Box::<Self>::default() }
}
```

---

## `#[node(...)]` 参数

| 键 | 类型 | 必填？ | 说明 |
|----|------|--------|------|
| `type_name` | `"string"` | ✅ | 跨序列化稳定标识；`NodeRegistry::spawn` 用它查表 |
| `display`   | `"string"` | ❌ | 缺省=struct 名称 |
| `category`  | `"string"` | ❌ | 缺省=`"Misc"`；写入常量 `CATEGORY` |
| `inputs`    | `["a","b"]`| ❌ | 每个 InputSocket 的 TypeTag.name |
| `outputs`   | `["x"]`    | ❌ | OutputSocket TypeTag.name |
| `dirty`     | `"always"` \| `"never"` \| `"on_input_change"` | ❌ 默认 `always` |
| `state`     | `"()"`     | ❌ | `state_type()` 返回的 NamedTypeTag |
| `draggable` | `true` \| `false` | ❌ 默认 `true` | 写入 `NodeMeta.interaction.draggable`；`false` 时标题栏不可拖拽 |
| `selectable`| `true` \| `false` | ❌ 默认 `true` | 写入 `NodeMeta.interaction.selectable`；`false` 时点击卡片不进选择集 |
| `resizable` | `true` \| `false` | ❌ 默认 `true` | 写入 `NodeMeta.interaction.resizable`；`false` 时卡片无 resize 握把、尺寸固定（如只读 Display 节点） |

> 交互属性只影响编辑器 UI（由命中策略/输入层读取元数据），不改变求值语义。
> 尺寸边界（`min_size/max_size`）如需自定义，手动构造 meta：
> `NodeMeta::new(..).with_interaction(NodeInteraction::default().min_size(200.0, 100.0))`。

## `#[property(...)]` 参数

| 键 | 类型 | 适用字段 | 说明 |
|----|------|----------|------|
| `label`  | `"string"` | 所有 | UI 显示名；缺省=字段名 |
| `range`  | `lo..=hi`  | Scalar/Double/Integer | 闭区间；等价于 min+max 同时写 |
| `min`    | `literal`  | Scalar/Double/Integer/VecN | 单写下界 |
| `max`    | `literal`  | Scalar/Double/Integer/VecN | 单写上界 |
| `step`   | `literal`  | Scalar/Double/Integer/VecN | UI 步长（None 表示连续） |
| `unit`   | `"string"` | Scalar/Double/Integer | 单位后缀（`"x"`, `"m/s"`…） |
| `default`| `literal`  | 所有 | Default impl 时的初始值；缺省走 `T::default()` |

## 支持的字段类型

| Rust 类型 | 映射 Property 变体 |
|-----------|-----------------|
| `f32` | `Scalar(ScalarConfig { value, ... })` |
| `f64` | `Double(DoubleConfig { ... })` |
| `i32` \| `i64` \| `u32` \| `u64` \| `isize` \| `usize` | `Integer(IntegerConfig)` (内部 `as i64`) |
| `bool` | `Bool(BoolConfig)` |
| `String` | `String(StringConfig { kind: Text })` |
| `[f32; 2]` \| `[f32; 3]` \| `[f32; 4]` | `VecN(VecNConfig { size: 2|3|4, ... })` |
| `[u8; 4]` | `Color(ColorConfig { with_alpha: true })` |

不支持的类型（如 `DynamicObject`、枚举、`Vec<_>`）请**不要**加
`#[property(...)]` 属性——它们将**不参与** `properties()` 枚举，留待手动
`impl NodeSpec` 补充即可（宏不会拒绝 struct 上存在非 property 字段）。

---

## 实现约束

- **仅支持具名字段 struct**（`struct Foo { ... }`）；tuple struct / enum
  会在展开时返回 `syn::Error`（"NodeSpec derive requires a struct with named fields"）。
- `dirty_tokens` 里 `on-input` / `oninput` 都等价于 `on_input_change`（容错）。
- `range = a..=b` 用 `ExprRange` 解析；如果解析失败会优雅降级到 ±INF。
- 错误会作为 `compile_error!` 输出到调用点，**不会 panic**（仅在 enum / union 场景
  保留了 `panic!` 作致命断言）。

---

## 依赖

```toml
[lib]
proc-macro = true

[dependencies]
proc-macro2 = "1"
quote        = "1"
syn          = { version = "2", features = ["full", "extra-traits"] }

[dev-dependencies]
ze-node-schema + ze-node-core + ze-graph-eval + ze-graph-algo  （测试链路）
```

## 测试

```bash
cargo test -p ze-node-schema-macros        # 展开 TokenStream 的快照式打印
# 真正的集成测试（验证 Node/NodeSpec 语义）在：
cargo test --test derive_spec -p ze-node-schema-macros
# 共 5 个测试，覆盖 Constant / Add / Sin / Color / VecN 五类字段
```

---

## 小抄：把 derive 用在 NodeRegistry 里

```rust
use ze_node_schema::NodeRegistry;
use ze_node_schema_macros::NodeSpec;

#[derive(NodeSpec)]
#[node(type_name = "demo.sin.f32", display = "Sin", category = "Math",
       inputs = ["f32"], outputs = ["f32"])]
struct SinF32 {
    #[property(label = "phase", range = 0.0..=6.283, step = 0.01)]
    phase: f32,
}

let mut reg = NodeRegistry::new();
reg.register::<SinF32>(SinF32::CATEGORY, SinF32::default_node);
let n = reg.spawn("demo.sin.f32").unwrap();
```
