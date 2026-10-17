//! Y0 验收：`CapabilityMeta` 可查询、缺省为"无"、**version 不参与协商匹配**。
#![allow(dead_code)] // 版本用例只关心 meta 与协商结果，不调用函数字段。

use xc_vm::{Capability, CapabilityMeta, RegistryVm, Vm};
use xc_vm_macro::capability;

#[capability(namespace = "meta", version = "1.0")]
struct Version1<'a> {
    f: fn(&'a dyn Vm, f32) -> f32,
}

#[capability(
    namespace = "meta",
    version = "2.0",
    description = "同一个函数的第二个版本"
)]
struct Version2<'a> {
    f: fn(&'a dyn Vm, f32) -> f32,
}

#[capability(namespace = "meta")]
struct NoMeta<'a> {
    f: fn(&'a dyn Vm, f32) -> f32,
}

fn registry() -> RegistryVm {
    let mut vm = RegistryVm::new();
    vm.register_fn1("meta::f", |_: &dyn Vm, x: f32| x);
    vm
}

/// meta 可查询，且缺省为"无"。
#[test]
fn meta_is_queryable_and_defaults_to_none() {
    let v1 = <Version1<'static> as Capability<'static>>::meta();
    assert_eq!(v1.version, Some("1.0"));
    assert_eq!(v1.description, None);

    let v2 = <Version2<'static> as Capability<'static>>::meta();
    assert_eq!(v2.version, Some("2.0"));
    assert_eq!(v2.description, Some("同一个函数的第二个版本"));

    let none = <NoMeta<'static> as Capability<'static>>::meta();
    assert_eq!(none, CapabilityMeta::NONE);
    assert!(none.is_empty(), "缺省 meta 必须为空");
}

/// **可证伪**：两个版本号不同的 capability（签名相同）都能协商成功。
///
/// 若 version 参与了匹配，其中一个（或两个）必然返回 `None`。
#[test]
fn version_does_not_participate_in_negotiation() {
    let vm = registry();

    let a: Version1<'_> = vm.negotiate().expect("v1.0 协商成功");
    let b: Version2<'_> = vm.negotiate().expect("v2.0 协商成功（version 不参与匹配）");

    assert_eq!(a.f(3.0), 3.0);
    assert_eq!(b.f(4.0), 4.0);
}
