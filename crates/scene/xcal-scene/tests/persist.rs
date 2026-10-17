//! S3/S6/S7 验收：27 个组件逐条 serde、`Components` 往返、`SceneScript`
//! 持久化往返与"克隆后重新 commit"。

use xc_notion::{FunctionX2Y, Path2D, Rect2D, Sampler1D, Vec2, Vec3};
use xc_object::{
    Arrow, Axes2D, AxesLine, Camera2D, Camera3D, Circle, FpsView, Framing2D, FunctionGraph, Line,
    Path, Projection, Rect, Transform, ZOrder,
};
use xc_render_ir::{FillSolid, Opacity, StrokeSolid};
use xcal_scene::animation::Timeline;
use xcal_scene::contract::{
    Component, Components, Group, GroupId, Name, ObjectId, Parent, SceneRead, Visibility,
};
use xcal_scene::domain::CameraMode;
use xcal_scene::persist::{
    PersistError, TaggedComponent, deserialize_components, serialize_components,
};
use xcal_scene::scene::Scene;
use xcal_scene::scene::components::{ContentTextureItem, MorphShape, PhaseAnim, Reveal};
use xcal_scene::{AnimValue, Curve, Interp, SceneEncoder, SceneScript};

/// 27 个登记组件的样例（与 `known_component_table!` 一一对应）。
fn samples() -> xcal_scene::contract::Components {
    vec![
        Box::new(Circle::new((0.0, 0.0), 1.0)),
        Box::new(Line::new((0.0, 0.0), (1.0, 0.0))),
        Box::new(Path::new(Path2D::new())),
        Box::new(Rect::new((0.0, 0.0), (1.0, 1.0))),
        Box::new(Arrow::new((0.0, 0.0), (1.0, 0.0), 0.1, 0.1)),
        Box::new(Transform::default()),
        Box::new(ZOrder::new(1)),
        Box::new(Name::new("x")),
        Box::new(Group::new(GroupId::new(1))),
        Box::new(Visibility::default()),
        Box::new(Opacity::new(1.0)),
        Box::new(Camera2D::new(
            Vec2::ZERO,
            40.0,
            Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0)),
        )),
        Box::new(Camera3D::new(
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        )),
        Box::new(Projection::perspective(1.0, 1.0, 0.1, 100.0)),
        Box::new(Framing2D::default()),
        Box::new(CameraMode::default()),
        Box::new(FpsView::new(0.0, 0.0)),
        Box::new(StrokeSolid::new(1.0, (0, 0, 0, 255))),
        Box::new(FillSolid::new((0, 0, 0, 255))),
        Box::new(Axes2D::new((-1.0, 1.0), (-1.0, 1.0))),
        Box::new(AxesLine {
            owner: ObjectId::DANGLING,
        }),
        Box::new(FunctionGraph::new(
            FunctionX2Y::named("test::identity", |x: f32| x),
            Sampler1D::new(0.0f32, 1.0, 1.0),
        )),
        Box::new(PhaseAnim::new(0.0, 1.0)),
        Box::new(MorphShape::new(Path2D::new(), Path2D::new())),
        Box::new(ContentTextureItem),
        Box::new(Reveal::new(Path2D::new())),
        Box::new(Parent(ObjectId::DANGLING)),
    ]
}

/// 验收 1/2（S3）：27 个组件**逐个**可序列化 + 解码后重编码一致。
#[test]
fn every_registered_component_round_trips() {
    let original = samples();
    let tagged = serialize_components(&original).expect("27 个组件都应可序列化");
    assert_eq!(tagged.len(), 27);

    let mut names: Vec<&str> = tagged.iter().map(|t| t.type_name.as_str()).collect();
    names.sort_unstable();
    let mut expected = vec![
        "Circle",
        "Line",
        "Path",
        "Rect",
        "Arrow",
        "Transform",
        "ZOrder",
        "Name",
        "Group",
        "Visibility",
        "Opacity",
        "Camera2D",
        "Camera3D",
        "Projection",
        "Framing2D",
        "CameraMode",
        "FpsView",
        "StrokeSolid",
        "FillSolid",
        "Axes2D",
        "AxesLine",
        "FunctionGraph",
        "PhaseAnim",
        "MorphShape",
        "ContentTextureItem",
        "Reveal",
        "Parent",
    ];
    expected.sort_unstable();
    assert_eq!(names, expected, "登记表 27 条必须逐条出现");

    let decoded = deserialize_components(tagged.clone()).expect("全部可解码");
    assert_eq!(decoded.len(), 27);

    // 再编码一次 ⇒ 与第一次逐项相等（往返一致；函数组件比较名字）。
    let again = serialize_components(&decoded).expect("解码后的组件仍可序列化");
    assert_eq!(again, tagged, "序列化 → 反序列化 → 序列化 必须逐位一致");
}

/// 验收 3（S3）：匿名函数**明确报错**，不静默丢组件。
#[test]
fn anonymous_function_is_an_explicit_error() {
    let components: Components = vec![Box::new(FunctionGraph::new(
        FunctionX2Y::new(|x: f32| x),
        Sampler1D::new(0.0f32, 1.0, 1.0),
    ))];
    let err = serialize_components(&components).expect_err("匿名函数不可序列化");
    assert!(
        matches!(
            err,
            PersistError::SerializeComponent {
                type_name: "FunctionGraph",
                ..
            }
        ),
        "实际 {err:?}"
    );
}

/// **两步走**：反序列化得到的函数是"只有名字的壳"，调用方按名字重新绑定。
#[test]
fn deserialized_function_can_be_rebound_by_name() {
    let components: Components = vec![Box::new(FunctionGraph::new(
        FunctionX2Y::named("xcal.math.sin", |x: f32| x.sin()),
        Sampler1D::new(0.0f32, 1.0, 0.25),
    ))];
    let decoded = deserialize_components(serialize_components(&components).unwrap()).unwrap();

    let shell = (*decoded[0])
        .as_any()
        .downcast_ref::<FunctionGraph>()
        .expect("FunctionGraph");
    let name = shell.func.name.as_deref().expect("具名函数");
    let rebound = xc_notion::builtin_f32(name).expect("内置表可绑定");
    let graph = FunctionGraph::new(rebound, shell.sampler.clone());

    let pts = graph.sample_points(0.0);
    assert_eq!(pts.len(), 5, "pts = {pts:?}");
    assert!(
        (pts[1].y - 0.25f32.sin()).abs() < f32::EPSILON,
        "pts = {pts:?}，期望 y1 = {}",
        0.25f32.sin()
    );
}

/// 未知类型名 ⇒ 明确错误。
#[test]
fn unknown_component_type_is_an_explicit_error() {
    let err = match deserialize_components(vec![TaggedComponent {
        type_name: "NotAComponent".to_string(),
        data: serde_json::Value::Null,
    }]) {
        Ok(_) => panic!("未知类型必须失败"),
        Err(e) => e,
    };
    assert_eq!(
        err,
        PersistError::UnknownComponentType("NotAComponent".into())
    );
}

/// 验收 8（S6）：`SceneScript` 级持久化往返 —— 对象 / 时长 / 绑定 / 组件值一致。
#[test]
fn scene_script_round_trips_through_json() {
    let mut enc = SceneEncoder::new();
    let h = enc.spawn(vec![
        Box::new(Circle::new((1.0, 2.0), 3.0)),
        Box::new(Name::new("hero")),
        Box::new(Opacity::new(0.75)),
    ]);
    enc.animate::<xcal_scene::animation::OpacityValue>(h, ramp(1.0), 1.0);
    enc.wait(0.5);
    let script = enc.finish();

    let json = script.to_json().expect("序列化成功");
    let back = SceneScript::from_json(&json).expect("反序列化成功");

    assert_eq!(back.len(), script.len());
    assert!((back.duration() - script.duration()).abs() < 1e-9);

    let mut a = Scene::new();
    let mut b = Scene::new();
    let ra = script.commit(&mut a).expect("原脚本 commit");
    let rb = back.commit(&mut b).expect("反序列化脚本 commit");
    assert_eq!(a.len(), b.len(), "对象数一致");
    assert_eq!(ra.bindings.bindings.len(), rb.bindings.bindings.len());
    assert_eq!(
        ra.bindings.bindings[0].target.name(),
        rb.bindings.bindings[0].target.name(),
        "动画目标语义名一致"
    );

    // 组件值一致（按名字取回）。
    let (ida, idb) = (
        xcal_scene::contract::find_by_name(&a, "hero").expect("a 中有 hero"),
        xcal_scene::contract::find_by_name(&b, "hero").expect("b 中有 hero"),
    );
    assert_eq!(
        a.with::<Circle, _>(ida, |c| (c.center, c.radius)),
        b.with::<Circle, _>(idb, |c| (c.center, c.radius)),
    );
    assert_eq!(
        a.with::<Opacity, _>(ida, |o| o.0),
        b.with::<Opacity, _>(idb, |o| o.0),
    );
}

/// 验收（S7）：**克隆 → 两次 commit** 得到语义等价的两个场景。
#[test]
fn cloned_script_can_be_committed_again() {
    let mut enc = SceneEncoder::new();
    let h = enc.spawn(vec![
        Box::new(Circle::new((0.0, 0.0), 2.0)),
        Box::new(Name::new("dot")),
    ]);
    enc.animate::<xcal_scene::animation::OpacityValue>(h, ramp(2.0), 2.0);
    let script = enc.finish();
    let twin = script.clone();

    let mut a = Scene::new();
    let mut b = Scene::new();
    let ra = script.commit(&mut a).expect("第一次 commit");
    let rb = twin.commit(&mut b).expect("克隆后第二次 commit");

    assert_eq!(a.len(), b.len());
    assert_eq!(ra.handles.len(), rb.handles.len());
    assert_eq!(
        ra.bindings.bindings.len(),
        rb.bindings.bindings.len(),
        "时序链路条数一致"
    );
    let (ida, idb) = (
        xcal_scene::contract::find_by_name(&a, "dot").unwrap(),
        xcal_scene::contract::find_by_name(&b, "dot").unwrap(),
    );
    assert_eq!(
        a.with::<Circle, _>(ida, |c| c.radius),
        b.with::<Circle, _>(idb, |c| c.radius),
    );
}

/// `Ready` 目标不可持久化（明确错误）。
#[test]
fn ready_target_is_not_persistable() {
    let mut tl = Timeline::new();
    tl.play(
        Curve::Const(AnimValue::Scalar(1.0)),
        xcal_scene::animation::CircleRadius {
            id: ObjectId::DANGLING,
        },
        1.0,
    );
    let err = serde_json::to_string(&tl).expect_err("Ready 目标必须报错");
    assert!(
        err.to_string().contains("不可持久化"),
        "错误信息必须明确，实际 {err}"
    );
}

fn ramp(dur: f64) -> Curve {
    Curve::Keyframes {
        keys: vec![
            (0.0, xcal_scene::AnimValue::Scalar(0.0)),
            (dur, xcal_scene::AnimValue::Scalar(1.0)),
        ],
        interpolation: Interp::Linear,
    }
}
