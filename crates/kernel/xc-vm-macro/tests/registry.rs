//! X5 验收：闭包注册表 VM（typed + erased），以及**有状态**的最小 VM 用例
//! （验证"不透明状态机"这一半：状态只能经 `&self` 的调用改变/读取）。
#![allow(dead_code)] // erased-only 的能力只用于证明协商失败，不调用其方法。

use std::any::Any;
use std::cell::Cell;

use xc_vm::{FunctionId, QualifiedName, RegistryVm, Vm, VmError, VmId};
use xc_vm_macro::capability;

/// 有状态 VM：**组合**一个 `RegistryVm` 作为函数集合，自己持有计数状态。
///
/// 状态用 `Cell`（内部可变性）⇒ 快慢两路径都用 `&self`（§0.2.1 第 3 条）。
/// 外部只能通过 capability 调用看到状态变化，`&mut` 从不暴露。
struct CounterVm {
    funcs: RegistryVm,
    count: Cell<u32>,
}

impl Vm for CounterVm {
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

    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError> {
        self.funcs.call(id, args)
    }
}

#[capability(namespace = "counter")]
struct Counter<'a> {
    bump: fn(&'a dyn Vm, u32) -> u32,
    get: fn(&'a dyn Vm) -> u32,
}

fn counter() -> CounterVm {
    let mut funcs = RegistryVm::new();
    // 非捕获闭包 ⇒ 裸 fn 指针（fast path）。状态从 VM 自己身上取。
    funcs.register_fn1_named::<u32, u32>("counter::bump", "u32", "u32", |vm: &dyn Vm, n: u32| {
        let state = vm
            .as_any()
            .downcast_ref::<CounterVm>()
            .expect("调用者必须是 CounterVm");
        let next = state.count.get() + n;
        state.count.set(next);
        next
    });
    funcs.register_fn0_named::<u32>("counter::get", "u32", |vm: &dyn Vm| {
        vm.as_any()
            .downcast_ref::<CounterVm>()
            .expect("调用者必须是 CounterVm")
            .count
            .get()
    });

    CounterVm {
        funcs,
        count: Cell::new(0),
    }
}

/// 验收 1/2（X5）：注册 → 协商 → 快路径调用；有状态 VM 的调用改变/读取内部状态。
#[test]
fn stateful_vm_state_changes_through_shared_borrow() {
    let vm = counter();
    let cap: Counter<'_> = vm.negotiate().expect("全命中");

    assert_eq!(cap.get(), 0, "初始状态");
    assert_eq!(cap.bump(2), 2);
    assert_eq!(cap.bump(3), 5);
    assert_eq!(cap.get(), 5);

    // 再取一个句柄：看到的是**同一个**状态机（状态在 VM 里，不在句柄里）。
    let cap2: Counter<'_> = vm.negotiate().expect("全命中");
    assert_eq!(cap2.get(), 5);
}

/// 验收 3（X5）：capability 与实际可调用集合一致。
#[test]
fn functions_are_the_registered_set() {
    let vm = counter();
    let mut names: Vec<String> = vm.functions().iter().map(|n| n.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "counter::bump(u32) -> u32".to_string(),
            "counter::get() -> u32".to_string(),
        ]
    );
}

/// X5 待定项：erased-only 函数只有慢路径 ⇒ 需要它的 capability 协商失败，
/// 但 `call` 仍可用。
#[test]
fn erased_only_functions_degrade_to_slow_path() {
    let mut vm = RegistryVm::new();
    vm.register_erased("erased::seven", &[], "u32", |_: &dyn Vm, _args| {
        Ok(Box::new(7u32))
    });

    #[capability(namespace = "erased")]
    struct ErasedOnly<'a> {
        seven: fn(&'a dyn Vm) -> u32,
    }

    assert!(
        vm.negotiate::<ErasedOnly<'_>>().is_none(),
        "erased-only ⇒ 无 typed 可调用 ⇒ 协商失败（退化优雅）"
    );

    let name = QualifiedName::parse("erased::seven() -> u32").unwrap();
    let id = vm.function_id(&name).expect("函数清单里有它");
    let value = vm
        .call(id, &[])
        .expect("慢路径仍可用")
        .downcast::<u32>()
        .expect("u32");
    assert_eq!(*value, 7);
}

/// 慢路径的实参校验是**明确错误**，不是静默失败。
#[test]
fn slow_path_reports_arity_and_type_errors() {
    let vm = counter();
    let name = QualifiedName::parse("counter::bump(u32) -> u32").unwrap();
    let id = vm.function_id(&name).unwrap();

    let zero = vm.call(id, &[]).unwrap_err();
    assert!(matches!(
        zero,
        VmError::ArityMismatch {
            expected: 1,
            got: 0
        }
    ));

    let wrong: f32 = 1.0;
    let arg: &dyn Any = &wrong;
    let err = vm.call(id, &[arg]).unwrap_err();
    assert!(
        matches!(err, VmError::ArgTypeMismatch { index: 0, .. }),
        "实参类型不匹配必须明确报错，实际 {err:?}"
    );
}
