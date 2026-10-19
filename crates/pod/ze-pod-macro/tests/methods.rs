//! Y1 验收：方法语义（receiver = `Handle`）、与自由函数共存、天然防跨 Pod。

use std::any::Any;
use std::cell::RefCell;

use ze_pod::{Capability, Handle, HandleTable, QualifiedName, RegistryPod, Pod, PodError, PodId};
use ze_pod_macro::capability;

/// 带不透明对象表的 Pod：方法调用的 receiver 是 `Handle`。
struct StorePod {
    funcs: RegistryPod,
    values: RefCell<HandleTable<f32>>,
}

impl Pod for StorePod {
    fn id(&self) -> PodId {
        self.funcs.id()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn functions(&self) -> &[QualifiedName] {
        self.funcs.functions()
    }

    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.funcs.resolve(name)
    }

    fn call(&self, id: ze_pod::FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        self.funcs.call(id, args)
    }
}

fn store() -> StorePod {
    let mut funcs = RegistryPod::new();
    let owner = funcs.id();

    funcs.register_fn1_named::<Handle, Result<f32, PodError>>(
        "store::get",
        "ze_pod.Handle",
        "Result<f32, PodError>",
        |pod: &dyn Pod, h: Handle| {
            let store = pod.as_any().downcast_ref::<StorePod>().expect("StorePod");
            store.values.borrow().get(&h).copied()
        },
    );
    funcs.register_fn2_named::<Handle, f32, Result<f32, PodError>>(
        "store::add",
        "ze_pod.Handle",
        "f32",
        "Result<f32, PodError>",
        |pod: &dyn Pod, h: Handle, x: f32| {
            let store = pod.as_any().downcast_ref::<StorePod>().expect("StorePod");
            let current = *store.values.borrow().get(&h)?;
            Ok(current + x)
        },
    );
    // 自由函数 —— 与上面的方法共处同一个 capability。
    funcs.register_fn1("store::square", |_: &dyn Pod, x: f32| x * x);

    StorePod {
        funcs,
        values: RefCell::new(HandleTable::new(owner)),
    }
}

#[capability(namespace = "store")]
struct Store<'a> {
    /// 读句柄里的值。
    #[pod_type(ret = "Result<f32, PodError>")]
    get: fn(&'a dyn Pod, Handle) -> Result<f32, PodError>,
    /// 句柄里的值加 `d`，返回新值。
    #[pod_type(ret = "Result<f32, PodError>")]
    add: fn(&'a dyn Pod, Handle, f32) -> Result<f32, PodError>,
    /// 平方。
    square: fn(&'a dyn Pod, f32) -> f32,
}

/// 验收 1/3：方法能生成并调用；自由函数与方法在同一 capability 里共存。
#[test]
fn methods_and_free_functions_coexist() {
    let pod = store();
    let h = pod.values.borrow_mut().insert(10.0);

    let cap: Store<'_> = pod.negotiate().expect("全命中");
    assert_eq!(cap.get(h).expect("receiver 有效"), 10.0);
    assert_eq!(cap.add(h, 2.5).expect("receiver 有效"), 12.5);
    assert_eq!(cap.square(3.0), 9.0, "自由函数仍然可用");

    // 签名里 receiver 就是第一个参数（`ze_pod.Handle`），与注册侧一致。
    let sigs = <Store<'static> as Capability<'static>>::signatures();
    assert_eq!(sigs[0].params.to_vec(), vec!["ze_pod.Handle"]);
    assert_eq!(sigs[1].params.to_vec(), vec!["ze_pod.Handle", "f32"]);
    assert_eq!(sigs[2].params.to_vec(), vec!["f32"]);
}

/// 验收 2（可证伪）：receiver 传**别的 Pod** 的 Handle ⇒ `CrossPodHandle`。
#[test]
fn foreign_receiver_is_rejected() {
    let pod = store();
    let h = pod.values.borrow_mut().insert(1.0);

    // 另一个 Pod 的槽位表（不同 owner），它的句柄对本 Pod 无效。
    let mut other: HandleTable<f32> = HandleTable::new(PodId::next());
    let foreign = other.insert(99.0);

    let cap: Store<'_> = pod.negotiate().expect("全命中");
    let err = cap.get(foreign).expect_err("跨 Pod receiver 必须失败");
    assert!(
        matches!(err, PodError::CrossPodHandle { .. }),
        "必须是 CrossPodHandle，实际 {err:?}"
    );

    // 本 Pod 的句柄不受影响。
    assert_eq!(cap.get(h).unwrap(), 1.0);
}
