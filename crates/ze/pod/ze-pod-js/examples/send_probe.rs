// send_probe.rs — **负向编译探针**（不是可运行示例）。
//
// B3-P §二.5：JS Pod 用 `PodNode::new` 还是 `new_sync`，取决于 boa 的
// `Context` 是否 `Send + Sync`。本文件把"它**不是** `Send + Sync`"钉成可复验的
// 编译事实：
//
// ```text
// cargo check -p ze-pod-js --features send-probe --example send_probe   # 必须**失败**
// ```
//
// 失败信息里应出现两处 `cannot be sent between threads safely` —— 那正是
// `Context: !Send` / `!Sync`。若哪天 boa 变成线程安全，本探针会**通过**，
// 届时应当把 `JsPod` 改成 `PodNode::new_sync` 并删除本文件。

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

fn main() {
    assert_send::<boa_engine::Context>();
    assert_sync::<boa_engine::Context>();
}
