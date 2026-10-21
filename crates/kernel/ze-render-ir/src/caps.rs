// caps.rs — 四个渲染 capability 的**契约签名集**（B3-0.5c / D4.1）。
//
// # 为什么拆四个 namespace
// `negotiate` 是**全有或全无**（ze-pod-core）：只做 2D 的后端必须拿到**显式协商失败**，
// 而不是"实现了但画不出来"或靠 `unimplemented!()` 撑着（铁律 3：不静默）。
//
// # 这里只有契约，没有实现
// 签名是跨边界契约；bulk 数据走 `PodBytes`（契约）或 typed 快路径（优化，D4.4）。
// wgpu 侧的落地见 `zela-render-backends`（B3-0.5d）；本模块**零 wgpu 类型**。
//
// # 粒度（D4.5）
// `submit_layer` 是**每帧每 layer 一次**（粗粒度）—— pod 调用次数 ∝ 帧数，
// 与 draw 数无关。不得拆成"每 draw 一次"。

use ze_pod_core::{FnSig, capability_fingerprint};

/// `zela.render2d`：2D 专用（描边 / Reveal / Morph / 文本轮廓 / 2D 离屏）。
pub mod render2d {
    use super::FnSig;

    pub const NAMESPACE: &str = "zela.render2d";

    /// 契约签名（顺序即契约；`capability_fingerprint` 依赖它）。
    pub const SIGNATURES: &[FnSig] = &[
        FnSig::new(NAMESPACE, "capabilities", &[], "Capabilities"),
        FnSig::new(NAMESPACE, "create_target", &["RenderTargetDesc"], "Handle"),
        FnSig::new(NAMESPACE, "upload", &["PodBytes"], "Handle"),
        FnSig::new(NAMESPACE, "draw_2d", &["Handle", "PodBytes"], "()"),
    ];
}

/// `zela.render3d`：3D 专用（光照 / 深度 / PBR / 3D 离屏）。
pub mod render3d {
    use super::FnSig;

    pub const NAMESPACE: &str = "zela.render3d";

    pub const SIGNATURES: &[FnSig] = &[
        FnSig::new(NAMESPACE, "capabilities", &[], "Capabilities"),
        FnSig::new(NAMESPACE, "create_target", &["RenderTargetDesc"], "Handle"),
        FnSig::new(NAMESPACE, "upload", &["PodBytes"], "Handle"),
        FnSig::new(NAMESPACE, "draw_3d", &["Handle", "PodBytes"], "()"),
    ];
}

/// `zela.render.mixed`：2D/3D 混合（共享队列 / 透明桶 / Surface 代理片）。
pub mod mixed {
    use super::FnSig;

    pub const NAMESPACE: &str = "zela.render.mixed";

    pub const SIGNATURES: &[FnSig] = &[
        FnSig::new(NAMESPACE, "capabilities", &[], "Capabilities"),
        FnSig::new(NAMESPACE, "create_target", &["RenderTargetDesc"], "Handle"),
        FnSig::new(NAMESPACE, "submit_layer", &["Handle", "PodBytes"], "()"),
    ];
}

/// `zela.render.shapes`：**直接吃图元**（`Path2D` 含曲线控制点），不经过顶点契约。
///
/// 战略意义（依据 #18/#19）：`tessellate` 会先把曲线展平，矢量后端因此拿不到原始
/// 曲线；本 capability 把 `PathList`（含 QuadTo / CubicTo / Arc）直接交给后端，
/// 为解析抗锯齿 / 矢量化渲染铺路。**不需要 `VertexLayout`**（D4.2）。
pub mod shapes {
    use super::FnSig;

    pub const NAMESPACE: &str = "zela.render.shapes";

    pub const SIGNATURES: &[FnSig] = &[
        FnSig::new(NAMESPACE, "capabilities", &[], "Capabilities"),
        FnSig::new(NAMESPACE, "submit_shapes", &["Handle", "PathList"], "()"),
    ];
}

/// 四个 namespace 的固定顺序（报告 / 指纹顺序）。
pub const NAMESPACES: [&str; 4] = [
    render2d::NAMESPACE,
    render3d::NAMESPACE,
    mixed::NAMESPACE,
    shapes::NAMESPACE,
];

/// 按 namespace 取签名集（未知 namespace ⇒ `None`，不静默兜底）。
pub fn signatures_of(namespace: &str) -> Option<&'static [FnSig]> {
    match namespace {
        render2d::NAMESPACE => Some(render2d::SIGNATURES),
        render3d::NAMESPACE => Some(render3d::SIGNATURES),
        mixed::NAMESPACE => Some(mixed::SIGNATURES),
        shapes::NAMESPACE => Some(shapes::SIGNATURES),
        _ => None,
    }
}

/// 四个 namespace 的**能力指纹**（`hash(signatures())`，D14 机制）。
pub fn fingerprints() -> [(&'static str, u64); 4] {
    [
        (render2d::NAMESPACE, capability_fingerprint(render2d::SIGNATURES)),
        (render3d::NAMESPACE, capability_fingerprint(render3d::SIGNATURES)),
        (mixed::NAMESPACE, capability_fingerprint(mixed::SIGNATURES)),
        (shapes::NAMESPACE, capability_fingerprint(shapes::SIGNATURES)),
    ]
}

/// 后端声明实现了哪些 capability（**显式集合**，不是"试着调用"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderCapabilitySet {
    pub render2d: bool,
    pub render3d: bool,
    pub mixed: bool,
    pub shapes: bool,
}

impl RenderCapabilitySet {
    /// 全部实现（wgpu 当前形态）。
    pub const ALL: Self = Self {
        render2d: true,
        render3d: true,
        mixed: true,
        shapes: false,
    };

    /// 从 namespace 名集合构造（未知名字 ⇒ 由调用方先校验）。
    pub fn from_names(names: &[&str]) -> Self {
        Self {
            render2d: names.contains(&render2d::NAMESPACE),
            render3d: names.contains(&render3d::NAMESPACE),
            mixed: names.contains(&mixed::NAMESPACE),
            shapes: names.contains(&shapes::NAMESPACE),
        }
    }

    /// 缺失的 namespace（按固定顺序）。
    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.render2d {
            out.push(render2d::NAMESPACE);
        }
        if !self.render3d {
            out.push(render3d::NAMESPACE);
        }
        if !self.mixed {
            out.push(mixed::NAMESPACE);
        }
        if !self.shapes {
            out.push(shapes::NAMESPACE);
        }
        out
    }
}

/// 协商失败：显式列出缺失的 namespace（**不降级兜底**）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("渲染能力缺失：{}（全有或全无，不静默降级）", .missing.join(", "))]
pub struct CapabilityNegotiationError {
    pub missing: Vec<&'static str>,
}

/// 按所需 namespace 协商：全部实现 ⇒ `Ok`；任一缺失 ⇒ **显式错误**。
///
/// `available` 是后端自述的 namespace 列表；未知名字 ⇒ 也视为缺失（契约外名字
/// 不得冒充实现）。
pub fn negotiate(
    available: &[&str],
    required: &[&str],
) -> Result<RenderCapabilitySet, CapabilityNegotiationError> {
    let set = RenderCapabilitySet::from_names(available);
    let missing: Vec<&'static str> = required
        .iter()
        .filter(|name| signatures_of(name).is_some_and(|_| !available.contains(name)))
        .map(|name| match *name {
            render2d::NAMESPACE => render2d::NAMESPACE,
            render3d::NAMESPACE => render3d::NAMESPACE,
            mixed::NAMESPACE => mixed::NAMESPACE,
            shapes::NAMESPACE => shapes::NAMESPACE,
            _ => "unknown",
        })
        .collect();
    if missing.is_empty() {
        Ok(set)
    } else {
        Err(CapabilityNegotiationError { missing })
    }
}

/// 钉住的指纹基线（**改签名必须同时更新此表**，否则测试红）。
///
/// 与 B2-9 的机制一致：指纹变 ⇒ 契约变 ⇒ 必须显式确认（版本 / 文档 / 记账）。
pub const PINNED_FINGERPRINTS: [(&str, u64); 4] = [
    (render2d::NAMESPACE, 11_093_885_516_640_437_004),
    (render3d::NAMESPACE, 15_423_555_431_816_673_099),
    (mixed::NAMESPACE, 2_683_605_281_506_751_807),
    (shapes::NAMESPACE, 1_097_982_529_554_556_752),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 四个 namespace 可枚举，且签名非空、全部属于本 namespace。
    #[test]
    fn four_namespaces_are_enumerable() {
        assert_eq!(NAMESPACES.len(), 4);
        for name in NAMESPACES {
            let sigs = signatures_of(name).unwrap_or_else(|| panic!("{name} 未注册"));
            assert!(!sigs.is_empty(), "{name} 的签名集不能为空");
            assert!(sigs.iter().all(|s| s.namespace == name));
        }
    }

    /// 能力指纹与钉住表一致（改动先失败，逼开发者显式确认）。
    #[test]
    fn render_fingerprints_are_pinned() {
        assert_eq!(
            fingerprints(),
            PINNED_FINGERPRINTS,
            "渲染 capability 签名变了 ⇒ 必须更新 PINNED_FINGERPRINTS 并记账"
        );
    }

    /// 只实现部分 capability 的后端 ⇒ **显式协商失败**（列出缺失项）。
    #[test]
    fn partial_backend_fails_negotiation_explicitly() {
        // 只做 2D 的后端请求 mixed：必须失败。
        let err = negotiate(&[render2d::NAMESPACE], &[render2d::NAMESPACE, mixed::NAMESPACE])
            .unwrap_err();
        assert_eq!(err.missing, vec![mixed::NAMESPACE]);
        assert!(err.to_string().contains("zela.render.mixed"));

        // 全部满足 ⇒ 成功，且集合正确。
        let set = negotiate(&NAMESPACES, &NAMESPACES).unwrap();
        assert!(set.mixed && set.render3d && set.render2d && set.shapes);

        // 未知名字不能冒充实现。
        let err = negotiate(&["zela.render.unknown"], &[mixed::NAMESPACE]).unwrap_err();
        assert_eq!(err.missing, vec![mixed::NAMESPACE]);
    }

    /// 签名集合里**不出现** wgpu 机制名（契约只表达意图，D19）。
    #[test]
    fn signatures_contain_no_gpu_mechanism_names() {
        for name in NAMESPACES {
            for sig in signatures_of(name).unwrap() {
                let mut all = vec![sig.name];
                all.extend_from_slice(sig.params);
                all.push(sig.ret);
                for token in all {
                    let lower = token.to_ascii_lowercase();
                    for forbidden in [
                        "wgpu", "device", "queue", "textureview", "cullmode", "blendstate",
                        "samplecount",
                    ] {
                        assert!(
                            !lower.contains(forbidden),
                            "{name} 的签名 `{token}` 泄漏了 GPU 机制名 `{forbidden}`"
                        );
                    }
                }
            }
        }
    }
}
