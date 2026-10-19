# ze-pod

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
- 名字不绑定任何语言：新增一门语言 = 新增一个 `ze-pod-*` 适配器 crate +
  App 的静态清单加一个节点；新增 domain capability = `zela-scene` 加一个函数。

## 命令 / 事件（泛型 capability）

`CommandQueue<C>` / `Broadcast<E>` / `EventSink<E>` 是**泛型 capability**：
命令枚举 `C` 与事件枚举 `E` 由实现方注入（`#[derive(PodType)]`），签名里的类型名
取自其 `PodType` 定义。

| | 命令队列 | 事件总线 |
|---|---|---|
| 方向 | 控制 | 通知 |
| 语义 | FIFO，顺序确定 | 异步、可丢 |
| 参与帧推进 | **是** | **否** |
| 典型 | `Advance(dt)` / `Batch(n, dt)` | `FrameFinished` / `Error` |

**任何影响场景状态的，必须走命令队列；事件只能用于观测。**

这条约束由类型系统保证：`submit(C)` 与 `publish(E)` 是不同类型，
把事件当命令提交 ⇒ **编译失败**（见 `src/command.rs` 的 `compile_fail` doctest）。
若退化成 `PodValue` 承载，两者签名会完全相同，约束就只剩约定。

`submit` 返回 [`Receipt`]：导出循环用 `receipt.wait()` 同步等一帧完成，
异步订阅方仍走事件总线观测 —— 同步与异步走同一条路径。
**主线程不得调用 `Receipt::wait()`**：它只自旋让出时间片、不驱动队列，
等待一个"本该由自己 pump 的队列"会死锁；主线程改用
`Receipt::wait_pumping(|| queue.pump())`（等待期间继续推进队列）。
`wait()` 只允许在工作线程、或同步驱动（命令已在 `submit` 内完成）时使用。
发布事件时按 broker 查询到的 `EventSink<E>` 逐个 **typed** 调用 `on_event`，
全程无 `Arc<dyn Fn>` 过 Pod 边界、无 `unsafe`。

## topic：广播 = 查询 + 迭代单播

`publish_topic(&broker, topic, &event)` 是普通辅助函数（不是 capability、不是 pod）：

1. `Broker::topic_members(topic)` 返回订阅者 `PodHandle` 列表，**按 `PodId` 升序**；
2. 逐个 `negotiate_on_handle::<EventSink<E>>(handle)` 并 typed 调用 `on_event`；
3. 返回 `PublishReport { delivered, order }` 供断言 / 排查。

发布方**不得**硬编码订阅者句柄列表；扇出顺序与节点声明顺序无关。
声明了 topic 却没提供对应 `EventSink<E>` ⇒ **明确错误**（不静默跳过）。

`Broadcast<E>` 保留为兼容入口（P4 后 Driver pod **已改走 topic**，
不再有第二套扇出路径），它的 `publish` 只调用提供方自己的实现，**本身不做扇出**；
新代码一律走 topic。

## Ord：逻辑时序（不是墙钟）

`SeqKey = (frame, source, seq)`：
- `frame`：逻辑帧号（`Advance` / `Batch` 推进）；
- `source`：发布方 [`PodHandle`]（`Ord` = `PodId` 的 `Ord`）；
- `seq`：该源在该帧内的递增序号（`Sequencer` 分配）。

`Sequenced<T>` 由框架提供 `Ord`（只比较键，**不要求 `T: Ord`**，也不让实现方
`derive(Ord)`）；`PartialEq` / `Eq` 同样只看键。**禁止**把 `Instant` / 时间戳放进排序键。

## flush：显式逻辑边界

`FlushBuffer<T>` 只在一个批次内排序；`flush(FlushBoundary::FrameEnd / BatchEnd)`
才产出有序序列。**不得**用「每 16ms flush 一次」这类墙钟边界。
`push_immediate` 是逃生舱，**不保证顺序**；默认路径是 `push` + `flush`。

## 寻址：`PodHandle` 而不是资源 `Handle`

`PodHandle`（`PodType` = `u64`）标识 **Pod 本身**，用于 topic 成员与 `SeqKey::source`；
资源 `Handle` 带 owner，跨 Pod 使用会触发 `CrossPodHandle`，**不得**用于寻址。

## 已知命名风险：`bytemuck::Pod`

`bytemuck` **已经在依赖树中**（wgpu / egui 等间接引入），其零拷贝 trait 也叫 `Pod`。
本仓库目前没有直接 `use bytemuck::Pod`，因此没有歧义；若将来某个 crate 同时引入
两者，用 `ze_pod::Pod` 显式限定。

> 风险等级：**已在依赖树**（不是"将来"）。

## 关于依赖环（验证说明）

`analyze_deps.py` 在仓库中不存在，不要执行它。

`cargo check` 通过**不能**证明"无循环依赖"——Cargo 自己会拒绝循环依赖；
编译通过是前提，不是证据。结论：**依赖环由 Cargo 保证不可能，此项无需单独验证**。

## 验证

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo check -p ze-pod -p ze-pod-binding --target wasm32-unknown-unknown
```
