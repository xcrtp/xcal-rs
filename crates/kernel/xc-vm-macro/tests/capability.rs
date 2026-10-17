//! X3/X4 验收：`#[capability]` 生成的 struct、签名逐条对应、协商全有或全无、
//! 快慢两路径结果一致、两个句柄可同时持有。
#![allow(dead_code)] // 部分能力只用于 `signatures()` 检查，不调用其方法。

use std::any::Any;

use xc_vm::{Capability, QualifiedName, RegistryVm, Vm};
use xc_vm_macro::capability;

#[capability(namespace = "math")]
struct MathVm<'a> {
    sin: fn(&'a dyn Vm, f32) -> f32,
    add: fn(&'a dyn Vm, f32, f32) -> f32,
}

#[capability(namespace = "math")]
struct IncompleteVm<'a> {
    sin: fn(&'a dyn Vm, f32) -> f32,
    missing: fn(&'a dyn Vm, f32) -> f32,
}

#[capability]
struct ZeroVm<'a> {
    answer: fn(&'a dyn Vm) -> u32,
}

fn qn(input: &str) -> QualifiedName {
    QualifiedName::parse(input).expect("合法完整函数名")
}

fn registry() -> RegistryVm {
    let mut vm = RegistryVm::new();
    vm.register_fn1("math::sin", |_: &dyn Vm, x: f32| x.sin());
    vm.register_fn2("math::add", |_: &dyn Vm, a: f32, b: f32| a + b);
    vm
}

/// 验收 2（X3）：`signatures()` 与声明**逐条对应**（顺序、名字、参数、返回）。
#[test]
fn signatures_match_declaration() {
    let sigs = <MathVm<'static> as Capability<'static>>::signatures();
    assert_eq!(sigs.len(), 2, "两个字段 ⇒ 两条签名");

    assert_eq!(sigs[0].name, "sin");
    assert_eq!(sigs[0].params.to_vec(), vec!["f32"]);
    assert_eq!(sigs[0].ret, "f32");

    assert_eq!(sigs[1].name, "add");
    assert_eq!(sigs[1].params.to_vec(), vec!["f32", "f32"]);
    assert_eq!(sigs[1].ret, "f32");

    assert_eq!(
        sigs[0].qualified().to_string(),
        "math::sin(f32) -> f32",
        "namespace + 完整签名"
    );
    assert_eq!(
        sigs[1].qualified().to_string(),
        "math::add(f32, f32) -> f32"
    );
}

/// 零参数函数：格式化仍然正确（`name() -> R`）。
#[test]
fn zero_arg_capability_works() {
    let mut vm = RegistryVm::new();
    vm.register_fn0("answer", |_: &dyn Vm| 42u32);
    let sigs = <ZeroVm<'static> as Capability<'static>>::signatures();
    assert_eq!(sigs[0].qualified().to_string(), "answer() -> u32");

    let cap: ZeroVm<'_> = vm.negotiate().expect("全命中");
    assert_eq!(cap.answer(), 42);
}

/// 验收 1（X4）：全有 ⇒ 句柄可用；缺一 ⇒ `None`（全有或全无）。
#[test]
fn negotiation_is_all_or_nothing() {
    let vm = registry();
    assert!(
        vm.negotiate::<MathVm<'_>>().is_some(),
        "所有签名都在 ⇒ 协商成功"
    );
    assert!(
        vm.negotiate::<IncompleteVm<'_>>().is_none(),
        "缺 `missing` ⇒ 整个协商失败（不返回半残句柄）"
    );
}

/// 验收 2（X4）：同一函数，快路径与慢路径**结果一致**（可证伪：逐位相等）。
#[test]
fn fast_and_slow_paths_agree() {
    let vm = registry();
    let cap: MathVm<'_> = vm.negotiate().expect("全命中");
    let id = vm
        .function_id(&qn("math::sin(f32) -> f32"))
        .expect("已注册");

    for x in [-2.0f32, -0.5, 0.0, 0.5, 2.0] {
        let fast = cap.sin(x);
        let arg: &dyn Any = &x;
        let slow = *vm
            .call(id, &[arg])
            .expect("慢路径成功")
            .downcast::<f32>()
            .expect("f32");
        assert_eq!(
            fast.to_bits(),
            slow.to_bits(),
            "x = {x}：两路径必须逐位一致"
        );
    }
}

/// 验收 4（X3）：两个 capability 句柄可同时持有（都持 `&Vm`，互不冲突）。
#[test]
fn two_handles_coexist() {
    let vm = registry();
    let a: MathVm<'_> = vm.negotiate().expect("第一个句柄");
    let b: MathVm<'_> = vm.negotiate().expect("第二个句柄");
    assert!((a.sin(1.0) - 1.0f32.sin()).abs() < f32::EPSILON);
    assert_eq!(b.add(1.5, 2.5), 4.0);
    // 两个句柄同时存活期间都能调用。
    assert!((a.sin(0.25) - 0.25f32.sin()).abs() < f32::EPSILON);
    assert_eq!(b.add(-1.0, 1.0), 0.0);
}

// ─── X1：显式标注 / 覆盖自动推断 ─────────────────────────────────────────

/// 没有 `IntoVmTypeName` 实现的域类型（模拟 `Path2D` 这类 kernel 不能依赖的类型）。
#[derive(Clone, Debug, PartialEq)]
struct Big(u32);

#[capability(namespace = "demo")]
struct BigVm<'a> {
    #[vm_type(param0 = "demo.Big", ret = "u32")]
    unwrap_big: fn(&'a dyn Vm, Big) -> u32,
}

/// 验收 2/3（X1）：显式标注可用于未实现 trait 的类型，且参与协商。
#[test]
fn explicit_vm_type_annotations_are_used() {
    let mut vm = RegistryVm::new();
    vm.register_fn1_named::<Big, u32>(
        "demo::unwrap_big",
        "demo.Big",
        "u32",
        |_: &dyn Vm, b: Big| b.0,
    );

    let sigs = <BigVm<'static> as Capability<'static>>::signatures();
    assert_eq!(sigs[0].params.to_vec(), vec!["demo.Big"]);
    assert_eq!(sigs[0].ret, "u32");

    let cap: BigVm<'_> = vm.negotiate().expect("显式类型名参与协商");
    assert_eq!(cap.unwrap_big(Big(7)), 7);
}

#[capability(namespace = "demo")]
struct OverrideVm<'a> {
    #[vm_type(ret = "custom.float")]
    f: fn(&'a dyn Vm, f32) -> f32,
}

/// 验收 3（X1）：显式标注**覆盖**自动推断（参数仍走推断）。
#[test]
fn explicit_annotation_overrides_inference() {
    let sigs = <OverrideVm<'static> as Capability<'static>>::signatures();
    assert_eq!(sigs[0].params.to_vec(), vec!["f32"], "参数走自动推断");
    assert_eq!(sigs[0].ret, "custom.float", "返回类型走显式标注");
}
