//! 能力覆盖矩阵的门禁逻辑（B7 Step 5）——纯函数用例，不碰文件系统。
//!
//! 覆盖三件事：能力清单从源码抽得对、使用判定两种形态（方法调用 / 构造器）都认、
//! 豁免必须指向真实存在的 GAPS 行且不能过期。

use xtask::{Capability, Exemption, check_coverage, collect_capabilities, parse_exemptions};

const API: &str = r#"
pub struct SceneApi<'a> {
    /// 构造圆组件。
    pub circle: fn(&'a dyn Pod, shape: Circle) -> Handle,

    /// 创建对象：组件句柄列表。
    pub spawn: fn(&'a dyn Pod, components: Vec<Handle>) -> Handle,

    /// 顺序播放一段动画。
    pub animate: fn(&'a dyn Pod, object: Handle, clip: Handle, duration: f64) -> (),
}
"#;

const TYPES: &str = r#"
pub enum Curve {
    // 注释要跳过
    Const { value: f32 },
    Phase { omega: f32 },
}

pub enum TransformChannel {
    Position,
    Rotation,
}
"#;

const TARGET: &str = r#"
pub enum TargetKind {
    CircleRadius,
    MorphT,
}
"#;

const GAPS: &str = r#"
| W1-05 | 某能力 | 层 | **记账** | 证据 |
"#;

fn capabilities() -> Vec<Capability> {
    collect_capabilities(API, TYPES, TARGET)
}

/// 方法名 / 枚举变体 / 构造器别名都从源码抽（顺序 = 源码顺序）。
#[test]
fn capabilities_come_from_source() {
    let caps = capabilities();
    let ids: Vec<String> = caps.iter().map(Capability::id).collect();
    assert_eq!(
        ids,
        vec![
            "scene:circle",
            "scene:spawn",
            "scene:animate",
            "Curve:Const",
            "Curve:Phase",
            "TransformChannel:Position",
            "TransformChannel:Rotation",
            "Target:CircleRadius",
            "Target:MorphT",
        ]
    );
    // 单结构体参数才有构造器别名；`Vec<Handle>` / 多参数没有。
    let alias = caps
        .iter()
        .find(|cap| cap.name == "circle")
        .and_then(|cap| cap.alias.as_deref());
    assert_eq!(alias, Some("Circle"));
    assert!(caps.iter().find(|c| c.name == "spawn").unwrap().alias.is_none());
    assert!(caps.iter().find(|c| c.name == "animate").unwrap().alias.is_none());
}

/// 方法调用与构造器两种写法都算覆盖；`scene.clip` 不得被 `scene.clip_transform` 误判。
#[test]
fn usage_accepts_method_and_constructor_forms() {
    let caps = capabilities();
    let scripts = r#"
        local c = Circle { x = 0.0, y = 0.0, r = 1.0 }
        scene.spawn({ c })
        scene.animate(c, scene.clip(Target.MorphT, Curve.Phase { omega = 1.0 }), 1.0)
        scene.parallel(c, scene.clip(Target.CircleRadius, Curve.Const { value = 1.0 }))
        scene.clip_transform(TransformChannel.Position, Curve.Const { value = 1.0 })
        scene.clip_transform(TransformChannel.Rotation, Curve.Const { value = 1.0 })
    "#;
    let exemptions: Vec<Exemption> = Vec::new();
    let report = check_coverage(&caps, scripts, &exemptions, GAPS).expect("全部覆盖");
    assert_eq!(report.total, caps.len());
    assert_eq!(report.covered.len(), caps.len());
}

/// 两项能力的最小夹具（豁免 / 过期用例不需要整份源码）。
fn two_caps() -> Vec<Capability> {
    vec![
        Capability {
            kind: "scene".to_string(),
            name: "spawn".to_string(),
            alias: None,
            location: "api.rs:7".to_string(),
        },
        Capability {
            kind: "scene".to_string(),
            name: "circle".to_string(),
            alias: Some("Circle".to_string()),
            location: "api.rs:4".to_string(),
        },
    ]
}

/// 未覆盖且未豁免 ⇒ 报错并点名（含出处）；补上豁免后通过。
#[test]
fn uncovered_capability_fails_unless_exempted() {
    let caps = two_caps();
    let scripts = "scene.spawn({})";
    let error = check_coverage(&caps, scripts, &[], GAPS).expect_err("未覆盖 ⇒ fail");
    assert!(error.contains("scene:circle"), "错误应点名：{error}");
    assert!(error.contains("api.rs:4"), "错误应给出处：{error}");

    let exemptions = parse_exemptions("scene:circle -> W1-05  # 说明\n").expect("合法豁免");
    let report = check_coverage(&caps, scripts, &exemptions, GAPS).expect("豁免后通过");
    assert_eq!(report.exempted.len(), 1);
}

/// 豁免必须指向 GAPS 里真实存在的表格行（不能只写在豁免文件里）。
#[test]
fn exemption_requires_a_real_gaps_row() {
    let caps = capabilities();
    let exemptions = parse_exemptions("scene:circle -> W1-99\n").expect("格式合法");
    let error = check_coverage(&caps, "scene.spawn({})", &exemptions, GAPS)
        .expect_err("GAPS 无此行 ⇒ fail");
    assert!(error.contains("W1-99"), "错误应点名：{error}");
}

/// 豁免过期（能力已被作品用到）⇒ fail：机制不允许残留的死豁免。
#[test]
fn stale_exemption_fails() {
    let caps = two_caps();
    let exemptions = parse_exemptions("scene:circle -> W1-05\n").expect("格式合法");
    let error = check_coverage(&caps, "scene.spawn({}) scene.circle(Circle {})", &exemptions, GAPS)
        .expect_err("已覆盖还豁免 ⇒ fail");
    assert!(error.contains("豁免已过期"), "错误应说明：{error}");
}

/// 豁免格式错误（缺 `->` / 缺 `kind:` / 不指向 W1）都要显式报错。
#[test]
fn exemption_format_is_enforced() {
    for source in ["scene:circle", "circle -> W1-05", "scene:circle -> G-101"] {
        assert!(
            parse_exemptions(source).is_err(),
            "`{source}` 应被拒绝"
        );
    }
}
