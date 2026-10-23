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
    /// 一行可读报告（B11 §6.4：选了谁、为什么、谁被拒、拒绝理由）。
    pub fn report(&self) -> String {
        let mut out = format!("选择 {:?}（{}）", self.backend, self.reason);
        for (kind, why) in &self.rejected {
            out.push_str(&format!("；{kind:?} 被拒：{why}"));
        }
        out
    }
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
    }
}
