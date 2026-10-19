# WASM 移植适配面探测报告

> 只读探测：未修改任何生产代码，未提交 commit。
> 探测环境：Windows / rustc 1.96.0 / target `wasm32-unknown-unknown`（已安装）/
> wgpu 30.0.1 / 本机 adapter = NVIDIA RTX 4060 Laptop（Vulkan）。
>
> 结论先行：**Lua 无法编译到 wasm（内容来源缺失）；渲染后端有 23 个编译错误；
> 主体 crate 虽然编译通过，但默认 telemetry 会在 wasm 运行时立刻 panic。**
> wgpu 本身没有踩 push constants / 硬编码 limits 这两个"黑屏"雷。
>
> ---
>
> **状态更新（2026-10-19）**：本报告的「未做项」多数已被后续阶段
> **S1–S7 解决**，详见仓库根 [`GAPS.md`](GAPS.md) 的 B 段。
> 本文件**保留作为当时的探测证据，不删除**；§7 的每条已补状态列。
> 阅读时请以状态列为准，不要把历史结论当成当前缺口。

---

## 0. 结论摘要

| 探测 | 结果 | 阻塞等级 |
|---|---|---|
| 一、Lua → wasm | **失败**：`lua-src` 构建脚本 "don't know how to build Lua for wasm32-unknown-unknown" | **P0（内容来源缺失）** |
| 二、主体编译 | `xcal-schedule` / `xcal-scene` / `xcal-render` **通过**；`xcal-render-backends` **23 × E0277** | 见 §4 |
| 二、运行时 | `Instant::now()` 每帧调用（telemetry 默认开）→ wasm 上 panic | **P0（编译通过但必炸）** |
| 三、wgpu | `is_webgpu_compliant = true`；**push constants 0 命中**；**无硬编码 limits**；wgpu 30.0.1 | 低风险 |

一句话：**浏览器移植的真正顺序是「先换 Lua 供给 → 再解 Send/Sync → 再关掉 wasm 不支持的 std 时间/线程」，而不是先改渲染。**

---

## 1. 方法与范围

- workspace 共 **31 个 crate**；逐 crate 执行
  `cargo check -p <crate> --target wasm32-unknown-unknown`。
- 编译错误按 `E0xxx` + 命中文件归类；运行时风险按 `std` 的 wasm PAL 行为判定
  （`library/std/src/sys/pal/wasm` 的注释明确：所有 OS 级函数都是 stub）。
- wgpu 能力面：临时 example（跑完即删）读取
  `Adapter::get_info()` / `get_downlevel_capabilities()` / `limits()` / `features()`；
  全仓 grep `push_constant` / `Limits::` / `required_limits` / `Features::`。
- 唯一临时文件 `examples/_probe_adapter.rs` 已删除，`git status` 干净。

---

## 2. 探测一：Lua 能否编译到 wasm（失败）

```text
   Compiling mlua-sys v0.13.0
error: failed to run custom build command for `mlua-sys v0.13.0`

Caused by:
  process didn't exit successfully: `...\build\mlua-sys-...\build-script-main` (exit code: 101)
  --- stderr
  thread 'main' panicked at lua-src-551.0.2\src\lib.rs:100:25:
  don't know how to build Lua for wasm32-unknown-unknown
```

### 判定

- **不是 setjmp/longjmp 问题**：失败在 C 源码构建脚本阶段，Rust 代码尚未开始编译。
- 根因是 [xc-pod-lua/Cargo.toml](../../crates/pod/xc-pod-lua/Cargo.toml)
  启用了 `mlua` 的 `vendored` feature，而 `lua-src` 的 `build.rs` 没有
  `wasm32-unknown-unknown` 分支，直接 `panic!`。
- 这是**构建目标缺失**，不是 ABI 不兼容。

### 影响

浏览器端**没有 Lua 内容来源**。`xcal-app` / `xcal-gui` 都在此被阻断
（`xc-pod-lua` 是 `xcal-app` 的无条件依赖，即使 `--no-default-features` 也绕不开）。

---

## 3. 探测二：全库 wasm 编译矩阵（31 个 crate）

| crate | 结果 | 备注 |
|---|---|---|
| xc-notion | PASS | |
| xc-render-ir | PASS | |
| xc-scene-contract | PASS | |
| xc-content | PASS | |
| xc-prerender | PASS | 默认 features（placeholder）；`--features typst` 也 PASS |
| xc-time | PASS | |
| xc-vertex | PASS | |
| xc-resource | PASS | 但 `Send + Sync` 约束是后端错误根因（见 §4.1） |
| xc-macro | PASS | |
| xc-pod | PASS | |
| xc-pod-binding | PASS | |
| xc-pod-macro | PASS | proc-macro（host 编译，不作为 wasm 产物） |
| xc-object | PASS | |
| xc-node | PASS | |
| xc-graph-algo | PASS | |
| xc-graph-eval | PASS | |
| xc-node-schema | PASS | |
| xc-node-schema-macros | PASS | proc-macro |
| xcal-scene | **PASS** | 含 hecs / serde |
| xcal-schedule | **PASS** | **运行时必炸（§5.1）** |
| xcal-render | **PASS** | 默认 `wgpu` feature 也通过 |
| xcal-render-backends | **FAIL** | 23 × E0277（§4.1） |
| xc-pod-lua | **FAIL** | `lua-src` 构建脚本（§2） |
| xc-pod-py | PASS（默认）/ **FAIL（`--features python`）** | pyo3 交叉编译要求 abi3（§4.3） |
| xc-pod-tests | 未探测 | 测试专用 crate，依赖两个语言适配器 |
| xcal-egui | **FAIL** | `NativeOptions` 仅 native（§4.2） |
| xcal-node-editor | PASS | |
| xcal-node-egui | PASS | |
| xcal-node-egui-ui | PASS | |
| xcal-app | **FAIL** | mlua（+ 默认 features 下的 pyo3） |
| xcal-gui | **FAIL** | 顶层 `--no-default-features` 也无效（§4.4） |

统计：**PASS 25 / FAIL 5 / 未探测 1**。

> 重要：CI 的 wasm gate 只覆盖 11 个 kernel/pod 核心 crate；
> 本轮证明 `xcal-schedule` / `xcal-scene` / `xcal-render` 也能编译到 wasm
> （但这不等于能运行，见 §5）。

---

## 4. 编译错误清单（按根因）

### 4.1 `xcal-render-backends`：23 × E0277（全部同一个根因）

**报错形态**（节选）：

```text
error[E0277]: `Rc<RefCell<wgpu::backend::webgpu::WebBufferMapState>>`
              cannot be shared between threads safely
  --> crates/render/xcal-render-backends/src/wgpu_3d.rs:997:26
  = help: within `RevealBuffers`, the trait `Sync` is not implemented for ...
```

**命中分布**：

| 文件 | 提及次数 |
|---|---|
| `crates/render/xcal-render-backends/src/wgpu_2d.rs` | 24 |
| `crates/render/xcal-render-backends/src/wgpu_3d.rs` | 22 |
| `crates/render/xcal-render-backends/src/gpu_util.rs` | 1 |

**约束点（真正的根因，全部在 kernel / trait 定义）**：

| 位置 | 约束 |
|---|---|
| [xc-resource/src/render_backend.rs:101](../../crates/kernel/xc-resource/src/render_backend.rs) | `GpuHandle::new<T: Any + Send + Sync>` |
| [xc-resource/src/render_backend.rs:122](../../crates/kernel/xc-resource/src/render_backend.rs) | `GpuHandle::downcast_ref<T: Any + Send + Sync>` |
| [xcal-render/src/render_backend.rs:56](../../crates/render/xcal-render/src/render_backend.rs) | `pub trait RenderBackend3D: Send + Sync` |

**原因**：native 上 wgpu 的 core backend 类型（`Buffer` / `Texture` / `BindGroup` …）
是 `Send + Sync`；wasm 上 wgpu 切到 **webgpu backend**，`WebBuffer` 内含
`Rc<RefCell<WebBufferMapState>>`、`Rc<Cell<u32>>`、`Rc<DropGuard>` —— 这些类型
**故意不是 `Send` / `Sync`**（浏览器资源只能在创建它的线程上用）。
只要后端把 wgpu 资源塞进要求 `Send + Sync` 的 `GpuHandle` / `RenderBackend3D`，
就必然编译失败。

**影响面**：只要走 GPU 就绕不开；不是"某个函数"的问题，而是
「资源句柄抽象假设了线程安全」这一层假设在 wasm 上不成立。

### 4.2 `xcal-egui`：2 × E0425 / E0422

```text
error[E0425]: cannot find type `NativeOptions` in crate `eframe`
  --> crates/ui/xcal-egui/src/frame.rs:24:36
error[E0422]: cannot find struct, variant or union type `NativeOptions` in crate `eframe`
  --> crates/ui/xcal-egui/src/frame.rs:25:13
```

**原因**：`eframe` 的 `NativeOptions` / `run_native` 都带
`#[cfg(not(target_arch = "wasm32"))]`；wasm 上入口是 `start_web` / `WebRunner`
一类的 API。

**影响**：GUI 壳需要一份 wasm 专用入口（窗口参数类型不同），
`window_options()` 与 `main()` 都不能原样复用。

### 4.3 `xc-pod-py --features python`

```text
error: PYO3_CROSS_PYTHON_VERSION or either an abi3-py3* or abi3t-py3* feature
       must be specified when cross-compiling and PYO3_CROSS_LIB_DIR is not set.
```

**原因**：pyo3 需要宿主 Python 或 abi3 交叉编译配置；wasm 上还额外要求
CPython 本身被编进 wasm（pyo3 支持 wasm32-wasi/emscripten 的特定配置，
本项目未做）。默认 feature 下 `xc-pod-py` 不编 pyo3，所以 PASS。

### 4.4 `xcal-app` / `xcal-gui`

- `xcal-app --no-default-features` 仍被 `mlua-sys` 阻断：`xc-pod-lua` 是**无条件依赖**，
  没有 feature 可以关掉它。
- `xcal-gui --no-default-features` 仍编译 `pyo3-ffi` + `mlua-sys`：因为
  [xcal-gui/Cargo.toml](../../crates/app/xcal-gui/Cargo.toml) 里
  `xcal-app = { path = "../xcal-app" }` **没有** `default-features = false`
  ⇒ Cargo feature 合并仍会打开 `xcal-app` 的 `python` 默认 feature。
  **顶层的 `--no-default-features` 关不掉下层默认 feature**。

---

## 5. 运行时致命点清单（编译通过，但 wasm 上不可用）

> std 的 wasm PAL 明确写着：OS 级功能都是 stub。
> `Instant::now()` / `SystemTime::now()` 直接 `panic!("time not implemented on this platform")`；
> `thread::sleep` 在无 atomics 时 `panic!("can't sleep")`；
> `thread::spawn` 返回 `Err(UNSUPPORTED)`；
> `fs` 返回错误；`process` 返回 `unsupported()`。

### 5.1 时间：`Instant::now()` —— **P0，最致命**

| 位置 | 语义 | wasm 后果 |
|---|---|---|
| [xcal-schedule/src/scheduler.rs:247](../../crates/schedule/xcal-schedule/src/scheduler.rs) | telemetry：**每个 System 每帧**取 `Instant::now()` | **第一帧即 panic**；`xcal-schedule` 的 `default = ["telemetry"]` ⇒ 默认中招 |
| [xcal-egui/src/frame.rs:46,53](../../crates/ui/xcal-egui/src/frame.rs) | 墙钟 dt（`now - last_frame`） | 第一帧 `Instant::now()` 即 panic |
| [xcal-app/src/export.rs:159,172](../../crates/app/xcal-app/src/export.rs) | 导出耗时统计 | 导出路径 panic（非主循环） |
| [xcal-render-backends/src/compositor.rs:1487,1491](../../crates/render/xcal-render-backends/src/compositor.rs) | GPU 测试计时 | 仅测试 |
| `examples/cube_3d.rs:123` | example 计时 | 仅 example |
| `xc-pod-macro/tests/hot_path.rs:42,51` | 基准测试 | 仅测试 |

**注意**：`xcal-schedule` 只是"编译通过"，**默认 telemetry 会把它变成运行时炸弹**。
浏览器侧必须 `default-features = false` 或接 web 时间源。

### 5.2 线程

| 位置 | 调用 | wasm 行为 |
|---|---|---|
| [xc-pod/src/command.rs:178,189](../../crates/pod/xc-pod/src/command.rs) | `thread::yield_now()`（`Receipt::wait` / `wait_pumping`） | no-op ⇒ 忙等；且主线程本来禁止 `wait()` |
| `xcal-render-backends/src/gpu_test.rs:60` | `thread::sleep` | 无 atomics ⇒ panic（有 atomics ⇒ 阻塞主线程） |
| `xcal-render-backends/src/gpu_test.rs:146` | `thread::Builder::spawn` | `Err(UNSUPPORTED)` ⇒ `.expect` panic |
| `xcal-scene/src/scene_script.rs:510` | `thread::spawn`（**仅测试**） | 仅测试 |

生产热路径没有 `thread::spawn`；`yield_now` 是唯一生产命中。

### 5.3 文件系统（`std::fs`）

| 位置 | 用途 |
|---|---|
| [xcal-egui/src/host.rs:81](../../crates/ui/xcal-egui/src/host.rs) | **CJK 字体按路径读盘**（GUI 启动路径） |
| [xcal-app/src/export.rs:146,215](../../crates/app/xcal-app/src/export.rs) | 导出目录 / PNG 写盘 |
| `xcal-app/src/bin/xcal-render.rs:128,223,274` | CLI 读脚本 / 写帧 |
| `xc-prerender/src/typst/mod.rs:217` | 测试写 SVG 快照 |
| `xcal-node-egui/examples/node_editor_demo.rs:473` | example 读字体 |

wasm 上 `fs::read` 返回 `Err` ⇒ 字体静默缺失（代码里是 `.ok()?`），
导出/CLI 路径不可用。浏览器需要 fetch/IndexedDB/下载替代。

### 5.4 子进程（`std::process::Command`）

| 位置 | 用途 |
|---|---|
| `xcal-app/src/bin/xcal-render.rs:257,277` | 调 ffmpeg 合成 mp4 |

仅 CLI 二进制；wasm 不构建该 bin。浏览器侧需要 `MediaRecorder`/WebCodecs 等替代。

### 5.5 阻塞式 GPU 等待

| 位置 | 语义 | wasm 影响 |
|---|---|---|
| [render_context.rs:47](../../crates/render/xcal-render-backends/src/render_context.rs) | `pollster::block_on(request_adapter/request_device)` | 主线程阻塞：isomorphic 环境下会死锁/触发 "can't sleep" |
| [render_context.rs:154-157](../../crates/render/xcal-render-backends/src/render_context.rs) | `map_async` + `poll(wait_indefinitely)` 同步回读 | 浏览器主线程禁止阻塞等待；必须改异步 |
| `wgpu_2d.rs` / `wgpu_3d.rs` / `compositor.rs` / `scene_callback.rs` 中 20 处 `poll(wait_indefinitely)` | **全部位于 `#[cfg(test)]`** | 仅测试 |

好消息：**主渲染循环不轮询/不阻塞**；阻塞只集中在
「上下文创建」与「像素回读（导出/测试）」。

### 5.6 `libc` / 平台依赖

| 位置 | 用途 |
|---|---|
| `xcal-render-backends/src/gpu_test.rs:174,185` | 直接 `libc::write(2, ...)` 绕过 libtest 捕获 |

仅测试汇报路径；但该模块在 `feature = "wgpu"` 下编进库（不是 `#[cfg(test)]`），
移植时需要单独处理。

---

## 6. 探测三：wgpu 能力面（native）

### 6.1 版本

- `wgpu 30.0.1`（`wgpu-core 30.0.1`、`wgpu-hal` / `wgpu-types` 同版本，见 `Cargo.lock`）。
- workspace 声明：`wgpu = "30"`。

### 6.2 Downlevel 等级

```text
ADAPTER: NVIDIA GeForce RTX 4060 Laptop GPU / Vulkan / DiscreteGpu
IS_WEBGPU_COMPLIANT: true
```

本机 adapter 的下限能力**覆盖 WebGPU 要求**（`DownlevelFlags` 含
`WEBGPU_TEXTURE_FORMAT_SUPPORT`、`FRAGMENT_STORAGE`、`INDEPENDENT_BLEND` 等）。
注意：这是 **native Vulkan** 的答案；浏览器里实际拿到的是浏览器的 WebGPU
adapter，需在浏览器内再测一次（本轮无浏览器环境）。

### 6.3 push constants

全仓 grep `push_constant` / `push_constant_ranges`：**0 命中**。

结论：**未使用 push constants**，不构成 WebGPU 阻塞。
（`adapter.limits().max_immediate_size = 256` 是 wgpu 30 的命名，
我们没有任何 `PushConstantRange`。）

### 6.4 硬编码 limits / features

全仓 grep `Limits::` / `required_limits` / `required_features` / `Features::`：**0 命中**。

- 设备创建使用 `wgpu::DeviceDescriptor::default()`
  （[render_context.rs:62](../../crates/render/xcal-render-backends/src/render_context.rs)）
  ⇒ 请求的是 wgpu 默认（= WebGPU 默认）limits / 空 features。
- 没有 `request_adapter` 的 `force_fallback_adapter` / 自定义 limits。
- 结论：**不存在"native 能跑、WebGPU 超限"的硬编码 limits 风险。**

### 6.5 实际使用的格式与特性（WebGPU 合规性抽查）

| 项 | 值 | WebGPU 合法性 |
|---|---|---|
| 离屏 2D 格式 | `Rgba8Unorm` | ✓ |
| 主目标格式 | `Bgra8UnormSrgb` | ✓ |
| 3D 深度 | `Depth32Float` | ✓ |
| 3D 颜色 | `Rgba8UnormSrgb` | ✓ |
| MSAA | `MSAA_SAMPLES = 4` | ✓ |
| 着色器 | WGSL（`include_str!` 的 `.wgsl`） | ✓ |

### 6.6 一处 API 层面的 wasm 适配点

`Instance::default()` + `request_adapter(...).await`（wgpu 30 返回 `Result`）
在 wasm 上是**必须异步驱动**的；当前 `headless()` 用 `pollster::block_on` 包住，
浏览器主线程不可用（见 §5.5）。GUI 路径走 `from_render_state`，不受影响。

---

## 7. 适配面地图（按优先级）

### P0 — 不做就无法产出画面

| # | 项 | 位置 | 说明 | 状态（2026-10-19） |
|---|---|---|---|---|
| P0-1 | **Lua 内容来源** | `xc-pod-lua` / `lua-src` | 必须换 wasm 可构建的 Lua 供给；否则浏览器无脚本 | **已解决（路线变更）**：浏览器只回放 XSB，`xcal-playback` / `xcal-web` 不依赖 Lua |
| P0-2 | **telemetry 的 `Instant::now()`** | `xcal-schedule/src/scheduler.rs:247` | 默认 feature 下每帧必 panic；需关 telemetry 或换时间源 | **已解决（W3）**：默认 `ZeroClock`，native 宿主注入 `InstantClock` |
| P0-3 | **`GpuHandle` / `RenderBackend3D` 的 `Send + Sync`** | `xc-resource` / `xcal-render` | wasm wgpu 资源不是 Send/Sync；23 个编译错误全源于此 | **已解决（第一阶段）**：`GpuHandle` 改为 `Box<dyn Any>`，trait 约束移除 |
| P0-4 | **GUI 的 dt 来源** | `xcal-egui/src/frame.rs:46,53` | `Instant` 不可用；需 `performance.now()` | **范围外**：浏览器薄壳用 rAF 固定 `dt`；`xcal-egui` 不进浏览器 |
| P0-5 | **GUI 入口** | `xcal-egui/src/frame.rs:24` / `xcal-gui/src/main.rs` | `NativeOptions` / `run_native` 仅 native；需 wasm 入口 | **范围外**：wasm 入口由 `xcal-web` 提供 |

### P1 — 画面能出，但功能缺失

| # | 项 | 位置 | 说明 | 状态（2026-10-19） |
|---|---|---|---|---|
| P1-1 | 字体按路径读盘 | `xcal-egui/src/host.rs:81` | 浏览器需 fetch / 打包字体 | **已解决**：XSB 在 native 把字形烘焙成路径；`xcal-web` 无字体代码 |
| P1-2 | 同步 GPU 回读 | `render_context.rs:154` | 导出/截图需改异步 | **已解决（S3）**：`read_texture_async` |
| P1-3 | 上下文创建 `block_on` | `render_context.rs:47` | headless 路径需 async | **已解决（S2）**：`request_adapter_and_device` + `blocking` feature |
| P1-4 | 导出 / CLI 的 fs / ffmpeg | `xcal-app/src/export.rs`、`bin/xcal-render.rs` | 浏览器侧另做导出 | **范围外**：浏览器不导出；XSB 路线绕开 |
| P1-5 | `xcal-gui` 无法关闭下层默认 features | `xcal-gui/Cargo.toml` | `xcal-app` 依赖缺 `default-features = false` | **降格（工效项）**：`xcal-gui` 不进浏览器，与 wasm 无关 |

### P2 — 清理项

| # | 项 | 说明 | 状态（2026-10-19） |
|---|---|---|---|
| P2-1 | `gpu_test` 的 `libc::write` / `thread::sleep` / `spawn` | 编进库的测试辅助，需 target 门控 | **已解决（S4）**：归入 `blocking` feature |
| P2-2 | `Receipt::wait` 的 `yield_now` 忙等 | wasm no-op；已有"主线程禁令"，可保持 | **已解决（W1）**：命令同步跑完后 `complete()`，`yield_now` 走不到 |
| P2-3 | CI wasm gate 未覆盖 scene/render/schedule | 本轮证明可编译，可考虑纳入（本轮不改） | **已解决（S7）**：Playback runtime wasm gate |
| P2-4 | `xc-pod-py` 的 wasm 形态 | 默认关闭，不影响；浏览器侧预计不启用 Python Pod | **范围外**：浏览器不启用 Python Pod |

---

## 8. 未覆盖 / 局限

1. **未做浏览器实跑**：所有结论基于编译与 std 行为，未验证 canvas / surface 集成。
2. **downlevel 只测了本机 Vulkan adapter**，不是浏览器 WebGPU adapter。
3. **`xc-pod-tests` 未探测**（测试专用，依赖两个语言适配器）。
4. **typst feature 虽然 wasm 编译通过**，但字体加载/系统字体探测在浏览器里的
   运行期行为未验证。
5. **未验证 wasm atomics / shared memory**；`thread::sleep` 的行为在
   `+atomics` 下会从 panic 变成阻塞主线程，两种都不可接受。
6. 本轮**未做任何修复**，包括未改 CI、未加 feature、未改 trait 约束。

---

## 9. 复现命令

```powershell
# 探测一
rustup target add wasm32-unknown-unknown
cargo check -p xc-pod-lua --target wasm32-unknown-unknown

# 探测二（逐 crate）
cargo check -p xcal-scene  --target wasm32-unknown-unknown
cargo check -p xcal-render  --target wasm32-unknown-unknown
cargo check -p xcal-schedule --target wasm32-unknown-unknown
cargo check -p xcal-render-backends --target wasm32-unknown-unknown
cargo check -p xcal-egui --target wasm32-unknown-unknown
cargo check -p xcal-app --no-default-features --target wasm32-unknown-unknown
cargo check -p xc-prerender --features typst --target wasm32-unknown-unknown

# 探测三（native；临时 example 已删除，可用等价片段复现）
#   Instance::default() -> request_adapter -> get_downlevel_capabilities()/limits()/features()
grep -rn "push_constant" crates/            # 0
grep -rn "Limits::\|required_limits\|Features::" crates/   # 0
```
