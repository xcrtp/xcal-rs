# ze-pod-lua

> **纯 Lua 适配器**（mlua / Lua 5.4，解释器 vendored 进二进制）。
>
> 生产依赖只有 `ze-pod` / `ze-pod-binding` / `mlua`：
> **不依赖任何 `zela-*`**，不知道 scene / content / 渲染为何物。

---

## 在架构中的位置

```
kernel:  ze-pod / ze-pod-binding / ze-pod-macro
                    ▲
                    │
script:        ze-pod-lua（本 crate：Lua codec + 全局表 + run 入口）
                    ▲
                    │
scene:      zela-scene::pod（domain：SceneApi / ContentApi / 两个 Service）
                    ▲
                    │
app:        zela-app（静态插件清单 → Broker 装配 → run → commit）
```

依赖方向单向：

- `ze-pod-lua` → `ze-pod-binding`：语言无关的绑定驱动（本 crate 只实现 `LuaCodec`）；
- `ze-pod-lua` → `ze-pod`：能力 / 句柄 / Injector 机制；
- **`ze-pod-lua` 不依赖 `zela-scene`**：domain 能力由 App 通过
  `LuaPod::register_capability::<SceneApi>("scene", true)` 注入；
- `ze-pod-lua` 与 `ze-pod-py` 互不依赖。

---

## 两个方向

```text
脚本 → 宿主：Lua 调注入进来的能力表（如 scene.*）；宿主侧是原生 Rust Service。
宿主 → 脚本：Pod::call("xcal.script::run", [source])（erased 慢路径）。
```

| 入口 | 形式 | 说明 |
|------|------|------|
| `LuaPod::register_capability::<C>(service, required)` | 原生 | 装配期注册一个能力的 Lua 绑定器 |
| `LuaPod::run::<T>(source)` | 原生 | 执行 Lua，取回入口 Service 的产物 `T` |
| `Broker::run_with("lua", RUN_ENTRY, &[&source])` | 原生 | App 的推荐路径（装配与执行分离） |
| `Pod::call("xcal.script::run")` | erased | 不依赖 `ze-pod-lua` 类型的宿主 |

内部交接函数 `xcal.scene::__reset` / `__finish` 不是脚本可见 API，只走 `Pod::call`：

- 每轮 `run` 前调用入口服务的 `__reset`；
- 脚本执行完后调用 `__finish`，约定返回 `Result<Box<dyn Any>, String>`。

`ENTRY_SERVICE` 的约定名是 `scene`（解释器与录制器之间的协议，不是 crate 依赖）。

---

## 能力注册与绑定生成

```rust
let lua = LuaPod::new();
lua.register_capability::<SceneApi>("scene", true);
lua.register_capability::<ContentApi>("content", true);
lua.register_capability::<PythonApi>("python", false); // optional
```

- `required = false` ⇒ 该 Service 是 **optional**：没有提供者也能装配，
  只是 Lua 全局表中不会出现对应的表；
- `#[capability(bindings)]` 生成 `calls()`（`FnIo` + 类型擦除 trampoline）；
- `ze-pod-binding` 的 `BindingPlan` / `invoke` 是语言无关的；
- 本 crate 的 `binding.rs` 只实现 `LuaCodec`，然后按计划生成能力表与类型构造器：
  - 单 `Struct` 参数 + `Opaque` 返回 ⇒ **组件构造器**（`Circle { ... }`）；
  - `Enum` 中无字段的变体 ⇒ **值**（`Target.RevealRange`）；
  - 有字段的变体 ⇒ 构造器函数（`Curve.Phase { omega = 0.5 }`）；
  - `Array<Struct>` ⇒ 值构造器 + 数组。
- 新增一个 capability 函数**不需要改 `binding.rs`**。

---

## 文档导出

```rust
let capabilities = [
    CapabilityDocs { name: "scene", calls: SceneApi::calls(), docs: SceneApi::docs(), meta: SceneApi::meta() },
    CapabilityDocs { name: "content", calls: ContentApi::calls(), docs: ContentApi::docs(), meta: ContentApi::meta() },
];
let [json, lua] = ze_pod_lua::export_docs(dir, &capabilities)?;
```

- JSON 是唯一真源（`ze-pod-binding` 手写，无外部依赖）；
- `scene_api.lua` 是 LuaLS 注解，从同一份 `CapabilityDocs` 生成；
- 本 crate 不引用任何具体 domain 类型。

App 侧的导出入口：
`cargo run -p zela-app --example export_scene_api`。

---

## 测试

```bash
cargo test -p ze-pod-lua
```

`tests/lua_mini.rs` 用**测试内自建的 mini domain Pod**（dev-dependency 不计入生产图）验证：

- 脚本 → 宿主（`scene.emit` / `scene.kind`，含结构体 / 枚举转换）与
  宿主 → 脚本（`LuaPod::run` / `Broker::run_with`）；
- 注册名 == `capability::signatures()`；
- 文档完整性 + LuaLS / JSON 结构；
- 明确错误通道：未注入、结构体缺字段；
- 纯数据 `AssemblyPlan` 的拓扑顺序。

完整 6 镜头的分镜等价性在 `zela-app`（那里能同时访问硬编码版与脚本版）。

---

## 已知边界

| 项 | 现状 |
|----|------|
| `Transform` / 具体组件 | 由 domain 定义；本 crate 只做通用转换 |
| 每帧脚本调用 | 未做（只承担装配期编排） |
| wasm | 未做（`mlua` 的解释器不适合 wasm 内核路径；不进 wasm gate） |
| Python 每帧调用 | 未做（见 `ze-pod-py`） |
