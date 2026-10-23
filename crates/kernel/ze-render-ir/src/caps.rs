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

/// 3D 网格的**形态标识**（B10 Step 1）。
///
/// 契约只说"后端接受哪种形态"，不搬运顶点布局细节 —— 布局是后端实现自己的事
/// （wgpu 用 `standard_3d()` 交错缓冲；矢量后端内部压成 `[pos, v] / [normal, u]`）。
///
/// - [`MeshForm::Tri`]：三角形面（`positions + indices`），有面积、可受光；
/// - [`MeshForm::Line`]：线段（无面积，只描线）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshForm {
    Tri,
    Line,
}

impl MeshForm {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tri => "tri",
            Self::Line => "line",
        }
    }
}

/// 后端接受的网格形态集合（B10 Step 1）。
///
/// 为什么是**集合**而不是单个枚举：同一后端可能同时接受两种形态（上游 `TriMesh` /
/// `LineMesh` 就是按 device features 二选一）。`default()` = 一种都不接受 ——
/// 新增后端必须**显式自述**，不靠默认值蒙混。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeshForms {
    pub tri: bool,
    pub line: bool,
}

impl MeshForms {
    /// 只接受三角形面。
    pub const fn tri() -> Self {
        Self {
            tri: true,
            line: false,
        }
    }

    /// 只接受线段。
    pub const fn line() -> Self {
        Self {
            tri: false,
            line: true,
        }
    }

    /// 两种都接受。
    pub const fn both() -> Self {
        Self {
            tri: true,
            line: true,
        }
    }

    pub const fn supports(self, form: MeshForm) -> bool {
        match form {
            MeshForm::Tri => self.tri,
            MeshForm::Line => self.line,
        }
    }
}

/// 请求了后端不接受的网格形态（B10 §3.2：**不支持即报错，不得静默选一个**）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("渲染能力缺网格形态：{}（后端自述接受 {}）", .requested.name(), .accepted)]
pub struct MeshFormError {
    pub requested: MeshForm,
    pub accepted: &'static str,
}

/// 后端声明实现了哪些 capability（**显式集合**，不是"试着调用"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RenderCapabilitySet {
    pub render2d: bool,
    pub render3d: bool,
    pub mixed: bool,
    pub shapes: bool,
    /// B10 Step 1：3D 网格形态。`from_names` 不猜（保持空集），必须显式给。
    pub mesh: MeshForms,
}

impl RenderCapabilitySet {
    /// **系统能力 = 已注册后端能力之并**（B4 Step 4）。
    ///
    /// 不得再用"某个后端的能力常量"冒充系统能力（那会把"只有一个后端"写进类型）。
    pub fn union(sets: &[Self]) -> Self {
        let mut out = Self::default();
        for set in sets {
            out.render2d |= set.render2d;
            out.render3d |= set.render3d;
            out.mixed |= set.mixed;
            out.shapes |= set.shapes;
            out.mesh.tri |= set.mesh.tri;
            out.mesh.line |= set.mesh.line;
        }
        out
    }

    /// 从 namespace 名集合构造（未知名字 ⇒ 由调用方先校验）。
    ///
    /// 网格形态**不由名字推出**（它不是 namespace）⇒ 这里留空集，
    /// 后端必须自己填 `mesh`（见 [`Self::with_mesh`]）。
    pub fn from_names(names: &[&str]) -> Self {
        Self {
            render2d: names.contains(&render2d::NAMESPACE),
            render3d: names.contains(&render3d::NAMESPACE),
            mixed: names.contains(&mixed::NAMESPACE),
            shapes: names.contains(&shapes::NAMESPACE),
            mesh: MeshForms::default(),
        }
    }

    /// 带网格形态的自述（链式；`from_names` 之后调用）。
    pub fn with_mesh(mut self, mesh: MeshForms) -> Self {
        self.mesh = mesh;
        self
    }

    /// **提交前按能力校验网格形态**：不支持 ⇒ 显式错误（不静默换一种）。
    pub fn require_mesh(self, form: MeshForm) -> Result<(), MeshFormError> {
        if self.mesh.supports(form) {
            return Ok(());
        }
        let accepted = match (self.mesh.tri, self.mesh.line) {
            (true, true) => "tri, line",
            (true, false) => "tri",
            (false, true) => "line",
            (false, false) => "（无）",
        };
        Err(MeshFormError {
            requested: form,
            accepted,
        })
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

    /// 系统能力是**并集**，不是某个后端的常量（B4 Step 4）。
    #[test]
    fn system_capabilities_are_a_union_of_backends() {
        let wgpu = RenderCapabilitySet::from_names(&[
            render2d::NAMESPACE,
            render3d::NAMESPACE,
            mixed::NAMESPACE,
        ])
        .with_mesh(MeshForms::tri());
        let vector = RenderCapabilitySet {
            shapes: true,
            mixed: true,
            mesh: MeshForms::line(),
            ..RenderCapabilitySet::default()
        };
        let system = RenderCapabilitySet::union(&[wgpu, vector]);
        assert!(system.render2d && system.render3d && system.mixed && system.shapes);
        // 单个后端仍如实自述（shapes 不因另一个后端支持就变 true）。
        assert!(!wgpu.shapes);
        // B10 Step 1：网格形态也是并集（各自仍如实自述）。
        assert!(system.mesh.tri && system.mesh.line);
        assert!(!wgpu.mesh.line);
    }

    // ─── B10 Step 1：网格形态是能力的一部分 ────────────────────────────────

    /// `from_names` **不猜**网格形态：不认识网格的后端必须是"一种都不接受"。
    #[test]
    fn from_names_does_not_guess_mesh_forms() {
        let set = RenderCapabilitySet::from_names(&[render3d::NAMESPACE]);
        assert!(set.render3d);
        assert_eq!(set.mesh, MeshForms::default());
        assert!(!set.mesh.tri && !set.mesh.line);
    }

    /// 请求不支持的形态 ⇒ **显式错误**（不静默换一种），错误文案点名两侧。
    #[test]
    fn unsupported_mesh_form_fails_explicitly() {
        let tri_only = RenderCapabilitySet::from_names(&[render3d::NAMESPACE])
            .with_mesh(MeshForms::tri());
        tri_only.require_mesh(MeshForm::Tri).expect("tri 受支持");

        let error = tri_only.require_mesh(MeshForm::Line).expect_err("line 不受支持");
        assert_eq!(error.requested, MeshForm::Line);
        assert_eq!(error.accepted, "tri");
        assert!(error.to_string().contains("line"), "{error}");
        assert!(error.to_string().contains("tri"), "{error}");

        let none = RenderCapabilitySet::default();
        let error = none.require_mesh(MeshForm::Tri).expect_err("空集不受支持");
        assert_eq!(error.accepted, "（无）");
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
