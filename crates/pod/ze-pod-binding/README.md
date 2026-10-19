# ze-pod-binding

> **语言无关的绑定驱动**。
>
> 把 capability 的 `calls()` 摊平成「函数 + 可达类型」，交给各语言适配器
> 自行建模块 / 闭包；参数校验、`PodValue` 编解码、返回值回写由本 crate
> 统一编排。**本 crate 不含任何语言运行时类型。**

---

## 在架构中的位置

```
kernel:  ze-pod ──► ze-pod-binding（本 crate）──► ze-pod-macro
                          ▲                ▲
                          │                │
script:            ze-pod-lua        ze-pod-py
                  ValueCodec=LuaValue ValueCodec=PyObject
                          ▲                ▲
                          └──── zela-app ──┘（静态清单装配）
```

依赖方向单向：`ze-pod-binding` 只依赖 `ze-pod`；适配器依赖本 crate，
本 crate **不反向依赖任何适配器**，也不依赖任何 `zela-*`。

---

## 三层职责

| 模块 | 关键类型 | 说明 |
|------|----------|------|
| `plan` | `BindingPlan` / `component_ctor` / `collect_types` / `short_name` | 把 `C::calls()` 摊平成函数清单 + 递归可达类型表 |
| `codec` | `ValueCodec` / `invoke` | 语言值 ↔ `PodValue` 的 trait，以及所有语言共用的调用编排 |
| `docs` | `CapabilityDocs` / `capabilities_json` / `reachable_types` | 手写 JSON 文档（唯一真源），供 LuaLS / `.pyi` 等派生 |

---

## BindingPlan — 能力计划

```rust
use ze_pod::Capability;
use ze_pod_binding::BindingPlan;

let plan = BindingPlan::new(MyApi::calls());
assert_eq!(plan.calls.len(), MyApi::signatures().len()); // 同名同序

// 可达命名类型（递归收集 struct / enum，不含 `()`）
for (name, spec) in &plan.types {
    println!("{name}: {spec:?}");
}

// 「单参数是该结构体 + 返回不透明句柄」⇒ 组件构造器
if let Some(ctor) = plan.component_ctor("xcal.Circle") {
    let out = (ctor.call)(&*pod, &[value])?; // 类型擦除调用
}
```

`component_ctor` 是各语言侧「`Circle { .. }` 构造语法」的判定依据：
适配器据此把结构体绑定成**组件构造器**而不是普通值构造器。

---

## ValueCodec + invoke — 一次调用的通用编排

适配器只需实现 `ValueCodec`：

```rust
use ze_pod::TypeSpec;
use ze_pod_binding::ValueCodec;

impl ValueCodec for MyCodec {
    type Value = MyValue;
    type Error = MyError;

    fn to_pod(&self, value: &MyValue, spec: &'static TypeSpec) -> Result<PodValue, MyError> { /* ... */ }
    fn from_pod(&self, value: &PodValue) -> Result<MyValue, MyError> { /* ... */ }
    fn error(&self, message: String) -> MyError { /* ... */ }
}
```

`invoke` 负责其余全部步骤：

1. 参数个数必须等于 schema（否则 `codec.error(..)`）；
2. 逐参 `codec.to_pod`；
3. 调 erased trampoline（一次间接寻址）；
4. `codec.from_pod` 回写返回值。

```rust
use ze_pod_binding::invoke;

let out = invoke(&codec, &service_pod, item, &args)?;
```

这也是 `ze-pod-lua` / `ze-pod-py` 能共享同一套调用语义、
新增 capability 函数**不需要改适配器**的原因。

---

## docs — JSON 是唯一真源

```rust
use ze_pod_binding::{CapabilityDocs, capabilities_json, reachable_types};

let capabilities = [
    CapabilityDocs { name: "scene", calls: SceneApi::calls(), docs: SceneApi::docs(), meta: SceneApi::meta() },
    CapabilityDocs { name: "content", calls: ContentApi::calls(), docs: ContentApi::docs(), meta: ContentApi::meta() },
];

let json = capabilities_json(&capabilities);
let types = reachable_types(&capabilities);
```

输出结构：

```json
{
  "api": "xcal.script",
  "capabilities": { "<name>": { "version": "...", "description": "...", "functions": [ ... ] } },
  "types": [ { "kind": "struct|enum", "name": "...", "fields": [ ... ] } ]
}
```

- 手写 JSON，**零外部依赖**；
- 类型引用（`params` / `returns`）只给名字，定义为顶层 `types` 表；
- `ze-pod-lua` 的 LuaLS 注解与 `ze-pod-py` 的 `.pyi` 都从同一份
  `CapabilityDocs` 派生，不各自维护第二份类型表。

---

## 约束与验证

- `#![forbid(unsafe_code)]`；
- `cargo tree -p ze-pod-binding` 只有 `ze-pod` + 自身；
- 可进 wasm gate（无语言运行时依赖）。

```bash
cargo check -p ze-pod-binding
cargo check -p ze-pod-binding --target wasm32-unknown-unknown
cargo tree -p ze-pod-binding
```

本 crate 的语义由下游适配器覆盖：`cargo test -p ze-pod-lua`（Lua 侧）
与 `cargo test -p ze-pod-py`（`.pyi` 生成）会实际驱动 `plan` / `codec` / `docs`。
