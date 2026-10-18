// layer.rs — 图层合成概念（S2：从 `xcal_scene::domain` 下沉）。
//
// 为什么在渲染 IR 层：`LayerId` / `LayerBlend` 是**合成语义**，不是场景状态；
// `xcal-render-backends` 生产代码需要它们，但不该为此依赖 `xcal-scene`。
// 本层不依赖相机 / 场景对象，故可安全下沉。

/// 图层唯一标识。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct LayerId(pub u32);

/// 图层与下层的混合方式（S3-2 合成器）。
///
/// 只声明语义，具体 wgpu 混合状态属于后端：
/// - [`Opaque`](Self::Opaque) → `BlendState::REPLACE`；
/// - [`Alpha`](Self::Alpha) → 预乘 alpha；
/// - [`Additive`](Self::Additive) → `src: One, dst: One`。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum LayerBlend {
    /// 不透明覆盖（默认）。
    #[default]
    Opaque,
    /// 预乘 alpha 混合。
    Alpha,
    /// 加法混合（发光/高亮）。
    Additive,
}
