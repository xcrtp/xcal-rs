//! L4/L5/L6 验收：双向对接、脚本化最小分镜的等价性、注册名一致性护栏、文档非空。

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

use xcal_script::{SceneApi, ScriptHost, STORYBOARD_LUA};

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

/// 手写版：与 `scripts/storyboard.lua` 表达同一个最小分镜（镜头 2–5）。
fn hand_written_storyboard() -> SceneScript {
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
    enc.animate::<Camera2DFraming>(
        camera,
        keyframes_scalar(1.0, 40.0, 110.0),
        1.0,
    )
    .parallel::<Camera2DFraming>(
        camera,
        keyframes_vec2(1.0, Vec2::ZERO, Vec2::new(std::f32::consts::FRAC_PI_2, 1.0)),
    );
    enc.finish()
}

/// 一条绑定的可比对指纹：目标对象下标 / 时序 / 目标名 / 曲线 Debug。
#[derive(Debug, PartialEq)]
struct Row {
    /// target 指向的对象在 `recipe.handles` 里的下标（不是 ObjectId：两场景的 id 不同）。
    object: usize,
    offset: f64,
    span: (f64, f64),
    mode: String,
    target: String,
    curve: String,
}

/// 提交并抽取指纹（对象组件集合 + 绑定行）。
fn observe(script: SceneScript) -> (Vec<Row>, SceneStruct, Vec<String>) {
    let mut scene = SceneStruct::new();
    let recipe = script.commit(&mut scene).expect("commit 成功（组件均已登记）");

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

    // 组件集合：逐对象列出"有哪些组件"（顺序 = 创建顺序）。
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

/// 场景里唯一一个 `Reveal` 的几何包围盒。
fn only_reveal_bounds(scene: &SceneStruct) -> xc_notion::Rect2D {
    let mut ids = Vec::new();
    scene.for_each::<Reveal>(|id, _| ids.push(id));
    assert_eq!(ids.len(), 1, "本分镜只有一个 Reveal 对象");
    scene
        .with::<Reveal, _>(ids[0], |r| r.path.compute_bounds())
        .expect("Reveal 在场景里")
        .expect("轮廓应有包围盒")
}

/// **L5 等价性（验收 #1）**：脚本产出 ⇔ 手写产出。
///
/// 判据：对象数、创建顺序 / 组件集合、每条绑定的 offset / span / mode / 目标名 / 曲线。
#[test]
fn lua_storyboard_matches_handwritten_encoder() {
    let host = ScriptHost::new().expect("装配成功（Injector / SceneApi 齐备）");
    let scripted = host.run(STORYBOARD_LUA).expect("Lua 脚本执行成功");

    let (script_rows, script_scene, script_components) = observe(scripted);
    let (hand_rows, hand_scene, hand_components) = observe(hand_written_storyboard());

    assert_eq!(script_components, hand_components, "组件集 / 创建顺序必须一致");
    assert_eq!(script_components, vec!["Framing2D", "Reveal+StrokeSolid"]);

    assert_eq!(script_rows.len(), 3, "曲线 1 段 + 相机 2 条并行 → 3 条绑定");
    assert_eq!(script_rows, hand_rows, "时序（offset/span/mode/curve）与目标必须全等");
    eprintln!(
        "[L5] 对象数 {} / 组件 {} / 绑定 {}；脚本侧指纹 = {script_rows:#?}",
        script_scene.len(),
        script_components.len(),
        script_rows.len()
    );

    // 实际数值（交付说明用）：0–2 曲线 / 2.5–3.5 相机（同节点两条并行）。
    assert_eq!((script_rows[0].offset, script_rows[0].span), (0.0, (0.0, 2.0)));
    assert_eq!(script_rows[0].target, "RevealRange");
    assert_eq!((script_rows[1].offset, script_rows[1].span), (2.5, (0.0, 1.0)));
    assert_eq!(script_rows[2].offset, 2.5);
    assert_eq!(script_rows[1].target, "Camera2DFraming");
    assert_eq!(script_rows[2].target, "Camera2DFraming");
    assert_eq!(script_rows[0].curve, "Phase { omega: 0.5 }");

    // 曲线几何按**容差**比对（Lua 侧用 f64 算 sin 再落到 f32，末位可能差 1 ulp）。
    let b_script = only_reveal_bounds(&script_scene);
    let b_hand = only_reveal_bounds(&hand_scene);
    assert!((b_script.min.x - b_hand.min.x).abs() < 1e-4);
    assert!((b_script.max.y - b_hand.max.y).abs() < 1e-4);

    // **组件值**（不只是类型名）逐项相等：相机取景 + 描边宽度 / 颜色。
    let (cam_script, stroke_script) = key_values(&script_scene);
    let (cam_hand, stroke_hand) = key_values(&hand_scene);
    assert_eq!(cam_script, cam_hand, "相机组件值必须相等");
    assert_eq!(stroke_script, stroke_hand, "描边组件值必须相等");
    eprintln!("[L5] 相机 = {cam_script:?}；描边 = {stroke_script:?}");

    assert_eq!(script_scene.len(), 2);
    assert_eq!(hand_scene.len(), 2);
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

/// **验收 #6**：脚本真的在跑（真读 Lua 源、真产出对象）。
#[test]
fn host_calls_script_and_gets_objects() {
    let host = ScriptHost::new().expect("装配成功");
    let script = host
        .run(
            r#"
            local cam = scene.spawn({ Camera2D { center = Vec2 { x = 0.0, y = 0.0 }, scale = 40.0 } })
            local dot = scene.spawn({
                Circle { x = 1.0, y = 2.0, r = 0.5 },
                Stroke { width = 1.0, color = Color { r = 255, g = 0, b = 0, a = 255 } },
            })
            scene.animate(dot, scene.clip("CircleRadius", Curve.Const { value = 0.5 }), 1.0)
            scene.despawn(cam)
            "#,
        )
        .expect("脚本成功");

    assert_eq!(script.len(), 2, "脚本产出 2 个对象");
    assert_eq!(script.duration(), 1.0);
    eprintln!(
        "[L4] 脚本产出对象数 = {}，编排总时长 = {}s",
        script.len(),
        script.duration()
    );

    let mut scene = SceneStruct::new();
    let recipe = script.commit(&mut scene).expect("commit");
    assert_eq!(recipe.bindings.bindings.len(), 1);
    // despawn 在 commit 时执行 ⇒ 最终只剩 1 个对象。
    assert_eq!(SceneRead::len(&scene), 1);
    assert_eq!(recipe.handles.len(), 2, "句柄映射仍保留两条（含已销毁）");
}

/// **L6 一致性护栏（验收 #4）**：Lua 注册的函数名集合 == `SceneApi::signatures()` 名字集合。
///
/// 防"新增了 capability 函数但忘了接进 Lua"（同型问题：新增要改多处）。
/// 可证伪：在 `inject_impl` 里多 `set` 一个函数（或漏接一个）⇒ 本用例 RED。
#[test]
fn lua_registered_names_match_scene_api_signatures() {
    let host = ScriptHost::new().expect("装配成功");
    let mut registered = host.lua.registered_scene_names();
    let mut expected: Vec<String> = SceneApi::signatures()
        .iter()
        .map(|sig| sig.name.to_string())
        .collect();
    registered.sort();
    expected.sort();

    assert_eq!(registered.len(), expected.len(), "注册数量必须一致");
    assert_eq!(registered, expected, "注册名必须一一对应（不多不少）");
}

/// **验收 #2**：文档非空 —— 条目数 == 函数数，且每条 description 非空。
#[test]
fn scene_api_docs_are_complete() {
    let sigs = SceneApi::signatures();
    let docs = SceneApi::docs();
    assert_eq!(docs.len(), sigs.len(), "docs 与 signatures 逐条对应");
    for (sig, doc) in sigs.iter().zip(docs) {
        assert_eq!(sig.name, doc.name);
        assert_eq!(sig.qualified(), doc.qualified());
        assert!(
            !doc.description.is_empty(),
            "`{}` 必须有 `///` 文档",
            doc.name
        );
    }
}

/// **V5 验收（导出侧）**：形参名 / 结构体 / 枚举都进了 JSON 与 LuaLS。
#[test]
fn exported_docs_are_structural() {
    let json_text = xcal_script::scene_api_json();
    let json: serde_json::Value = serde_json::from_str(&json_text).expect("合法 JSON");
    let functions = json["functions"].as_array().expect("functions 数组");
    assert_eq!(functions.len(), SceneApi::signatures().len());
    assert!(
        functions
            .iter()
            .all(|f| !f["description"].as_str().unwrap_or("").is_empty()),
        "每个函数都要有 description"
    );

    let by_name = |name: &str| {
        functions
            .iter()
            .find(|f| f["name"] == name)
            .unwrap_or_else(|| panic!("缺函数 {name}"))
    };

    // 形参名不再是 p1（来自 schema）。
    let circle = by_name("circle");
    assert_eq!(circle["params"][0]["name"], "shape");
    assert_eq!(circle["params"][0]["type"]["kind"], "struct");
    assert_eq!(circle["params"][0]["type"]["name"], "xcal.Circle");
    assert_eq!(circle["returns"]["kind"], "opaque");

    // stroke 的颜色是结构体（不是 u32）。
    let stroke = by_name("stroke");
    assert_eq!(stroke["params"][0]["name"], "style");
    assert_eq!(stroke["params"][0]["type"]["name"], "xcal.Stroke");

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
    assert_eq!(color_field["type"]["kind"], "struct");
    assert_eq!(color_field["type"]["name"], "xcal.Color");

    // 曲线是同一枚举的五个变体。
    let curve_ty = type_named("xcal.Curve");
    assert_eq!(curve_ty["kind"], "enum");
    let variants: Vec<&str> = curve_ty["variants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(variants, vec!["Const", "Sin", "Phase", "Ramp", "Ramp2"]);

    let lua_text = xcal_script::scene_api_lua();
    for sig in SceneApi::signatures() {
        let item = SceneApi::calls()
            .iter()
            .find(|c| c.signature.name == sig.name)
            .expect("calls 与 signatures 逐条对应");
        let names: Vec<&str> = item.io.params.iter().map(|p| p.name).collect();
        assert!(
            lua_text.contains(&format!(
                "function scene.{}({}) end",
                sig.name,
                names.join(", ")
            )),
            "LuaLS 注解缺 `scene.{}`:\n{lua_text}",
            sig.name
        );
    }
    assert!(lua_text.contains("---@class Color"));
    assert!(lua_text.contains("---@field r integer"));
    assert!(lua_text.contains("function Curve.Phase(spec) end"));
}

/// **默认拒绝 + 明确错误**：未注入时 `run` 报错；类型不符报错；未知 target 静默不排期。
#[test]
fn errors_are_explicit() {
    let bare = xcal_script::LuaVm::new();
    let err = bare.run("return 1").err().expect("未注入 ⇒ 必须失败");
    assert!(err.to_string().contains("尚未注入"), "实际：{err}");

    let host = ScriptHost::new().expect("装配成功");
    let err = host
        .run("scene.wait('一秒')")
        .err()
        .expect("类型不符 ⇒ 必须失败");
    assert!(err.to_string().contains("期望 number"), "实际：{err}");

    // 未知 target：集中表里没有 ⇒ `animate_named` 不排期（结构化接口下没有错误通道）。
    let script = host
        .run(
            r#"
            local dot = scene.spawn({ Circle { x = 0.0, y = 0.0, r = 1.0 } })
            scene.animate(dot, scene.clip("NoSuchTarget", Curve.Const { value = 1.0 }), 1.0)
            "#,
        )
        .expect("脚本本身合法");
    let mut scene = SceneStruct::new();
    let recipe = script.commit(&mut scene).expect("commit");
    assert_eq!(recipe.bindings.bindings.len(), 0, "未知 target 不产生绑定");
}
