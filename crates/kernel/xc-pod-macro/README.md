# xc-pod-macro

> **`xc-pod` 的过程宏支撑**。
> 提供 `#[capability]`（把「函数集合」struct 变成运行时协商出的 typed 句柄）
> 与 `#[derive(PodType)]`（类型定义 → `TypeSpec` + 双向转换）。
>
> 单独成 crate 是 Rust 的硬性要求：proc-macro 必须独立编译；
> `xc-pod` 本身保持零依赖。

---

## 在架构中的位置

```
xc-pod（机制：Pod / Handle / Capability / Injector）
   │
   ├──► xc-pod-macro  ← 本 crate（生成 impl Capability / impl PodType）
   │
   ├──► xc-pod-binding（消费 calls()/signatures()/docs()）
   │
   └──► xc-pod-lua / xc-pod-py（各语言适配器）
```

---

## `#[capability]`

```rust
use xc_pod::Pod;
use xc_pod_macro::capability;

#[capability(namespace = "xcal.math", version = "1", description = "数学函数", bindings)]
struct MathPod<'a> {
    /// sin(x)
    sin: fn(&'a dyn Pod, x: f32) -> f32,

    /// 2D 路径采样
    #[pod_type(param0 = "xcal.Path2D", ret = "xcal.Vec2")]
    sample: fn(&'a dyn Pod, Path2D, t: f32) -> Vec2,

    /// 方法语义：次参为 `Handle` ⇒ receiver
    bounds: fn(&'a dyn Pod, Handle) -> f32,
}
```

### 字段约定

- 每个字段就是一个函数：**字段名 = 函数名**；
- 第一个形参必须是 `&'a dyn Pod`，由宏剥掉（Pod 由句柄自己持有）；
- 其余形参（可带名字）与返回值就是最终方法签名；
- 若第一个业务形参是 `Handle`，按 **方法语义** 生成
  `pub fn bounds(&self, receiver: Handle) -> f32`
  （语法级判断类型路径最后一段；类型别名不会被识别）。

### `#[capability(...)]` 参数

| 键 | 类型 | 必填？ | 说明 |
|----|------|--------|------|
| `namespace` | `"string"` | ❌（缺省空串） | 写入每条 `FnSig.namespace` |
| `version` | `"string"` | ❌ | 写入 `CapabilityMeta.version` |
| `description` | `"string"` | ❌ | 写入 `CapabilityMeta.description` |
| `bindings` | flag | ❌ | 生成 `calls()`（`FnIo` + 类型擦除 trampoline） |

> **`bindings` 默认关闭**：开启后要求每个参数 / 返回类型都实现 `PodType`；
> 未开启时只生成 typed 句柄，不要求字段类型可绑定。

### 生成物

1. struct 本身：`__pod: &'a dyn Pod` + 各函数的裸 fn 指针字段；
2. inherent 方法：`pub fn sin(&self, x: f32) -> f32`（调用 = 一次间接寻址）
   与 `pub fn pod(&self) -> &'a dyn Pod`；
3. `impl Capability<'a>`：`signatures()` / `meta()` / `docs()` /
   `from_negotiated()`，开启 `bindings` 时另有 `calls()`；
4. 字段上的 `///` 会编译进 `docs()`；
   有参数却缺少 `///` ⇒ 编译期 `deprecated` 告警；
5. 编译期断言：`signatures()` / `docs()` / `calls()` 长度必须逐条对应。

### 类型名解析

未显式标注时生成 `<T as ::xc_pod::IntoPodTypeName>::NAME`：
类型未实现该 trait ⇒ **编译错误**（「缺失即编译错误」的落点）。

| 标注 | 作用 |
|------|------|
| `#[pod_type = "xcal.Vec2"]` | 只覆盖返回类型名 |
| `#[pod_type(param0 = "...")]` | 覆盖第 0 个业务参数的类型名 |
| `#[pod_type(paramN = "...", ret = "...")]` | 同时覆盖参数与返回 |

显式标注优先于自动推断；`paramN` 越界会在宏展开时报错。

---

## `#[derive(PodType)]`

```rust
use xc_pod_macro::PodType;

#[derive(Clone, PodType)]
#[pod_type(name = "xcal.Style")]
struct Style {
    width: f32,
    color: Color,
}

#[derive(Clone, PodType)]
#[pod_type(name = "xcal.Target")]
enum Target {
    RevealRange,
    Phase { omega: f32 },
}
```

生成：

- `impl PodType`：`SPEC`（`TypeSpec::Struct` / `TypeSpec::Enum`）、
  `to_pod_value`、`from_pod_value`；
- `impl IntoPodTypeName`：`NAME` 取自 `#[pod_type(name = "...")]`，缺省为类型名。

**支持**：具名结构体；枚举的 unit 变体与具名字段变体。

**不支持**（宏展开时返回 `syn::Error`，不 panic）：

- 泛型 / 常量参数；
- tuple struct、枚举元组变体（字段没有名字）；
- union。

---

## 测试

```bash
cargo test -p xc-pod-macro
```

`tests/` 覆盖：

| 文件 | 覆盖点 |
|------|--------|
| `bridge.rs` | dyn 侧协商、Injector（requirements / optional）、Pod ↔ 宿主回调 |
| `capability.rs` | 声明与 `signatures()` 逐条对应、全有或全无协商、快慢路径一致、显式 `pod_type` 覆盖 |
| `docs.rs` | `///` → `docs()`、与 `signatures()` 并行对应、文档不参与协商 |
| `handle.rs` | 不透明句柄：显式 release、跨 Pod 明确报错、数据留在 Pod 内 |
| `hot_path.rs` | 热路径基准（只测不优化）：快路径 vs `Pod::call` |
| `meta.rs` | `CapabilityMeta` 可查询、缺省为空、`version` 不参与协商匹配 |
| `methods.rs` | `Handle` receiver 方法语义、与自由函数共存、天然防跨 Pod |
| `pod_type.rs` | `TypeSpec` 反射、struct / enum 双向转换、`calls()` 擦除调用 |
| `registry.rs` | 闭包注册表 Pod（typed + erased）与有状态的最小 Pod |

---

## 依赖

```toml
[lib]
proc-macro = true

[dependencies]
proc-macro2 = "1"
quote = "1"
syn = { version = "2", features = ["full"] }

[dev-dependencies]
xc-pod = { path = "../xc-pod" }
```
