// aa.rs
//
// AA 模型与通用槽语义映射（V2，§4.2）。
//
// §4.1：通用槽 attrib0(F32x2) / attrib1(F32) 的含义由 AaModel 决定。
// 本模块给出 `AaModel -> (attrib0 语义, attrib1 语义)` 的**纯数据映射表**，
// 供 shader 作者与三角化器共享同一份声明（可 headless 测试）。

use serde::{Deserialize, Serialize};

/// AA（抗锯齿）模型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AaModel {
    /// 硬边，纯靠 MSAA。顶点槽无定义语义。
    None,
    /// 顶点直接带 coverage，shader 插值即可。
    Coverage,
    /// 顶点带边法线 + side；shader 由两者解析出到边距离，用 smoothstep 算 coverage
    /// （lyon 方案，见 §0）。
    Analytic,
    /// 距离场：顶点带图集 UV，场值从纹理采样（msdf / atlas 常规路径）。
    Sdf,
}

/// 通用槽的语义标签（§4.1 词汇表的枚举化）。
///
/// 标注"预留"的变体当前没有 AaModel 使用，保留它们是为了把 §4.1 的槽语义
/// 词汇表完整编码进类型系统；新增 AaModel 时优先复用，而非扩表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttribSemantic {
    /// 无定义语义。shader 不得读取该槽。
    Undefined,
    // ── attrib0 (F32x2) 候选语义 ──
    /// 单位边法线（解析 AA，lyon 方案）。
    Normal,
    /// 纹理 / 图集坐标（SDF 采样）。
    Uv,
    /// SDF 场梯度方向（预留：顶点插值 SDF 的备选方案）。
    SdfGradient,
    // ── attrib1 (F32) 候选语义 ──
    /// coverage [0,1]。
    Coverage,
    /// 边侧符号：+1 边内 / -1 边外（解析 AA，lyon 方案）。
    Side,
    /// 沿边方向的推进量（预留：屏幕空间描边）。
    Advance,
    /// SDF 场值（预留：顶点插值 SDF 的备选方案）。
    SdfDistance,
}

/// 一个 [`AaModel`] 对两个通用槽的完整解释声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AaSlotSemantics {
    pub attrib0: AttribSemantic,
    pub attrib1: AttribSemantic,
    /// shader 解释说明（契约文档的一部分，可测试、可展示）。
    pub shader_note: &'static str,
}

impl AaModel {
    /// 全部模型（用于穷举测试：映射表必须全覆盖、无未定义分支）。
    pub const ALL: [AaModel; 4] = [Self::None, Self::Coverage, Self::Analytic, Self::Sdf];

    /// 语义映射表（§4.2）：每个模型必须给出两个槽的明确解释。
    pub const fn slots(self) -> AaSlotSemantics {
        match self {
            Self::None => AaSlotSemantics {
                attrib0: AttribSemantic::Undefined,
                attrib1: AttribSemantic::Undefined,
                shader_note: "硬边，纯靠 MSAA；shader 不得读取 attrib0/attrib1。",
            },
            Self::Coverage => AaSlotSemantics {
                attrib0: AttribSemantic::Undefined,
                attrib1: AttribSemantic::Coverage,
                shader_note: "attrib1 携带 [0,1] coverage，插值即得；attrib0 无 AA 语义。",
            },
            Self::Analytic => AaSlotSemantics {
                attrib0: AttribSemantic::Normal,
                attrib1: AttribSemantic::Side,
                shader_note: "attrib0 = 单位边法线，attrib1 = side(±1)；shader 由两者解析到边距离后 smoothstep（lyon 方案）。",
            },
            Self::Sdf => AaSlotSemantics {
                attrib0: AttribSemantic::Uv,
                attrib1: AttribSemantic::Undefined,
                shader_note: "attrib0 = 图集 UV，场值从纹理采样；attrib1 无 AA 语义。",
            },
        }
    }
}

// ─── 单测（V2） ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 映射表全覆盖：每个模型都有明确语义声明，无未定义分支（不 panic、逐项锁定）。
    #[test]
    fn every_model_has_explicit_slot_semantics() {
        for model in AaModel::ALL {
            let s = model.slots(); // match 必须穷尽 —— 编译期即保证无未定义分支
            match model {
                AaModel::None => {
                    assert_eq!(s.attrib0, AttribSemantic::Undefined);
                    assert_eq!(s.attrib1, AttribSemantic::Undefined);
                }
                AaModel::Coverage => {
                    assert_eq!(s.attrib0, AttribSemantic::Undefined);
                    assert_eq!(s.attrib1, AttribSemantic::Coverage);
                }
                AaModel::Analytic => {
                    assert_eq!(s.attrib0, AttribSemantic::Normal);
                    assert_eq!(s.attrib1, AttribSemantic::Side);
                }
                AaModel::Sdf => {
                    assert_eq!(s.attrib0, AttribSemantic::Uv);
                    assert_eq!(s.attrib1, AttribSemantic::Undefined);
                }
            }
            assert!(!s.shader_note.is_empty(), "{model:?} 必须带 shader 解释说明");
        }
    }

    /// 硬边模型必须明确声明两个槽都不可读；其余模型至少有一个槽有定义语义。
    #[test]
    fn aa_models_define_at_least_one_slot() {
        assert_eq!(
            AaModel::None.slots(),
            AaSlotSemantics {
                attrib0: AttribSemantic::Undefined,
                attrib1: AttribSemantic::Undefined,
                shader_note: AaModel::None.slots().shader_note,
            }
        );
        for model in [AaModel::Coverage, AaModel::Analytic, AaModel::Sdf] {
            let s = model.slots();
            assert!(
                s.attrib0 != AttribSemantic::Undefined
                    || s.attrib1 != AttribSemantic::Undefined,
                "{model:?} 作为 AA 模型至少要定义一个通用槽"
            );
        }
    }
}
