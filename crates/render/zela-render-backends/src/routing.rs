// routing.rs — 后端路由（B11 Step 4）。
//
// # 为什么在 backends（而不是宿主）
//
// 「用哪个后端」是后端的装配问题，不是某个宿主的问题：native `zela-app` 与浏览器
// `zela-web` 都必须用**同一份**决策（否则两边会漂成两套规则）。
//
// # 硬约束
//
// - C1 动态 ≠ 不确定：输入只有帧内容与 adapter 事实，没有负载 / 墙钟 / 随机数；
// - C4 不支持即报错，不得静默选一个：顺序回退时**带理由**，全部被拒则报错。

use crate::registry::{AdapterFacts, BackendKind};

/// 出帧后端 / 路由偏好（`zela-render --backend` 同义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopBackend {
    /// 按能力自动选，作品级（第一帧决定，全程一致）。
    Auto,
    /// 自动选，但每帧重算（同一作品内可能换画风，留作诊断开关）。
    AutoFrame,
    /// 显式 wgpu（现状基线路径；不做回退）。
    Wgpu,
    /// **偏好**矢量；矢量拒绝该帧时回退 wgpu，并打印理由（B11 §6.3）。
    Vector,
}

impl LoopBackend {
    /// 具体后端（`Auto*` 未决前为 `None`）。
    pub fn concrete(self) -> Option<BackendKind> {
        match self {
            LoopBackend::Auto | LoopBackend::AutoFrame => None,
            LoopBackend::Wgpu => Some(BackendKind::Wgpu),
            LoopBackend::Vector => Some(BackendKind::Vector),
        }
    }

    /// 解析 `--backend` 的取值。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(LoopBackend::Auto),
            "auto-frame" => Some(LoopBackend::AutoFrame),
            "wgpu" => Some(LoopBackend::Wgpu),
            "vector" => Some(LoopBackend::Vector),
            _ => None,
        }
    }
}

/// 一帧"画什么"的特征（**只描述内容**，不含任何后端偏好）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameContent {
    pub has_3d_camera: bool,
    /// 2D 图元条数（`DrawQueue` 里的 2D 记录）。
    pub has_2d_primitives: usize,
    pub has_meshes: usize,
    /// 网格里是否含非三角形（线框）。
    pub has_line_meshes: bool,
    /// 是否含内容纹理 / Surface 图层。
    pub has_content_texture: bool,
}

/// 路由决定（可观测面；下一批"可观测性专题"接成结构化输出）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingDecision {
    pub backend: BackendKind,
    /// 为什么选它（人可读）。
    pub reason: String,
    /// 谁被拒、为什么（按尝试顺序；被选中的不在其中）。
    pub rejected: Vec<(BackendKind, String)>,
}

impl RoutingDecision {
    /// 显式 `--backend wgpu` 的决策。
    ///
    /// 该路径**不取帧快照**（省一次构图），因此没有内容事实可路由；但决策仍要记录一条，
    /// 否则默认门禁路径在 B13 的决策日志里会缺条目（看起来像"没做决策"）。
    pub fn explicit_wgpu() -> Self {
        Self {
            backend: BackendKind::Wgpu,
            reason: "显式指定 wgpu：不取帧快照、不做回退".to_string(),
            rejected: Vec::new(),
        }
    }

    /// 一行可读报告（B11 §6.4：选了谁、为什么、谁被拒、拒绝理由）。
    pub fn report(&self) -> String {
        let mut out = format!("选择 {:?}（{}）", self.backend, self.reason);
        for (kind, why) in &self.rejected {
            out.push_str(&format!("；{kind:?} 被拒：{why}"));
        }
        out
    }

    /// 结构化一行（B12 Step 2）：可 diff、可归档，B13 对拍时作为"第一现场"。
    ///
    /// 只含决策本身的事实，不含时间戳 / 运行次数 —— 同一输入必须产出同一行（C1）。
    pub fn json(&self, work: &str, pass: &str) -> String {
        format!("[decision] {}", self.object_json(work, pass))
    }

    /// 决策对象本体（B12 Step 3：结构化报告要把决策嵌进同一个 JSON 对象）。
    pub fn object_json(&self, work: &str, pass: &str) -> String {
        let rejected: Vec<String> = self
            .rejected
            .iter()
            .map(|(kind, why)| {
                format!(
                    "{{\"backend\":\"{kind:?}\",\"reason\":\"{}\"}}",
                    escape_json(why)
                )
            })
            .collect();
        format!(
            "{{\"work\":\"{}\",\"pass\":\"{}\",\"backend\":\"{:?}\",\
             \"reason\":\"{}\",\"rejected\":[{}]}}",
            escape_json(work),
            escape_json(pass),
            self.backend,
            escape_json(&self.reason),
            rejected.join(",")
        )
    }

    /// C4：拒绝理由不得为空（空理由 = 静默决策），且"被选中的后端不得同时出现在拒绝里"。
    pub fn validate(&self) -> Result<(), String> {
        if self.reason.trim().is_empty() {
            return Err(format!(
                "{:?} 的选择理由为空 ⇒ 静默决策（B12 Step 2 / C4）",
                self.backend
            ));
        }
        for (kind, why) in &self.rejected {
            if why.trim().is_empty() {
                return Err(format!(
                    "{kind:?} 的拒绝理由为空 ⇒ 静默决策（B12 Step 2 / C4）"
                ));
            }
            if *kind == self.backend {
                return Err(format!("{kind:?} 既被选中又被拒 ⇒ 决策自相矛盾"));
            }
        }
        Ok(())
    }
}

/// JSON 字符串转义（拒绝理由里有中文与 `⇒`，不能假设安全字符集）。
fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// 按偏好得到尝试顺序。
fn order_of(preference: LoopBackend) -> &'static [BackendKind] {
    match preference {
        LoopBackend::Wgpu => &[BackendKind::Wgpu],
        // 偏好矢量：先矢量，被拒才回退（回退**打印理由**，不是静默）。
        LoopBackend::Vector => &[BackendKind::Vector, BackendKind::Wgpu],
        // 自动默认 wgpu：保住现有 16 条基线（C3），矢量只在 wgpu 拒绝时接手。
        LoopBackend::Auto | LoopBackend::AutoFrame => &[BackendKind::Wgpu, BackendKind::Vector],
    }
}

/// 某后端拒绝该帧的理由（能画 ⇒ `None`）。
fn reject_reason(
    kind: BackendKind,
    content: FrameContent,
    facts: Option<AdapterFacts>,
) -> Option<String> {
    match kind {
        // wgpu 三角化不依赖 adapter feature；内容纹理 / 混排 / 网格形态都由它自己的
        // 后端实现覆盖（那是今天的默认路径）。
        BackendKind::Wgpu => None,
        BackendKind::Vector => {
            // 1) adapter 硬前置（B11 Step 0）。
            match facts {
                Some(facts) => {
                    if let Some(why) = kind.unavailable_reason_for(facts) {
                        return Some(why);
                    }
                }
                None => {
                    return Some(
                        "上下文没有 adapter ⇒ 无法确认 FLOAT32_BLENDABLE / DEPTH32FLOAT_STENCIL8（C4 不允许假设可用）"
                            .to_string(),
                    );
                }
            }
            // 2) 内容能力（与 frame_loop 里原先"硬报错"的判据同一组事实）。
            if content.has_content_texture {
                return Some(
                    "本帧含内容纹理 / Surface 图层（矢量后端还没有映射，GAPS V2-12/V2-13）"
                        .to_string(),
                );
            }
            if content.has_3d_camera && content.has_2d_primitives > 0 {
                return Some(format!(
                    "本帧 2D 图元（{} 条）与 3D 相机混排（GAPS X1-06）",
                    content.has_2d_primitives
                ));
            }
            if content.has_3d_camera && content.has_meshes == 0 {
                return Some("本帧有 3D 相机但没有任何网格".to_string());
            }
            if content.has_line_meshes {
                return Some("网格含线框（矢量后端只接受三角形面，MeshForm::Tri）".to_string());
            }
            None
        }
    }
}

/// 按给定顺序路由（测试可传入自定义顺序，生产用 [`route`]）。
pub fn route_with_order(
    order: &[BackendKind],
    preference: LoopBackend,
    content: FrameContent,
    facts: Option<AdapterFacts>,
) -> Result<RoutingDecision, String> {
    let mut rejected: Vec<(BackendKind, String)> = Vec::new();
    for kind in order {
        match reject_reason(*kind, content, facts) {
            None => {
                let reason = if rejected.is_empty() {
                    match preference {
                        LoopBackend::Wgpu => "显式指定 wgpu".to_string(),
                        LoopBackend::Vector => "显式指定矢量".to_string(),
                        LoopBackend::Auto | LoopBackend::AutoFrame => {
                            format!("自动路由：{:?} 能力覆盖本帧", kind)
                        }
                    }
                } else {
                    let (who, _) = &rejected[0];
                    format!("首选 {who:?} 被拒 ⇒ 回退到 {kind:?}")
                };
                return Ok(RoutingDecision {
                    backend: *kind,
                    reason,
                    rejected,
                });
            }
            Some(reason) => rejected.push((*kind, reason)),
        }
    }
    Err(format!(
        "没有后端能画这一帧：{}",
        rejected
            .iter()
            .map(|(kind, why)| format!("{kind:?}：{why}"))
            .collect::<Vec<_>>()
            .join("；")
    ))
}

/// 按偏好路由（生产入口）。
pub fn route(
    preference: LoopBackend,
    content: FrameContent,
    facts: Option<AdapterFacts>,
) -> Result<RoutingDecision, String> {
    route_with_order(order_of(preference), preference, content, facts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capable() -> Option<AdapterFacts> {
        Some(AdapterFacts {
            features: wgpu::Features::FLOAT32_BLENDABLE
                | wgpu::Features::DEPTH32FLOAT_STENCIL8,
            blends_float32: true,
        })
    }

    fn plain_2d() -> FrameContent {
        FrameContent {
            has_2d_primitives: 3,
            ..Default::default()
        }
    }

    /// 默认（自动）路径：wgpu 能画 ⇒ 选 wgpu，且没有后端被拒。
    /// 这是 16 条基线不变的**决策级**理由。
    #[test]
    fn auto_prefers_wgpu_so_baselines_stay_put() {
        let decision = route(LoopBackend::Auto, plain_2d(), capable()).unwrap();
        assert_eq!(decision.backend, BackendKind::Wgpu);
        assert!(decision.rejected.is_empty());
        assert!(decision.reason.contains("自动"));
    }

    /// §6.3 的核心增量：`--backend vector` + 内容纹理 ⇒ 回退 wgpu，**理由非空**。
    #[test]
    fn vector_falls_back_to_wgpu_with_a_reason() {
        let content = FrameContent {
            has_content_texture: true,
            ..plain_2d()
        };
        let decision = route(LoopBackend::Vector, content, capable()).unwrap();
        assert_eq!(decision.backend, BackendKind::Wgpu);
        assert_eq!(decision.rejected.len(), 1);
        assert_eq!(decision.rejected[0].0, BackendKind::Vector);
        assert!(!decision.rejected[0].1.is_empty(), "拒绝必须带理由");
        assert!(decision.reason.contains("回退"));
    }

    /// 显式矢量 + 能力齐备 + 单纯 2D ⇒ 真的用矢量（不回退）。
    #[test]
    fn vector_is_used_when_it_can_draw_the_frame() {
        let decision = route(LoopBackend::Vector, plain_2d(), capable()).unwrap();
        assert_eq!(decision.backend, BackendKind::Vector);
        assert!(decision.rejected.is_empty());
    }

    /// adapter 缺 feature ⇒ 矢量被拒（理由来自 Step 0 的自述），回退 wgpu。
    #[test]
    fn adapter_without_features_rejects_vector() {
        let weak = Some(AdapterFacts {
            features: wgpu::Features::empty(),
            blends_float32: false,
        });
        let decision = route(LoopBackend::Vector, plain_2d(), weak).unwrap();
        assert_eq!(decision.backend, BackendKind::Wgpu);
        assert!(decision.rejected[0].1.contains("R32Float"));
    }

    /// 没有 adapter ⇒ 不假设矢量可用（C4）。
    #[test]
    fn missing_adapter_is_not_assumed_available() {
        let decision = route(LoopBackend::Vector, plain_2d(), None).unwrap();
        assert_eq!(decision.backend, BackendKind::Wgpu);
        assert!(decision.rejected[0].1.contains("adapter"));
    }

    /// 显式 wgpu 不做回退：顺序里只有 wgpu。
    #[test]
    fn explicit_wgpu_has_no_fallback() {
        let decision = route(LoopBackend::Wgpu, plain_2d(), capable()).unwrap();
        assert_eq!(decision.backend, BackendKind::Wgpu);
        assert!(decision.rejected.is_empty());
    }

    /// 全部后端都拒绝 ⇒ 显式错误，且**每条**拒绝理由都非空（A4）。
    #[test]
    fn all_rejected_is_an_explicit_error_with_reasons() {
        let content = FrameContent {
            has_line_meshes: true,
            has_meshes: 1,
            has_3d_camera: true,
            ..Default::default()
        };
        let error = route_with_order(
            &[BackendKind::Vector],
            LoopBackend::Vector,
            content,
            capable(),
        )
        .unwrap_err();
        assert!(error.contains("没有后端能画这一帧"));
        assert!(error.contains("线框"));
    }

    /// C1：同一输入跑两次，决定逐字段相同。
    #[test]
    fn routing_is_repeatable() {
        let content = FrameContent {
            has_3d_camera: true,
            has_meshes: 7,
            ..Default::default()
        };
        let a = route(LoopBackend::Vector, content, capable()).unwrap();
        let b = route(LoopBackend::Vector, content, capable()).unwrap();
        assert_eq!(a, b);
        // B12 Step 2：可重复也包括**落盘行**逐字节相同（不含时间 / 次数）。
        assert_eq!(a.json("work", "script"), b.json("work", "script"));
    }

    /// B12 Step 2：生产路径产生的每条决策都能通过校验（拒绝理由非空）。
    #[test]
    fn every_production_decision_passes_validation() {
        let weak = Some(AdapterFacts {
            features: wgpu::Features::empty(),
            blends_float32: false,
        });
        let contents = [
            plain_2d(),
            FrameContent {
                has_content_texture: true,
                ..plain_2d()
            },
            FrameContent {
                has_3d_camera: true,
                has_meshes: 5,
                ..Default::default()
            },
            FrameContent {
                has_3d_camera: true,
                has_2d_primitives: 2,
                has_meshes: 1,
                ..Default::default()
            },
            FrameContent {
                has_3d_camera: true,
                has_meshes: 1,
                has_line_meshes: true,
                ..Default::default()
            },
        ];
        for content in contents {
            for preference in [
                LoopBackend::Auto,
                LoopBackend::AutoFrame,
                LoopBackend::Wgpu,
                LoopBackend::Vector,
            ] {
                let decision = route(preference, content, capable()).unwrap();
                decision
                    .validate()
                    .unwrap_or_else(|e| panic!("{preference:?} / {content:?}：{e}"));
            }
            // adapter 不足 / 缺失也必须带理由（不能"悄悄回退"）。
            for facts in [weak, None] {
                let decision = route(LoopBackend::Vector, content, facts).unwrap();
                decision.validate().unwrap();
            }
        }
        RoutingDecision::explicit_wgpu().validate().unwrap();
    }

    /// 空理由 = 静默决策 ⇒ 校验必须报错（不是警告）。
    #[test]
    fn empty_reason_fails_validation() {
        let mut decision = RoutingDecision::explicit_wgpu();
        decision.reason.clear();
        assert!(decision.validate().unwrap_err().contains("静默决策"));

        let mut decision = RoutingDecision::explicit_wgpu();
        decision.rejected.push((BackendKind::Vector, "  ".to_string()));
        assert!(decision.validate().unwrap_err().contains("静默决策"));

        let mut decision = RoutingDecision::explicit_wgpu();
        decision.rejected.push((BackendKind::Wgpu, "不该同时出现".to_string()));
        assert!(decision.validate().unwrap_err().contains("自相矛盾"));
    }

    /// 落盘行是合法 JSON 形状：转义引号后仍能定位到拒绝理由。
    #[test]
    fn decision_json_escapes_and_carries_the_rejection() {
        let content = FrameContent {
            has_content_texture: true,
            ..plain_2d()
        };
        let decision = route(LoopBackend::Vector, content, capable()).unwrap();
        let line = decision.json("surface_matrix", "script");
        assert!(line.starts_with("[decision] {"));
        assert!(line.contains("\"backend\":\"Wgpu\""));
        assert!(line.contains("\"backend\":\"Vector\",\"reason\":\""));
        assert!(line.contains("内容纹理"));
        assert!(!line.contains('\n'));
    }
}
