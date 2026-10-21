// target.rs — 渲染目标的**纯描述**（B3-0.5b / §4.6）。
//
// 描述进 IR（本文件），GPU 句柄（纹理 / 视图）留在后端。
// 与 B3-0 拆顶点生产是同一个动作：**跨边界只传数据，不传 GPU 句柄**。
//
// `sample_count` 是**能力查询项**（D22）：调用方先用 `Capabilities` 校验再填。

use serde::{Deserialize, Serialize};

use crate::pipeline::ColorFormat;

/// 渲染目标描述（纯数据，无 GPU 句柄）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RenderTargetDesc {
    pub width: u32,
    pub height: u32,
    pub format: ColorFormat,
    /// MSAA 采样数（1 = 单采样）。合法性由 [`Self::validate`] 与能力查询共同约束。
    pub sample_count: u32,
}

/// 目标描述非法（显式错误，不静默纠正）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetDescError {
    #[error("渲染目标尺寸非法：{width}×{height}（宽高必须 ≥ 1）")]
    EmptySize { width: u32, height: u32 },
    #[error("采样数 {sample_count} 非法（必须为 2 的幂且 ≥ 1）")]
    InvalidSampleCount { sample_count: u32 },
}

impl RenderTargetDesc {
    /// 单采样离屏目标（常用默认）。
    pub const fn new(width: u32, height: u32, format: ColorFormat) -> Self {
        Self {
            width,
            height,
            format,
            sample_count: 1,
        }
    }

    /// 指定采样数（消费者应先用 `Capabilities::require_sample_count` 校验）。
    pub const fn with_sample_count(mut self, sample_count: u32) -> Self {
        self.sample_count = sample_count;
        self
    }

    pub fn validate(&self) -> Result<(), TargetDescError> {
        if self.width == 0 || self.height == 0 {
            return Err(TargetDescError::EmptySize {
                width: self.width,
                height: self.height,
            });
        }
        if self.sample_count == 0 || !self.sample_count.is_power_of_two() {
            return Err(TargetDescError::InvalidSampleCount {
                sample_count: self.sample_count,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **已知向量**（B3-0.5b）：默认字段值 + 采样数链式设置。
    #[test]
    fn known_vector() {
        let desc = RenderTargetDesc::new(640, 480, ColorFormat::Rgba8Unorm);
        assert_eq!(desc.width, 640);
        assert_eq!(desc.height, 480);
        assert_eq!(desc.format, ColorFormat::Rgba8Unorm);
        assert_eq!(desc.sample_count, 1);
        assert_eq!(desc.with_sample_count(4).sample_count, 4);
        desc.validate().unwrap();
    }

    /// 可 serde 往返 + 可 Hash（计划 🟡 判据）。
    #[test]
    fn serde_roundtrip_and_hash() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let desc = RenderTargetDesc::new(320, 240, ColorFormat::Bgra8UnormSrgb).with_sample_count(4);
        let json = serde_json::to_string(&desc).unwrap();
        let back: RenderTargetDesc = serde_json::from_str(&json).unwrap();
        assert_eq!(back, desc);

        let h = |d: &RenderTargetDesc| {
            let mut s = DefaultHasher::new();
            d.hash(&mut s);
            s.finish()
        };
        assert_eq!(h(&desc), h(&back));
        assert_ne!(
            h(&desc),
            h(&RenderTargetDesc::new(320, 240, ColorFormat::Bgra8UnormSrgb))
        );
    }

    /// 非法尺寸 / 采样数 ⇒ 显式错误。
    #[test]
    fn invalid_desc_is_explicit_error() {
        let zero = RenderTargetDesc::new(0, 10, ColorFormat::Rgba8Unorm);
        assert!(matches!(zero.validate(), Err(TargetDescError::EmptySize { .. })));
        let bad_samples = RenderTargetDesc::new(10, 10, ColorFormat::Rgba8Unorm).with_sample_count(3);
        assert!(matches!(
            bad_samples.validate(),
            Err(TargetDescError::InvalidSampleCount { sample_count: 3 })
        ));
    }
}
