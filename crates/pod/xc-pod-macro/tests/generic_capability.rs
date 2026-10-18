//! P1 验收：`#[capability]` 支持**类型泛型**（`struct X<'a, C>`）。
//!
//! 覆盖：
//! 1. 泛型 capability 能被 `negotiate::<LocalQueue<MyCommand>>()` 拿到；
//! 2. 签名中的类型名取自 `C` 的 `PodType` 定义（`IntoPodTypeName::NAME`）；
//! 3. 既有非泛型 capability 行为不变（由 `tests/capability.rs` 回归）。
#![allow(dead_code)]

use xc_pod::{
    Broadcast, Capability, CommandQueue, EventSink, IntoPodTypeName, Pod, QualifiedName,
    Receipt, RegistryPod,
};
use xc_pod_macro::{PodType, capability};

#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "demo.MyCommand")]
struct MyCommand {
    dt: f32,
}

#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "demo.MyEvent")]
struct MyEvent {
    frame: u32,
}

/// 一个**泛型** capability：字段参数类型就是 `C`。
#[capability(namespace = "demo", description = "泛型命令队列")]
struct LocalQueue<'a, C> {
    /// 提交一条命令。
    #[pod_type(ret = "Receipt")]
    submit: fn(&'a dyn Pod, C) -> Receipt,
}

fn submit(_: &dyn Pod, _command: MyCommand) -> Receipt {
    Receipt::completed()
}

fn publish(_: &dyn Pod, _event: MyEvent) {}

/// 一个**带 bindings** 的泛型 capability（验证类型擦除调用入口也支持泛型）。
#[capability(namespace = "demo", bindings)]
struct LocalEcho<'a, C> {
    /// 回显一条命令的读数。
    echo: fn(&'a dyn Pod, C) -> u32,
}

#[test]
fn generic_capability_with_bindings_exposes_calls() {
    let calls = LocalEcho::<MyCommand>::calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].signature.name, "echo");
    assert_eq!(calls[0].signature.params.to_vec(), vec!["demo.MyCommand"]);
    assert!(
        LocalEcho::<MyCommand>::signatures()[0]
            .qualified()
            .to_string()
            .contains("demo.MyCommand"),
        "泛型参数的类型名必须来自 PodType 定义"
    );
}

#[test]
fn generic_capability_negotiates_and_names_types_from_pod_type() {
    let mut pod = RegistryPod::new();
    pod.register_fn1_named::<MyCommand, Receipt>(
        "demo::submit",
        <MyCommand as IntoPodTypeName>::NAME,
        "Receipt",
        submit,
    );

    let queue: LocalQueue<'_, MyCommand> = pod.negotiate().expect("泛型能力可协商");
    queue.submit(MyCommand { dt: 0.25 }).wait();

    let sigs = <LocalQueue<'static, MyCommand> as Capability<'static>>::signatures();
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].name, "submit");
    assert_eq!(sigs[0].params.to_vec(), vec!["demo.MyCommand"]);
    assert_eq!(sigs[0].ret, "Receipt");
}

#[test]
fn generic_capability_can_be_used_as_a_bound() {
    fn assert_capability<'a, C: Capability<'a>>() {}
    assert_capability::<LocalQueue<'static, MyCommand>>();
    assert_capability::<LocalQueue<'static, MyEvent>>();
}

#[test]
fn built_in_generic_capabilities_negotiate() {
    let mut pod = RegistryPod::new();
    pod.register_fn1_named::<MyCommand, Receipt>(
        "xc_pod::command::submit",
        <MyCommand as IntoPodTypeName>::NAME,
        "Receipt",
        submit,
    );
    pod.register_fn1_named::<MyEvent, ()>(
        "xc_pod::broadcast::publish",
        <MyEvent as IntoPodTypeName>::NAME,
        "()",
        publish,
    );

    let queue: CommandQueue<'_, MyCommand> = pod.negotiate().expect("CommandQueue<MyCommand>");
    queue.submit(MyCommand { dt: 0.1 }).wait();
    let bus: Broadcast<'_, MyEvent> = pod.negotiate().expect("Broadcast<MyEvent>");
    bus.publish(MyEvent { frame: 1 });

    // 两个能力的完整签名不同（命名空间 + 参数类型），类型系统保证不可互换。
    let command_sig: QualifiedName = CommandQueue::<MyCommand>::signatures()[0].qualified();
    let event_sig: QualifiedName = Broadcast::<MyEvent>::signatures()[0].qualified();
    assert_ne!(command_sig, event_sig);
    assert_eq!(
        event_sig.to_string(),
        "xc_pod::broadcast::publish(demo.MyEvent) -> ()"
    );

    // 未被注册的订阅方能力 ⇒ 协商失败（全有或全无），而不是半残句柄。
    assert!(
        pod.negotiate::<EventSink<'_, MyEvent>>().is_none(),
        "未提供 on_event ⇒ EventSink 协商失败"
    );
}
