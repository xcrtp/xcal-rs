//! A 线验收：`ze-pod-core` 的生产消费方（`zela_scene_ecs::math_pod`）。
//!
//! 1. 7 个内置函数全部可从 Pod 取到，且与 `ze_notion::builtin_f32` **逐位一致**；
//! 2. 适配器产物 `Send + Sync`、可深 Clone；
//! 3. 空 Pod ⇒ 取不到（可证伪）；
//! 4. 反序列化的内置函数**无需手动绑定**即可采样。

use std::sync::Arc;

use zela_scene_ecs::contract::Components;
use zela_scene_ecs::math_pod::{SharedPod, builtin_f32, function_from_pod, math_pod};
use zela_scene_ecs::{deserialize_components, serialize_components};
use ze_notion::{FunctionX2Y, Sampler1D};
use ze_scene_core::FunctionGraph;
use ze_pod_core::RegistryPod;

const NAMES: [&str; 7] = [
    "zela.math.sin",
    "zela.math.cos",
    "zela.math.tan",
    "zela.math.abs",
    "zela.math.sqrt",
    "zela.math.exp",
    "zela.math.ln",
];

const INPUTS: [f32; 6] = [-2.0, -0.5, 0.0, 0.3, 1.0, 2.5];

/// A2 验收：7 个函数全部可从 Pod 取到。
#[test]
fn all_builtins_are_registered_in_the_pod() {
    assert_eq!(math_pod().functions().len(), 7, "恰好 7 个内置函数");
    for name in NAMES {
        assert!(
            builtin_f32(name).is_some(),
            "{name} 必须能从 Pod 反查到"
        );
    }
    assert!(
        builtin_f32("zela.math.nope").is_none(),
        "未注册的名字必须返回 None"
    );
}

/// A2/A3 验收：Pod 路径与标准数学函数**逐位一致**（表已从 ze-notion 搬进 Pod）。
#[test]
fn pod_path_matches_std_math_bit_for_bit() {
    for (name, expected) in [
        ("zela.math.sin", f32::sin as fn(f32) -> f32),
        ("zela.math.cos", f32::cos),
        ("zela.math.tan", f32::tan),
        ("zela.math.abs", f32::abs),
        ("zela.math.sqrt", f32::sqrt),
        ("zela.math.exp", f32::exp),
        ("zela.math.ln", f32::ln),
    ] {
        let via_pod = builtin_f32(name).expect("Pod 路径");
        for x in INPUTS {
            assert_eq!(
                (via_pod.f)(x).to_bits(),
                expected(x).to_bits(),
                "{name}({x})：Pod 路径必须与标准函数逐位一致"
            );
        }
    }
}

/// A1 验收：适配器产物 `Send + Sync`，且深 Clone（新 `Arc`、行为一致）。
#[test]
fn pod_function_is_send_sync_and_deep_cloneable() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<FunctionX2Y<f32, f32>>();

    let a = builtin_f32("zela.math.sin").unwrap();
    let b = a.clone();
    assert!(!Arc::ptr_eq(&a.f, &b.f), "深克隆必须产生新的 Arc");
    assert_eq!((a.f)(0.7).to_bits(), (b.f)(0.7).to_bits());
    assert_eq!(a.name, b.name, "name 必须随克隆保留");
}

/// A3 可证伪：空 Pod（没有任何注册）⇒ 取不到函数。
#[test]
fn empty_pod_cannot_provide_functions() {
    let empty: SharedPod = Arc::new(RegistryPod::new());
    assert!(
        function_from_pod::<f32, f32>(&empty, "zela.math.sin").is_none(),
        "移除 Pod 注册后必须失败"
    );
}

/// A4 验收：反序列化内置函数后**无需手动绑定**即可采样。
#[test]
fn deserialized_builtin_is_auto_bound() {
    let components: Components = vec![Box::new(FunctionGraph::new(
        FunctionX2Y::named("zela.math.sin", |x: f32| x.sin()),
        Sampler1D::new(0.0f32, 1.0, 0.25),
    ))];
    let decoded =
        deserialize_components(serialize_components(&components).expect("编码")).expect("解码");

    let graph = (*decoded[0])
        .as_any()
        .downcast_ref::<FunctionGraph>()
        .expect("FunctionGraph");
    assert!(graph.func.is_bound(), "内置名字必须由 Pod 自动绑定");

    let pts = graph.sample_points(0.0);
    assert_eq!(pts.len(), 5);
    assert!((pts[1].y - 0.25f32.sin()).abs() < f32::EPSILON);
}
