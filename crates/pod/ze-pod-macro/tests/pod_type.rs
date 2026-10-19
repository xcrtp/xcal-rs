//! V0/V1/V2 验收：`TypeSpec` 可反射、`#[derive(PodType)]` 一处定义、
//! `#[capability(bindings)]` 生成的 `calls()` 能类型擦除调用（含结构体/枚举转换）。
#![allow(dead_code)] // capability 的 inherent 方法不全部调用。

use ze_pod::{
    Capability, ErasedFn, Field, Handle, RegistryPod, ScalarKind, TypeSpec, Variant, Pod, PodId,
    PodType, PodValue, PodValueError,
};
use ze_pod_macro::{PodType, capability};

#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "test.Color")]
struct Color {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "test.Style")]
struct Style {
    width: f32,
    color: Color,
}

#[derive(Clone, Debug, PartialEq, PodType)]
#[pod_type(name = "test.Curve")]
enum Curve {
    Phase { omega: f32 },
    Const { value: f32 },
    Unit,
}

#[capability(namespace = "test", bindings)]
struct BoundApi<'a> {
    /// 用样式造句柄（把参数编码进句柄，便于断言转换结果）。
    style: fn(&'a dyn Pod, style: Style) -> Handle,
    /// 用曲线造句柄（把变体编码进 generation）。
    curve: fn(&'a dyn Pod, curve: Curve) -> Handle,
}

#[capability(namespace = "test")]
struct NoBindings<'a> {
    /// 未开启 bindings：不要求字段类型实现 PodType。
    #[pod_type(param0 = "test.Big")]
    big: fn(&'a dyn Pod, Big) -> u32,
}

struct Big(u32);

fn style_handle(_: &dyn Pod, style: Style) -> Handle {
    let slot = ((style.width.to_bits() as u64) << 32) | style.color.r as u64;
    Handle::new(PodId::new(7), slot, style.color.g as u32)
}

fn curve_handle(_: &dyn Pod, curve: Curve) -> Handle {
    match curve {
        Curve::Phase { omega } => Handle::new(PodId::new(8), omega.to_bits() as u64, 1),
        Curve::Const { value } => Handle::new(PodId::new(8), value.to_bits() as u64, 2),
        Curve::Unit => Handle::new(PodId::new(8), 0, 3),
    }
}

fn registry() -> RegistryPod {
    let mut pod = RegistryPod::new();
    pod.register_fn1::<Style, Handle>("test::style", style_handle);
    pod.register_fn1::<Curve, Handle>("test::curve", curve_handle);
    pod
}

/// V0：`TypeSpec` 的结构与字段名来自类型定义本身。
#[test]
fn type_spec_is_reflective() {
    let spec = <Style as PodType>::SPEC;
    let TypeSpec::Struct { name, fields } = spec else {
        panic!("Style 必须是结构体，实际 {spec:?}");
    };
    assert_eq!(*name, "test.Style");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0], Field { name: "width", ty: <f32 as PodType>::SPEC });
    assert_eq!(fields[1].name, "color");
    assert_eq!(fields[1].ty, <Color as PodType>::SPEC);

    let TypeSpec::Struct { fields, .. } = <Color as PodType>::SPEC else {
        panic!("Color 必须是结构体");
    };
    assert_eq!(fields.len(), 4);
    assert_eq!(fields[0].name, "r");
    assert_eq!(
        *fields[0].ty,
        TypeSpec::Scalar(ScalarKind::U8),
        "颜色分量是 u8"
    );

    // 枚举：variant + 字段都在 schema 里。
    let TypeSpec::Enum { name, variants } = <Curve as PodType>::SPEC else {
        panic!("Curve 必须是枚举");
    };
    assert_eq!(*name, "test.Curve");
    assert_eq!(
        *variants,
        &[
            Variant {
                name: "Phase",
                fields: &[Field { name: "omega", ty: <f32 as PodType>::SPEC }],
            },
            Variant {
                name: "Const",
                fields: &[Field { name: "value", ty: <f32 as PodType>::SPEC }],
            },
            Variant { name: "Unit", fields: &[] },
        ]
    );

    // derive 同时生成 IntoPodTypeName（与 schema 同名，signatures() 不漂移）。
    assert_eq!(<Style as ze_pod::IntoPodTypeName>::NAME, "test.Style");
    assert_eq!(<Curve as ze_pod::IntoPodTypeName>::NAME, "test.Curve");
}

/// V1：值 ⇄ `PodValue` 双向、字段名按名字匹配（不是位置）。
#[test]
fn pod_type_round_trips_nested_structs_and_enums() {
    let style = Style {
        width: 3.5,
        color: Color {
            r: 1,
            g: 2,
            b: 3,
            a: 4,
        },
    };
    let value = style.to_pod_value();
    // 字段顺序按声明；名字与 schema 一致。
    assert_eq!(
        value,
        PodValue::Struct(
            "test.Style",
            vec![
                ("width", PodValue::Scalar(ze_pod::ScalarValue::F32(3.5))),
                (
                    "color",
                    PodValue::Struct(
                        "test.Color",
                        vec![
                            ("r", PodValue::Scalar(ze_pod::ScalarValue::U8(1))),
                            ("g", PodValue::Scalar(ze_pod::ScalarValue::U8(2))),
                            ("b", PodValue::Scalar(ze_pod::ScalarValue::U8(3))),
                            ("a", PodValue::Scalar(ze_pod::ScalarValue::U8(4))),
                        ]
                    )
                ),
            ]
        )
    );
    assert_eq!(Style::from_pod_value(&value).unwrap(), style);

    // 字段乱序也能按名字取到。
    let shuffled = PodValue::Struct(
        "test.Style",
        vec![
            (
                "color",
                PodValue::Struct(
                    "test.Color",
                    vec![
                        ("r", PodValue::Scalar(ze_pod::ScalarValue::U8(1))),
                        ("g", PodValue::Scalar(ze_pod::ScalarValue::U8(2))),
                        ("b", PodValue::Scalar(ze_pod::ScalarValue::U8(3))),
                        ("a", PodValue::Scalar(ze_pod::ScalarValue::U8(4))),
                    ],
                ),
            ),
            ("width", PodValue::Scalar(ze_pod::ScalarValue::F32(3.5))),
        ],
    );
    assert_eq!(Style::from_pod_value(&shuffled).unwrap(), style);

    let phase = Curve::Phase { omega: 0.5 };
    assert_eq!(Curve::from_pod_value(&phase.to_pod_value()).unwrap(), phase);
    let unit = Curve::Unit;
    assert_eq!(Curve::from_pod_value(&unit.to_pod_value()).unwrap(), unit);

    let err = Curve::from_pod_value(&style.to_pod_value()).unwrap_err();
    assert!(matches!(err, PodValueError { .. }));
}

/// V2：`calls()` 由 `signatures()` + `TypeSpec` 驱动，参数/返回值自动转换。
#[test]
fn generated_calls_convert_values() {
    let pod = registry();
    assert_eq!(BoundApi::calls().len(), 2);
    assert_eq!(BoundApi::calls().len(), BoundApi::signatures().len());

    // 形参名进 schema（V3 文档可读性的地基）。
    let style_fn: &ErasedFn = &BoundApi::calls()[0];
    assert_eq!(style_fn.io.params.len(), 1);
    assert_eq!(style_fn.io.params[0].name, "style");
    assert_eq!(style_fn.io.params[0].ty, <Style as PodType>::SPEC);
    assert_eq!(style_fn.io.ret, <Handle as PodType>::SPEC);

    let style = Style {
        width: 3.5,
        color: Color {
            r: 9,
            g: 2,
            b: 3,
            a: 4,
        },
    };
    let out = (style_fn.call)(&pod, &[style.to_pod_value()]).expect("调用成功");
    let PodValue::Opaque(handle) = out else {
        panic!("返回值必须是句柄，实际 {out:?}");
    };
    // 编码断言：参数**值**真的穿过了边界（不是只有类型对）。
    assert_eq!(handle.owner(), PodId::new(7));
    assert_eq!(handle.slot() >> 32, 3.5f32.to_bits() as u64);
    assert_eq!(handle.slot() & 0xffff_ffff, 9);
    assert_eq!(handle.generation(), 2);

    let curve_fn = &BoundApi::calls()[1];
    assert_eq!(curve_fn.io.params[0].name, "curve");
    let out = (curve_fn.call)(&pod, &[(Curve::Phase { omega: 0.25 }).to_pod_value()])
        .expect("调用成功");
    let PodValue::Opaque(handle) = out else {
        panic!("返回值必须是句柄");
    };
    assert_eq!(handle.slot(), 0.25f32.to_bits() as u64);
    assert_eq!(handle.generation(), 1, "Phase 变体编码为 1");
}

/// 未开启 `bindings` 的 capability：`calls()` 为空（不强制 `Big: PodType`）。
#[test]
fn bindings_are_opt_in() {
    assert!(NoBindings::calls().is_empty());
}
