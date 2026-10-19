# ze-pod-py

> **纯 Python 适配器**（pyo3，`python` feature 门控）。
>
> 生产依赖只有 `ze-pod` / `ze-pod-binding` / `ze-pod-macro`（+ 可选 `pyo3`）：
> **不依赖任何 `zela-*`**，不知道 scene / content / 渲染为何物。
>
> 只承担**装配期**的活：一次算一批点 → 固化成对象。
> 每帧脚本调用不在本 crate 的范围内。

---

## 在架构中的位置

```
kernel:  ze-pod / ze-pod-binding / ze-pod-macro
                    ▲
                    │
script:        ze-pod-py（本 crate：PyCodec + 模块绑定 + run 入口）
                    ▲
                    │
scene:      zela-scene::pod（domain：SceneApi / ContentApi / 两个 Service）
                    ▲
                    │
app:        zela-app（静态插件清单 → Broker 装配 → run → commit）
```

- `ze-pod-py` → `ze-pod-binding`：只实现 `PyCodec`，
  调用编排 / 类型收集 / JSON 真源都由绑定驱动提供；
- `ze-pod-py` → `ze-pod`：能力 / 句柄 / Injector 机制；
- **不依赖 `zela-scene`**：domain 能力由 App 通过
  `PyPod::register_capability::<SceneApi>("scene", true)` 注入；
- `ze-pod-py` 与 `ze-pod-lua` 互不依赖。两者可以同时装配：
  在 Lua 侧把 `PythonApi` 注册为 optional 能力即可（见 `zela-app`）。

---

## 为什么是可选 feature

`pyo3` 需要系统 Python（或 abi3 目标），与 Lua 的
「解释器 vendored 进二进制」正面冲突。因此：

- crate 层：`python` feature 默认关闭，`cargo test -p ze-pod-py` 不需要 Python；
- App 层：`zela-app` 的 `default = ["typst", "python"]`，默认开启 Python 分镜；
  关闭后 `--script`（Lua）仍可用，只是没有 Python 节点。

```toml
[features]
default = []
python = ["dep:pyo3"]
```

---

## 两个方向

```text
Python → 宿主：装配期。注入的 scene.* / content.* 由 calls() 自动生成；
宿主 → Python：Pod::call("xcal.python::run", [source])（erased 慢路径）。
```

| 入口 | 形式 | 说明 |
|------|------|------|
| `PyPod::register_capability::<C>(service, required)` | 原生 | 装配期注册一个能力的 Python 绑定器 |
| `PyPod::run::<T>(source)` | 原生 | 执行 Python 源码，取回入口 Service 的产物 `T` |
| `Broker::run_with("python", RUN_ENTRY, &[&source])` | 原生 | App 的推荐路径（装配与执行分离） |
| `Pod::call("xcal.python::run")` | erased | 不依赖 `ze-pod-py` 类型的宿主 |

内部交接函数 `xcal.scene::__reset` / `__finish` 不是脚本可见 API，只走 `Pod::call`：

- 每轮 `run` 前调用入口服务的 `__reset`；
- 脚本执行完后调用 `__finish`，约定返回 `Result<Box<dyn Any>, String>`。

`ENTRY_SERVICE` 的约定名是 `scene`（与 `ze-pod-lua` 相同的协议，不是 crate 依赖）。

### 常量

| 常量 | 值 |
|------|-----|
| `RUN_PATH` | `xcal.python::run` |
| `RUN_ENTRY` | `xcal.python::run(String) -> Any` |
| `ENTRY_SERVICE` | `scene` |
| `RESET_PATH` | `xcal.scene::__reset` |
| `FINISH_PATH` | `xcal.scene::__finish` |

---

## 装配与执行

```rust
use std::sync::Arc;
use ze_pod_py::PyPod;

let python = Arc::new(PyPod::new()?);
python.register_capability::<SceneApi>("scene", true);
python.register_capability::<ContentApi>("content", true);

// Broker 按 requirements 注入：Python 侧出现真实的 xcal.scene / xcal.content 模块
let script = r#"
shapes = [Circle(x=0.0, y=0.0, r=1.0)]
scene.spawn(shapes)
"#;
let out: MyRecord = python.run(script)?;
```

- `required = false` ⇒ 该 Service 是 **optional**：没有提供者也能装配，
  只是 Python 侧不会出现对应模块；
- 能力模块按 namespace 落位：`xcal.scene` / `xcal.content` / `xcal.python`；
- 组件类型（单 `Struct` 参数 + `Opaque` 返回）在 Python 侧是**构造器函数**，
  返回 `PyHandle`；`Enum` 生成模块级常量与变体构造器；
- Python 侧调用注入能力时经由 `PyHandle` / `PyPodValue`，
  **不绕过句柄检查**，与 Lua 侧语义一致。

### 本 crate 自带的能力

`PythonApi`（`bindings` 开启）提供装配期批量能力：

| 函数 | 签名 | 说明 |
|------|------|------|
| `linspace` | `(f32, f32, u32) -> Array<f32>` | 真实执行 Python 代码，返回批量数据 |
| `make_dot` | `(f32, f32, f32) -> ()` | Python 侧经注入的 `scene.*` 在宿主里建对象 |

`FloatList` 是 `Array<f32>` 的具名载体（orphan 规则：不能为 `Vec<f32>`
实现外部 trait）。

---

## 线程模型

`Pod` 不要求 `Send + Sync`，而 `PyCFunction::new_closure` 要求闭包
`Send + Sync`。因此：

- Python 侧闭包只捕获一个**槽位号**，真正的 `Arc<dyn Pod>` 放在
  thread-local 服务表里（Python 调用始终在同一线程 + GIL 下）；
- 每个 `PyPod` 的模块名唯一（`sys.modules` 会复用同名模块）；
- 脚本执行期间 `sys.modules["xcal"]` 是进程级共享的，故安装 + 执行
  在 `PY_RUN_LOCK` 下串行；
- 全程不用 `unsafe`（`#![forbid(unsafe_code)]`）。

---

## 文档导出（`.pyi`）

```rust
use ze_pod_binding::CapabilityDocs;

let capabilities = [
    CapabilityDocs { name: "scene", calls: SceneApi::calls(), docs: SceneApi::docs(), meta: SceneApi::meta() },
    CapabilityDocs { name: "content", calls: ContentApi::calls(), docs: ContentApi::docs(), meta: ContentApi::meta() },
    CapabilityDocs { name: "python", calls: PythonApi::calls(), docs: PythonApi::docs(), meta: PythonApi::meta() },
];

// 只生成内存中的文件树（相对路径 → 内容）
let files = ze_pod_py::pyi_files(&capabilities);

// 或直接落盘
let written = ze_pod_py::export_docs(dir, &capabilities)?;
```

落盘布局与 capability 命名空间一致：

```
xcal/__init__.pyi     # Handle + 共享类型 + 子模块再导出
xcal/scene.pyi
xcal/content.pyi
xcal/python.pyi
```

这样 `import xcal.scene` / `from xcal import scene` 与普通 Python 模块一致；
JSON 真源在 `ze-pod-binding`，本 crate 只做翻译，不维护第二份类型表。
`.pyi` 生成**不依赖 pyo3**，因此无 `python` feature 也能运行。

---

## 测试

```bash
cargo test -p ze-pod-py                    # 不需要系统 Python
cargo test -p ze-pod-py --features python  # 需要系统 Python（pyo3）
```

`src/export.rs` 的 3 个测试验证：

- `.pyi` 路径跟随 capability namespace（`xcal/scene.pyi` 等）；
- 根包声明 `Handle`、组件构造器是函数、子模块再导出；
- capability 模块再导出 `Handle`，组件类型映射为 `Any`。

完整的双向对称性用例（Python → 宿主 → Python）在 `zela-app`。

---

## 已知边界

| 项 | 现状 |
|----|------|
| 每帧 Python 调用 | 未做（只承担装配期编排） |
| `FunctionX2Y` / 跨 Pod batch | 未做 |
| wasm | 未做（pyo3 需要宿主 Python；不进 wasm gate） |
| 生产依赖 | 不含任何 `zela-*` |
