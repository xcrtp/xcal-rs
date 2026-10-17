//! Y1 验收：方法语义（receiver = `Handle`）、与自由函数共存、天然防跨 VM。

use std::any::Any;
use std::cell::RefCell;

use xc_vm::{Capability, Handle, HandleTable, QualifiedName, RegistryVm, Vm, VmError, VmId};
use xc_vm_macro::capability;

/// 带不透明对象表的 VM：方法调用的 receiver 是 `Handle`。
struct StoreVm {
    funcs: RegistryVm,
    values: RefCell<HandleTable<f32>>,
}

impl Vm for StoreVm {
    fn id(&self) -> VmId {
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

    fn call(&self, id: xc_vm::FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        self.funcs.call(id, args)
    }
}

fn store() -> StoreVm {
    let mut funcs = RegistryVm::new();
    let owner = funcs.id();

    funcs.register_fn1_named::<Handle, Result<f32, VmError>>(
        "store::get",
        "xc_vm.Handle",
        "Result<f32, VmError>",
        |vm: &dyn Vm, h: Handle| {
            let store = vm.as_any().downcast_ref::<StoreVm>().expect("StoreVm");
            store.values.borrow().get(&h).copied()
        },
    );
    funcs.register_fn2_named::<Handle, f32, Result<f32, VmError>>(
        "store::add",
        "xc_vm.Handle",
        "f32",
        "Result<f32, VmError>",
        |vm: &dyn Vm, h: Handle, x: f32| {
            let store = vm.as_any().downcast_ref::<StoreVm>().expect("StoreVm");
            let current = *store.values.borrow().get(&h)?;
            Ok(current + x)
        },
    );
    // 自由函数 —— 与上面的方法共处同一个 capability。
    funcs.register_fn1("store::square", |_: &dyn Vm, x: f32| x * x);

    StoreVm {
        funcs,
        values: RefCell::new(HandleTable::new(owner)),
    }
}

#[capability(namespace = "store")]
struct Store<'a> {
    #[vm_type(ret = "Result<f32, VmError>")]
    get: fn(&'a dyn Vm, Handle) -> Result<f32, VmError>,
    #[vm_type(ret = "Result<f32, VmError>")]
    add: fn(&'a dyn Vm, Handle, f32) -> Result<f32, VmError>,
    square: fn(&'a dyn Vm, f32) -> f32,
}

/// 验收 1/3：方法能生成并调用；自由函数与方法在同一 capability 里共存。
#[test]
fn methods_and_free_functions_coexist() {
    let vm = store();
    let h = vm.values.borrow_mut().insert(10.0);

    let cap: Store<'_> = vm.negotiate().expect("全命中");
    assert_eq!(cap.get(h).expect("receiver 有效"), 10.0);
    assert_eq!(cap.add(h, 2.5).expect("receiver 有效"), 12.5);
    assert_eq!(cap.square(3.0), 9.0, "自由函数仍然可用");

    // 签名里 receiver 就是第一个参数（`xc_vm.Handle`），与注册侧一致。
    let sigs = <Store<'static> as Capability<'static>>::signatures();
    assert_eq!(sigs[0].params.to_vec(), vec!["xc_vm.Handle"]);
    assert_eq!(sigs[1].params.to_vec(), vec!["xc_vm.Handle", "f32"]);
    assert_eq!(sigs[2].params.to_vec(), vec!["f32"]);
}

/// 验收 2（可证伪）：receiver 传**别的 VM** 的 Handle ⇒ `CrossVmHandle`。
#[test]
fn foreign_receiver_is_rejected() {
    let vm = store();
    let h = vm.values.borrow_mut().insert(1.0);

    // 另一个 VM 的槽位表（不同 owner），它的句柄对本 VM 无效。
    let mut other: HandleTable<f32> = HandleTable::new(VmId::next());
    let foreign = other.insert(99.0);

    let cap: Store<'_> = vm.negotiate().expect("全命中");
    let err = cap.get(foreign).expect_err("跨 VM receiver 必须失败");
    assert!(
        matches!(err, VmError::CrossVmHandle { .. }),
        "必须是 CrossVmHandle，实际 {err:?}"
    );

    // 本 VM 的句柄不受影响。
    assert_eq!(cap.get(h).unwrap(), 1.0);
}
