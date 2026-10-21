// pipeline.rs — 管线状态声明（B2-0b）+ 能力查询（B2-0c）。
//
// 为什么在 IR：管线状态是**意图层声明**（拓扑 / 着色变体 / 是否双面 / 混合语义 /
// 顶点布局），不是 GPU 命令。后端负责把它翻译成 `wgpu::RenderPipelineDescriptor`
// （铁律 5：作者与 IR 都不暴露 `cull_mode` / `blend` / `sample_count` / `format`）。
//
// 🔴 本文件**不得出现任何 wgpu 类型**（计划 #1 的直接修正、D19 的核心）。

use crate::layer::{LayerBlend, LayerId};
use crate::mesh::{PrimitiveTopology, ShaderVariant};

/// 背面剔除模式（后端中立的 `cull_mode` 表示）。
///
/// IR 侧通常只声明意图（[`PipelineDesc::double_sided`]），本枚举供后端翻译与
/// 显式状态查询使用；`Front` 保留给镜像/负缩放场景的显式需求。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CullMode {
    /// 不剔除（双面可见）。
    None,
    /// 剔除正面。
    Front,
    /// 剔除背面（默认的封闭实体语义）。
    Back,
}

/// 一条管线的状态声明（**纯数据**）。
///
/// - `double_sided`：意图层声明（D19）。`true` ⇒ 后端 `cull_mode = None`；
///   `false` ⇒ 后端 `cull_mode = Back`（三角形）/ `None`（线段、点）。
/// - `blend`：混合语义沿用 IR 既有 [`LayerBlend`]（#14 先例），后端翻译成
///   `BlendState`（`Opaque → REPLACE` / `Alpha → 预乘 over` / `Additive → One,One`）。
/// - `vertex_layout`：顶点布局契约（D23，ze-vertex）。同布局 + 同其余维度才共享
///   pipeline；队列合并后两套 Layout 共存，故布局是 key 的必要维度（D15）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PipelineDesc {
    pub topology: PrimitiveTopology,
    pub variant: ShaderVariant,
    /// 意图层：是否双面可见（`true` ⇒ 不剔除背面）。
    pub double_sided: bool,
    pub blend: LayerBlend,
    pub vertex_layout: ze_vertex::VertexLayout,
}

/// 渲染通道的深度处理意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PassDepth {
    /// 不挂深度附件（2D 离屏 / 合成 pass）。
    None,
    /// 深度测试（Less）+ 写入（3D 两个 pass）。
    TestAndWrite,
}

/// 一个渲染通道的声明（**纯数据**）。
///
/// `sample_count` / `color_format` / `depth_format` **不在本结构里** —— 它们由
/// target 决定，属[能力](Capabilities)而非命令（D22）。
///
/// 注：`Color32` 未实现 `Hash`，故 `PassDesc` 不派发 `Hash`（通道声明不进
/// pipeline 缓存 key，无需可哈希）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassDesc {
    /// 目标图层（离屏目标按此 id 持有）。
    pub layer: LayerId,
    /// 清屏颜色。
    pub clear_color: ze_notion::Color32,
    pub depth: PassDepth,
}

// ─── B2-0c：能力查询（D22） ────────────────────────────────────────────────

/// 颜色目标格式（后端中立的有限集合；由 target 决定）。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ColorFormat {
    /// sRGB 8bit（默认窗口/离屏目标）。
    Rgba8UnormSrgb,
    /// 线性 8bit（回读/测试目标）。
    Rgba8Unorm,
    /// sRGB 8bit（原生窗口常见目标格式，B3-0.5b 补齐）。
    Bgra8UnormSrgb,
    /// 线性 8bit（部分平台窗口目标）。
    Bgra8Unorm,
}

/// 深度格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DepthFormat {
    Depth32Float,
}

/// 后端声明的**能力**（不是命令）：target 相关的采样数 / 格式。
///
/// 消费者（如 B2-5 的 AA 统一）先查询，再决定请求值；**不支持 ⇒ 显式报错**
/// （铁律 2），不得静默降级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Capabilities {
    /// 目标颜色格式支持的最大 MSAA 采样数（1 = 无 MSAA）。
    pub max_sample_count: u32,
    pub color_format: ColorFormat,
    pub depth_format: DepthFormat,
}

impl Capabilities {
    /// 请求 `requested` 采样数：≤ 最大支持 ⇒ 原样返回；否则**显式报错**并命名。
    pub fn require_sample_count(&self, requested: u32) -> Result<u32, CapabilityError> {
        if requested == 0 || !requested.is_power_of_two() {
            return Err(CapabilityError::InvalidSampleCount { requested });
        }
        if requested > self.max_sample_count {
            return Err(CapabilityError::UnsupportedSampleCount {
                requested,
                max_supported: self.max_sample_count,
            });
        }
        Ok(requested)
    }
}

/// 能力不满足时的显式错误（铁律 2：不支持就命名报错，不静默降级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityError {
    #[error("采样数 {requested} 非法（必须为 2 的幂且 ≥ 1）")]
    InvalidSampleCount { requested: u32 },
    #[error("后端目标最多支持 MSAA {max_supported}x，请求了 {requested}x（不静默降级）")]
    UnsupportedSampleCount {
        requested: u32,
        max_supported: u32,
    },
    #[error("目标颜色格式不受 IR 能力声明支持（仅 Rgba8UnormSrgb / Rgba8Unorm；不静默）")]
    UnsupportedColorFormat,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_vertex::{AaModel, CacheStrategy, VertexLayout};

    fn desc() -> PipelineDesc {
        PipelineDesc {
            topology: PrimitiveTopology::Triangles,
            variant: ShaderVariant::Pbr,
            double_sided: false,
            blend: LayerBlend::Opaque,
            vertex_layout: VertexLayout::standard_3d(),
        }
    }

    /// 同一组状态必须相等且同哈希（pipeline 缓存 key 的前提）。
    #[test]
    fn test_pipeline_desc_hash_eq() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let h = |d: &PipelineDesc| {
            let mut s = DefaultHasher::new();
            d.hash(&mut s);
            s.finish()
        };
        assert_eq!(desc(), desc());
        assert_eq!(h(&desc()), h(&desc()));

        // 任一维度变化 ⇒ 不相等、哈希不同
        let mut m = desc();
        m.double_sided = true;
        assert_ne!(m, desc());
        assert_ne!(h(&m), h(&desc()));

        let mut m = desc();
        m.blend = LayerBlend::Alpha;
        assert_ne!(h(&m), h(&desc()));

        let mut m = desc();
        m.variant = ShaderVariant::Lambert;
        assert_ne!(h(&m), h(&desc()));

        let mut m = desc();
        m.topology = PrimitiveTopology::Lines;
        assert_ne!(h(&m), h(&desc()));

        // D15：布局是 key 的必要维度（2D / 3D 布局共存）
        let mut m = desc();
        m.vertex_layout = VertexLayout::standard(AaModel::None, CacheStrategy::Static);
        assert_ne!(h(&m), h(&desc()));
    }

    #[test]
    fn test_pass_desc() {
        let p = PassDesc {
            layer: LayerId(3),
            clear_color: ze_notion::Color32::TRANSPARENT,
            depth: PassDepth::TestAndWrite,
        };
        assert_eq!(p.layer, LayerId(3));
        assert_eq!(p.depth, PassDepth::TestAndWrite);
        assert_ne!(PassDepth::None, PassDepth::TestAndWrite);
        assert_eq!(p, p);
    }

    /// D22：能力查询 —— 满足则返回请求值；超限 / 非法 ⇒ 显式错误（不静默降级）。
    #[test]
    fn test_capabilities_require_sample_count() {
        let caps = Capabilities {
            max_sample_count: 4,
            color_format: ColorFormat::Rgba8UnormSrgb,
            depth_format: DepthFormat::Depth32Float,
        };
        assert_eq!(caps.require_sample_count(1).unwrap(), 1);
        assert_eq!(caps.require_sample_count(4).unwrap(), 4);
        assert_eq!(
            caps.require_sample_count(8),
            Err(CapabilityError::UnsupportedSampleCount {
                requested: 8,
                max_supported: 4
            })
        );
        assert_eq!(
            caps.require_sample_count(3),
            Err(CapabilityError::InvalidSampleCount { requested: 3 })
        );
        assert!(caps.require_sample_count(0).is_err());
    }
}
