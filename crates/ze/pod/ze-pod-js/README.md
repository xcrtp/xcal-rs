# ze-pod-js

> **纯 JS 适配器**（boa 0.22，纯 Rust 引擎，无 C/C++ 依赖）。
>
> 生产依赖只有 `ze-pod-core` / `ze-pod-binding` / `boa_engine`：
> **不依赖任何 `zela-*`**，不知道 scene / content / 渲染为何物。
>
> 只承担**装配期**的活：一次脚本 → `SceneScript`。每帧脚本调用不在本 crate 的范围内。

---

## 在架构中的位置

```
kernel:  ze-pod-core / ze-pod-binding / ze-pod-macro
                    ▲
                    │
script:        ze-pod-js（本 crate：JsCodec + 命名空间绑定 + run 入口）
                    ▲
                    │
scene:      zela-scene::pod（domain：SceneApi / ContentApi / 两个 Service）
                    ▲
                    │
app:        zela-app（静态插件清单 → Broker 装配 → run → commit）
```

依赖方向单向：

- `ze-pod-js` → `ze-pod-binding`：语言无关的绑定驱动（本 crate 只实现 `JsCodec`）；
- `ze-pod-js` → `ze-pod-core`：能力 / 句柄 / Injector 机制；
- **`ze-pod-js` 不依赖 `zela-scene`**：domain 能力由 App 通过
  `JsPod::register_capability::<SceneApi>("scene", true)` 注入；
- `ze-pod-js` 与 `ze-pod-lua` / `ze-pod-py` 互不依赖。三者可以同时装配
  （`zela-app` 里 Lua 侧把 `PythonApi` 注册为 optional 即可）。

---

## 为什么是 boa

这是本 crate 唯一的引擎选择，理由不是"轻"，而是**默认引擎必须跨平台一致**：

| 引擎 | 实现 | 能编到 `wasm32-unknown-unknown` |
|---|---|---|
| **boa** | 纯 Rust | ✅ |
| `deno_core` | 依赖 V8（C++） | ❌ |

若 native 用 V8、wasm 用浏览器原生 JS，就等于**在两个不同实现上断言"产出一致"** ——
与项目已踩过的 `[browser]` 段"1 帧 1 像素"差异是同型问题。
因此 native 与 wasm 跑**同一个** boa；可选宿主引擎只能由显式 feature 打开。

为压低 wasm 体积，`boa_engine` 关闭了默认 features（`float16` / `xsum` / `temporal`），
wasm 目标额外打开 `js`（getrandom `wasm_js` + time `wasm-bindgen`）。

---

## feature

| feature | 目标 | 行为 |
|---|---|---|
| `boa`（默认） | native + wasm | 纯 Rust 引擎，两边同一个实现 |
| `host-js`（可选，**本批次未接线**） | 仅 wasm | 非 wasm 启用 ⇒ **编译期报错**（J3）；wasm 启用 ⇒ 显式"尚未接线" |
| `send-probe` | 任意 | 只用于负向编译探针 `examples/send_probe.rs`，正常构建不启用 |

```toml
[features]
default = ["boa"]
boa = ["dep:boa_engine"]
host-js = []
```

`zela-app` 侧对应 `js = ["dep:ze-pod-js"]`，**默认关闭**：
不开 `js` 时 App 行为与改动前完全一致，`zela-web` 的浏览器产物也不变。

---

## 两个方向

```text
脚本 → 宿主：装配期。注入的 scene.* / content.* 由 calls() 自动生成；
宿主 → 脚本：Pod::call("zela.script::run", [source])（erased 慢路径）。
```

| 入口 | 形式 | 说明 |
|------|------|------|
| `JsPod::register_capability::<C>(service, required)` | 原生 | 装配期注册一个能力的 JS 绑定器 |
| `JsPod::run::<T>(source)` | 原生 | 执行 JS 源码，取回入口 Service 的产物 `T` |
| `Broker::run_with("js", RUN_ENTRY, &[&source])` | 原生 | App 的推荐路径（装配与执行分离） |
| `Pod::call("zela.script::run")` | erased | 不依赖 `ze-pod-js` 类型的宿主 |

内部交接函数 `zela.scene::__reset` / `__finish` 不是脚本可见 API，只走 `Pod::call`：

- 每轮 `run` 前调用入口服务的 `__reset`；
- 脚本执行完后调用 `__finish`，约定返回 `Result<Box<dyn Any>, String>`。

`ENTRY_SERVICE` 的约定名是 `scene`（与 `ze-pod-lua` 相同的协议，不是 crate 依赖）。

### 常量

| 常量 | 值 |
|------|-----|
| `RUN_PATH` | `zela.script::run`（与 Lua 同名：入口协议一致） |
| `RUN_ENTRY` | `zela.script::run(String) -> Any` |
| `ENTRY_SERVICE` | `scene` |
| `RESET_PATH` | `zela.scene::__reset` |
| `FINISH_PATH` | `zela.scene::__finish` |

---

## 能力注册与绑定生成

```rust
use std::sync::Arc;
use ze_pod_js::JsPod;

let js = Arc::new(JsPod::new());
js.register_capability::<SceneApi>("scene", true);
js.register_capability::<ContentApi>("content", true);

let script = r#"
const dot = Circle({ x: 0.0, y: 0.0, r: 1.0 });
scene.spawn([dot]);
"#;
let out: MyRecord = js.run(script)?;
```

- `required = false` ⇒ 该 Service 是 **optional**：没有提供者也能装配，
  只是 JS 全局对象上不会出现对应命名空间；
- `#[capability(bindings)]` 生成 `calls()`（`FnIo` + 类型擦除 trampoline）；
- 语言无关部分（`BindingPlan` / `invoke` / 类型收集 / JSON 真源）在 `ze-pod-binding`；
- 本 crate 的 `binding.rs` 只实现 `JsCodec`，然后按计划生成命名空间与类型构造器。

### JS 侧的形状（与 Lua / Python 的对应关系）

| 概念 | Lua | JS |
|------|-----|----|
| 能力命名空间 | `scene.emit(1)` | `scene.emit(1)` |
| 组件构造器（`Struct` 入参 + `Opaque` 返回） | `Circle { x = 0, y = 0, r = 1 }` | `Circle({ x: 0, y: 0, r: 1 })` ⇒ 句柄令牌 |
| 纯值结构体 | `Color { r = .. }` | `Color({ r: .. })` ⇒ 值令牌 |
| `Enum` unit 变体 | `Target.RevealRange` | `Target.RevealRange` ⇒ 值令牌 |
| `Enum` 带字段变体 | `Curve.Ramp { from = .. }` | `Curve.Ramp({ from: .. })` |
| `Array<T>` | Lua table | JS 数组 |
| `()` 返回值 | `nil` | `undefined` |
| 句柄 / 值 / 枚举令牌 | userdata | 带隐藏属性 `__zela_pod` 的对象（令牌表下标） |

结构体入参同时接受**普通对象**（`{ x: 1, y: 2 }`）与令牌；枚举必须用构造器创建
（与 Lua 一致，保证变体身份不丢）。新增一个 capability 函数**不需要改 `binding.rs`**。

---

## JS 只有 number：整数映射必须显式

JS 没有 `i32` / `u32` / `u64` 的区分，只有 f64。本 crate **不静默截断**：

| `ScalarKind` | JS 侧校验 |
|---|---|
| `F32` / `F64` | 必须是 `number`；`F32` 按 f32 落值（与 Lua 同口径） |
| `I32` | 整值 + `[i32::MIN, i32::MAX]` |
| `U32` | 整值 + `[0, u32::MAX]` |
| `U8` | 整值 + `[0, 255]` |
| `U64` | 整值 + `[0, 2^53 - 1]`（JS **可精确表示**的整数上限） |
| `Bool` / `String` | 类型不符 ⇒ 明确错误 |

越界 / 小数 ⇒ `PodError::CallFailed`（错误信息带实际值），不会悄悄变成 `0` 或截断。
反向（`PodValue` → JS）同样：`U64` 超过 `2^53 - 1` 显式报错。

---

## 线程模型

`boa_engine::Context` 是 **`!Send + !Sync`**（负向编译探针见下），因此：

- `JsPod` 与 `LuaPod` 一样登记**非 sync** 节点（`PodNode::new`），不声明 `SyncPod` 徽章；
- `NativeFunction` 的闭包必须是 `Copy`，捕获不了 `Arc<dyn Pod>`；
  ⇒ Service 表与令牌表放在 `Context` 的 host-defined 数据里（`HostState`），
  闭包只捕获 `ErasedFn`（`Copy`）与 **service 下标**（`usize`）；
- 所有转换在装配期发生，单线程，无锁。

---

## 文档导出（`.d.ts`）

```rust
use ze_pod_binding::CapabilityDocs;

let capabilities = [
    CapabilityDocs { name: "scene", calls: SceneApi::calls(), docs: SceneApi::docs(), meta: SceneApi::meta() },
    CapabilityDocs { name: "content", calls: ContentApi::calls(), docs: ContentApi::docs(), meta: ContentApi::meta() },
];

// 只生成文本
let text = ze_pod_js::d_ts(&capabilities);

// 或直接落盘（scene_api.json + scene_api.d.ts）
let written = ze_pod_js::export_docs(dir, &capabilities)?;
```

- JSON 是唯一真源（`ze-pod-binding`），`.d.ts` 只是翻译，不维护第二份类型表；
- JS 侧没有 LuaLS 那种注解方言，TypeScript 声明是**工具链原生**的补全来源；
- 本 crate 不引用任何具体 domain 类型。

App 侧的导出入口（需 `js` feature）：

```bash
cargo run -p zela-app --example export_scene_api
# target/scene_api/ 下多一个 scene_api.d.ts
```

---

## 测试

```bash
cargo test -p ze-pod-js
```

`tests/js_mini.rs` 用**测试内自建的 mini domain Pod**（dev-dependency 不计入生产图）验证：

- 脚本 → 宿主（`scene.emit` / `scene.kind`，含结构体 / 枚举转换）与
  宿主 → 脚本（`JsPod::run` / `Broker::run_with`）；
- 注册名 == `capability::signatures()`；
- 文档完整性 + `.d.ts` / JSON 结构；
- 明确错误通道：未注入、结构体缺字段、**整数小数 / 越界**；
- 纯数据 `AssemblyPlan` 的拓扑顺序。

跨语言与真实 domain 的用例在别处：

| 位置 | 断言 |
|---|---|
| `ze-pod-tests/tests/languages.rs` | `lua_and_js_agree_on_the_same_binding_cases`（同一份 `MiniApi` 逐字段相同） |
| `zela-app`（`js` feature） | `js_and_lua_produce_the_same_scene_script`（`SceneScript` 规范 JSON 逐字节相同） |

### 负向编译探针

```bash
cargo check -p ze-pod-js --features send-probe --example send_probe   # 必须失败
```

失败信息是 `Rc<dyn HostHooks>` / `Rc<dyn Clock>` 等"cannot be sent between threads safely"，
即 `Context: !Send + !Sync`。若哪天 boa 变成线程安全，本探针会通过 ——
届时应把 `JsPod` 改成 `PodNode::new_sync` 并删除该文件。

---

## 已知边界

| 项 | 现状 |
|----|------|
| `host-js`（浏览器原生引擎） | **未接线**（B3-Pe 可选）：非 wasm 启用 ⇒ 编译期报错；wasm 启用 ⇒ 显式"尚未接线" |
| wasm | 可编译（`cargo check --target wasm32-unknown-unknown` 通过），但**未**做 wasm 运行时对拍；不进 wasm gate |
| 每帧脚本调用 | 未做（只承担装配期编排） |
| wasm 体积 | `js` 默认关闭且 `zela-web` 不引用本 crate ⇒ 浏览器产物体积不变 |
| 生产依赖 | 不含任何 `zela-*` |
