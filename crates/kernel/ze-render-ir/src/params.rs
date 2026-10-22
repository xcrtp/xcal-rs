// params.rs — 渲染参数契约（B4 Step 1）。
//
// # 为什么在这里
// 分辨率 / 采样 / 过滤 / 色调映射 / 泛光 / AO / 环境图，过去全散落在后端常量与 CLI
// 默认值里（`MSAA_SAMPLES = 4`、`let mut size = (640, 480)`…）。本模块把它们收口成
// **一份纯数据**：宿主构造 → 传给后端；不进 Pod 边界、不进 `ze-scene-api`（D5）。
//
// # 三条纪律（B4 §0）
// - **默认等效**：`RenderParams::default()` 逐字段等价于当前行为；
// - **有效性 ≠ 一致性**：参数非法 ⇒ [`RenderParams::validate`] 显式报错；
// - **不支持 ⇒ 报错**：回放端用 [`RenderParams::is_supported_by`] 在渲染前拦下，
//   不得静默降级（降级会让"同后端 + 同机器一致"失效）。
//
// # 单一真源
// `sample_count` 等 target 字段**只由本结构决定**；[`RenderTargetDesc`] 是它的下游投影
// （见 [`RenderParams::target_desc`]），不得反向成为第二入口。

use serde::{Deserialize, Serialize};

use crate::mesh::TextureHandle;
use crate::pipeline::{Capabilities, ColorFormat, CapabilityError};
use crate::target::RenderTargetDesc;

/// 纹理采样过滤（B4 Step 1 移入 IR：`SurfaceTexture.filter` 与 `RenderParams.filter` 同型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextureFilter {
    Nearest,
    Linear,
}

/// MSAA 档位（**只有契约允许的档位**，非法值无法构造）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Msaa {
    Off,
    /// 当前行为：wgpu 后端既有 `MSAA_SAMPLES = 4`。
    #[default]
    X4,
    X8,
}

impl Msaa {
    /// 采样数（1 / 4 / 8）。
    pub const fn samples(self) -> u32 {
        match self {
            Msaa::Off => 1,
            Msaa::X4 => 4,
            Msaa::X8 => 8,
        }
    }
}

/// 色调映射曲线 —— **只有上游 `tone.wgsl` 真正实现的两种**（B6 §2.3 契约修正）。
///
/// ⚠️ B4 Step 1 曾写成 `Reinhard` / `Aces`：那是按常见命名猜的，从未与着色器核对。
/// `tone.wgsl:45` 的实际分支是 `tone == 1 ⇒ agx(c)`，否则 `clamp(c, 0, 1)`（clip）。
/// 契约与实现不符 ⇒ 本批改为 `Clip` / `AgX`，默认 `AgX`（着色器里的 1 分支）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ToneCurve {
    /// 直接钳制到 [0,1]（`tone = 0`）：高光会死白。
    Clip,
    /// AgX（`tone = 1`，上游默认形态）。
    #[default]
    AgX,
}

/// 色调映射参数（默认关闭：`RenderParams::tone = None`）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ToneMapping {
    /// 曝光（线性域的乘数）。
    pub exposure: f32,
    pub curve: ToneCurve,
}

impl Default for ToneMapping {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            curve: ToneCurve::AgX,
        }
    }
}

/// 泛光参数（默认关闭）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bloom {
    /// 亮度阈值（低于它不参与泛光）。
    pub threshold: f32,
    /// 强度倍率（0 = 无贡献）。
    pub intensity: f32,
    /// 采样半径（屏幕像素）。
    pub radius: f32,
}

impl Default for Bloom {
    fn default() -> Self {
        Self {
            threshold: 1.0,
            intensity: 0.6,
            radius: 4.0,
        }
    }
}

/// 环境光遮蔽参数（默认关闭）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AmbientOcclusion {
    /// 采样半径（世界单位）。
    pub radius: f32,
    /// 强度倍率（0 = 无贡献）。
    pub intensity: f32,
}

impl Default for AmbientOcclusion {
    fn default() -> Self {
        Self {
            radius: 0.5,
            intensity: 1.0,
        }
    }
}

/// 渲染参数（**纯数据**，宿主构造，不经过 Pod 边界）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RenderParams {
    /// 目标分辨率（逻辑像素；`supersample` 会再放大，见 [`Self::target_size`]）。
    pub size: (u32, u32),
    /// 超采样倍数（1 = 关闭）。
    pub supersample: u32,
    pub msaa: Msaa,
    pub filter: TextureFilter,
    /// 以下四项**默认 `None`**（B4 §0.2：新能力默认关闭，默认值 = 当前行为）。
    pub tone: Option<ToneMapping>,
    pub bloom: Option<Bloom>,
    pub ao: Option<AmbientOcclusion>,
    /// 环境图（IBL）；`None` = 不用。
    pub env: Option<TextureHandle>,
}

impl Default for RenderParams {
    fn default() -> Self {
        Self {
            // 与既有 CLI 默认一致（`zela-render` 的 640×480）。
            size: (640, 480),
            supersample: 1,
            msaa: Msaa::X4,
            filter: TextureFilter::Linear,
            tone: None,
            bloom: None,
            ao: None,
            env: None,
        }
    }
}

/// 参数非法 / 当前后端不支持（显式错误，不静默纠正）。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RenderParamError {
    #[error("渲染尺寸非法：{width}×{height}（宽高必须 ≥ 1）")]
    EmptySize { width: u32, height: u32 },
    #[error("supersample 非法：{supersample}（必须 ≥ 1）")]
    ZeroSupersample { supersample: u32 },
    #[error("supersample 过大：{supersample}（上限 {max}）")]
    SupersampleTooLarge { supersample: u32, max: u32 },
    #[error("尺寸 × supersample 溢出：{width}×{height} × {supersample}")]
    SizeOverflow {
        width: u32,
        height: u32,
        supersample: u32,
    },
    #[error("tone.exposure 非法：{exposure}（必须为有限正数）")]
    InvalidExposure { exposure: f32 },
    #[error("bloom 参数非法：threshold={threshold} intensity={intensity} radius={radius}（须有限且 ≥ 0）")]
    InvalidBloom {
        threshold: f32,
        intensity: f32,
        radius: f32,
    },
    #[error("ao 参数非法：radius={radius} intensity={intensity}（须有限且 ≥ 0）")]
    InvalidAo { radius: f32, intensity: f32 },
    #[error(transparent)]
    Msaa(#[from] CapabilityError),
    #[error("当前后端不支持参数 `{param}`（显式失败，不降级）")]
    Unsupported { param: &'static str },
}

/// 超采样倍数上限（1 = 关闭；4 = 4×4 采样，再大没有实际收益）。
pub const MAX_SUPERSAMPLE: u32 = 4;

impl RenderParams {
    /// 上游目标分辨率（逻辑尺寸 × `supersample`）。
    pub fn target_size(&self) -> Result<(u32, u32), RenderParamError> {
        let s = self.supersample;
        let width = self
            .size
            .0
            .checked_mul(s)
            .ok_or(RenderParamError::SizeOverflow {
                width: self.size.0,
                height: self.size.1,
                supersample: s,
            })?;
        let height = self
            .size
            .1
            .checked_mul(s)
            .ok_or(RenderParamError::SizeOverflow {
                width: self.size.0,
                height: self.size.1,
                supersample: s,
            })?;
        Ok((width, height))
    }

    /// **有效性**校验（B4 §0.3 ①）：非法 ⇒ 显式错误，不做"纠正"。
    pub fn validate(&self) -> Result<(), RenderParamError> {
        if self.size.0 == 0 || self.size.1 == 0 {
            return Err(RenderParamError::EmptySize {
                width: self.size.0,
                height: self.size.1,
            });
        }
        if self.supersample == 0 {
            return Err(RenderParamError::ZeroSupersample {
                supersample: self.supersample,
            });
        }
        if self.supersample > MAX_SUPERSAMPLE {
            return Err(RenderParamError::SupersampleTooLarge {
                supersample: self.supersample,
                max: MAX_SUPERSAMPLE,
            });
        }
        self.target_size()?;
        if let Some(tone) = self.tone {
            if !tone.exposure.is_finite() || tone.exposure <= 0.0 {
                return Err(RenderParamError::InvalidExposure {
                    exposure: tone.exposure,
                });
            }
        }
        if let Some(bloom) = self.bloom {
            let ok = [bloom.threshold, bloom.intensity, bloom.radius]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0);
            if !ok {
                return Err(RenderParamError::InvalidBloom {
                    threshold: bloom.threshold,
                    intensity: bloom.intensity,
                    radius: bloom.radius,
                });
            }
        }
        if let Some(ao) = self.ao {
            let ok = [ao.radius, ao.intensity]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0);
            if !ok {
                return Err(RenderParamError::InvalidAo {
                    radius: ao.radius,
                    intensity: ao.intensity,
                });
            }
        }
        Ok(())
    }

    /// **一致性**校验（B4 §0.3 ②）：后端不支持 ⇒ 显式错误，不静默降级。
    ///
    /// 调用时机是 negotiate / 开渲**之前**，不是渲染中。
    pub fn is_supported_by(&self, caps: &Capabilities) -> Result<(), RenderParamError> {
        caps.require_sample_count(self.msaa.samples())?;
        if self.tone.is_some() && !caps.post.tone {
            return Err(RenderParamError::Unsupported { param: "tone" });
        }
        if self.bloom.is_some() && !caps.post.bloom {
            return Err(RenderParamError::Unsupported { param: "bloom" });
        }
        if self.ao.is_some() && !caps.post.ao {
            return Err(RenderParamError::Unsupported { param: "ao" });
        }
        if self.env.is_some() && !caps.post.env {
            return Err(RenderParamError::Unsupported { param: "env" });
        }
        Ok(())
    }

    /// **默认值绑定后端能力**（B6 §0.3）。
    ///
    /// 后端自述支持哪几项后处理，就把哪几项的默认值打开；不支持的后端沿用
    /// legacy 默认（全关）。这样"矢量后端默认带着色栈"与"wgpu 后端不挂"同时成立。
    ///
    /// ⚠️ **这不是静默降级**：
    /// - 静默降级 = 调用方**请求了**不支持的参数，后端却画出来；
    /// - 本机制 = 调用方**没有请求**，按该后端能力的默认值取。
    /// 显式请求仍然走 [`Self::is_supported_by`]，不支持即报错（严格性不变）。
    ///
    /// `env` 不参与默认：它需要一个具体的环境图句柄，不能凭空造。
    pub fn default_for(caps: &Capabilities) -> Self {
        let mut params = Self::default();
        if caps.post.tone {
            params.tone = Some(ToneMapping::default());
        }
        if caps.post.bloom {
            params.bloom = Some(Bloom::default());
        }
        if caps.post.ao {
            params.ao = Some(AmbientOcclusion::default());
        }
        params
    }

    /// 下游投影：target 描述（**唯一入口是 `RenderParams`**，§2.3）。
    ///
    /// 尺寸取 `target_size()`（已含 `supersample`）：超采样意味着先渲到大 target 再缩。
    pub fn target_desc(&self, format: ColorFormat) -> RenderTargetDesc {
        let (width, height) = self
            .target_size()
            .expect("target_size 已在 validate() 中校验；此处不引入第二条失败路径");
        RenderTargetDesc::new(width, height, format).with_sample_count(self.msaa.samples())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{DepthFormat, PostCaps};

    fn caps(max_sample_count: u32, post: PostCaps) -> Capabilities {
        Capabilities {
            max_sample_count,
            color_format: ColorFormat::Rgba8UnormSrgb,
            depth_format: DepthFormat::Depth32Float,
            post,
        }
    }

    /// **A2**：默认值逐字段等价于当前行为（640×480 / MSAA 4 / Linear / 后处理全关）。
    #[test]
    fn default_matches_current_behaviour() {
        let p = RenderParams::default();
        assert_eq!(p.size, (640, 480));
        assert_eq!(p.supersample, 1);
        assert_eq!(p.msaa.samples(), 4);
        assert_eq!(p.filter, TextureFilter::Linear);
        assert!(p.tone.is_none() && p.bloom.is_none() && p.ao.is_none() && p.env.is_none());
        assert_eq!(p.target_size().unwrap(), (640, 480));
        p.validate().unwrap();
        assert!(p.is_supported_by(&caps(4, PostCaps::none())).is_ok());
    }

    /// **A3**：非法参数 ⇒ 显式错误。
    #[test]
    fn invalid_params_are_explicit_errors() {
        let mut p = RenderParams::default();
        p.supersample = 0;
        assert!(matches!(
            p.validate(),
            Err(RenderParamError::ZeroSupersample { .. })
        ));

        let mut p = RenderParams::default();
        p.supersample = MAX_SUPERSAMPLE + 1;
        assert!(matches!(
            p.validate(),
            Err(RenderParamError::SupersampleTooLarge { .. })
        ));

        let mut p = RenderParams::default();
        p.size = (0, 480);
        assert!(matches!(p.validate(), Err(RenderParamError::EmptySize { .. })));

        let mut p = RenderParams::default();
        p.tone = Some(ToneMapping {
            exposure: 0.0,
            ..Default::default()
        });
        assert!(matches!(
            p.validate(),
            Err(RenderParamError::InvalidExposure { .. })
        ));

        let mut p = RenderParams::default();
        p.bloom = Some(Bloom {
            intensity: -1.0,
            ..Default::default()
        });
        assert!(matches!(
            p.validate(),
            Err(RenderParamError::InvalidBloom { .. })
        ));
    }

    /// **A4**：后端不支持 ⇒ 显式失败，不降级。
    #[test]
    fn unsupported_params_fail_explicitly() {
        let mut p = RenderParams::default();
        p.msaa = Msaa::X8;
        assert!(matches!(
            p.is_supported_by(&caps(4, PostCaps::none())),
            Err(RenderParamError::Msaa(CapabilityError::UnsupportedSampleCount { .. }))
        ));

        let mut p = RenderParams::default();
        p.tone = Some(ToneMapping::default());
        assert!(matches!(
            p.is_supported_by(&caps(4, PostCaps::none())),
            Err(RenderParamError::Unsupported { param: "tone" })
        ));

        // 后端声明支持 ⇒ 通过。
        let full = PostCaps {
            tone: true,
            bloom: true,
            ao: true,
            env: true,
        };
        p.bloom = Some(Bloom::default());
        p.ao = Some(AmbientOcclusion::default());
        p.env = Some(TextureHandle::new(7));
        assert!(p.is_supported_by(&caps(8, full)).is_ok());
    }

    /// target 描述是下游投影：尺寸 / 采样数都来自 `RenderParams`（`supersample` 参与尺寸）。
    #[test]
    fn target_desc_is_a_projection() {
        let mut p = RenderParams::default();
        p.msaa = Msaa::X8;
        p.supersample = 2;
        let desc = p.target_desc(ColorFormat::Bgra8UnormSrgb);
        assert_eq!((desc.width, desc.height), (1280, 960));
        assert_eq!(desc.sample_count, 8);
        desc.validate().unwrap();
    }

    /// serde 往返（Meta 承载用）。
    #[test]
    fn serde_roundtrip() {
        let mut p = RenderParams::default();
        p.tone = Some(ToneMapping {
            exposure: 1.4,
            curve: ToneCurve::Clip,
        });
        p.bloom = Some(Bloom::default());
        p.env = Some(TextureHandle::new(3));
        let json = serde_json::to_string(&p).unwrap();
        let back: RenderParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    /// **B6 §0.3**：默认值绑定后端能力 —— 支持后处理的后端默认开 tone/bloom/ao，
    /// 不支持的后端沿用 legacy（全关）。这不是静默降级：显式请求不支持项仍报错。
    #[test]
    fn default_for_follows_backend_capabilities() {
        let legacy = RenderParams::default_for(&caps(4, PostCaps::none()));
        assert!(legacy.tone.is_none() && legacy.bloom.is_none() && legacy.ao.is_none());

        let full = PostCaps {
            tone: true,
            bloom: true,
            ao: true,
            env: true,
        };
        let modern = RenderParams::default_for(&caps(8, full));
        assert_eq!(modern.tone.map(|t| t.curve), Some(ToneCurve::AgX));
        assert!(modern.bloom.is_some() && modern.ao.is_some());
        assert!(modern.env.is_none(), "env 需要具体纹理句柄，不默认开");
        modern.validate().unwrap();
        modern.is_supported_by(&caps(8, full)).unwrap();

        // 严格性保留：legacy 后端 + 显式请求 tone ⇒ 仍然报错。
        let mut explicit = legacy;
        explicit.tone = Some(ToneMapping::default());
        assert!(matches!(
            explicit.is_supported_by(&caps(4, PostCaps::none())),
            Err(RenderParamError::Unsupported { param: "tone" })
        ));
    }
}
