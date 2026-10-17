//! W1–W4 验收：target 枚举消除静默、双向对接、绑定生成、文档结构、错误通道。
//!
//! 完整 6 镜头与硬编码分镜的等价性在 `xcal-app` 的
//! `script_storyboard_matches_hardcoded` 里（那里能同时访问两者）。

use xcal_scene::animation::{Camera2DFraming, RevealRange};
use xcal_scene::contract::SceneRead;
use xcal_scene::scene::components::Reveal;
use xcal_scene::scene::Scene as SceneStruct;
use xcal_scene::scene_script::{SceneEncoder, SceneScript};
use xcal_scene::{AnimValue, Curve, Interp};
use xc_notion::{Path2D, Vec2};
use xc_object::Framing2D;
use xc_render_ir::{MeasureSpace, StrokeSolid};
use xc_vm::Capability;

use xcal_script::{ContentApi, SceneApi, ScriptHost};

/// 与 `xcal-app/src/storyboard.rs::sin_samples` 逐式相同（f32 采样）。
fn sin_samples() -> Vec<Vec2> {
    const N: usize = 128;
    let step = std::f32::consts::TAU / N as f32;
    (0..=N)
        .map(|i| {
            let x = -std::f32::consts::PI + (i as f32) * step;
            Vec2::new(x, x.sin())
        })
        .collect()
}

fn keyframes_scalar(dur: f64, from: f32, to: f32) -> Curve {
    Curve::Keyframes {
        keys: vec![(0.0, AnimValue::Scalar(from)), (dur, AnimValue::Scalar(to))],
        interpolation: Interp::Linear,
    }
}

fn keyframes_vec2(dur: f64, from: Vec2, to: Vec2) -> Curve {
    Curve::Keyframes {
        keys: vec![(0.0, AnimValue::Vec2(from)), (dur, AnimValue::Vec2(to))],
        interpolation: Interp::Linear,
    }
}

/// 脚本化的镜头 2–5（核心等价性单元；完整 6 镜头在 app 侧对照）。
const SUBSET_LUA: &str = r#"
local camera = scene.spawn({
    Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 40.0 },
})
local points = {}
for i = 0, 128 do
    local x = -math.pi + i * (2 * math.pi / 128)
    points[#points + 1] = Vec2 { x = x, y = math.sin(x) }
end
local curve = scene.spawn({
    Reveal { points = points, closed = false },
    Stroke { width = 2.0, color = Color { r = 0, g = 200, b = 200, a = 255 } },
})
scene.animate(curve, scene.clip(Target.RevealRange, Curve.Phase { omega = 0.5 }), 2.0)
scene.wait(0.5)
scene.animate(
    camera,
    scene.clip(Target.Camera2DFraming, Curve.Ramp { from = 40.0, to = 110.0, dur = 1.0 }),
    1.0
)
scene.parallel(
    camera,
    scene.clip(Target.Camera2DFraming, Curve.Ramp2 {
        from = Vec2 { x = 0.0, y = 0.0 },
        to = Vec2 { x = math.pi / 2, y = 1.0 },
        dur = 1.0,
    })
)
"#;

/// 手写版：与 `SUBSET_LUA` 表达同一个最小分镜。
fn hand_written_subset() -> SceneScript {
    let mut enc = SceneEncoder::new();
    let camera = enc.spawn(vec![Box::new(Framing2D::new(Vec2::ZERO, 40.0))]);
    let curve = enc.spawn(vec![
        Box::new(Reveal::new(Path2D::from_polyline(&sin_samples(), false))),
        Box::new(StrokeSolid::new_in(
            2.0,
            (0u8, 200u8, 200u8, 255u8),
            MeasureSpace::Screen,
        )),
    ]);

    enc.animate::<RevealRange>(curve, Curve::Phase { omega: 0.5 }, 2.0);
    enc.wait(0.5);
    enc.animate::<Camera2DFraming>(camera, keyframes_scalar(1.0, 40.0, 110.0), 1.0)
        .parallel::<Camera2DFraming>(
            camera,
            keyframes_vec2(1.0, Vec2::ZERO, Vec2::new(std::f32::consts::FRAC_PI_2, 1.0)),
        );
    enc.finish()
}

#[derive(Debug, PartialEq)]
struct Row {
    object: usize,
    offset: f64,
    span: (f64, f64),
    mode: String,
    target: String,
    curve: String,
}

fn observe(script: SceneScript) -> (Vec<Row>, SceneStruct, Vec<String>) {
    let mut scene = SceneStruct::new();
    let recipe = script.commit(&mut scene).expect("commit 成功");

    let rows: Vec<Row> = recipe
        .bindings
        .bindings
        .iter()
        .map(|b| {
            let node = recipe.forest.get(b.clock).expect("时钟节点在");
            let span = node.span.expect("Timeline 编译的节点必有 span");
            let object = recipe
                .handles
                .iter()
                .position(|id| *id == b.target.target_id())
                .expect("target 必须指向本脚本创建的对象");
            Row {
                object,
                offset: node.local_offset,
                span,
                mode: format!("{:?}", node.mode),
                target: b.target.name().to_string(),
                curve: format!("{:?}", b.curve),
            }
        })
        .collect();

    let mut components = Vec::new();
    for id in &recipe.handles {
        let mut kinds = Vec::new();
        if scene.with::<Framing2D, _>(*id, |_| ()).is_some() {
            kinds.push("Framing2D");
        }
        if scene.with::<Reveal, _>(*id, |_| ()).is_some() {
            kinds.push("Reveal");
        }
        if scene.with::<StrokeSolid, _>(*id, |_| ()).is_some() {
            kinds.push("StrokeSolid");
        }
        components.push(kinds.join("+"));
    }

    (rows, scene, components)
}

/// 场景里的关键组件值 `(Framing2D, StrokeSolid 的宽度 + rgba)`。
fn key_values(scene: &SceneStruct) -> ((f32, f32, f32), (f32, u8, u8, u8, u8)) {
    let mut camera = None;
    scene.for_each::<Framing2D>(|_, f| {
        camera = Some((f.center.x, f.center.y, f.scale));
    });
    let mut stroke = None;
    scene.for_each::<StrokeSolid>(|_, s| {
        stroke = Some((
            s.width,
            s.color.r(),
            s.color.g(),
            s.color.b(),
            s.color.a(),
        ));
    });
    (camera.expect("相机对象在场景里"), stroke.expect("描边对象在场景里"))
}

fn only_reveal_bounds(scene: &SceneStruct) -> xc_notion::Rect2D {
    let mut ids = Vec::new();
    scene.for_each::<Reveal>(|id, _| ids.push(id));
    assert_eq!(ids.len(), 1, "本子集只有一个 Reveal 对象");
    scene
        .with::<Reveal, _>(ids[0], |r| r.path.compute_bounds())
        .expect("Reveal 在场景里")
        .expect("轮廓应有包围盒")
}

/// **W1 前置验收**：脚本子集与手写编码器结构 + 时序 + 组件值全等。
#[test]
fn lua_subset_matches_handwritten_encoder() {
    let host = ScriptHost::new().expect("装配成功（scene + content）");
    let scripted = host.run(SUBSET_LUA).expect("Lua 脚本执行成功");

    let (script_rows, script_scene, script_components) = observe(scripted);
    let (hand_rows, hand_scene, hand_components) = observe(hand_written_subset());

    assert_eq!(script_components, hand_components, "组件集 / 创建顺序必须一致");
    assert_eq!(script_components, vec!["Framing2D", "Reveal+StrokeSolid"]);
    assert_eq!(script_rows.len(), 3);
    assert_eq!(script_rows, hand_rows, "时序（offset/span/mode/curve）与目标必须全等");

    let (cam_script, stroke_script) = key_values(&script_scene);
    let (cam_hand, stroke_hand) = key_values(&hand_scene);
    assert_eq!(cam_script, cam_hand);
    assert_eq!(stroke_script, stroke_hand);

    let b_script = only_reveal_bounds(&script_scene);
    let b_hand = only_reveal_bounds(&hand_scene);
    assert!((b_script.min.x - b_hand.min.x).abs() < 1e-4);
    assert!((b_script.max.y - b_hand.max.y).abs() < 1e-4);
}

/// 宿主 → 脚本（erased `run`）与脚本 → 宿主（`scene.*`）两条方向。
#[test]
fn host_calls_script_and_script_calls_host() {
    let host = ScriptHost::new().expect("装配成功");
    let script = host
        .run(
            r#"
            local cam = scene.spawn({ Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 40.0 } })
            local dot = scene.spawn({
                Circle { x = 1.0, y = 2.0, r = 0.5 },
                Stroke { width = 1.0, color = Color { r = 255, g = 0, b = 0, a = 255 } },
            })
            scene.animate(dot, scene.clip(Target.CircleRadius, Curve.Const { value = 0.5 }), 1.0)
            scene.despawn(cam)
            "#,
        )
        .expect("脚本成功");

    assert_eq!(script.len(), 2);
    assert_eq!(script.duration(), 1.0);

    let mut scene = SceneStruct::new();
    let recipe = script.commit(&mut scene).expect("commit");
    assert_eq!(recipe.bindings.bindings.len(), 1);
    assert_eq!(SceneRead::len(&scene), 1);
}

/// **W4 一致性护栏**：两个 capability 的 Lua 注册名 == 各自 `signatures()`。
#[test]
fn lua_registered_names_match_signatures() {
    let host = ScriptHost::new().expect("装配成功");

    let mut scene_registered = host.lua.registered_scene_names();
    let mut scene_expected: Vec<String> = SceneApi::signatures()
        .iter()
        .map(|sig| sig.name.to_string())
        .collect();
    scene_registered.sort();
    scene_expected.sort();
    assert_eq!(scene_registered, scene_expected, "scene 注册名必须一一对应");

    let mut content_registered = host.lua.registered_content_names();
    let mut content_expected: Vec<String> = ContentApi::signatures()
        .iter()
        .map(|sig| sig.name.to_string())
        .collect();
    content_registered.sort();
    content_expected.sort();
    assert_eq!(
        content_registered, content_expected,
        "content 注册名必须一一对应"
    );
}

/// 文档：条目数 == 函数数，description 非空。
#[test]
fn docs_are_complete() {
    for (sigs, docs) in [
        (SceneApi::signatures(), SceneApi::docs()),
        (ContentApi::signatures(), ContentApi::docs()),
    ] {
        assert_eq!(docs.len(), sigs.len());
        for (sig, doc) in sigs.iter().zip(docs) {
            assert_eq!(sig.name, doc.name);
            assert!(!doc.description.is_empty(), "`{}` 缺 ///", doc.name);
        }
    }
}

/// 导出：形参名 / 结构体 / 枚举 / 两组能力 / unit 变体。
#[test]
fn exported_docs_are_structural() {
    let json_text = xcal_script::scene_api_json();
    let json: serde_json::Value = serde_json::from_str(&json_text).expect("合法 JSON");

    let scene_functions = json["capabilities"]["scene"]["functions"]
        .as_array()
        .expect("scene functions");
    let content_functions = json["capabilities"]["content"]["functions"]
        .as_array()
        .expect("content functions");
    assert_eq!(scene_functions.len(), SceneApi::signatures().len());
    assert_eq!(content_functions.len(), ContentApi::signatures().len());

    let by_name = |name: &str| {
        scene_functions
            .iter()
            .find(|f| f["name"] == name)
            .unwrap_or_else(|| panic!("缺函数 {name}"))
    };

    let circle = by_name("circle");
    assert_eq!(circle["params"][0]["name"], "shape");
    assert_eq!(circle["params"][0]["type"]["name"], "xcal.Circle");

    let stroke = by_name("stroke");
    assert_eq!(stroke["params"][0]["name"], "style");
    assert_eq!(stroke["params"][0]["type"]["name"], "xcal.Stroke");

    let clip = by_name("clip");
    assert_eq!(clip["params"][0]["name"], "target");
    assert_eq!(clip["params"][0]["type"]["name"], "xcal.Target");

    let types = json["types"].as_array().expect("types 数组");
    let type_named = |name: &str| {
        types
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("缺类型 {name}"))
    };

    let stroke_ty = type_named("xcal.Stroke");
    let color_field = stroke_ty["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "color")
        .expect("Stroke.color");
    assert_eq!(color_field["type"]["name"], "xcal.Color");

    // 曲线：同一枚举的五个变体。
    let curve_ty = type_named("xcal.Curve");
    let variants: Vec<&str> = curve_ty["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(variants, vec!["Const", "Sin", "Phase", "Ramp", "Ramp2"]);

    // target：8 个 unit 变体。
    let target_ty = type_named("xcal.Target");
    let targets: Vec<&str> = target_ty["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(targets.len(), 8);
    assert!(targets.contains(&"RevealRange"));

    // 内容能力：返回 Array<Path>。
    let formula = content_functions
        .iter()
        .find(|f| f["name"] == "formula")
        .expect("content.formula");
    assert_eq!(formula["params"][0]["name"], "spec");
    assert_eq!(formula["params"][0]["type"]["name"], "xcal.Formula");
    assert_eq!(formula["returns"]["kind"], "array");
    assert_eq!(formula["returns"]["elem"]["name"], "xcal.Path");

    let lua_text = xcal_script::scene_api_lua();
    assert!(lua_text.contains("function scene.circle(shape) end"));
    assert!(lua_text.contains("function content.formula(spec) end"));
    assert!(lua_text.contains("function Curve.Phase(spec) end"));
    assert!(lua_text.contains("Target.RevealRange"), "unit 变体应是值");
    assert!(lua_text.contains("---@class Color"));
}

/// **W1 验收**：拼错 target 变体 ⇒ 明确错误（不是静默不排期）。
#[test]
fn misspelled_target_variant_is_an_explicit_error() {
    let host = ScriptHost::new().expect("装配成功");
    let err = host
        .run(
            r#"
            local dot = scene.spawn({ Circle { x = 0.0, y = 0.0, r = 1.0 } })
            scene.animate(dot, scene.clip(Target.RevealRage, Curve.Const { value = 1.0 }), 1.0)
            "#,
        )
        .err()
        .expect("拼错 variant ⇒ 必须失败");
    let message = err.to_string();
    assert!(
        message.contains("枚举 `xcal.Target`") || message.contains("nil"),
        "实际：{message}"
    );
}

/// **W4 验收**：把对象句柄当组件传给 `spawn` ⇒ 明确错误（不是静默丢句柄）。
#[test]
fn invalid_component_handle_is_an_explicit_error() {
    let host = ScriptHost::new().expect("装配成功");
    let err = host
        .run(
            r#"
            local object = scene.spawn({ Circle { x = 0.0, y = 0.0, r = 1.0 } })
            scene.spawn({ object })
            "#,
        )
        .err()
        .expect("非法组件句柄 ⇒ 必须失败");
    assert!(
        err.to_string().contains("组件句柄无效"),
        "实际：{err}"
    );
}

/// 未注入时 `run` 报错（默认拒绝）。
#[test]
fn missing_injection_is_explicit() {
    let bare = xcal_script::LuaVm::new();
    let err = bare.run("return 1").err().expect("未注入 ⇒ 必须失败");
    assert!(err.to_string().contains("尚未注入"), "实际：{err}");
}
