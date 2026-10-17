# xc-pod

> **A Pod is not a virtual machine, and not a scripting-language runtime.**
>
> A Pod is an **opaque state machine plus a capability set**: its internal state is
> hidden, and callers can only use the function set obtained through capability
> negotiation. Large values cross the boundary as opaque handles.
> Lua / Python interpreters, graph evaluators, closure registries, and future HTTP
> peers are just concrete Pod implementations.

> **Pod 不是虚拟机，也不是脚本语言运行时。**
>
> Pod 是「不透明状态机 + 能力集合」的抽象：内部状态不可见，只能通过协商得到的
> 函数集合访问；大类型跨边界只传句柄。Lua / Python 解释器、节点图求值器、
> 闭包表、未来的 HTTP 对端，都只是它的具体实现。

## 它是什么 / 不是什么

- **是**：capability 协商机制（`#[capability]`）、不透明句柄、typed 快路径 +
  erased 慢路径、生命周期与注入。
- **不是**：语言运行时、字节码解释器、脚本引擎。
- 名字不绑定任何语言：新增一门语言 = 新增一个 `xc-pod-*` 适配器 crate +
  App 的静态清单加一个节点；新增 domain capability = `xcal-scene` 加一个函数。

## 已知命名风险：`bytemuck::Pod`

`bytemuck` **已经在依赖树中**（wgpu / egui 等间接引入），其零拷贝 trait 也叫 `Pod`。
本仓库目前没有直接 `use bytemuck::Pod`，因此没有歧义；若将来某个 crate 同时引入
两者，用 `xc_pod::Pod` 显式限定。

> 风险等级：**已在依赖树**（不是"将来"）。

## 关于依赖环（验证说明）

`analyze_deps.py` 在仓库中不存在，不要执行它。

`cargo check` 通过**不能**证明"无循环依赖"——Cargo 自己会拒绝循环依赖；
编译通过是前提，不是证据。结论：**依赖环由 Cargo 保证不可能，此项无需单独验证**。

## 验证

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo check -p xc-pod -p xc-pod-binding --target wasm32-unknown-unknown
```
