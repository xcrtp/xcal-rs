//! L2 验收：`///` → `Capability::docs()`（编译期常量，与 `signatures()` 逐条对应）。
#![allow(dead_code)] // 只检查文档，不调用函数字段。

use ze_pod_core::{Capability, RegistryPod, Pod};
use ze_pod_macro::capability;

#[capability(namespace = "doc", description = "文档测试能力")]
struct DocApi<'a> {
    /// 把 `x` 平方。
    square: fn(&'a dyn Pod, f32) -> f32,
    /// 常量 1。
    one: fn(&'a dyn Pod) -> f32,
}

#[test]
fn docs_are_parallel_to_signatures() {
    let sigs = <DocApi<'static> as Capability<'static>>::signatures();
    let docs = <DocApi<'static> as Capability<'static>>::docs();

    assert_eq!(docs.len(), sigs.len(), "docs 与 signatures 必须逐条对应");
    for (sig, doc) in sigs.iter().zip(docs) {
        assert_eq!(sig.name, doc.name, "顺序 / 名字必须一致");
        assert_eq!(sig.namespace, doc.namespace);
        assert_eq!(sig.params, doc.params);
        assert_eq!(sig.ret, doc.ret);
        assert_eq!(sig.qualified(), doc.qualified());
    }
}

/// **可证伪**：改一处 `///`，这里的断言随之变化（文档确实来自 `///`，不是宏写死的）。
#[test]
fn descriptions_come_from_doc_comments() {
    let docs = <DocApi<'static> as Capability<'static>>::docs();
    assert_eq!(docs[0].name, "square");
    assert_eq!(docs[0].description, "把 `x` 平方。");
    assert_eq!(docs[0].params, &["f32"]);
    assert_eq!(docs[0].ret, "f32");
    assert_eq!(docs[0].qualified().to_string(), "doc::square(f32) -> f32");

    assert_eq!(docs[1].name, "one");
    assert_eq!(docs[1].description, "常量 1。");
    assert!(docs[1].params.is_empty());
}

/// 协商不受 docs 影响（docs 只是自述信息）。
#[test]
fn docs_do_not_participate_in_negotiation() {
    let mut pod = RegistryPod::new();
    pod.register_fn1("doc::square", |_: &dyn Pod, x: f32| x * x);
    pod.register_fn0("doc::one", |_: &dyn Pod| 1.0f32);
    let api: DocApi<'_> = pod.negotiate().expect("全命中");
    assert_eq!(api.square(3.0), 9.0);
    assert_eq!(api.one(), 1.0);
}
