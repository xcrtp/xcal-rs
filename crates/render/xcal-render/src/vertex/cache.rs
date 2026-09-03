// cache.rs
//
// 缓存策略声明（V3，§4.3）。
//
// 决策锁定（§3）：
//   - 策略挂在**布局描述符**（VertexLayout）上，不挂在单个 buffer 实例上 ——
//     格式相同的数据，更新语义必须一致，否则内容寻址的 key 会成为薛定谔状态。
//   - `ViewDependent` 本阶段行为**完全等同 `PerFrame`**：枚举位保留，
//     但实现上不缓存（不进 LRU、不做内容寻址、每帧重新生成）。

use serde::{Deserialize, Serialize};

/// 顶点数据的更新频率语义（决定缓存层走哪条路径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheStrategy {
    /// 与视图无关的纯几何 → 内容寻址缓存，上传一次。
    Static,
    /// 烘焙了视图相关信息（如屏幕空间描边宽度）。
    ///
    /// **本阶段退化为 PerFrame**：不进 LRU、不做内容寻址、每帧重新生成。
    /// 枚举位保留 —— 将来做屏幕空间恒定描边时无需改接口。
    ///
    /// 理由（§4.3）：屏幕空间恒定描边要求三角化时知道当前缩放，
    /// 与"三角化结果可缓存"直接冲突。没有实测数据前，先走简单正确的路径。
    ViewDependent,
    /// 每帧变 → 走 arena 或流式上传，不进 LRU。
    PerFrame,
    /// 成对缓存（from/to），运行时只传一个 lerp 因子；两份网格各自内容寻址。
    MorphTarget,
}

impl CacheStrategy {
    /// 归一化口径：`ViewDependent` 本阶段行为等同 `PerFrame`。
    ///
    /// 缓存层应先调用本函数再分流，保证两种策略**在当前实现下走同一条路径**；
    /// 将来 ViewDependent 真正独立实现时，只需改这一处。
    pub const fn effective(self) -> Self {
        match self {
            Self::ViewDependent => Self::PerFrame,
            other => other,
        }
    }

    /// 是否进内容寻址 LRU（上传一次、跨帧复用）。
    pub const fn is_content_addressed(self) -> bool {
        matches!(self, Self::Static | Self::MorphTarget)
    }

    /// 是否每帧重新生成（本阶段 `ViewDependent` 退化于此）。
    pub const fn is_per_frame(self) -> bool {
        matches!(self, Self::ViewDependent | Self::PerFrame)
    }

    /// 是否绕过 LRU（不污染淘汰策略：每帧插入、每帧淘汰）。
    pub const fn bypasses_lru(self) -> bool {
        matches!(self, Self::ViewDependent | Self::PerFrame)
    }
}

// ─── 单测（V3） ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// ViewDependent 与 PerFrame 在当前实现下行为一致
    /// （防止有人偷偷改了实现却没更新口径）。
    #[test]
    fn view_dependent_behaves_exactly_like_per_frame() {
        let vd = CacheStrategy::ViewDependent;
        let pf = CacheStrategy::PerFrame;

        assert_eq!(vd.effective(), pf);
        assert_eq!(vd.is_per_frame(), pf.is_per_frame());
        assert_eq!(vd.bypasses_lru(), pf.bypasses_lru());
        assert_eq!(vd.is_content_addressed(), pf.is_content_addressed());

        assert!(vd.is_per_frame());
        assert!(vd.bypasses_lru());
        assert!(!vd.is_content_addressed());
    }

    /// Static 走内容寻址、不绕过 LRU、不每帧重建。
    #[test]
    fn static_is_content_addressed() {
        let s = CacheStrategy::Static;
        assert!(s.is_content_addressed());
        assert!(!s.is_per_frame());
        assert!(!s.bypasses_lru());
        assert_eq!(s.effective(), s);
    }

    /// MorphTarget 成对缓存：两份网格内容寻址、可跨帧复用。
    #[test]
    fn morph_target_is_content_addressed() {
        let m = CacheStrategy::MorphTarget;
        assert!(m.is_content_addressed());
        assert!(!m.is_per_frame());
        assert!(!m.bypasses_lru());
        assert_eq!(m.effective(), m);
    }

    /// 四档策略的行为矩阵必须两两自洽：effective 归一后只剩三档行为
    /// （Static / PerFrame / MorphTarget）。
    #[test]
    fn effective_normalizes_to_three_behaviors() {
        let per_frame = [CacheStrategy::PerFrame, CacheStrategy::ViewDependent];
        for a in per_frame {
            for b in per_frame {
                assert_eq!(a.effective(), b.effective());
                assert_eq!(a.is_per_frame(), b.is_per_frame());
            }
        }
        let behaviors = [
            CacheStrategy::Static.effective(),
            CacheStrategy::PerFrame.effective(),
            CacheStrategy::MorphTarget.effective(),
        ];
        assert_eq!(behaviors[0], CacheStrategy::Static);
        assert_eq!(behaviors[1], CacheStrategy::PerFrame);
        assert_eq!(behaviors[2], CacheStrategy::MorphTarget);
    }
}
