// send_probe.rs — **平台对照探针**（不是产品示例）。
//
// B3-R ④：「Send + Sync 是硬阻塞」是否成立？答案取决于**目标平台**，本文件把
// 两边都钉成可复验的编译事实：
//
// ```text
// # native：**应当通过**（后端带 S1 徽章）
// cargo check -p zela-render-backends --features send-probe --example send_probe
//
// # wasm32：**应当失败**（徽章按 cfg 关掉；wgpu 资源在 wasm 上含 Rc<RefCell<..>>）
// cargo check -p zela-render-backends --target wasm32-unknown-unknown \
//     --no-default-features --features wgpu,send-probe --example send_probe
// ```
//
// # 为什么 native 能过
//
// `WgpuBackend2D` 在 `wgpu_2d.rs:352-362` 有一段**显式 unsafe 徽章**
// （`#[cfg(not(target_arch = "wasm32"))] unsafe impl Send/Sync`）：
// egui 的 `CallbackResources` 在 native 上要求 `Send + Sync`，而 `GpuHandle`
// （`Box<dyn Any>`）本身是 `!Send + !Sync`。徽章只覆盖 native，理由写在原处。
//
// ⇒ 结论：**native 上渲染 Pod 可以登记 `SyncPod`**（把 `RefCell` 换成 `Mutex` 即可）；
// **wasm 上不行**（徽章不存在，且 wgpu 资源的跨线程能力本就不成立）。
// `Pod` 自身不要求 `Send + Sync`（`PodNode::new` 合法）⇒ 本探针能跑通。
//
// 若哪天 wasm 侧探针也通过了，说明"跨平台统一 SyncPod"的前提成立 —— 届时应重估。

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

fn main() {
    assert_send::<zela_render_backends::WgpuBackend2D>();
    assert_sync::<zela_render_backends::WgpuBackend2D>();
}
