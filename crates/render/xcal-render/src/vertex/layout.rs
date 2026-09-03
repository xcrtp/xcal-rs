// layout.rs
//
// 顶点布局描述符（V1 + V4 + 实例化扩展）。
//
// 契约（§4.1）：固定属性 position + 两个通用槽 attrib0/attrib1，不再增加固定属性。
// 通用槽的含义由 [`AaModel`](super::aa::AaModel) 决定（见 super::aa）。
//
// 留两个通用槽而不是按需扩展的意义：从 MSAA 切到解析 AA 时，
// 顶点格式不动，只换 shader 变体和三角化器实现。
//
// 实例化扩展：wgpu 的 VertexBufferLayout 描述**单个**顶点缓冲，而实例化需要
// **多个**缓冲（buffer0 = 顶点数据 step=Vertex + buffer1 = 实例数据 step=Instance）。
// 因此契约从"一个 layout"演进为"一组 buffer 描述"（[`VertexInputDesc`]），
// 同时保持单 buffer 用法不变（[`VertexLayout::step_mode`] 默认 Vertex）。
//
// 校验分层（实例数据语义是宿主自定义的，不归 AA 契约管）：
//   - step_mode = Vertex 的 buffer → 严格契约词汇表（AA 契约管通用槽）
//   - step_mode = Instance 的 buffer → 宽松校验（宿主自定义语义，
//     只校验 offset 不重叠、stride 合法、命名非空）

use serde::{Deserialize, Deserializer, Serialize};

use super::aa::AaModel;
use super::cache::CacheStrategy;

// ─── 步进模式（实例化支持） ───────────────────────────────────────────────

/// 顶点数据步进模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StepMode {
    /// 每个顶点步进一次（默认；现有单 buffer 用法）。
    Vertex,
    /// 每个实例步进一次（实例化渲染的 per-instance 数据）。
    Instance,
}

impl Default for StepMode {
    fn default() -> Self {
        Self::Vertex
    }
}

// ─── 属性类型（后端中立） ─────────────────────────────────────────────────

/// 属性元素类型。不含任何 wgpu 类型 —— 翻译在 xcal-render-backends::vertex_layout。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttribType {
    F32,
    F32x2,
    F32x3,
    F32x4,
}

impl AttribType {
    /// 字节大小。
    pub const fn size(self) -> u32 {
        match self {
            Self::F32 => 4,
            Self::F32x2 => 8,
            Self::F32x3 => 12,
            Self::F32x4 => 16,
        }
    }

    /// 对齐规则：对齐到属性自身大小。
    ///
    /// 保守且后端安全 —— offset 是自身大小的整数倍时，必然满足各后端
    /// （含 wgpu 校验）更宽松的 4 字节对齐要求。
    pub const fn alignment(self) -> u32 {
        self.size()
    }
}

// ─── 单个属性 ─────────────────────────────────────────────────────────────

/// 单个顶点属性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct VertexAttrib {
    /// 语义标签。契约词汇表固定为 `"position"` / `"attrib0"` / `"attrib1"`（§4.1）。
    pub semantic: &'static str,
    pub ty: AttribType,
    /// 字节偏移。由 [`VertexLayout::compute_layout`] 填写；手工构造时置 0 即可。
    pub offset: u32,
}

impl VertexAttrib {
    /// 契约词汇表（§4.1：固定属性 + 两个通用槽，不再增加）。
    pub const VOCABULARY: [&'static str; 3] = ["position", "attrib0", "attrib1"];
}

// 反序列化宽松：接受任意非空语义名。
//
// 为什么不再在序列化边界锁词汇表：实例化 buffer 的语义是**宿主自定义的**
// （instance_offset、instance_color…），不归 AA 契约管；边界锁死会让实例数据
// 根本没法命名。分层校验移到 [`VertexLayout::validate`]：
// Vertex 步进 buffer 严格词汇表 / Instance 步进 buffer 宽松。
//
// 代价：`&'static str` 字段无法持有运行期 String，语义名在加载期经 `Box::leak`
// 进入 'static 存储。泄漏**有界** —— 语义名来自宿主的有限标识符集合，且只发生在
// 布局加载期（构造 pipeline 变体缓存时），不在每帧热路径上。
impl<'de> Deserialize<'de> for VertexAttrib {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr {
            semantic: String,
            ty: AttribType,
            offset: u32,
        }
        let r = Repr::deserialize(deserializer)?;
        if r.semantic.is_empty() {
            return Err(serde::de::Error::custom("顶点属性语义不能为空字符串"));
        }
        Ok(Self {
            semantic: Box::leak(r.semantic.into_boxed_str()),
            ty: r.ty,
            offset: r.offset,
        })
    }
}

// ─── 布局描述符 ───────────────────────────────────────────────────────────

/// 顶点格式声明：三角化器填写，管线与缓存消费。
///
/// 必须可哈希、可比较相等、可序列化 —— pipeline 变体缓存的 key 要包含它（§0/§4.4）。
///
/// # 哈希漂移警示
///
/// `step_mode` 字段加入后**所有布局的哈希值都变了** —— 已缓存 / persist 的
/// 布局哈希全部失效，pipeline 变体缓存 key 需重建。这是可接受的代价
/// （趁契约刚落地、外部依赖少时改，正是最佳时机）；本注释即为存证。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VertexLayout {
    /// 属性列表。**顺序即 shader location 顺序**（单 buffer 时 location = 下标；
    /// 多 buffer 时全局连续编号，见 [`VertexInputDesc`]），必须稳定。
    pub attribs: Vec<VertexAttrib>,
    /// 步长（字节）。由 [`Self::compute_layout`] 计算后填入。
    pub stride: u32,
    pub aa_model: AaModel,
    pub cache: CacheStrategy,
    /// 步进模式：Vertex = 顶点数据（默认），Instance = 实例数据。
    ///
    /// `#[serde(default)]` —— 旧序列化数据（无此字段）加载后 = Vertex，向后兼容。
    #[serde(default)]
    pub step_mode: StepMode,
}

/// 布局自洽性校验错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VertexLayoutError {
    #[error("顶点布局至少需要一个属性")]
    Empty,
    #[error("属性语义为空字符串（下标 {index}）")]
    EmptySemantic { index: usize },
    #[error(
        "属性 {semantic}（下标 {index}）offset {offset} 未对齐到自身大小 {align}"
    )]
    Misaligned {
        semantic: &'static str,
        index: usize,
        offset: u32,
        align: u32,
    },
    #[error("属性 {semantic}（下标 {index}）offset {offset} 与前一属性重叠")]
    Overlap {
        semantic: &'static str,
        index: usize,
        offset: u32,
    },
    #[error("stride {stride} 未覆盖属性 {semantic} 的结尾 {needed}")]
    StrideTooSmall {
        stride: u32,
        semantic: &'static str,
        needed: u32,
    },
    // ── 实例化扩展 ──
    #[error("Vertex 步进 buffer 的属性（下标 {index}）语义 {semantic:?} 不在契约词汇表 [\"position\", \"attrib0\", \"attrib1\"] 内（宿主自定义语义请放 Instance 步进 buffer）")]
    IllegalSemantic { index: usize, semantic: String },
    #[error("顶点输入描述至少需要一个 buffer")]
    EmptyInput,
    #[error("顶点输入至少要有一个 Vertex 步进 buffer（Instance 不能孤立存在而无顶点数据）")]
    InstanceOnly,
}

impl VertexLayout {
    /// 标准布局（§4.4）：position(F32x2) + attrib0(F32x2) + attrib1(F32) → stride = 20。
    pub fn standard(aa: AaModel, cache: CacheStrategy) -> Self {
        let mut attribs = vec![
            VertexAttrib {
                semantic: "position",
                ty: AttribType::F32x2,
                offset: 0,
            },
            VertexAttrib {
                semantic: "attrib0",
                ty: AttribType::F32x2,
                offset: 0,
            },
            VertexAttrib {
                semantic: "attrib1",
                ty: AttribType::F32,
                offset: 0,
            },
        ];
        let stride = Self::compute_layout(&mut attribs);
        Self {
            attribs,
            stride,
            aa_model: aa,
            cache,
            step_mode: StepMode::Vertex,
        }
    }

    /// 计算 stride 与每个属性的 offset（考虑对齐），返回 stride。
    ///
    /// 规则：offset 从 0 起，按 `attribs` 顺序累加；每个属性对齐到自身大小
    /// （[`AttribType::alignment`]）。该函数是纯计算，不依赖任何全局状态，
    /// 相同输入必得相同输出（属性顺序稳定）。
    pub fn compute_layout(attribs: &mut [VertexAttrib]) -> u32 {
        let mut cursor = 0u32;
        for attrib in attribs.iter_mut() {
            let align = attrib.ty.alignment();
            cursor = cursor.div_ceil(align) * align;
            attrib.offset = cursor;
            cursor += attrib.ty.size();
        }
        cursor
    }

    /// 校验布局自洽（校验分层，见模块文档头）：
    ///
    /// - `step_mode = Vertex`（顶点数据 buffer）→ **严格契约词汇表**
    ///   （AA 契约管通用槽，语义必须是 position/attrib0/attrib1）；
    /// - `step_mode = Instance`（实例数据 buffer）→ **宽松**：语义名宿主自定义，
    ///   只要求非空；
    /// - 两者共同：offset 对齐 / 无重叠（属性须按 offset 升序）/ stride 覆盖全部属性。
    pub fn validate(&self) -> Result<(), VertexLayoutError> {
        if self.attribs.is_empty() {
            return Err(VertexLayoutError::Empty);
        }
        if matches!(self.step_mode, StepMode::Vertex) {
            for (index, a) in self.attribs.iter().enumerate() {
                if !VertexAttrib::VOCABULARY.contains(&a.semantic) {
                    return Err(VertexLayoutError::IllegalSemantic {
                        index,
                        semantic: a.semantic.to_owned(),
                    });
                }
            }
        }
        let mut end = 0u32;
        for (index, a) in self.attribs.iter().enumerate() {
            if a.semantic.is_empty() {
                return Err(VertexLayoutError::EmptySemantic { index });
            }
            let align = a.ty.alignment();
            if a.offset % align != 0 {
                return Err(VertexLayoutError::Misaligned {
                    semantic: a.semantic,
                    index,
                    offset: a.offset,
                    align,
                });
            }
            if a.offset < end {
                return Err(VertexLayoutError::Overlap {
                    semantic: a.semantic,
                    index,
                    offset: a.offset,
                });
            }
            end = a.offset + a.ty.size();
        }
        if self.stride < end {
            let semantic = self.attribs[self.attribs.len() - 1].semantic;
            return Err(VertexLayoutError::StrideTooSmall {
                stride: self.stride,
                semantic,
                needed: end,
            });
        }
        Ok(())
    }
}

// ─── 完整顶点输入（实例化支持） ───────────────────────────────────────────

/// 完整顶点输入描述：**一组 buffer**。
///
/// wgpu 的 `VertexBufferLayout` 描述单个顶点缓冲，而实例化需要多个：
/// `buffers[0]` = 顶点数据（step_mode = Vertex）+ `buffers[1]` = 实例数据
/// （step_mode = Instance）。**buffer 顺序即 wgpu 的 vertex buffer slot（0..n）**。
///
/// shader location 口径（wgpu 语义）：**跨 buffer 全局连续编号**——
/// buffer0 的属性占 0..n，buffer1 的属性从 n 续编，以此类推。翻译层
/// （xcal-render-backends::vertex_layout::to_wgpu_buffers）按此实现，测试锁死。
///
/// 同样必须可哈希 / 可比较相等 / 可序列化（pipeline 变体缓存 key 的完整形态）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VertexInputDesc {
    pub buffers: Vec<VertexLayout>,
}

impl VertexInputDesc {
    /// 单 buffer 升级路径：从旧单 buffer 用法零成本升级。
    ///
    /// 翻译结果与直接 [`to_wgpu`](crate) 单 buffer 版完全一致（测试锁定）。
    pub fn single(layout: VertexLayout) -> Self {
        Self {
            buffers: vec![layout],
        }
    }

    /// 校验 buffer 间 step_mode 组合合法，并逐 buffer 校验：
    ///
    /// - 至少一个 buffer；
    /// - 至少一个 Vertex 步进 buffer —— **Instance 不能孤立存在**而无顶点数据；
    /// - 每个 buffer 自身合法（Vertex 严格词汇表 / Instance 宽松，见
    ///   [`VertexLayout::validate`]）。
    pub fn validate(&self) -> Result<(), VertexLayoutError> {
        if self.buffers.is_empty() {
            return Err(VertexLayoutError::EmptyInput);
        }
        if self
            .buffers
            .iter()
            .all(|b| matches!(b.step_mode, StepMode::Instance))
        {
            return Err(VertexLayoutError::InstanceOnly);
        }
        for b in &self.buffers {
            b.validate()?;
        }
        Ok(())
    }
}

// ─── 单测（V1 / V4） ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn standard() -> VertexLayout {
        VertexLayout::standard(AaModel::None, CacheStrategy::Static)
    }

    // ── V1：offset 递增无重叠、stride 正确 ────────────────────────────────

    #[test]
    fn standard_layout_offsets_stride() {
        let l = standard();
        assert_eq!(l.attribs[0].semantic, "position");
        assert_eq!(l.attribs[0].offset, 0);
        assert_eq!(l.attribs[1].semantic, "attrib0");
        assert_eq!(l.attribs[1].offset, 8);
        assert_eq!(l.attribs[2].semantic, "attrib1");
        assert_eq!(l.attribs[2].offset, 16);
        assert_eq!(l.stride, 20);
        l.validate().unwrap();
    }

    #[test]
    fn alignment_pads_f32x3_after_f32() {
        let mut attribs = vec![
            VertexAttrib {
                semantic: "position",
                ty: AttribType::F32,
                offset: 0,
            },
            VertexAttrib {
                semantic: "attrib0",
                ty: AttribType::F32x3,
                offset: 0,
            },
            VertexAttrib {
                semantic: "attrib1",
                ty: AttribType::F32x2,
                offset: 0,
            },
        ];
        let stride = VertexLayout::compute_layout(&mut attribs);
        // F32@0(4) → F32x3 对齐到 12 → F32x2 对齐到 24
        assert_eq!(attribs[1].offset, 12);
        assert_eq!(attribs[2].offset, 24);
        assert_eq!(stride, 32);
    }

    #[test]
    fn compute_layout_is_deterministic() {
        let make = || {
            let mut a = vec![
                VertexAttrib {
                    semantic: "position",
                    ty: AttribType::F32x2,
                    offset: 99,
                },
                VertexAttrib {
                    semantic: "attrib0",
                    ty: AttribType::F32,
                    offset: 99,
                },
                VertexAttrib {
                    semantic: "attrib1",
                    ty: AttribType::F32x4,
                    offset: 99,
                },
            ];
            let stride = VertexLayout::compute_layout(&mut a);
            (a, stride)
        };
        let (a1, s1) = make();
        let (a2, s2) = make();
        assert_eq!(s1, s2);
        assert_eq!(a1, a2, "相同输入必须产出相同 offset（顺序稳定）");
        // F32x2@0(0..8) → F32@8(8..12) → F32x4 对齐到 16(16..32)
        assert_eq!(s1, 32);
    }

    #[test]
    fn validate_rejects_overlap_and_bad_stride() {
        let mut l = standard();
        l.attribs[1].offset = 0; // 与 position 重叠
        assert!(matches!(
            l.validate(),
            Err(VertexLayoutError::Overlap { .. })
        ));

        let mut l = standard();
        l.stride = 19; // 未覆盖 attrib1 结尾（16+4=20）
        assert!(matches!(
            l.validate(),
            Err(VertexLayoutError::StrideTooSmall { .. })
        ));

        let mut l = standard();
        l.attribs[1].offset = 5; // 未对齐到 8
        assert!(matches!(
            l.validate(),
            Err(VertexLayoutError::Misaligned { .. })
        ));

        let l = VertexLayout {
            attribs: vec![],
            stride: 0,
            aa_model: AaModel::None,
            cache: CacheStrategy::Static,
            step_mode: StepMode::Vertex,
        };
        assert!(matches!(l.validate(), Err(VertexLayoutError::Empty)));
    }

    // ── V4：可哈希 / 可序列化 ─────────────────────────────────────────────

    #[test]
    fn serde_roundtrip_is_identical() {
        let l = standard();
        let json = serde_json::to_string(&l).unwrap();
        let back: VertexLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }

    #[test]
    fn unknown_semantic_parses_but_fails_validate() {
        // 反序列化宽松（实例化扩展后宿主语义合法）；严格词汇表校验分层在
        // validate()：无 step_mode 字段 → 默认 Vertex → 非词汇表语义 Err
        let bad = r#"{"attribs":[{"semantic":"tangent","ty":"F32x2","offset":0}],"stride":8,"aa_model":"None","cache":"Static"}"#;
        let parsed: VertexLayout = serde_json::from_str(bad).unwrap();
        assert!(matches!(
            parsed.validate(),
            Err(VertexLayoutError::IllegalSemantic { .. })
        ));
    }

    #[test]
    fn serde_rejects_empty_semantic() {
        let bad = r#"{"attribs":[{"semantic":"","ty":"F32x2","offset":0}],"stride":8,"aa_model":"None","cache":"Static"}"#;
        assert!(serde_json::from_str::<VertexLayout>(bad).is_err());
    }

    fn hash_of(l: &VertexLayout) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        l.hash(&mut h);
        h.finish()
    }

    #[test]
    fn equal_layouts_hash_equal() {
        let l1 = standard();
        let l2 = standard();
        assert_eq!(hash_of(&l1), hash_of(&l2), "相同布局哈希必须相同");
    }

    #[test]
    fn any_field_change_changes_hash() {
        let base = standard();
        let base_hash = hash_of(&base);

        // 改任一属性字段 → 哈希不同
        let mut m = base.clone();
        m.attribs[1].offset = 9;
        assert_ne!(hash_of(&m), base_hash);

        let mut m = base.clone();
        m.attribs[1].ty = AttribType::F32x3;
        assert_ne!(hash_of(&m), base_hash);

        let mut m = base.clone();
        m.attribs[0].semantic = "attrib0";
        assert_ne!(hash_of(&m), base_hash);

        // 改 stride / aa_model / cache → 哈希不同
        let mut m = base.clone();
        m.stride += 4;
        assert_ne!(hash_of(&m), base_hash);

        let mut m = base.clone();
        m.aa_model = AaModel::Coverage;
        assert_ne!(hash_of(&m), base_hash);

        let mut m = base.clone();
        m.cache = CacheStrategy::PerFrame;
        assert_ne!(hash_of(&m), base_hash);

        // 属性顺序不同 → 哈希不同
        let mut m = base.clone();
        m.attribs.swap(1, 2);
        assert_ne!(hash_of(&m), base_hash);
    }

    // ── 实例化扩展 ────────────────────────────────────────────────────────

    /// 向后兼容：旧 JSON（无 step_mode 字段）能加载，解析为 Vertex。
    #[test]
    fn old_json_without_step_mode_loads_as_vertex() {
        let old = r#"{"attribs":[{"semantic":"position","ty":"F32x2","offset":0},{"semantic":"attrib0","ty":"F32x2","offset":8},{"semantic":"attrib1","ty":"F32","offset":16}],"stride":20,"aa_model":"None","cache":"Static"}"#;
        let l: VertexLayout = serde_json::from_str(old).unwrap();
        assert_eq!(l.step_mode, StepMode::Vertex);
        assert_eq!(l, standard(), "旧数据应与 standard() 等价");
        l.validate().unwrap();
    }

    /// 哈希区分：step_mode 不同 → 布局哈希不同（pipeline 变体 key 必须能区分）。
    #[test]
    fn step_mode_change_changes_hash() {
        let mut instance = standard();
        instance.step_mode = StepMode::Instance;
        assert_ne!(hash_of(&instance), hash_of(&standard()));
    }

    fn instance_layout() -> VertexLayout {
        let mut attribs = vec![VertexAttrib {
            semantic: "instance_offset",
            ty: AttribType::F32x4,
            offset: 0,
        }];
        let stride = VertexLayout::compute_layout(&mut attribs);
        VertexLayout {
            attribs,
            stride,
            aa_model: AaModel::None,
            cache: CacheStrategy::PerFrame,
            step_mode: StepMode::Instance,
        }
    }

    /// 校验分层：主（Vertex）buffer 用非法语义名 → Err；
    /// 实例（Instance）buffer 用自定义语义名 → Ok。
    #[test]
    fn validation_layers_by_step_mode() {
        // Vertex buffer + 宿主自定义语义 → Err（严格词汇表）
        let mut l = standard();
        l.attribs[0].semantic = "instance_offset";
        assert!(matches!(
            l.validate(),
            Err(VertexLayoutError::IllegalSemantic { index: 0, .. })
        ));

        // Instance buffer + 宿主自定义语义 → Ok（宽松）
        let l = instance_layout();
        l.validate().unwrap();

        // Instance buffer + 空语义名 → 仍 Err（命名非空是底线）
        let mut l = instance_layout();
        l.attribs[0].semantic = "";
        assert!(matches!(
            l.validate(),
            Err(VertexLayoutError::EmptySemantic { index: 0 })
        ));

        // Instance buffer 的 offset/stride 规则照常生效
        let mut l = instance_layout();
        l.stride = 12; // F32x4 需要 16
        assert!(matches!(
            l.validate(),
            Err(VertexLayoutError::StrideTooSmall { .. })
        ));
    }

    /// 实例布局（宿主自定义语义）可序列化往返。
    #[test]
    fn instance_layout_serde_roundtrip() {
        let l = instance_layout();
        let json = serde_json::to_string(&l).unwrap();
        let back: VertexLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(back, l);
        assert!(json.contains("instance_offset"));
        assert!(json.contains("\"step_mode\":\"Instance\""));
    }

    /// VertexInputDesc 校验：非空 / Instance 不能孤立 / 逐 buffer 分层校验。
    #[test]
    fn vertex_input_desc_validate() {
        // 空 → Err
        let d = VertexInputDesc { buffers: vec![] };
        assert!(matches!(d.validate(), Err(VertexLayoutError::EmptyInput)));

        // 只有 Instance → Err（Instance 不能孤立存在）
        let d = VertexInputDesc {
            buffers: vec![instance_layout(), instance_layout()],
        };
        assert!(matches!(d.validate(), Err(VertexLayoutError::InstanceOnly)));

        // 单 Vertex buffer → Ok（旧用法的等价形态）
        let d = VertexInputDesc::single(standard());
        assert!(d.validate().is_ok());

        // Vertex + Instance 混合 → Ok
        let d = VertexInputDesc {
            buffers: vec![standard(), instance_layout()],
        };
        assert!(d.validate().is_ok());

        // 主 buffer 内有非法语义 → 经逐 buffer 校验冒泡
        let mut bad = standard();
        bad.attribs[1].semantic = "whatever";
        let d = VertexInputDesc {
            buffers: vec![bad, instance_layout()],
        };
        assert!(matches!(
            d.validate(),
            Err(VertexLayoutError::IllegalSemantic { .. })
        ));
    }

    /// VertexInputDesc 可哈希 / 可序列化：相同输入同哈希，改 buffer 组合哈希变。
    #[test]
    fn vertex_input_desc_hash_serde() {
        use std::hash::{Hash, Hasher};
        let d1 = VertexInputDesc {
            buffers: vec![standard(), instance_layout()],
        };
        let d2 = VertexInputDesc {
            buffers: vec![standard(), instance_layout()],
        };
        let h = |d: &VertexInputDesc| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            d.hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(h(&d1), h(&d2));

        // 序列化往返
        let json = serde_json::to_string(&d1).unwrap();
        let back: VertexInputDesc = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d1);

        // 只有单 buffer 的 desc 与 mixed 不同哈希
        let single = VertexInputDesc::single(standard());
        assert_ne!(h(&single), h(&d1));
    }
}
